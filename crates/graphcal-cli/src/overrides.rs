//! Typed parsing for CLI parameter bindings.
//!
//! Evaluation commands accept direct bindings through repeatable
//! `--param name=value` arguments and one optional JSON parameter document from
//! `--params-json` or `--params-json-file`. All forms cross the CLI boundary
//! into typed parameter names immediately; closed-shape, type, dimension,
//! completeness, and constraint checks happen later against the prepared entry
//! ports.
//!
//! Direct and JSON bindings may be combined only when their parameter names are
//! disjoint. A repeated name is an error rather than an implicit precedence
//! rule.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use graphcal_compiler::desugar::desugared_ast::Expr;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::names::NameAtomError;
use graphcal_compiler::syntax::parser::ParseError;
use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::json_input::{self, JsonInputError};

/// Default maximum size for a JSON parameter document: 1 MiB.
pub const DEFAULT_PARAMS_JSON_MAX_BYTES: u64 = 1024 * 1024;

/// Shared parameter-binding arguments for every command that evaluates a model.
#[derive(Args, Debug, Default)]
pub struct ParameterArgs {
    /// Bind one param to a closed Graphcal value; repeatable
    #[arg(long, value_name = "NAME=VALUE", help_heading = "Parameter bindings")]
    param: Vec<String>,
    /// Bind params from an inline JSON object
    #[arg(
        long,
        value_name = "JSON",
        group = "params_json_source",
        help_heading = "Parameter bindings"
    )]
    params_json: Option<String>,
    /// Bind params from a JSON file; use `-` to read stdin
    #[arg(
        long,
        value_name = "FILE",
        group = "params_json_source",
        help_heading = "Parameter bindings"
    )]
    params_json_file: Option<PathBuf>,
    /// Maximum JSON parameter document size in bytes; defaults to 1 MiB
    #[arg(
        long,
        value_name = "BYTES",
        requires = "params_json_source",
        help_heading = "Parameter bindings"
    )]
    params_json_max_bytes: Option<u64>,
}

impl ParameterArgs {
    fn json_source(&self) -> Result<Option<ParameterJsonSource>, OverrideParseError> {
        match (&self.params_json, &self.params_json_file) {
            (Some(_), Some(_)) => Err(OverrideParseError::ConflictingJsonSources),
            (Some(json), None) => Ok(Some(ParameterJsonSource::Inline(json.clone()))),
            (None, Some(path)) if path == Path::new("-") => Ok(Some(ParameterJsonSource::Stdin)),
            (None, Some(path)) => Ok(Some(ParameterJsonSource::File(path.clone()))),
            (None, None) => Ok(None),
        }
    }
}

/// The explicitly selected source of a bulk JSON parameter document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterJsonSource {
    /// JSON supplied directly as the `--params-json` argument.
    Inline(String),
    /// JSON read from the path supplied to `--params-json-file`.
    File(PathBuf),
    /// JSON read from stdin via `--params-json-file -`.
    Stdin,
}

/// One validated direct `--param` binding, retaining its source spelling for
/// report hydration and reproduction commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectParameterBinding {
    /// Typed entry-parameter name.
    pub name: DeclName,
    /// Closed Graphcal value syntax supplied after `=`.
    pub expression: String,
}

/// Parsed CLI bindings plus source metadata for diagnostics and report replay.
#[derive(Debug)]
pub struct ParsedOverrides {
    /// Values keyed by their typed entry parameter names.
    pub values: HashMap<DeclName, Expr>,
    /// Diagnostic sources for values synthesized from a JSON document.
    pub json_sources: HashMap<DeclName, JsonOverrideSource>,
    /// Direct bindings in command-line order.
    pub direct_parameters: Vec<DirectParameterBinding>,
    /// Bulk JSON source selected for this invocation, including an empty object.
    pub json_source: Option<ParameterJsonSource>,
    /// Explicit JSON size limit selected for reproduction, if any.
    pub json_max_bytes: Option<u64>,
}

/// Source information for one value synthesized from a JSON parameter document.
#[derive(Debug, Clone)]
pub struct JsonOverrideSource {
    /// The JSON document displayed by miette.
    pub source: NamedSource<Arc<String>>,
    /// Span of the top-level parameter key in the JSON document.
    pub span: SourceSpan,
}

/// Diagnostic identity of a JSON parameter document without its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterJsonOrigin {
    /// The `--params-json` command-line argument.
    Inline,
    /// A filesystem path supplied through `--params-json-file`.
    File(PathBuf),
    /// Standard input selected with `--params-json-file -`.
    Stdin,
}

impl ParameterJsonOrigin {
    fn diagnostic_name(&self) -> String {
        match self {
            Self::Inline => "<--params-json>".to_string(),
            Self::File(path) => path.display().to_string(),
            Self::Stdin => "<stdin>".to_string(),
        }
    }
}

impl fmt::Display for ParameterJsonOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline => formatter.write_str("`--params-json`"),
            Self::File(path) => write!(formatter, "file {}", path.display()),
            Self::Stdin => formatter.write_str("stdin"),
        }
    }
}

#[derive(Debug)]
struct LoadedParameterJson {
    origin: ParameterJsonOrigin,
    text: String,
}

/// Errors that can occur when parsing CLI parameter bindings.
#[derive(Debug, Error, Diagnostic)]
pub enum OverrideParseError {
    /// A `--param` argument is missing the `=` separator.
    #[error("invalid --param format: {raw:?} (expected 'name=value')")]
    #[diagnostic(code(graphcal::cli::O001))]
    InvalidFormat {
        /// The raw string as received from the command line.
        raw: String,
    },

    /// A `--param` argument has an empty name (e.g. `=42`).
    #[error("invalid --param format: {raw:?} (name is empty)")]
    #[diagnostic(code(graphcal::cli::O002))]
    EmptyName {
        /// The raw string as received from the command line.
        raw: String,
    },

    /// A `--param` argument has an empty expression (e.g. `x=`).
    #[error("invalid --param format: {raw:?} (expression is empty)")]
    #[diagnostic(code(graphcal::cli::O003))]
    EmptyExpression {
        /// The raw string as received from the command line.
        raw: String,
    },

    /// A `--param` name is not an unqualified parameter leaf name.
    #[error(
        "invalid --param name `{name}` in {raw:?}: binding names must be unqualified parameter names ({reason})"
    )]
    #[diagnostic(code(graphcal::cli::O008))]
    InvalidName {
        /// The raw string as received from the command line.
        raw: String,
        /// The invalid name segment.
        name: String,
        /// The validation failure.
        reason: NameAtomError,
    },

    /// A `--param` value failed to parse as Graphcal value syntax.
    #[error("failed to parse --param value for `{name}`: {source}")]
    #[diagnostic(code(graphcal::cli::O004))]
    ExpressionParse {
        /// The typed name of the param being bound.
        name: DeclName,
        /// The underlying parser error.
        ///
        /// Boxed because `ParseError` carries source / span context that makes
        /// it large; boxing keeps the enum compact on the `Ok` path.
        #[source]
        source: Box<ParseError>,
    },

    /// The same parameter name was supplied by more than one CLI source.
    #[error("duplicate parameter binding for `{name}`")]
    #[diagnostic(code(graphcal::cli::O009))]
    DuplicateParameterBinding { name: DeclName },

    /// Both mutually exclusive bulk JSON sources were supplied.
    #[error("`--params-json` and `--params-json-file` cannot be used together")]
    #[diagnostic(code(graphcal::cli::O010))]
    ConflictingJsonSources,

    /// A JSON size limit was supplied without a JSON source.
    #[error("`--params-json-max-bytes` requires `--params-json` or `--params-json-file`")]
    #[diagnostic(code(graphcal::cli::O011))]
    JsonLimitWithoutSource,

    /// A JSON parameter document could not be opened or read.
    #[error("cannot read parameter JSON from {origin}: {source}")]
    #[diagnostic(code(graphcal::cli::O005))]
    JsonRead {
        /// Where the JSON was read from.
        origin: ParameterJsonOrigin,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A JSON parameter document exceeded the configured size cap.
    #[error(
        "parameter JSON from {origin} is {size} bytes, exceeds limit of {limit} bytes (use --params-json-max-bytes to override)"
    )]
    #[diagnostic(code(graphcal::cli::O006))]
    JsonTooLarge {
        /// Where the JSON was supplied.
        origin: ParameterJsonOrigin,
        /// The document size in bytes.
        size: u64,
        /// The active limit in bytes.
        limit: u64,
    },

    /// A JSON parameter document failed to match the parameter schema.
    #[error("cannot parse parameter JSON from {origin}: {source}")]
    #[diagnostic(code(graphcal::cli::O007))]
    JsonParse {
        /// Where the JSON was supplied.
        origin: ParameterJsonOrigin,
        /// The underlying schema error.
        #[source]
        source: JsonInputError,
    },
}

/// Parse direct and bulk JSON parameter arguments into one binding map.
///
/// The JSON size limit applies equally to inline, file, and stdin documents;
/// `None` uses [`DEFAULT_PARAMS_JSON_MAX_BYTES`]. A parameter name may occur in
/// only one source.
///
/// # Errors
///
/// Returns [`OverrideParseError`] for malformed direct bindings, invalid source
/// combinations, bounded-read failures, invalid JSON parameter documents, or
/// duplicate names across any source.
pub fn parse_overrides_with_sources(
    args: &ParameterArgs,
) -> Result<ParsedOverrides, OverrideParseError> {
    let json_source = args.json_source()?;
    if args.params_json_max_bytes.is_some() && json_source.is_none() {
        return Err(OverrideParseError::JsonLimitWithoutSource);
    }

    let mut overrides = HashMap::new();
    let mut json_sources = HashMap::new();
    let mut direct_parameters = Vec::new();

    for raw in &args.param {
        let binding = DirectParameterBinding::parse(raw)?;
        let raw_expr = graphcal_compiler::syntax::parser::Parser::new(&binding.expression)
            .parse_single_expr()
            .map_err(|source| OverrideParseError::ExpressionParse {
                name: binding.name.clone(),
                source: Box::new(source),
            })?;
        match overrides.entry(binding.name.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(resolve_parameter_expr(raw_expr));
                direct_parameters.push(binding);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(OverrideParseError::DuplicateParameterBinding { name: binding.name });
            }
        }
    }

    if let Some(source) = &json_source {
        let limit = args
            .params_json_max_bytes
            .unwrap_or(DEFAULT_PARAMS_JSON_MAX_BYTES);
        let loaded = load_parameter_json(source, limit)?;
        let json_overrides = json_input::json_to_overrides(&loaded.text).map_err(|source| {
            OverrideParseError::JsonParse {
                origin: loaded.origin.clone(),
                source,
            }
        })?;
        let parameter_spans = top_level_parameter_spans(&loaded.text);
        let diagnostic_source =
            NamedSource::new(loaded.origin.diagnostic_name(), Arc::new(loaded.text));
        for (name, expression) in json_overrides {
            match overrides.entry(name.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(resolve_parameter_expr(expression));
                    let span = parameter_spans
                        .get(&name)
                        .copied()
                        .unwrap_or_else(|| (0usize, 0usize).into());
                    json_sources.insert(
                        name,
                        JsonOverrideSource {
                            source: diagnostic_source.clone(),
                            span,
                        },
                    );
                }
                std::collections::hash_map::Entry::Occupied(_) => {
                    return Err(OverrideParseError::DuplicateParameterBinding { name });
                }
            }
        }
    }

    Ok(ParsedOverrides {
        values: overrides,
        json_sources,
        direct_parameters,
        json_source,
        json_max_bytes: args.params_json_max_bytes,
    })
}

impl DirectParameterBinding {
    fn parse(raw: &str) -> Result<Self, OverrideParseError> {
        let Some((name, expression)) = raw.split_once('=') else {
            return Err(OverrideParseError::InvalidFormat {
                raw: raw.to_string(),
            });
        };
        let name = name.trim();
        let expression = expression.trim();
        if name.is_empty() {
            return Err(OverrideParseError::EmptyName {
                raw: raw.to_string(),
            });
        }
        if expression.is_empty() {
            return Err(OverrideParseError::EmptyExpression {
                raw: raw.to_string(),
            });
        }
        let name = DeclName::try_new(name.to_string()).map_err(|reason| {
            OverrideParseError::InvalidName {
                raw: raw.to_string(),
                name: name.to_string(),
                reason,
            }
        })?;
        Ok(Self {
            name,
            expression: expression.to_string(),
        })
    }
}

/// Locate top-level object keys after serde has validated the JSON. This small
/// lexical pass retains only boundary-source metadata; semantic values still
/// come exclusively from `serde_json` and [`json_input::json_to_overrides`].
fn top_level_parameter_spans(json: &str) -> HashMap<DeclName, SourceSpan> {
    let bytes = json.as_bytes();
    let mut containers = Vec::new();
    let mut spans = HashMap::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'"' => {
                let start = cursor;
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor] != b'"' {
                    cursor += usize::from(bytes[cursor] == b'\\') + 1;
                }
                cursor = cursor.saturating_add(1).min(bytes.len());

                let mut after = cursor;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if containers.as_slice() == b"{" && bytes.get(after) == Some(&b':') {
                    let key_literal = &json[start..cursor];
                    if let Some(name) = serde_json::from_str::<String>(key_literal)
                        .ok()
                        .and_then(|key| DeclName::try_new(key).ok())
                    {
                        spans.insert(name, (start, cursor - start).into());
                    }
                }
                continue;
            }
            b'{' | b'[' => containers.push(bytes[cursor]),
            b'}' if containers.last() == Some(&b'{') => {
                containers.pop();
            }
            b']' if containers.last() == Some(&b'[') => {
                containers.pop();
            }
            _ => {}
        }
        cursor += 1;
    }

    spans
}

fn load_parameter_json(
    source: &ParameterJsonSource,
    limit: u64,
) -> Result<LoadedParameterJson, OverrideParseError> {
    match source {
        ParameterJsonSource::Inline(text) => {
            let origin = ParameterJsonOrigin::Inline;
            let size = u64::try_from(text.len()).unwrap_or(u64::MAX);
            ensure_json_size(&origin, size, limit)?;
            Ok(LoadedParameterJson {
                origin,
                text: text.clone(),
            })
        }
        ParameterJsonSource::File(path) => read_parameter_json_file(path, limit),
        ParameterJsonSource::Stdin => {
            let origin = ParameterJsonOrigin::Stdin;
            read_parameter_json_bounded(std::io::stdin().lock(), origin, limit)
        }
    }
}

fn read_parameter_json_file(
    path: &Path,
    limit: u64,
) -> Result<LoadedParameterJson, OverrideParseError> {
    let origin = ParameterJsonOrigin::File(path.to_path_buf());
    let file = std::fs::File::open(path).map_err(|source| OverrideParseError::JsonRead {
        origin: origin.clone(),
        source,
    })?;
    let size = file
        .metadata()
        .map_err(|source| OverrideParseError::JsonRead {
            origin: origin.clone(),
            source,
        })?
        .len();
    ensure_json_size(&origin, size, limit)?;
    read_parameter_json_bounded(file, origin, limit)
}

fn read_parameter_json_bounded(
    reader: impl Read,
    origin: ParameterJsonOrigin,
    limit: u64,
) -> Result<LoadedParameterJson, OverrideParseError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| OverrideParseError::JsonRead {
            origin: origin.clone(),
            source,
        })?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    ensure_json_size(&origin, size, limit)?;
    let text = String::from_utf8(bytes).map_err(|source| OverrideParseError::JsonRead {
        origin: origin.clone(),
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })?;
    Ok(LoadedParameterJson { origin, text })
}

fn ensure_json_size(
    origin: &ParameterJsonOrigin,
    size: u64,
    limit: u64,
) -> Result<(), OverrideParseError> {
    if size > limit {
        return Err(OverrideParseError::JsonTooLarge {
            origin: origin.clone(),
            size,
            limit,
        });
    }
    Ok(())
}

/// Lift a raw parameter expression into the desugared AST.
///
/// CLI parameter expressions are user-provided literals — they never carry
/// sugar variants, so the `Raw → Desugared` lift is a structural rebind.
/// Reference resolution happens once, in HIR lowering, when the binding
/// replaces the target param's default in the compiled file's scope.
fn resolve_parameter_expr(
    raw: graphcal_compiler::syntax::ast::Expr,
) -> graphcal_compiler::desugar::desugared_ast::Expr {
    raw.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_json_size_limit_is_one_mebibyte() {
        assert_eq!(DEFAULT_PARAMS_JSON_MAX_BYTES, 0x10_0000);
    }

    #[test]
    fn json_origins_have_explicit_display_names() {
        assert_eq!(ParameterJsonOrigin::Inline.to_string(), "`--params-json`");
        assert_eq!(
            ParameterJsonOrigin::File(PathBuf::from("params.json")).to_string(),
            "file params.json"
        );
        assert_eq!(ParameterJsonOrigin::Stdin.to_string(), "stdin");
    }

    #[test]
    fn json_size_limit_is_inclusive() {
        let origin = ParameterJsonOrigin::Inline;
        assert!(ensure_json_size(&origin, 42, 42).is_ok());
        assert!(matches!(
            ensure_json_size(&origin, 43, 42),
            Err(OverrideParseError::JsonTooLarge {
                size: 43,
                limit: 42,
                ..
            })
        ));
    }

    fn direct_args(parameters: &[&str]) -> ParameterArgs {
        ParameterArgs {
            param: parameters
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            ..ParameterArgs::default()
        }
    }

    fn parse_values(args: &ParameterArgs) -> Result<HashMap<DeclName, Expr>, OverrideParseError> {
        parse_overrides_with_sources(args).map(|parsed| parsed.values)
    }

    #[test]
    fn direct_parameters_parse_and_retain_typed_source_values() {
        let args = direct_args(&["x=1.0 m", "y=2"]);
        let parsed = parse_overrides_with_sources(&args).unwrap();

        assert_eq!(parsed.values.len(), 2);
        assert_eq!(
            parsed.direct_parameters,
            [
                DirectParameterBinding {
                    name: DeclName::expect_valid("x"),
                    expression: "1.0 m".to_string(),
                },
                DirectParameterBinding {
                    name: DeclName::expect_valid("y"),
                    expression: "2".to_string(),
                },
            ]
        );
    }

    #[test]
    fn direct_parameter_trims_whitespace() {
        let overrides = parse_values(&direct_args(&["  x  =  42  "])).unwrap();
        assert!(overrides.contains_key(&DeclName::expect_valid("x")));
    }

    #[test]
    fn direct_parameter_requires_equals() {
        let err = parse_values(&direct_args(&["just_a_name"])).unwrap_err();
        assert!(matches!(err, OverrideParseError::InvalidFormat { .. }));
    }

    #[test]
    fn direct_parameters_reject_duplicate_name() {
        let err = parse_values(&direct_args(&["x=1", "x=2"])).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::DuplicateParameterBinding { name } if name.as_str() == "x")
        );
    }

    #[test]
    fn direct_parameter_rejects_empty_name() {
        let err = parse_values(&direct_args(&["=42"])).unwrap_err();
        assert!(matches!(err, OverrideParseError::EmptyName { .. }));
    }

    #[test]
    fn direct_parameter_rejects_empty_expression() {
        let err = parse_values(&direct_args(&["x="])).unwrap_err();
        assert!(matches!(err, OverrideParseError::EmptyExpression { .. }));
    }

    #[test]
    fn direct_parameter_rejects_unparsable_expression() {
        let err = parse_values(&direct_args(&["x=###"])).unwrap_err();
        match err {
            OverrideParseError::ExpressionParse { name, .. } => assert_eq!(name.as_str(), "x"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn direct_parameter_rejects_qualified_name_without_panicking() {
        let err = parse_values(&direct_args(&["module.x=1"])).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::InvalidName { ref name, .. } if name == "module.x"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn missing_json_parameter_file_is_an_error() {
        let args = ParameterArgs {
            params_json_file: Some(PathBuf::from("/nonexistent/path/file.json")),
            ..ParameterArgs::default()
        };
        let err = parse_values(&args).unwrap_err();
        assert!(matches!(err, OverrideParseError::JsonRead { .. }));
    }

    #[test]
    fn json_parameter_file_obeys_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("params.json");
        let payload = format!("{{\"x\": {}}}", "1".repeat(2048));
        std::fs::write(&path, payload).unwrap();
        let args = ParameterArgs {
            params_json_file: Some(path),
            params_json_max_bytes: Some(256),
            ..ParameterArgs::default()
        };

        let err = parse_values(&args).unwrap_err();
        match err {
            OverrideParseError::JsonTooLarge { size, limit, .. } => {
                assert_eq!(limit, 256);
                assert!(size > 256);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn inline_json_obeys_size_limit() {
        let args = ParameterArgs {
            params_json: Some(r#"{"x":"123"}"#.to_string()),
            params_json_max_bytes: Some(4),
            ..ParameterArgs::default()
        };
        assert!(matches!(
            parse_values(&args),
            Err(OverrideParseError::JsonTooLarge {
                origin: ParameterJsonOrigin::Inline,
                ..
            })
        ));
    }

    #[test]
    fn json_parameter_file_within_limit_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("params.json");
        std::fs::write(&path, r#"{"x": "1.0 m"}"#).unwrap();
        let args = ParameterArgs {
            params_json_file: Some(path.clone()),
            params_json_max_bytes: Some(DEFAULT_PARAMS_JSON_MAX_BYTES),
            ..ParameterArgs::default()
        };

        let parsed = parse_overrides_with_sources(&args).unwrap();
        assert!(parsed.values.contains_key(&DeclName::expect_valid("x")));
        assert_eq!(parsed.json_source, Some(ParameterJsonSource::File(path)));
    }

    #[test]
    fn inline_json_retains_top_level_key_span_and_virtual_source() {
        let args = ParameterArgs {
            params_json: Some(
                "{\n  \"matrix\": {\"index\": \"Row\", \"entries\": {}}\n}".to_string(),
            ),
            ..ParameterArgs::default()
        };

        let parsed = parse_overrides_with_sources(&args).unwrap();
        let source = &parsed.json_sources[&DeclName::expect_valid("matrix")];
        assert_eq!(source.span.offset(), 4);
        assert_eq!(source.span.len(), 8);
        assert_eq!(source.source.name(), "<--params-json>");
    }

    #[test]
    fn direct_and_json_parameters_must_be_disjoint() {
        let args = ParameterArgs {
            param: vec!["x=42".to_string()],
            params_json: Some(r#"{"x": "10.0"}"#.to_string()),
            ..ParameterArgs::default()
        };

        let err = parse_values(&args).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::DuplicateParameterBinding { name } if name.as_str() == "x")
        );
    }

    #[test]
    fn direct_and_json_parameters_can_supply_disjoint_names() {
        let args = ParameterArgs {
            param: vec!["x=42".to_string()],
            params_json: Some(r#"{"y": "10.0"}"#.to_string()),
            ..ParameterArgs::default()
        };
        let overrides = parse_values(&args).unwrap();
        assert_eq!(overrides.len(), 2);
    }

    #[test]
    fn json_limit_requires_a_json_source_even_outside_clap() {
        let args = ParameterArgs {
            params_json_max_bytes: Some(42),
            ..ParameterArgs::default()
        };
        assert!(matches!(
            parse_values(&args),
            Err(OverrideParseError::JsonLimitWithoutSource)
        ));
    }

    #[test]
    fn json_sources_are_mutually_exclusive_even_outside_clap() {
        let args = ParameterArgs {
            params_json: Some("{}".to_string()),
            params_json_file: Some(PathBuf::from("params.json")),
            ..ParameterArgs::default()
        };
        assert!(matches!(
            parse_values(&args),
            Err(OverrideParseError::ConflictingJsonSources)
        ));
    }
}
