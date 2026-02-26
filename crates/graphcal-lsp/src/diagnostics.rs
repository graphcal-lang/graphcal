//! Diagnostic production from compile errors and evaluation results.

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Range,
    Url,
};

use graphcal_eval::eval::CompileError;

use crate::convert::byte_offset_to_position;

/// Convert per-node runtime errors and assertion failures in an `EvalResult` to LSP diagnostics.
pub fn eval_result_to_diagnostics(
    result: &graphcal_eval::eval::EvalResult,
    source: &str,
) -> Vec<Diagnostic> {
    // Node/param evaluation errors
    let mut diagnostics: Vec<Diagnostic> = result
        .all
        .iter()
        .filter_map(|(name, r, _)| match r {
            Err(err) => Some(Diagnostic {
                range: Range::default(),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("graphcal::E001".to_string())),
                source: Some("graphcal".to_string()),
                message: format!("{}: {err}", name.as_str()),
                ..Default::default()
            }),
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

                let start = byte_offset_to_position(source, span.offset());
                let end = byte_offset_to_position(source, span.offset() + span.len());

                Some(Diagnostic {
                    range: Range { start, end },
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
            let primary_start = byte_offset_to_position(source, primary.offset());
            let primary_end = byte_offset_to_position(source, primary.offset() + primary.len());

            let primary_msg = primary.label().map_or_else(
                || format!("{message}{help_suffix}"),
                |l| format!("{message}: {l}{help_suffix}"),
            );

            // Remaining labels become related information.
            let related: Vec<DiagnosticRelatedInformation> = labels[1..]
                .iter()
                .map(|label| {
                    let start = byte_offset_to_position(source, label.offset());
                    let end = byte_offset_to_position(source, label.offset() + label.len());
                    DiagnosticRelatedInformation {
                        location: Location {
                            uri: doc_uri.clone(),
                            range: Range { start, end },
                        },
                        message: label.label().unwrap_or("related location").to_string(),
                    }
                })
                .collect();

            diagnostics.push(Diagnostic {
                range: Range {
                    start: primary_start,
                    end: primary_end,
                },
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

    use graphcal_eval::eval::{compile_and_eval_named, compile_and_eval_project};

    use super::*;

    fn produce_diagnostics(source: &str, name: &str) -> Vec<Diagnostic> {
        match compile_and_eval_named(source, name) {
            Ok(result) => eval_result_to_diagnostics(&result, source),
            Err(e) => compile_error_to_diagnostics(&e, source),
        }
    }

    fn produce_diagnostics_for_file(path: &std::path::Path, source: &str) -> Vec<Diagnostic> {
        match compile_and_eval_project(path, &HashMap::new(), None, true) {
            Ok(result) => eval_result_to_diagnostics(&result, source),
            Err(e) => compile_error_to_diagnostics(&e, source),
        }
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
        // The span should point to the import statement on line 0 of the root file,
        // not to the assertion declaration in the dependency file.
        assert_eq!(
            assert_diags[0].range.start.line, 0,
            "expected diagnostic span on line 0 (the import statement), got line {}",
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
