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
use graphcal_syntax::ast::DeclKind;

use crate::convert::position_to_byte_offset;
use crate::diagnostics::compile_error_to_diagnostics;
use crate::symbol_table::{self, DefinitionInfo, SymbolTable};

/// A definition from an imported file, for cross-file go-to-definition and hover.
pub struct ImportedDefinition {
    /// URI of the file containing the definition.
    pub uri: Url,
    /// Source text of the imported file (needed for span-to-range conversion).
    pub source: String,
    /// The definition info (name, category, spans, type description).
    pub definition: DefinitionInfo,
}

/// Cached analysis result for a document.
pub struct AnalysisResult {
    /// The raw source text.
    pub source: String,
    /// The symbol table (built from AST, enriched from TIR if available).
    pub symbol_table: SymbolTable,
    /// Definitions from imported files, keyed by symbol name.
    pub imported_definitions: HashMap<String, ImportedDefinition>,
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
            .field("imported_defs", &self.imported_definitions.len())
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
        Ok((tir, project)) => {
            // Full pipeline succeeded. Build symbol table from the AST embedded in the TIR,
            // then enrich with type info.
            // We need to re-parse to get the AST since TIR doesn't store it directly.
            // However, we know parsing succeeded (since TIR was produced).
            let ast = graphcal_syntax::parser::Parser::with_name(text, name)
                .parse_file()
                .expect("parse should succeed since TIR was produced");
            let mut symbol_table = symbol_table::build_from_ast(&ast);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir);

            // Resolve imported definitions from the project.
            let imported_definitions = project.map_or_else(HashMap::new, |project| {
                collect_imported_definitions(uri, &ast, &project, Some(&tir))
            });

            // No compile errors, but check for eval warnings by running the full pipeline.
            let diagnostics = uri.to_file_path().map_or_else(
                |()| crate::diagnostics::produce_diagnostics(text, name),
                |path| crate::diagnostics::produce_diagnostics_for_file(&path, text),
            );

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
            }
        }
        Err(e) => {
            // Compilation failed. Try to parse for a partial symbol table.
            let (symbol_table, imported_definitions) =
                graphcal_syntax::parser::Parser::with_name(text, name)
                    .parse_file()
                    .map_or_else(
                        |_| (SymbolTable::default(), HashMap::new()),
                        |ast| {
                            let st = symbol_table::build_from_ast(&ast);
                            let imports = collect_imported_definitions_from_ast(uri, &ast);
                            (st, imports)
                        },
                    );

            let diagnostics = compile_error_to_diagnostics(&e, text);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
            }
        }
    }
}

/// Collect imported definitions from a loaded project.
///
/// For each `use` declaration in the root file, resolves the import path,
/// looks up the imported file in the project, and builds a symbol table
/// from the imported file's AST to extract the definition info.
fn collect_imported_definitions(
    root_uri: &Url,
    root_ast: &graphcal_syntax::ast::File,
    project: &graphcal_eval::loader::LoadedProject,
    tir: Option<&graphcal_eval::tir::TIR>,
) -> HashMap<String, ImportedDefinition> {
    let mut result = HashMap::new();

    let Ok(root_path) = root_uri.to_file_path() else {
        return result;
    };
    let root_dir = root_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    for decl in &root_ast.declarations {
        if let DeclKind::Use(use_decl) = &decl.kind {
            let import_path = root_dir.join(&use_decl.path);
            let Ok(canonical) = import_path.canonicalize() else {
                continue;
            };
            let Some(loaded_file) = project.files.get(&canonical) else {
                continue;
            };

            let mut imported_table = symbol_table::build_from_ast(&loaded_file.ast);
            if let Some(tir) = tir {
                symbol_table::enrich_from_tir(&mut imported_table, tir);
            }

            let imported_uri = Url::from_file_path(&loaded_file.path).unwrap_or_else(|()| {
                Url::parse(&format!("file://{}", loaded_file.path.display()))
                    .unwrap_or_else(|_| root_uri.clone())
            });
            let source = loaded_file.source.to_string();

            for imported_name in &use_decl.names {
                if let Some(def) = imported_table.definitions.remove(&imported_name.name) {
                    result.insert(
                        imported_name.name.clone(),
                        ImportedDefinition {
                            uri: imported_uri.clone(),
                            source: source.clone(),
                            definition: def,
                        },
                    );
                }
            }
        }
    }

    result
}

/// Fallback: collect imported definitions by reading and parsing imported files directly.
/// Used when `compile_to_tir_project` fails but the root file parses successfully.
fn collect_imported_definitions_from_ast(
    root_uri: &Url,
    root_ast: &graphcal_syntax::ast::File,
) -> HashMap<String, ImportedDefinition> {
    let mut result = HashMap::new();

    let Ok(root_path) = root_uri.to_file_path() else {
        return result;
    };
    let root_dir = root_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    for decl in &root_ast.declarations {
        if let DeclKind::Use(use_decl) = &decl.kind {
            let import_path = root_dir.join(&use_decl.path);
            let Ok(canonical) = import_path.canonicalize() else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&canonical) else {
                continue;
            };
            let file_name = canonical.display().to_string();
            let Ok(ast) =
                graphcal_syntax::parser::Parser::with_name(&source, &file_name).parse_file()
            else {
                continue;
            };

            let mut imported_table = symbol_table::build_from_ast(&ast);

            let imported_uri = Url::from_file_path(&canonical).unwrap_or_else(|()| {
                Url::parse(&format!("file://{}", canonical.display()))
                    .unwrap_or_else(|_| root_uri.clone())
            });

            for imported_name in &use_decl.names {
                if let Some(def) = imported_table.definitions.remove(&imported_name.name) {
                    result.insert(
                        imported_name.name.clone(),
                        ImportedDefinition {
                            uri: imported_uri.clone(),
                            source: source.clone(),
                            definition: def,
                        },
                    );
                }
            }
        }
    }

    result
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
