//! Diagnostic production from compile errors and evaluation results.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Range,
    Url,
};

use graphcal_eval::eval::CompileError;

use crate::convert::LineIndex;
use crate::symbol_table::{SymbolKey, SymbolTable};

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
                    .definitions
                    .get(&SymbolKey::from_scoped_name(name))
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
    let diag: &dyn miette::Diagnostic = match error {
        CompileError::Parse(e) => e,
        CompileError::Eval(e) => e,
    };
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
        diagnostics.push(Diagnostic {
            range: Range::default(),
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

    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_eval::eval::{compile_and_eval_named, compile_and_eval_project};
    use graphcal_io::RealFileSystem;

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
    fn parse_error_produces_diagnostic() {
        let source = "param = ;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].source, Some("graphcal".to_string()));
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
    fn v004_pub_bind_index_variant_literal_produces_diagnostic() {
        // V004 fires on `node` / `const` bodies that mention a
        // `pub(bind)` index's variant literal (A10(c)). Param defaults
        // are OK because `param` is implicitly bindable.
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
