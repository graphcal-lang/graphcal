//! LSP server backend: state management and `LanguageServer` trait implementation.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, InlayHint, InlayHintParams, Location, MessageType, OneOf, ReferenceParams,
    SaveOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use graphcal_eval::eval::{
    EvalResult, Value, compile_and_eval_named, compile_and_eval_project, compile_to_tir,
    compile_to_tir_project,
};
use graphcal_syntax::ast::DeclKind;

use crate::convert::position_to_byte_offset;
use crate::diagnostics::{compile_error_to_diagnostics, eval_result_to_diagnostics};
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
    /// Computed values from evaluation, keyed by declaration name.
    /// Each value is a formatted display string (e.g., `"9.81 [m/s^2]"`).
    pub eval_values: HashMap<String, String>,
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
            .field("eval_values_count", &self.eval_values.len())
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

        // Ask the client to re-fetch inlay hints now that analysis is complete.
        // Inlay hints are pull-based (client requests them), so without this
        // refresh notification the client may show stale or missing hints.
        let _ = self.client.inlay_hint_refresh().await;
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

    // Always parse the in-memory text first for the symbol table.
    // This may differ from the on-disk version when the user has unsaved edits.
    let parse_result = graphcal_syntax::parser::Parser::with_name(text, name).parse_file();

    match (tir_result, parse_result) {
        (Ok((tir, project)), Ok(ast)) => {
            // Both TIR and in-memory parse succeeded.
            let mut symbol_table = symbol_table::build_from_ast(&ast);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir);

            let imported_definitions = project.map_or_else(HashMap::new, |project| {
                collect_imported_definitions(uri, &ast, &project, Some(&tir))
            });

            // Run full evaluation for diagnostics and computed values.
            let (diagnostics, eval_values) = run_eval(uri, text, name);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values,
            }
        }
        (Ok((tir, project)), Err(_)) => {
            // TIR succeeded (from disk) but in-memory text has parse errors.
            // This happens when the user has unsaved edits that break parsing.
            // Produce diagnostics from the in-memory text (not disk) so the
            // user sees the parse error.
            let diagnostics = match compile_and_eval_named(text, name) {
                Ok(result) => eval_result_to_diagnostics(&result),
                Err(e) => compile_error_to_diagnostics(&e, text),
            };

            // Use the on-disk AST for a partial symbol table and eval values.
            let (symbol_table, imported_definitions, eval_values) = uri
                .to_file_path()
                .ok()
                .and_then(|path| std::fs::read_to_string(&path).ok())
                .and_then(|disk_text| {
                    graphcal_syntax::parser::Parser::with_name(&disk_text, name)
                        .parse_file()
                        .ok()
                        .map(|ast| (disk_text, ast))
                })
                .map_or_else(
                    || (SymbolTable::default(), HashMap::new(), HashMap::new()),
                    |(_, disk_ast)| {
                        let mut st = symbol_table::build_from_ast(&disk_ast);
                        symbol_table::enrich_from_tir(&mut st, &tir);
                        let imports = project.map_or_else(HashMap::new, |p| {
                            collect_imported_definitions(uri, &disk_ast, &p, Some(&tir))
                        });
                        // Keep eval values from the last valid (disk) version.
                        let (_, vals) = run_eval(uri, text, name);
                        (st, imports, vals)
                    },
                );

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values,
            }
        }
        (Err(e), Ok(ast)) => {
            // TIR failed but in-memory parse succeeded — use AST for symbol table.
            let symbol_table = symbol_table::build_from_ast(&ast);
            let imported_definitions = collect_imported_definitions_from_ast(uri, &ast);
            let diagnostics = compile_error_to_diagnostics(&e, text);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values: HashMap::new(),
            }
        }
        (Err(e), Err(_)) => {
            // Both failed — minimal result with diagnostics.
            let diagnostics = compile_error_to_diagnostics(&e, text);

            AnalysisResult {
                source: text.to_string(),
                symbol_table: SymbolTable::default(),
                imported_definitions: HashMap::new(),
                diagnostics,
                eval_values: HashMap::new(),
            }
        }
    }
}

/// Run evaluation and extract both diagnostics and formatted values.
fn run_eval(uri: &Url, text: &str, name: &str) -> (Vec<Diagnostic>, HashMap<String, String>) {
    let eval_result = uri.to_file_path().map_or_else(
        |()| compile_and_eval_named(text, name),
        |path| compile_and_eval_project(&path, &HashMap::new()),
    );

    match eval_result {
        Ok(result) => {
            let diagnostics = eval_result_to_diagnostics(&result);
            let values = format_eval_values(&result);
            (diagnostics, values)
        }
        Err(e) => {
            let diagnostics = compile_error_to_diagnostics(&e, text);
            (diagnostics, HashMap::new())
        }
    }
}

/// Format all successfully evaluated values into display strings.
fn format_eval_values(result: &EvalResult) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value_result, _decl_type) in &result.all {
        if let Ok(value) = value_result {
            map.insert(
                name.as_str().to_string(),
                format_value_inline(value, &result.base_dim_symbols),
            );
        }
    }
    map
}

/// Format a single `Value` as a compact inline string for inlay hints.
///
/// - Scalar: `"9.81 [m/s^2]"` or `"3.14159"` (dimensionless)
/// - Bool: `"true"` / `"false"`
/// - Int: `"42"`
/// - Struct/Indexed: type name only
fn format_value_inline(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
) -> String {
    match value {
        Value::Scalar { .. } => {
            let formatted = format_number(value.display_value());
            value.display_label(symbols).map_or_else(
                || formatted.clone(),
                |label| format!("{formatted} [{label}]"),
            )
        }
        Value::Bool(b) => format!("{b}"),
        Value::Int(i) => format!("{i}"),
        Value::Struct { type_name, .. } => type_name.to_string(),
        Value::Indexed { index_name, .. } => format!("[{index_name}]"),
    }
}

/// Format a number for display: integers without decimal point, floats with
/// reasonable precision (up to 6 decimal places, trailing zeros stripped).
#[expect(
    clippy::cast_possible_truncation,
    reason = "guarded by abs() < 1e15 check"
)]
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        let s = format!("{value:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
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
                references_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
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

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::references::references(analysis, &uri, offset, include_declaration);
        drop(docs);
        Ok(result)
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let result = crate::inlay_hints::inlay_hints(analysis, params.range);
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
