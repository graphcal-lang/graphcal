//! Diagnostic production from compile errors and evaluation results.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Range,
    Url,
};

use graphcal_eval::eval::CompileError;

use crate::convert::LineIndex;
use crate::symbol_table::SymbolTable;

/// Typed LSP payload category for an unknown-name auto-import repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoImportCategory {
    Term,
    Type,
    Dimension,
    Unit,
    Index,
}

impl AutoImportCategory {
    pub const fn namespace(self) -> graphcal_compiler::syntax::ast::ImportItemNamespace {
        use graphcal_compiler::syntax::ast::ImportItemNamespace;
        match self {
            Self::Term => ImportItemNamespace::Term,
            Self::Type => ImportItemNamespace::Type,
            Self::Dimension => ImportItemNamespace::Dimension,
            Self::Unit => ImportItemNamespace::Unit,
            Self::Index => ImportItemNamespace::Index,
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoImportDiagnosticData {
    pub auto_import_name: String,
    pub auto_import_category: AutoImportCategory,
}

fn auto_import_data(
    name: impl Into<String>,
    category: AutoImportCategory,
) -> Option<serde_json::Value> {
    serde_json::to_value(AutoImportDiagnosticData {
        auto_import_name: name.into(),
        auto_import_category: category,
    })
    .ok()
}

/// Convert per-node runtime errors and assertion failures in an `EvalResult` to LSP diagnostics.
///
/// `symbol_table` is used to resolve the declaration span for each per-node
/// error, so the diagnostic points at the failing declaration rather than the
/// start of the file.
pub fn eval_result_to_diagnostics(
    result: &graphcal_eval::eval::EvalResult,
    source: &str,
    symbol_table: &SymbolTable,
) -> Vec<Diagnostic> {
    let lines = LineIndex::new(source);
    // Node/param evaluation errors
    let mut diagnostics: Vec<Diagnostic> = result
        .all
        .iter()
        .filter_map(|(name, r, _)| match r {
            Err(err) => {
                let range = symbol_table
                    .definition_for_scoped_decl(name)
                    .map_or_else(Range::default, |def| lines.span_to_range(def.name_span));
                Some(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String("graphcal::E001".to_string())),
                    source: Some("graphcal".to_string()),
                    message: format!("{name}: {err}"),
                    ..Default::default()
                })
            }
            Ok(_) => None,
        })
        .collect();

    // Assertion failures
    diagnostics.extend(
        result
            .assertions
            .iter()
            .filter_map(|(name, assert_result, span)| {
                use graphcal_eval::eval::AssertResult;

                let (message, severity) = match assert_result {
                    AssertResult::Pass => return None,
                    AssertResult::Fail { message } => {
                        let msg = result.assumes_map.get(name).map_or_else(
                            || format!("assertion `{name}` failed: {message}"),
                            |affected| {
                                let affected: Vec<String> =
                                    affected.iter().map(ToString::to_string).collect();
                                format!(
                                    "assertion `{name}` failed: {message} (affected: {})",
                                    affected.join(", ")
                                )
                            },
                        );
                        (msg, DiagnosticSeverity::WARNING)
                    }
                    AssertResult::Error { message } => (
                        format!("assertion `{name}` error: {message}"),
                        DiagnosticSeverity::WARNING,
                    ),
                };

                Some(Diagnostic {
                    range: lines.span_to_range(*span),
                    severity: Some(severity),
                    code: Some(NumberOrString::String("graphcal::A001".to_string())),
                    source: Some("graphcal".to_string()),
                    message,
                    ..Default::default()
                })
            }),
    );

    diagnostics
}

/// Convert a `CompileError` to LSP diagnostics, grouped by the URI of the
/// source file the error actually belongs to.
///
/// The (URI, source) pair is read directly from the error's
/// [`CompileError::named_source`]: the source text is the exact buffer whose
/// byte offsets the error's labels index into, and the URI is parsed from the
/// name miette already carries on the source. This makes it structurally
/// impossible to pair offsets from one file with the line index of another —
/// e.g., an `import`ed file's parse error cannot be rendered against the
/// importer's text, no matter how nested the import graph is.
///
/// `fallback_uri` is used only for source-less errors (loader-stage
/// `FileNotFound`, `CircularImport`, `ManifestError`) and as a last-ditch
/// fallback when the error's source name can be neither parsed as a URL nor
/// converted from a file path (e.g., synthetic test names).
///
/// When an error has multiple labeled spans (e.g., "duplicate definition here" +
/// "first defined here"), the first label becomes the primary diagnostic and the
/// remaining labels are attached as `DiagnosticRelatedInformation`. All labels
/// of a single error share one source per the miette API, so the related
/// information references the same URI.
pub fn compile_error_to_diagnostics_grouped(
    error: &CompileError,
    fallback_uri: &Url,
) -> HashMap<Url, Vec<Diagnostic>> {
    let diag: &dyn miette::Diagnostic = error;
    let data = structured_data(error);

    // Source-less errors carry no labels either, so an empty source is safe
    // for the LineIndex (which is unused when there are no labels).
    let (uri, source) = error.named_source().map_or_else(
        || (fallback_uri.clone(), ""),
        |ns| {
            let name = ns.name();
            let uri = Url::parse(name)
                .ok()
                .or_else(|| Url::from_file_path(name).ok())
                .unwrap_or_else(|| fallback_uri.clone());
            (uri, ns.inner().as_str())
        },
    );

    let diags = compile_error_to_diagnostics_in_source(diag, source, &uri, data);

    let mut grouped: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    grouped.insert(uri, diags);
    grouped
}

/// Structured payload for diagnostics whose quick fixes need typed fields.
///
/// The compiler error carries these fields as types; flattening them into
/// the rendered message and re-parsing it in `code_actions` was exactly the
/// string-convention round trip the project bans. `Diagnostic::data` rides
/// along with the diagnostic to the `textDocument/codeAction` request.
fn structured_data(error: &CompileError) -> Option<serde_json::Value> {
    use graphcal_compiler::registry::error::GraphcalError;
    let CompileError::Eval(e) = error else {
        return None;
    };
    match e {
        // V003: the private item that needs `pub`.
        GraphcalError::PrivateInPublic { ref_name, .. } => {
            Some(serde_json::json!({ "referencedName": ref_name }))
        }
        // V006: the leaked private item that needs `pub`.
        GraphcalError::GenericsLeakage { leaked_name, .. } => {
            Some(serde_json::json!({ "referencedName": leaked_name }))
        }
        // D020: an exact replacement exists only when the decimal spelling
        // maps exactly into the dimension rational model.
        GraphcalError::FloatPowerExponent {
            replacement: Some(replacement),
            ..
        } => Some(serde_json::json!({ "replacement": replacement })),
        GraphcalError::UnknownDimension { name, .. } => name
            .as_bare()
            .and_then(|name| auto_import_data(name.as_str(), AutoImportCategory::Dimension)),
        GraphcalError::UnknownUnit { name, .. } if !name.is_qualified() => {
            auto_import_data(name.name().as_str(), AutoImportCategory::Unit)
        }
        GraphcalError::UnknownIndex { name, .. } => {
            auto_import_data(name.as_str(), AutoImportCategory::Index)
        }
        GraphcalError::UnknownStructType { name, .. } => {
            auto_import_data(name, AutoImportCategory::Type)
        }
        GraphcalError::UnknownLocalRef { name, .. } => {
            auto_import_data(name, AutoImportCategory::Term)
        }
        GraphcalError::UnknownGraphRef { name, .. } if name.qualifier().is_empty() => {
            auto_import_data(name.member().as_str(), AutoImportCategory::Term)
        }
        _ => None,
    }
}

/// Build the LSP diagnostics for one error against a known (URI, source).
fn compile_error_to_diagnostics_in_source(
    diag: &dyn miette::Diagnostic,
    source: &str,
    uri: &Url,
    data: Option<serde_json::Value>,
) -> Vec<Diagnostic> {
    let message = format!("{diag}");
    let code = diag.code().map(|c| NumberOrString::String(c.to_string()));

    let help_suffix = diag
        .help()
        .map_or_else(String::new, |help| format!("\n\nhint: {help}"));

    let mut diagnostics = Vec::new();

    if let Some(labels) = diag.labels() {
        let labels: Vec<_> = labels.collect();

        if let Some(primary) = labels.first() {
            let lines = LineIndex::new(source);
            let primary_range = lines.offset_len_to_range(primary.offset(), primary.len());

            let primary_msg = primary.label().map_or_else(
                || format!("{message}{help_suffix}"),
                |l| format!("{message}: {l}{help_suffix}"),
            );

            let related: Vec<DiagnosticRelatedInformation> = labels[1..]
                .iter()
                .map(|label| DiagnosticRelatedInformation {
                    location: Location {
                        uri: uri.clone(),
                        range: lines.offset_len_to_range(label.offset(), label.len()),
                    },
                    message: label.label().unwrap_or("related location").to_string(),
                })
                .collect();

            diagnostics.push(Diagnostic {
                range: primary_range,
                severity: Some(DiagnosticSeverity::ERROR),
                code: code.clone(),
                source: Some("graphcal".to_string()),
                message: primary_msg,
                related_information: if related.is_empty() {
                    None
                } else {
                    Some(related)
                },
                data: data.clone(),
                ..Default::default()
            });
        }
    }

    if diagnostics.is_empty() {
        let range = if source.is_empty() {
            Range::default()
        } else {
            LineIndex::new(source).offset_len_to_range(0, source.len())
        };
        diagnostics.push(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            code,
            source: Some("graphcal".to_string()),
            message: format!("{message}{help_suffix}"),
            data,
            ..Default::default()
        });
    }

    diagnostics
}

#[cfg(test)]
fn compile_error_to_diagnostics(error: &CompileError) -> Vec<Diagnostic> {
    // Static URL literal — always parses.
    let fallback_uri = Url::parse("file:///unknown").unwrap();
    compile_error_to_diagnostics_grouped(error, &fallback_uri)
        .into_values()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
    use graphcal_compiler::registry::error::GraphcalError;
    use graphcal_compiler::syntax::names::{NameAtom, NamePath};
    use graphcal_compiler::syntax::non_empty::NonEmpty;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::syntax::span::Span;
    use graphcal_eval::eval::{compile_and_eval_named, compile_and_eval_project};
    use graphcal_io::RealFileSystem;
    use miette::NamedSource;
    use tower_lsp::lsp_types::Position;

    use super::*;
    use crate::symbol_table::build_for_buffer;

    fn build_symbol_table(source: &str) -> SymbolTable {
        let mut parser = Parser::new(source);
        parser
            .parse_file()
            .map(|raw_ast| {
                let desugared =
                    graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);
                let ast = desugared;
                build_for_buffer(&ast, source)
            })
            .unwrap_or_default()
    }

    fn produce_diagnostics(source: &str, name: &str) -> Vec<Diagnostic> {
        let symbol_table = build_symbol_table(source);
        match compile_and_eval_named(source, name) {
            Ok(result) => eval_result_to_diagnostics(&result, source, &symbol_table),
            Err(e) => compile_error_to_diagnostics(&e),
        }
    }

    #[test]
    fn source_less_internal_anchor_uses_the_whole_document_range() {
        let source = "node x";
        let named_source = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let error = CompileError::Eval(GraphcalError::internal_error(
            "synthetic failure",
            &named_source,
            DiagnosticAnchor::Builtin,
        ));

        let diagnostics = compile_error_to_diagnostics(&error);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range.start, Position::new(0, 0));
        assert_eq!(
            diagnostics[0].range.end,
            Position::new(0, u32::try_from(source.len()).unwrap())
        );
    }

    #[test]
    fn qualified_unknown_dimension_retains_its_path_without_auto_import_data() {
        let source = "missing.Dimension";
        let named_source = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let path = NamePath::new(NonEmpty::new(
            NameAtom::parse("missing").unwrap(),
            vec![NameAtom::parse("Dimension").unwrap()],
        ));
        let error = CompileError::Eval(GraphcalError::UnknownDimension {
            name: path,
            src: named_source,
            span: Span::new(0, source.len()).into(),
        });
        let diagnostics = compile_error_to_diagnostics(&error);
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("expected one unknown-dimension diagnostic");
        };

        assert!(diagnostic.message.contains("missing.Dimension"));
        assert_eq!(diagnostic.data, None);
    }

    #[test]
    fn bare_unknown_dimension_keeps_auto_import_data() {
        let diagnostics = produce_diagnostics("param value: Missing;", "test.gcl");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code.as_ref().is_some_and(
                    |code| matches!(code, NumberOrString::String(value) if value == "graphcal::D004"),
                )
            })
            .expect("expected unknown-dimension diagnostic");
        let auto_import: AutoImportDiagnosticData = serde_json::from_value(
            diagnostic
                .data
                .clone()
                .expect("expected bare-name auto-import payload"),
        )
        .unwrap();

        assert_eq!(auto_import.auto_import_name, "Missing");
        assert_eq!(
            auto_import.auto_import_category,
            AutoImportCategory::Dimension
        );
    }

    fn produce_diagnostics_for_file(path: &std::path::Path, source: &str) -> Vec<Diagnostic> {
        let symbol_table = build_symbol_table(source);
        match compile_and_eval_project(path, &HashMap::new(), None, &RealFileSystem::default()) {
            Ok(result) => eval_result_to_diagnostics(&result, source, &symbol_table),
            Err(e) => compile_error_to_diagnostics(&e),
        }
    }

    #[test]
    fn eval_error_range_points_at_declaration_name() {
        // `@bad` references a non-existent node; the E001 runtime error for
        // `broken` should point at `broken`'s name-span, not at line 0 col 0.
        let source = "\n\nnode broken: Dimensionless = 1.0 / 0.0;\n";
        let diags = produce_diagnostics(source, "test.gcl");
        let e001: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code.as_ref().is_some_and(
                    |c| matches!(c, NumberOrString::String(s) if s == "graphcal::E001"),
                )
            })
            .collect();
        assert!(!e001.is_empty(), "expected at least one E001 diagnostic");
        // The declaration is on line 2 (0-based), so the diagnostic must land
        // there rather than at the default (0, 0) range.
        assert_eq!(e001[0].range.start.line, 2);
        assert_ne!(e001[0].range, Range::default());
    }

    #[test]
    fn valid_source_produces_no_diagnostics() {
        let source = "param x: Dimensionless = 1.0;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(diags.is_empty());
    }

    #[test]
    fn invalid_base_unit_reports_code_and_name_range() {
        let diagnostics = produce_diagnostics("base unit furlong: Length;", "test.gcl");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code.as_ref().is_some_and(
                    |code| matches!(code, NumberOrString::String(value) if value == "graphcal::D036"),
                )
            })
            .expect("expected invalid-base-unit diagnostic");

        assert!(
            diagnostic
                .message
                .contains("base unit of dimension `Length`")
        );
        assert_eq!(diagnostic.range.start, Position::new(0, 10));
        assert_eq!(diagnostic.range.end, Position::new(0, 17));
    }

    #[test]
    fn reassigned_codes_survive_lsp_conversion_by_variant() {
        use std::sync::Arc;

        use graphcal_compiler::builtin::BuiltinFnName;
        use graphcal_compiler::datetime_literal::DatetimeLiteralExpectation;
        use graphcal_compiler::registry::error::GraphcalError;
        use graphcal_compiler::syntax::names::NameAtom;
        use miette::NamedSource;

        let src = || NamedSource::new("file:///test.gcl", Arc::new("x".to_string()));
        let span = || miette::SourceSpan::from((0, 1));
        let cases = [
            (
                GraphcalError::InvalidSourcePath {
                    path: "bad.txt".to_string(),
                    reason: "wrong extension".to_string(),
                },
                "graphcal::M023",
            ),
            (
                GraphcalError::AggregationCardinalityUnknown {
                    function: BuiltinFnName::Product,
                    src: src(),
                    span: span(),
                },
                "graphcal::D027",
            ),
            (
                GraphcalError::MaterializedShapeTooLarge {
                    maximum: 1_000_000,
                    src: src(),
                    span: span(),
                },
                "graphcal::D035",
            ),
            (
                GraphcalError::InvalidDatetimeLiteral {
                    expectation: DatetimeLiteralExpectation::OffsetDateTime,
                    reason: "invalid".to_string(),
                    src: src(),
                    span: span(),
                },
                "graphcal::D028",
            ),
            (
                GraphcalError::InvalidEpochTimeScaleArgument {
                    expected: "UTC".to_string(),
                    src: src(),
                    span: span(),
                },
                "graphcal::D029",
            ),
            (
                GraphcalError::UnsupportedEpochTimeScale {
                    name: NameAtom::parse("BAD").unwrap(),
                    expected: "UTC".to_string(),
                    src: src(),
                    span: span(),
                },
                "graphcal::D030",
            ),
        ];

        for (error, expected_code) in cases {
            let diagnostics = compile_error_to_diagnostics(&CompileError::Eval(error));
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert_eq!(
                diagnostics[0].code,
                Some(NumberOrString::String(expected_code.to_string()))
            );
        }
    }

    #[test]
    fn plot_encoding_type_and_axis_errors_retain_codes_and_ranges() {
        let cases = [
            (
                r"
type Pair { Pair(x: Dimensionless) }
node pair: Pair = Pair(x: 1.0);
plot p = { mark: point, encode: { x: @pair } };
",
                "graphcal::D033",
                3,
            ),
            (
                r"
index Step = { A, B };
index Pair = { Left, Right };
plot p = {
    mark: point,
    encode: {
        x: for step: Step { step },
        y: for pair: Pair { pair },
    },
};
",
                "graphcal::D034",
                7,
            ),
        ];

        for (source, expected_code, expected_line) in cases {
            let diagnostics = produce_diagnostics(source, "test.gcl");
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| {
                    matches!(
                        &diagnostic.code,
                        Some(NumberOrString::String(code)) if code == expected_code
                    )
                })
                .unwrap_or_else(|| panic!("missing {expected_code}: {diagnostics:?}"));
            assert_eq!(diagnostic.range.start.line, expected_line);
            assert_ne!(diagnostic.range, Range::default());
        }
    }

    #[test]
    fn generic_field_and_type_argument_constraints_retain_codes_and_ranges() {
        let cases = [
            (
                r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node bad: Box<Time> = Box<Time>(x: 1.0 s);
",
                "graphcal::C002",
                1,
            ),
            (
                r"
type Wrapper<T: Type> { Wrapper(value: T) }
type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
",
                "graphcal::C006",
                2,
            ),
            (
                r"
dag nested {
    type Wrapper<T: Type> { Wrapper(value: T) }
    type Bad { Bad(value: Wrapper<Length(min: 0.0 m)>) }
}
",
                "graphcal::C006",
                3,
            ),
        ];

        for (source, expected_code, expected_line) in cases {
            let diagnostics = produce_diagnostics(source, "test.gcl");
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| {
                    matches!(
                        &diagnostic.code,
                        Some(NumberOrString::String(code)) if code == expected_code
                    )
                })
                .unwrap_or_else(|| panic!("missing {expected_code}: {diagnostics:?}"));
            assert_eq!(diagnostic.range.start.line, expected_line);
            assert_ne!(diagnostic.range, Range::default());
        }
    }

    #[test]
    fn epoch_static_time_scale_argument_produces_no_diagnostics() {
        let source = "node t: Datetime<TT> = epoch<TT>(\"2024-11-05T12:00:00\");";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn datetime_domain_constraints_accept_matching_scales() {
        let source = r#"
param event: Datetime<TT>(
    min: epoch<TT>("2024-01-01T00:00:00"),
    max: epoch<TT>("2024-12-31T23:59:59"),
) = epoch<TT>("2024-06-01T00:00:00");
"#;
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn datetime_domain_scale_mismatch_has_a_specific_diagnostic() {
        let source = r#"
param event: Datetime<TT>(
    min: datetime("2024-01-01T00:00:00Z"),
) = epoch<TT>("2024-06-01T00:00:00");
"#;
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::C007"
            ) && diagnostic.message.contains("target is Datetime<TT>")
                && diagnostic.message.contains("min bound is Datetime")
        }));
    }

    #[test]
    fn invalid_datetime_literal_is_reported_during_checking() {
        let source = "node bad: Datetime = datetime(\"2024-11-05T12:00:00 TT\");";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::D028"
            ) && diagnostic.message.contains("invalid datetime literal")
        }));
    }

    #[test]
    fn date_only_civil_datetime_literals_require_an_explicit_time() {
        for source in [
            "node bad: Datetime<TT> = epoch<TT>(\"2024-11-05\");",
            "node bad: Datetime = datetime(\"2024-11-05\", \"UTC\");",
        ] {
            let diagnostics = produce_diagnostics(source, "test.gcl");
            assert!(diagnostics.iter().any(|diagnostic| {
                matches!(
                    &diagnostic.code,
                    Some(NumberOrString::String(code)) if code == "graphcal::D028"
                ) && diagnostic
                    .message
                    .contains("explicit local time is required")
            }));
        }
    }

    #[test]
    fn dst_gap_and_fold_have_distinct_check_diagnostics() {
        let cases = [
            (
                "node gap: Datetime = datetime(\"2024-03-10T02:30:00\", \"America/New_York\");",
                "graphcal::D024",
                "does not exist",
            ),
            (
                "node fold: Datetime = datetime(\"2024-11-03T01:30:00\", \"America/New_York\");",
                "graphcal::D025",
                "occurs twice",
            ),
        ];
        for (source, expected_code, expected_message) in cases {
            let diagnostics = produce_diagnostics(source, "test.gcl");
            assert!(diagnostics.iter().any(|diagnostic| {
                matches!(
                    &diagnostic.code,
                    Some(NumberOrString::String(code)) if code == expected_code
                ) && diagnostic.message.contains(expected_message)
            }));
        }
    }

    #[test]
    fn invalid_dynamic_unit_scale_has_type_diagnostic_at_scale_expression() {
        let source = "param factor: Bool = true;\nunit Bad: Length = (@factor) m;";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                matches!(
                    &diagnostic.code,
                    Some(NumberOrString::String(code)) if code == "graphcal::D032"
                )
            })
            .expect("expected D032 dynamic-unit diagnostic");
        assert!(diagnostic.message.contains("scalar Dimensionless"));
        assert_eq!(diagnostic.range.start.line, 1);
        assert_eq!(diagnostic.range.start.character, 20);
        assert_eq!(diagnostic.range.end.character, 27);
    }

    #[test]
    fn positional_epoch_scale_suggests_static_argument() {
        let source = "node bad: Datetime<TT> = epoch(\"2024-11-05T12:00:00\", TT);";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::D023"
            ) && diagnostic.message.contains("epoch<TT>")
        }));
    }

    #[test]
    fn invalid_datetime_timezone_points_at_the_timezone_argument() {
        let source = "node bad: Datetime = datetime(\"2024-11-05T10:00\", \"Not/A_Timezone\");";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                matches!(
                    &diagnostic.code,
                    Some(NumberOrString::String(code)) if code == "graphcal::D007"
                )
            })
            .expect("expected D007 timezone diagnostic");
        assert_eq!(diagnostic.range.start.character, 50);
        assert_eq!(diagnostic.range.end.character, 66);
        assert!(diagnostic.message.contains("bundled IANA tzdb"));
    }

    #[test]
    fn parse_error_produces_diagnostic() {
        let source = "param = ;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].source, Some("graphcal".to_string()));
    }

    #[test]
    fn duplicate_dag_binding_has_original_as_related_information() {
        let source = "node result: Dimensionless = @combine(a: 1.0, a: 2.0).out;";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(matches!(
            &diagnostics[0].code,
            Some(NumberOrString::String(code)) if code == "graphcal::P025"
        ));
        assert!(diagnostics[0].related_information.is_some());
    }

    #[test]
    fn duplicate_constructor_field_has_original_as_related_information() {
        let source = "pub type Pair {\n    Pair(\n        value: Length,\n        other: Bool,\n        value: Time,\n    )\n}";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert!(matches!(
            &diagnostic.code,
            Some(NumberOrString::String(code)) if code == "graphcal::N016"
        ));
        assert_eq!(diagnostic.range.start.line, 4);
        let related = diagnostic
            .related_information
            .as_ref()
            .expect("first field declaration should be related information");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.range.start.line, 2);
    }

    #[test]
    fn duplicate_extern_param_has_original_as_related_information() {
        let source = concat!(
            "import plugin \"graphcal:demo\" as demo {\n",
            "    fn lerp<D: Dim>(a: D, a: D, t: Dimensionless) -> D;\n",
            "}\n",
        );
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(matches!(
            &diagnostics[0].code,
            Some(NumberOrString::String(code)) if code == "graphcal::P011"
        ));
        assert!(diagnostics[0].related_information.is_some());
    }

    #[test]
    fn sort_aware_nat_generic_arguments_produce_no_diagnostics() {
        let source = "pub type Fixed<N: Nat> { Fixed(value: Dimensionless) }\n\
                      param x: Fixed<3> = Fixed<3>(value: 1.0);";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn indexed_comparison_produces_explicit_for_diagnostic() {
        let source = "index Case = { A, B };\n\
                      node values: Length[Case] = { Case.A: 1.0 m, Case.B: 2.0 m };\n\
                      node bad: Bool[Case] = @values < 3.0 m;";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::D019"
            ) && diagnostic.message.contains("explicit `for` comprehension")
        }));
    }

    #[test]
    fn unfold_self_reference_produces_cycle_diagnostic() {
        let source = "index Step = range(0.0 s, 1.0 s, step: 1.0 s);\n\
                      node y: Dimensionless[Step] = unfold(\n\
                          Step,\n\
                          0.0,\n\
                          |prev_y, prev_t, t| @y[t] + 1.0\n\
                      );";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::G001"
            )
        }));
    }

    #[test]
    fn multi_axis_count_produces_rank_diagnostic() {
        let source = "index Row = { A, B };\n\
                      index Column = { X, Y };\n\
                      node matrix: Bool[Row, Column] = for row: Row, column: Column { true };\n\
                      node n: Int = count(@matrix);";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::D021"
            ) && diagnostic.message.contains("rank-2")
        }));
    }

    #[test]
    fn float_power_on_dimensioned_base_produces_exact_syntax_diagnostic() {
        let source = "param x: Length = 1.0 m;\nnode bad: Length^(1/4) = @x ^ 0.25;";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::D020"
            ) && diagnostic.message.contains("`(1/4)`")
                && diagnostic.data == Some(serde_json::json!({ "replacement": "(1/4)" }))
        }));
    }

    #[test]
    fn nat_subtraction_produces_targeted_parse_diagnostic() {
        let diagnostics = produce_diagnostics("param x: Dimensionless[N - 1];", "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::P022"
            )
        }));
    }

    #[test]
    fn bare_nat_table_axis_suggests_fin() {
        let diagnostics = produce_diagnostics(
            "param x: Dimensionless[Fin(2)] = table[2] { 1.0; 2.0; };",
            "test.gcl",
        );
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::P023"
            ) && diagnostic.message.contains("Fin(2)")
        }));
    }

    #[test]
    fn structural_range_binding_suggests_fin() {
        let diagnostics = produce_diagnostics(
            "node x: Dimensionless[Fin(2)] = for i: range(2) { 1.0 };",
            "test.gcl",
        );
        assert!(diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.code,
                Some(NumberOrString::String(code)) if code == "graphcal::P024"
            )
        }));
    }

    #[test]
    fn function_generic_arguments_produce_a_diagnostic() {
        let diagnostics = produce_diagnostics("node x: Dimensionless = sqrt<3>(4.0);", "test.gcl");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("function `sqrt` does not accept generic arguments")
        }));
    }

    #[test]
    fn named_arguments_on_builtin_produce_positional_call_diagnostic() {
        let source = "node angle: Angle = atan2(y: 1.0 m, x: 2.0 m);";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert!(matches!(
            &diagnostic.code,
            Some(NumberOrString::String(code)) if code == "graphcal::N015"
        ));
        assert!(
            diagnostic
                .message
                .contains("function `atan2` uses positional arguments")
        );
        assert!(
            diagnostic
                .message
                .contains("write `atan2(y_value, x_value)`")
        );
        assert_eq!(diagnostic.range.start.line, 0);
        assert_eq!(diagnostic.range.start.character, 20);
        assert_eq!(diagnostic.range.end.character, 45);
    }

    #[test]
    fn named_arguments_on_extern_produce_positional_call_diagnostic() {
        let source = concat!(
            "import plugin \"graphcal:demo\" as demo {\n",
            "    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;\n",
            "}\n",
            "node x: Dimensionless = demo.lerp(a: 1.0, b: 2.0, t: 0.5);",
        );
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert!(matches!(
            &diagnostic.code,
            Some(NumberOrString::String(code)) if code == "graphcal::N015"
        ));
        assert!(
            diagnostic
                .message
                .contains("function `demo.lerp` uses positional arguments")
        );
        assert!(
            diagnostic
                .message
                .contains("write `demo.lerp(a_value, b_value, t_value)`")
        );
        assert_eq!(diagnostic.range.start.line, 3);
        assert_eq!(diagnostic.range.start.character, 24);
        assert_eq!(diagnostic.range.end.character, 57);
    }

    #[test]
    fn unknown_ref_produces_diagnostic() {
        let source = "node x: Dimensionless = @nonexistent;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(!diags.is_empty());
        let code = diags[0].code.as_ref();
        assert!(
            code.is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("N002"))),
            "expected N002 error code, got {code:?}"
        );
        let auto_import: AutoImportDiagnosticData =
            serde_json::from_value(diags[0].data.clone().expect("auto-import payload")).unwrap();
        assert_eq!(auto_import.auto_import_name, "nonexistent");
        assert_eq!(auto_import.auto_import_category, AutoImportCategory::Term);
    }

    #[test]
    fn bare_graph_declaration_ref_points_at_missing_sigil() {
        let source = "node target: Dimensionless = 1.0;\n\
                      node output: Dimensionless = target;";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("expected one diagnostic, got {diagnostics:?}");
        };

        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("graphcal::N017".to_string()))
        );
        assert!(diagnostic.message.contains("write `@target`"));
        assert_eq!(diagnostic.range.start.line, 1);
        assert_eq!(diagnostic.range.start.character, 29);
        assert_eq!(diagnostic.range.end.character, 35);
    }

    #[test]
    fn passing_assertion_produces_no_diagnostic() {
        let source = "param x: Dimensionless = 1.0;\nassert x_pos = @x > 0.0;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(diags.is_empty());
    }

    #[test]
    fn failing_assertion_produces_warning() {
        let source = "param x: Dimensionless = 10.0;\nparam y: Dimensionless = 20.0;\nassert x_greater = @x > @y;";
        let diags = produce_diagnostics(source, "test.gcl");
        let assert_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code.as_ref().is_some_and(
                    |c| matches!(c, NumberOrString::String(s) if s == "graphcal::A001"),
                )
            })
            .collect();
        assert_eq!(assert_diags.len(), 1);
        assert_eq!(assert_diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(assert_diags[0].message.contains("x_greater"));
        assert!(assert_diags[0].message.contains("failed"));
        // Verify span is non-default (points to the assert declaration)
        assert_ne!(assert_diags[0].range, Range::default());
    }

    #[test]
    fn failing_assertion_with_assumes_mentions_affected() {
        let source = concat!(
            "param pressure: Dimensionless = 200.0;\n",
            "param max_pressure: Dimensionless = 150.0;\n",
            "assert pressure_safe = @pressure < @max_pressure;\n",
            "#[assumes(pressure_safe)]\n",
            "node margin: Dimensionless = @max_pressure - @pressure;\n",
        );
        let diags = produce_diagnostics(source, "test.gcl");
        let assert_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code.as_ref().is_some_and(
                    |c| matches!(c, NumberOrString::String(s) if s == "graphcal::A001"),
                )
            })
            .collect();
        assert_eq!(assert_diags.len(), 1);
        assert!(
            assert_diags[0].message.contains("margin"),
            "expected diagnostic to mention affected node 'margin', got: {}",
            assert_diags[0].message
        );
    }
    #[test]
    fn v001_import_private_item_produces_diagnostic() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/invalid/multi/import_private_item/src/lib/main.gcl");
        let source = std::fs::read_to_string(&root).unwrap();
        let diags = produce_diagnostics_for_file(&root, &source);
        assert_eq!(diags.len(), 1);
        let code = diags[0].code.as_ref();
        assert!(
            code.is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("V001"))),
            "expected V001 error code, got {code:?}"
        );
        assert!(diags[0].message.contains("secret"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn m022_wrong_import_category_points_at_item_and_lists_all_alternatives() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/lib.gcl"),
            "pub const node JPY: Length = 1.0 m;\n\
             pub const unit JPY: Length = 1.0 m;\n",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");
        let source = "import app.lib.{ dim JPY };\n";
        std::fs::write(&main_path, source).unwrap();

        let diagnostics = produce_diagnostics_for_file(&main_path, source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert!(matches!(
            diagnostic.code.as_ref(),
            Some(NumberOrString::String(code)) if code == "graphcal::M022"
        ));
        assert!(
            diagnostic
                .message
                .contains("did you mean `JPY` or `unit JPY`?")
        );
        assert_eq!(diagnostic.range.start.line, 0);
        assert!(diagnostic.range.start.character > 0, "{diagnostic:?}");
    }

    #[test]
    fn runtime_module_boundary_diagnostics_reach_lsp_clients() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/lib.gcl"),
            "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\npub assert positive = @rate > 0.0;\n",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");

        for (source, expected) in [
            ("import app.lib.{ positive };\n", "graphcal::M024"),
            (
                "import app.lib as lib;\nnode price: lib.Money = 1.0 lib.EUR;\n",
                "graphcal::M025",
            ),
            ("include app.lib() as lib;\n", "graphcal::M026"),
        ] {
            std::fs::write(&main_path, source).unwrap();
            let diagnostics = produce_diagnostics_for_file(&main_path, source);
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert!(matches!(
                diagnostics[0].code.as_ref(),
                Some(NumberOrString::String(code)) if code == expected
            ));
            assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        }
    }

    #[test]
    fn inline_self_import_policy_diagnostics_reach_lsp_clients() {
        for (source, expected) in [
            (
                "pub assert okay = true;\n\
                 dag calculation { import self.{ okay }; }\n",
                "graphcal::M024",
            ),
            (
                "pub plot chart = { mark: point, encode: { x: 1.0 } };\n\
                 dag calculation { import self.{ chart }; }\n",
                "graphcal::M021",
            ),
        ] {
            let diagnostics = produce_diagnostics(source, "self.gcl");
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert!(matches!(
                diagnostics[0].code.as_ref(),
                Some(NumberOrString::String(code)) if code == expected
            ));
            assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        }
    }

    #[test]
    fn pub_param_produces_parse_diagnostic() {
        // `pub param` is rejected at parse time with a P001 unexpected-token
        // diagnostic — params are annotation-free per axioms §4.0.
        let source = "pub param x: Dimensionless = 1.0;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        let code = diags[0].code.as_ref();
        assert!(
            code.is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("P001"))),
            "expected P001 parse error code, got {code:?}"
        );
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn empty_required_bracket_and_angle_lists_produce_parse_diagnostics() {
        for source in [
            "param value: Dimensionless[];",
            "param value: Wrapper<>;",
            "node value: Dimensionless = sqrt<>(4.0);",
            "type Marker<> { Marker }",
            "node value: Dimensionless = @values[];",
        ] {
            let diagnostics = produce_diagnostics(source, "test.gcl");
            assert!(
                diagnostics.iter().any(|diagnostic| matches!(
                    diagnostic.code.as_ref(),
                    Some(NumberOrString::String(code)) if code == "graphcal::P001"
                )),
                "expected P001 for `{source}`, got {diagnostics:?}"
            );
        }
    }

    #[test]
    fn inline_dag_param_projection_produces_no_diagnostic() {
        let source = "\
dag config {
    param factor: Dimensionless = 2.0;
}
node factor: Dimensionless = @config().factor;
";
        assert!(produce_diagnostics(source, "test.gcl").is_empty());
    }

    #[test]
    fn finite_index_include_binding_produces_no_diagnostic() {
        let source = r"
dag scale {
    pub(bind) index Axis;
    param xs: Dimensionless[Axis];
    pub node ys: Dimensionless[Axis] = for i: Axis { @xs[i] * 2.0 };
}
include scale(
    Axis: Fin(3),
    xs: table[Fin(3)] { 1.0; 2.0; 3.0; },
) as scaled;
node out: Dimensionless[Fin(3)] = @scaled.ys;
";
        assert!(produce_diagnostics(source, "test.gcl").is_empty());
    }

    #[test]
    fn i009_inline_dag_coordinate_index_binding_produces_diagnostic() {
        let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include pass_through(
    Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let code = diagnostics[0].code.as_ref();
        assert!(
            code.is_some_and(|code| {
                matches!(code, NumberOrString::String(value) if value.contains("I009"))
            }),
            "expected I009 error code, got {code:?}"
        );
        assert!(diagnostics[0].message.contains("Time"));
        assert!(diagnostics[0].message.contains("Length"));
    }

    #[test]
    fn v002_required_index_not_pub_produces_diagnostic() {
        let source = "index Phase;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert_eq!(diags.len(), 1);
        let code = diags[0].code.as_ref();
        assert!(
            code.is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("V002"))),
            "expected V002 error code, got {code:?}"
        );
        assert!(diags[0].message.contains("index"));
    }

    #[test]
    fn v003_private_in_public_produces_diagnostic() {
        let source = "dim Speed = Length / Time;\nparam kmh: Speed = 36.0 km/h;\npub node speed: Speed = @kmh;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert_eq!(diags.len(), 1);
        let code = diags[0].code.as_ref();
        assert!(
            code.is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("V003"))),
            "expected V003 error code, got {code:?}"
        );
        assert!(diags[0].message.contains("Speed"));
        // V003 has related information pointing to the pub declaration
        assert!(diags[0].related_information.is_some());
    }

    #[test]
    fn private_generic_default_produces_v003_diagnostic() {
        let source =
            "type Secret { Secret }\npub type Wrapper<T: Type = Secret> { Wrapper(value: T) }";
        let diagnostics = produce_diagnostics(source, "test.gcl");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(matches!(
            &diagnostics[0].code,
            Some(NumberOrString::String(code)) if code == "graphcal::V003"
        ));
        assert!(diagnostics[0].message.contains("Secret"));
        assert_eq!(diagnostics[0].range.start.line, 1);
    }

    #[test]
    fn v004_pub_bind_index_variant_literal_produces_diagnostic() {
        // V004 fires on `node` / `const` bodies that mention a
        // `pub(bind)` index's variant literal (A10(c)). Param defaults
        // are OK because `param` directly declares an input port.
        let source = concat!(
            "pub(bind) index Phase = { Design, Test };\n",
            "param x: Dimensionless[Phase] = { Phase.Design: 1.0, Phase.Test: 2.0 };\n",
            "node design: Dimensionless = @x[Phase.Design];\n",
        );
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(!diags.is_empty());
        let has_v004 = diags.iter().any(|d| {
            d.code
                .as_ref()
                .is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("V004")))
        });
        assert!(
            has_v004,
            "expected at least one V004 diagnostic, got: {diags:?}"
        );
    }
}
