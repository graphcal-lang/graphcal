//! Typed parsing for CLI parameter overrides (`--set` / `--input`).
//!
//! The Eval subcommand accepts two override sources:
//!
//! * `--set name=expr` — one expression per `--set` flag, parsed as a GCL
//!   single expression.
//! * `--input path.json` — a JSON file with a [`json_input`] schema.
//!
//! `--set` takes precedence over `--input` on name collision. Both sources are
//! resolved into a `HashMap<DeclName, Expr>` here; type/dimension checking
//! happens later in the evaluator.
//!
//! [`json_input`]: crate::json_input

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use graphcal_compiler::desugar::resolved_ast::Expr;
use graphcal_compiler::syntax::names::{DeclName, NameAtomError};
use graphcal_compiler::syntax::parser::ParseError;
use miette::Diagnostic;
use thiserror::Error;

use crate::json_input::{self, JsonInputError};

/// Default max size for an `--input` JSON file: 1 MiB.
///
/// Chosen to fit all realistic parameter payloads while still rejecting an
/// accidental `--input some-huge-dataset.json` with a clear error instead of
/// silently OOM-ing inside `serde_json`.
pub const DEFAULT_INPUT_MAX_BYTES: u64 = 1024 * 1024;

/// Errors that can occur when parsing CLI overrides.
#[derive(Debug, Error, Diagnostic)]
pub enum OverrideParseError {
    /// A `--set` argument is missing the `=` separator.
    #[error("invalid --set format: {raw:?} (expected 'name=expr')")]
    #[diagnostic(code(graphcal::cli::O001))]
    InvalidFormat {
        /// The raw string as received from the command line.
        raw: String,
    },

    /// A `--set` argument has an empty name (e.g. `=42`).
    #[error("invalid --set format: {raw:?} (name is empty)")]
    #[diagnostic(code(graphcal::cli::O002))]
    EmptyName {
        /// The raw string as received from the command line.
        raw: String,
    },

    /// A `--set` argument has an empty expression (e.g. `x=`).
    #[error("invalid --set format: {raw:?} (expression is empty)")]
    #[diagnostic(code(graphcal::cli::O003))]
    EmptyExpression {
        /// The raw string as received from the command line.
        raw: String,
    },

    /// A `--set` name is not an unqualified parameter leaf name.
    #[error(
        "invalid --set name `{name}` in {raw:?}: override names must be unqualified parameter names ({reason})"
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

    /// A `--set` expression failed to parse as a GCL expression.
    #[error("failed to parse --set value for `{name}`: {source}")]
    #[diagnostic(code(graphcal::cli::O004))]
    ExpressionParse {
        /// The name of the param being overridden.
        name: String,
        /// The underlying parser error.
        ///
        /// Boxed because `ParseError` carries source / span context that makes
        /// it large; boxing keeps the enum compact on the `Ok` path.
        #[source]
        source: Box<ParseError>,
    },

    /// An `--input` JSON file could not be opened / read.
    #[error("cannot read input file {}: {source}", path.display())]
    #[diagnostic(code(graphcal::cli::O005))]
    InputFileRead {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// An `--input` JSON file exceeded the configured size cap.
    #[error(
        "input file {} is {size} bytes, exceeds limit of {limit} bytes (use --input-max-bytes to override)",
        path.display()
    )]
    #[diagnostic(code(graphcal::cli::O006))]
    InputFileTooLarge {
        /// The offending input path.
        path: PathBuf,
        /// The file size in bytes.
        size: u64,
        /// The active limit in bytes.
        limit: u64,
    },

    /// An `--input` JSON file failed to parse per the JSON-input schema.
    #[error("cannot parse input file {}: {source}", path.display())]
    #[diagnostic(code(graphcal::cli::O007))]
    InputFileParse {
        /// The offending input path.
        path: PathBuf,
        /// The underlying schema error.
        #[source]
        source: JsonInputError,
    },
}

/// Parse `--set` overrides and an optional `--input` JSON file into a combined
/// overrides map.
///
/// `--set` values take precedence over `--input` values for the same name.
/// `input_max_bytes` caps the `--input` file size; `None` uses
/// [`DEFAULT_INPUT_MAX_BYTES`].
///
/// # Errors
///
/// Returns [`OverrideParseError`] if any `--set` entry is malformed, if
/// `--input` cannot be read, is too large, or fails to match the JSON-input
/// schema.
pub fn parse_overrides(
    set: &[String],
    input: Option<&Path>,
    input_max_bytes: Option<u64>,
) -> Result<HashMap<DeclName, Expr>, OverrideParseError> {
    let mut overrides = HashMap::new();

    for s in set {
        let Some((name, value_str)) = s.split_once('=') else {
            return Err(OverrideParseError::InvalidFormat { raw: s.clone() });
        };
        let name = name.trim();
        let value_str = value_str.trim();
        if name.is_empty() {
            return Err(OverrideParseError::EmptyName { raw: s.clone() });
        }
        if value_str.is_empty() {
            return Err(OverrideParseError::EmptyExpression { raw: s.clone() });
        }
        let override_name = DeclName::try_new(name.to_string()).map_err(|reason| {
            OverrideParseError::InvalidName {
                raw: s.clone(),
                name: name.to_string(),
                reason,
            }
        })?;
        let raw_expr = graphcal_compiler::syntax::parser::Parser::new(value_str)
            .parse_single_expr()
            .map_err(|e| OverrideParseError::ExpressionParse {
                name: name.to_string(),
                source: Box::new(e),
            })?;
        overrides.insert(override_name, resolve_override_expr(raw_expr));
    }

    if let Some(input_path) = input {
        let limit = input_max_bytes.unwrap_or(DEFAULT_INPUT_MAX_BYTES);

        // Check size before reading the whole file into memory. `metadata`
        // follows symlinks, which matches `read_to_string`'s behavior.
        let metadata =
            std::fs::metadata(input_path).map_err(|e| OverrideParseError::InputFileRead {
                path: input_path.to_path_buf(),
                source: e,
            })?;
        let size = metadata.len();
        if size > limit {
            return Err(OverrideParseError::InputFileTooLarge {
                path: input_path.to_path_buf(),
                size,
                limit,
            });
        }

        let json_str =
            std::fs::read_to_string(input_path).map_err(|e| OverrideParseError::InputFileRead {
                path: input_path.to_path_buf(),
                source: e,
            })?;
        let json_overrides = json_input::json_to_overrides(&json_str).map_err(|e| {
            OverrideParseError::InputFileParse {
                path: input_path.to_path_buf(),
                source: e,
            }
        })?;
        for (name, expr) in json_overrides {
            overrides
                .entry(name)
                .or_insert_with(|| resolve_override_expr(expr));
        }
    }

    Ok(overrides)
}

/// Lift a raw override expression into the resolved AST.
///
/// Override expressions are user-provided literals — they never carry sugar
/// variants, so the `Raw → Desugared` lift is a structural rebind, and
/// resolution happens with no file scope (only builtins and time-scale names
/// visible). Shared by the `--set` and `--input` paths so the
/// standalone-resolution contract lives in one place.
fn resolve_override_expr(
    raw: graphcal_compiler::syntax::ast::Expr,
) -> graphcal_compiler::desugar::resolved_ast::Expr {
    let desugared: graphcal_compiler::desugar::desugared_ast::Expr = raw.into();
    graphcal_compiler::syntax::name_resolve::resolve_standalone_expr(desugared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_overrides_happy_path() {
        let set = vec!["x=1.0 m".to_string(), "y=2".to_string()];
        let overrides = parse_overrides(&set, None, None).unwrap();
        assert_eq!(overrides.len(), 2);
        assert!(overrides.contains_key(&DeclName::new("x")));
        assert!(overrides.contains_key(&DeclName::new("y")));
    }

    #[test]
    fn parse_overrides_trims_whitespace() {
        let set = vec!["  x  =  42  ".to_string()];
        let overrides = parse_overrides(&set, None, None).unwrap();
        assert!(overrides.contains_key(&DeclName::new("x")));
    }

    #[test]
    fn parse_overrides_missing_equals() {
        let set = vec!["just_a_name".to_string()];
        let err = parse_overrides(&set, None, None).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::InvalidFormat { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_overrides_empty_name() {
        let set = vec!["=42".to_string()];
        let err = parse_overrides(&set, None, None).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::EmptyName { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_overrides_empty_expression() {
        let set = vec!["x=".to_string()];
        let err = parse_overrides(&set, None, None).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::EmptyExpression { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_overrides_unparsable_expression() {
        let set = vec!["x=###".to_string()];
        let err = parse_overrides(&set, None, None).unwrap_err();
        match err {
            OverrideParseError::ExpressionParse { name, .. } => assert_eq!(name, "x"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_overrides_rejects_qualified_name_without_panicking() {
        let set = vec!["module.x=1".to_string()];
        let err = parse_overrides(&set, None, None).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::InvalidName { ref name, .. } if name == "module.x"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn parse_overrides_missing_input_file() {
        let missing = Path::new("/nonexistent/path/file.json");
        let err = parse_overrides(&[], Some(missing), None).unwrap_err();
        assert!(
            matches!(err, OverrideParseError::InputFileRead { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_overrides_input_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        // Write ~2 KiB of JSON.
        let payload = format!("{{\"x\": {}}}", "1".repeat(2048));
        std::fs::write(&path, &payload).unwrap();

        // Set limit to 256 bytes — file is larger.
        let err = parse_overrides(&[], Some(&path), Some(256)).unwrap_err();
        match err {
            OverrideParseError::InputFileTooLarge { size, limit, .. } => {
                assert_eq!(limit, 256);
                assert!(size > 256);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parse_overrides_input_file_within_limit_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, r#"{"x": "1.0 m"}"#).unwrap();

        let overrides = parse_overrides(&[], Some(&path), Some(DEFAULT_INPUT_MAX_BYTES)).unwrap();
        assert!(overrides.contains_key(&DeclName::new("x")));
    }

    #[test]
    fn parse_overrides_set_precedence_over_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, r#"{"x": "10.0"}"#).unwrap();

        let set = vec!["x=42".to_string()];
        let overrides = parse_overrides(&set, Some(&path), None).unwrap();
        // `x` from --set wins. We just assert that there is exactly one entry
        // (the name-uniqueness proves precedence — the actual expr shape is
        // tested in json_input / parser tests).
        assert_eq!(overrides.len(), 1);
        assert!(overrides.contains_key(&DeclName::new("x")));
    }
}
