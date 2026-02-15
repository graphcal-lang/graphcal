//! Graphcal Language Server Protocol implementation.

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, MessageType,
    NumberOrString, Position, Range, SaveOptions, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use std::collections::HashMap;

use graphcal_eval::eval::{CompileError, compile_and_eval_named, compile_and_eval_project};

#[derive(Debug)]
struct Backend {
    client: Client,
}

impl Backend {
    fn is_graphcal_file(uri: &Url) -> bool {
        std::path::Path::new(uri.path())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gcl"))
    }

    async fn analyze_and_publish(&self, uri: Url, text: String) {
        if !Self::is_graphcal_file(&uri) {
            return;
        }
        let diagnostics = uri.to_file_path().map_or_else(
            |()| produce_diagnostics(&text, uri.as_str()),
            |path| produce_diagnostics_for_file(&path, &text),
        );
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "graphcal-lsp initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.analyze_and_publish(params.text_document.uri, params.text_document.text)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            self.analyze_and_publish(params.text_document.uri, change.text)
                .await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.analyze_and_publish(params.text_document.uri, text)
                .await;
        }
    }
}

/// Convert a byte offset in `source` to an LSP `Position` (0-based line and character).
fn byte_offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position {
        line,
        character: col,
    }
}

/// Run project evaluation for a file on disk, falling back to single-file evaluation.
///
/// When the file is on disk, `compile_and_eval_project` is used so that `use` imports
/// are resolved correctly. If project loading fails (e.g. file not yet saved),
/// we fall back to single-file evaluation.
fn produce_diagnostics_for_file(path: &std::path::Path, source: &str) -> Vec<Diagnostic> {
    match compile_and_eval_project(path, &HashMap::new()) {
        Ok(result) => eval_result_to_diagnostics(&result),
        Err(e) => compile_error_to_diagnostics(&e, source),
    }
}

/// Run `compile_and_eval_named` and convert any errors to LSP diagnostics.
fn produce_diagnostics(source: &str, name: &str) -> Vec<Diagnostic> {
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
fn compile_error_to_diagnostics(error: &CompileError, source: &str) -> Vec<Diagnostic> {
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

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend { client });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn position_at_start() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn position_mid_first_line() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 3);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn position_start_second_line() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 6);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn position_mid_second_line() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 8);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 2);
    }

    #[test]
    fn position_past_end_clamps() {
        let source = "hi";
        let pos = byte_offset_to_position(source, 100);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 2);
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
