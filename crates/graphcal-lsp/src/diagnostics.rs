//! Diagnostic production from compile errors and evaluation results.

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Range,
    Url,
};

use graphcal_eval::eval::CompileError;

use crate::convert::{offset_len_to_range, span_to_range};
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
    // Node/param evaluation errors
    let mut diagnostics: Vec<Diagnostic> = result
        .all
        .iter()
        .filter_map(|(name, r, _)| match r {
            Err(err) => {
                let range = symbol_table
                    .definitions
                    .get(&SymbolKey::TopLevel(name.as_str().to_string()))
                    .map_or_else(Range::default, |def| span_to_range(source, def.name_span));
                Some(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String("graphcal::E001".to_string())),
                    source: Some("graphcal".to_string()),
                    message: format!("{}: {err}", name.as_str()),
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
                        let msg = result.assumes_map.get(name.as_str()).map_or_else(
                            || format!("assertion `{}` failed: {message}", name.as_str()),
                            |affected| {
                                format!(
                                    "assertion `{}` failed: {message} (affected: {})",
                                    name.as_str(),
                                    affected.join(", ")
                                )
                            },
                        );
                        (msg, DiagnosticSeverity::WARNING)
                    }
                    AssertResult::Error { message } => (
                        format!("assertion `{}` error: {message}", name.as_str()),
                        DiagnosticSeverity::WARNING,
                    ),
                };

                Some(Diagnostic {
                    range: span_to_range(source, *span),
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

/// Convert a `CompileError` to a list of LSP diagnostics using the miette `Diagnostic` trait.
///
/// When an error has multiple labeled spans (e.g., "duplicate definition here" +
/// "first defined here"), the first label becomes the primary diagnostic and the
/// remaining labels are attached as `DiagnosticRelatedInformation`.
pub fn compile_error_to_diagnostics(error: &CompileError, source: &str) -> Vec<Diagnostic> {
    let diag: &dyn miette::Diagnostic = match error {
        CompileError::Parse(e) => e,
        CompileError::Eval(e) => e,
    };

    let message = format!("{diag}");
    let code = diag.code().map(|c| NumberOrString::String(c.to_string()));

    let help_suffix = diag
        .help()
        .map_or_else(String::new, |help| format!("\n\nhint: {help}"));

    // Use a synthetic URI for related information locations.
    // The source name from miette is embedded in the error, but we use a
    // placeholder URI since all spans are within the same document.
    let doc_uri = diag
        .source_code()
        .and_then(|sc| sc.read_span(&(0, 0).into(), 0, 0).ok())
        .and_then(|data| {
            let name = data.name()?;
            Url::parse(name)
                .ok()
                .or_else(|| Url::from_file_path(name).ok())
        })
        .unwrap_or_else(|| {
            // "file:///unknown" is a valid static URL; this parse cannot fail.
            #[expect(clippy::unwrap_used, reason = "static URL literal is always valid")]
            Url::parse("file:///unknown").unwrap()
        });

    let mut diagnostics = Vec::new();

    if let Some(labels) = diag.labels() {
        let labels: Vec<_> = labels.collect();

        if let Some(primary) = labels.first() {
            let primary_range = offset_len_to_range(source, primary.offset(), primary.len());

            let primary_msg = primary.label().map_or_else(
                || format!("{message}{help_suffix}"),
                |l| format!("{message}: {l}{help_suffix}"),
            );

            // Remaining labels become related information.
            let related: Vec<DiagnosticRelatedInformation> = labels[1..]
                .iter()
                .map(|label| DiagnosticRelatedInformation {
                    location: Location {
                        uri: doc_uri.clone(),
                        range: offset_len_to_range(source, label.offset(), label.len()),
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
                ..Default::default()
            });
        }
    }

    // Fallback: error with no labeled spans → report at start of file
    if diagnostics.is_empty() {
        diagnostics.push(Diagnostic {
            range: Range::default(),
            severity: Some(DiagnosticSeverity::ERROR),
            code,
            source: Some("graphcal".to_string()),
            message: format!("{message}{help_suffix}"),
            ..Default::default()
        });
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use std::collections::HashMap;

    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_eval::eval::{compile_and_eval_named, compile_and_eval_project};
    use graphcal_io::RealFileSystem;

    use super::*;
    use crate::symbol_table::build_from_ast;

    fn build_symbol_table(source: &str) -> SymbolTable {
        let mut parser = Parser::new(source);
        parser
            .parse_file()
            .map(|ast| build_from_ast(&ast, source))
            .unwrap_or_default()
    }

    fn produce_diagnostics(source: &str, name: &str) -> Vec<Diagnostic> {
        let symbol_table = build_symbol_table(source);
        match compile_and_eval_named(source, name) {
            Ok(result) => eval_result_to_diagnostics(&result, source, &symbol_table),
            Err(e) => compile_error_to_diagnostics(&e, source),
        }
    }

    fn produce_diagnostics_for_file(path: &std::path::Path, source: &str) -> Vec<Diagnostic> {
        let symbol_table = build_symbol_table(source);
        match compile_and_eval_project(
            path,
            &HashMap::new(),
            None,
            true,
            &RealFileSystem::default(),
        ) {
            Ok(result) => eval_result_to_diagnostics(&result, source, &symbol_table),
            Err(e) => compile_error_to_diagnostics(&e, source),
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
    fn multi_file_project_no_false_errors() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/multi/rocket_split/main.gcl");
        let source = std::fs::read_to_string(&root).unwrap();
        let diags = produce_diagnostics_for_file(&root, &source);
        assert!(
            diags.is_empty(),
            "expected no diagnostics for multi-file project, got: {diags:?}"
        );
    }

    #[test]
    fn v001_import_private_item_produces_diagnostic() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/multi/import_private_item/main.gcl");
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
        let source = "dim Velocity = Length / Time;\nparam kmh: Velocity = 36.0 km/h;\npub node speed: Velocity = @kmh;";
        let diags = produce_diagnostics(source, "test.gcl");
        assert_eq!(diags.len(), 1);
        let code = diags[0].code.as_ref();
        assert!(
            code.is_some_and(|c| matches!(c, NumberOrString::String(s) if s.contains("V003"))),
            "expected V003 error code, got {code:?}"
        );
        assert!(diags[0].message.contains("Velocity"));
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

    #[test]
    fn imported_assertion_failure_span_points_to_import() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/multi/imported_assert_fail/main.gcl");
        let source = std::fs::read_to_string(&root).unwrap();
        let diags = produce_diagnostics_for_file(&root, &source);
        let assert_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code.as_ref().is_some_and(
                    |c| matches!(c, NumberOrString::String(s) if s == "graphcal::A001"),
                )
            })
            .collect();
        assert_eq!(
            assert_diags.len(),
            1,
            "expected exactly 1 assertion diagnostic, got: {assert_diags:?}"
        );
        // The span should point to the import statement on line 1 of the root file
        // (line 0 is the `include` for the runtime value, line 1 is the `import` for the assert),
        // not to the assertion declaration in the dependency file.
        assert_eq!(
            assert_diags[0].range.start.line, 1,
            "expected diagnostic span on line 1 (the import statement), got line {}",
            assert_diags[0].range.start.line
        );
        // The span should be within the root file's source (valid byte offsets).
        let end_offset = assert_diags[0].range.end;
        assert!(
            (end_offset.line as usize) < source.lines().count(),
            "diagnostic end line {} exceeds root file line count {}",
            end_offset.line,
            source.lines().count()
        );
    }
}
