//! Diagnostic production from compile errors and evaluation results.

use std::collections::HashMap;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};

use graphcal_eval::eval::{CompileError, compile_and_eval_named, compile_and_eval_project};

use crate::convert::byte_offset_to_position;

/// Run project evaluation for a file on disk.
///
/// When the file is on disk, `compile_and_eval_project` is used so that `use` imports
/// are resolved correctly.
pub fn produce_diagnostics_for_file(path: &std::path::Path, source: &str) -> Vec<Diagnostic> {
    match compile_and_eval_project(path, &HashMap::new()) {
        Ok(result) => eval_result_to_diagnostics(&result),
        Err(e) => compile_error_to_diagnostics(&e, source),
    }
}

/// Run `compile_and_eval_named` and convert any errors to LSP diagnostics.
pub fn produce_diagnostics(source: &str, name: &str) -> Vec<Diagnostic> {
    match compile_and_eval_named(source, name) {
        Ok(result) => eval_result_to_diagnostics(&result),
        Err(e) => compile_error_to_diagnostics(&e, source),
    }
}

/// Convert per-node runtime errors in an `EvalResult` to LSP diagnostics.
fn eval_result_to_diagnostics(result: &graphcal_eval::eval::EvalResult) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (name, r, _) in &result.all {
        if let Err(err) = r {
            diagnostics.push(Diagnostic {
                range: Range::default(),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("graphcal::E001".to_string())),
                source: Some("graphcal".to_string()),
                message: format!("{}: {err}", name.as_str()),
                ..Default::default()
            });
        }
    }
    diagnostics
}

/// Convert a `CompileError` to a list of LSP diagnostics using the miette `Diagnostic` trait.
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

    let mut diagnostics = Vec::new();

    if let Some(labels) = diag.labels() {
        for label in labels {
            let start = byte_offset_to_position(source, label.offset());
            let end = byte_offset_to_position(source, label.offset() + label.len());

            let label_msg = label.label().map_or_else(
                || format!("{message}{help_suffix}"),
                |l| format!("{message}: {l}{help_suffix}"),
            );

            diagnostics.push(Diagnostic {
                range: Range { start, end },
                severity: Some(DiagnosticSeverity::ERROR),
                code: code.clone(),
                source: Some("graphcal".to_string()),
                message: label_msg,
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
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

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
}
