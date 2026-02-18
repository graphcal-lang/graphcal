//! LSP server backend: state management and `LanguageServer` trait implementation.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, MessageType, OneOf, SaveOptions, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use graphcal_eval::eval::{compile_to_tir, compile_to_tir_project};

use crate::convert::position_to_byte_offset;
use crate::diagnostics::compile_error_to_diagnostics;
use crate::symbol_table::{self, SymbolTable};

/// Cached analysis result for a document.
pub struct AnalysisResult {
    /// The raw source text.
    pub source: String,
    /// The symbol table (built from AST, enriched from TIR if available).
    pub symbol_table: SymbolTable,
    /// Diagnostics to publish.
    pub diagnostics: Vec<Diagnostic>,
}

/// The LSP server backend.
#[derive(Debug)]
pub struct Backend {
    client: Client,
    /// Per-document analysis results, keyed by URI.
    documents: Arc<RwLock<HashMap<Url, AnalysisResult>>>,
}

impl std::fmt::Debug for AnalysisResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisResult")
            .field("source_len", &self.source.len())
            .field("symbol_table_defs", &self.symbol_table.definitions.len())
            .field("diagnostics_count", &self.diagnostics.len())
            .finish()
    }
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

        let analysis = run_analysis(&uri, &text);

        let diagnostics = analysis.diagnostics.clone();
        self.documents.write().await.insert(uri.clone(), analysis);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

/// Run the analysis pipeline, producing an `AnalysisResult`.
fn run_analysis(uri: &Url, text: &str) -> AnalysisResult {
    let name = uri.as_str();

    // Try to parse and compile to TIR.
    let tir_result = uri.to_file_path().map_or_else(
        |()| compile_to_tir(text, name).map(|tir| (tir, None)),
        |path| compile_to_tir_project(&path).map(|(tir, project)| (tir, Some(project))),
    );

    match tir_result {
        Ok((tir, _project)) => {
            // Full pipeline succeeded. Build symbol table from the AST embedded in the TIR,
            // then enrich with type info.
            // We need to re-parse to get the AST since TIR doesn't store it directly.
            // However, we know parsing succeeded (since TIR was produced).
            let ast = graphcal_syntax::parser::Parser::with_name(text, name)
                .parse_file()
                .expect("parse should succeed since TIR was produced");
            let mut symbol_table = symbol_table::build_from_ast(&ast);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir);

            // No compile errors, but check for eval warnings by running the full pipeline.
            let diagnostics = uri.to_file_path().map_or_else(
                |()| crate::diagnostics::produce_diagnostics(text, name),
                |path| crate::diagnostics::produce_diagnostics_for_file(&path, text),
            );

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                diagnostics,
            }
        }
        Err(e) => {
            // Compilation failed. Try to parse for a partial symbol table.
            let symbol_table = graphcal_syntax::parser::Parser::with_name(text, name)
                .parse_file()
                .map(|ast| symbol_table::build_from_ast(&ast))
                .unwrap_or_default();

            let diagnostics = compile_error_to_diagnostics(&e, text);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                diagnostics,
            }
        }
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
                document_symbol_provider: Some(OneOf::Left(true)),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
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

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let result = crate::document_symbols::build_document_symbols(analysis);
        drop(docs);
        Ok(Some(DocumentSymbolResponse::Nested(result)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::goto_definition::goto_definition(analysis, &uri, offset);
        drop(docs);
        Ok(result)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::hover::hover(analysis, offset);
        drop(docs);
        Ok(result)
    }
}

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(RwLock::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
