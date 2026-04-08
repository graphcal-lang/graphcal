//! LSP server backend: state management and `LanguageServer` trait implementation.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeActionParams, CodeActionProviderCapability, CodeActionResponse, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, DocumentLink, DocumentLinkOptions, DocumentLinkParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, InlayHint, InlayHintParams, Location, MessageType, OneOf,
    PrepareRenameResponse, ReferenceParams, RenameOptions, RenameParams, SaveOptions,
    ServerCapabilities, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions,
    WorkspaceEdit,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use graphcal_compiler::syntax::ast::DeclKind;
use graphcal_compiler::syntax::names::VariantName;
use graphcal_eval::builtins::{DimSignature, ParamDim, ResultDim, builtin_functions};
use graphcal_eval::eval::{
    CompileError, EvalResult, Value, compile_and_eval_from_project, compile_to_tir_from_project,
};
use graphcal_eval::loader::LoadedProject;
use indexmap::IndexMap;

use crate::convert::position_to_byte_offset;
use crate::diagnostics::{compile_error_to_diagnostics, eval_result_to_diagnostics};
use crate::symbol_table::{self, DefinitionInfo, SymbolKey, SymbolTable};

/// A definition from an imported file, for cross-file go-to-definition and hover.
pub struct ImportedDefinition {
    /// URI of the file containing the definition.
    pub uri: Url,
    /// Source text of the imported file (needed for span-to-range conversion).
    pub source: String,
    /// The definition info (name, category, spans, type description).
    pub definition: DefinitionInfo,
}

/// Info about an `import` declaration for Document Links.
pub struct ImportDeclInfo {
    /// The import path (file or module).
    pub path: graphcal_compiler::syntax::ast::ImportPath,
}

/// Structured function signature for Signature Help.
pub struct FnSignatureInfo {
    /// Full signature label, e.g. `"fn sqrt(x: D) -> D^(1/2)"`.
    pub label: String,
    /// Individual parameter labels, e.g. `["x: D"]`.
    pub parameters: Vec<String>,
}

/// Cached analysis result for a document.
pub struct AnalysisResult {
    /// The raw source text.
    pub source: String,
    /// The symbol table (built from AST, enriched from TIR if available).
    pub symbol_table: SymbolTable,
    /// Definitions from imported files, keyed by symbol key.
    pub imported_definitions: HashMap<SymbolKey, ImportedDefinition>,
    /// Diagnostics to publish.
    pub diagnostics: Vec<Diagnostic>,
    /// Computed values from evaluation, keyed by declaration name.
    /// Each value is a formatted display string (e.g., `"9.81 [m/s^2]"`).
    pub eval_values: HashMap<String, String>,
    /// Structured function signatures, keyed by function name.
    pub fn_signatures: HashMap<String, FnSignatureInfo>,
    /// Use declarations in this file (for Document Links).
    pub import_decls: Vec<ImportDeclInfo>,
}

/// Debounce delay for `did_change` notifications (milliseconds).
const DEBOUNCE_DELAY_MS: u64 = 300;

/// The LSP server backend.
#[derive(Debug)]
pub struct Backend {
    client: Client,
    /// Per-document analysis results, keyed by URI.
    documents: Arc<RwLock<HashMap<Url, AnalysisResult>>>,
    /// Generation counter per URI, used for debouncing `did_change`.
    /// Each change increments the counter; a delayed task only runs analysis
    /// if its generation matches the current counter (no newer change arrived).
    change_generations: Arc<RwLock<HashMap<Url, u64>>>,
}

impl std::fmt::Debug for AnalysisResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisResult")
            .field("source_len", &self.source.len())
            .field("symbol_table_defs", &self.symbol_table.definitions.len())
            .field("imported_defs", &self.imported_definitions.len())
            .field("diagnostics_count", &self.diagnostics.len())
            .field("eval_values_count", &self.eval_values.len())
            .field("fn_signatures_count", &self.fn_signatures.len())
            .field("import_decls_count", &self.import_decls.len())
            .finish()
    }
}

impl Backend {
    fn is_graphcal_file(uri: &Url) -> bool {
        std::path::Path::new(uri.path())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gcl"))
    }

    /// Look up cached analysis for a document and apply a closure to it.
    ///
    /// Returns `Ok(None)` if the document has not been analyzed yet.
    async fn with_analysis<F, R>(&self, uri: &Url, f: F) -> Result<Option<R>>
    where
        F: FnOnce(&AnalysisResult) -> Option<R>,
    {
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(uri) else {
            return Ok(None);
        };
        let result = f(analysis);
        drop(docs);
        Ok(result)
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

/// Build a `LoadedProject` from a URI and in-memory text.
///
/// For file-backed URIs, loads the project from disk with the in-memory text
/// overlaid on the root file via [`graphcal_io::OverlayFileSystem`].
/// For untitled/non-file URIs, builds a single-file project from the
/// in-memory text alone.
fn build_project(uri: &Url, text: &str) -> std::result::Result<LoadedProject, Box<CompileError>> {
    let name = uri.as_str();
    match uri.to_file_path() {
        Ok(path) => {
            use graphcal_io::FileSystemReader as _;
            let base_fs = graphcal_io::RealFileSystem;
            let canonical = base_fs.canonicalize(&path).map_err(|_| {
                Box::new(CompileError::Eval(
                    graphcal_eval::error::GraphcalError::FileNotFound {
                        path: path.display().to_string(),
                    },
                ))
            })?;
            let fs = graphcal_io::OverlayFileSystem::new(base_fs, canonical, text.to_string());
            graphcal_eval::loader::load_project(&path, None, &fs).map_err(Box::new)
        }
        Err(()) => LoadedProject::from_source(text, name).map_err(Box::new),
    }
}

/// Run the analysis pipeline, producing an `AnalysisResult`.
///
/// The pipeline has two stages:
/// 1. Build a `LoadedProject` from the in-memory text (+ disk imports).
/// 2. Compile TIR from the project.
///
/// Both stages use the same source text, eliminating data provenance mismatches.
fn run_analysis(uri: &Url, text: &str) -> AnalysisResult {
    // Stage 1: Build project (parse + load imports).
    // If this fails, no AST is available — return minimal diagnostics.
    let project = match build_project(uri, text) {
        Ok(project) => project,
        Err(e) => {
            return AnalysisResult {
                source: text.to_string(),
                symbol_table: SymbolTable::default(),
                imported_definitions: HashMap::new(),
                diagnostics: compile_error_to_diagnostics(&e, text),
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(),
                import_decls: Vec::new(),
            };
        }
    };

    let root_ast = &project.files[&project.root].ast;

    // Stage 2: Compile TIR from the project.
    match compile_to_tir_from_project(&project) {
        Ok(tir) => {
            // Full success: symbol table from AST + TIR enrichment.
            let mut symbol_table = symbol_table::build_from_ast(root_ast);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir);

            let imported_definitions = collect_imported_definitions(uri, &project, Some(&tir));
            let fn_signatures = build_fn_signatures();
            let import_decls = collect_import_decl_info(root_ast);
            let (diagnostics, eval_values) = run_eval_from_project(&project, text);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values,
                fn_signatures,
                import_decls,
            }
        }
        Err(e) => {
            // TIR failed (type/dim error) but parse succeeded — use AST for partial info.
            let symbol_table = symbol_table::build_from_ast(root_ast);
            let imported_definitions = collect_imported_definitions(uri, &project, None);
            let diagnostics = compile_error_to_diagnostics(&e, text);
            let import_decls = collect_import_decl_info(root_ast);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(),
                import_decls,
            }
        }
    }
}

/// Run evaluation from a loaded project and extract diagnostics and formatted values.
fn run_eval_from_project(
    project: &LoadedProject,
    text: &str,
) -> (Vec<Diagnostic>, HashMap<String, String>) {
    match compile_and_eval_from_project(project, &HashMap::new(), true) {
        Ok(result) => {
            let diagnostics = eval_result_to_diagnostics(&result, text);
            let values = format_eval_values(&result);
            (diagnostics, values)
        }
        Err(e) => {
            let diagnostics = compile_error_to_diagnostics(&e, text);
            (diagnostics, HashMap::new())
        }
    }
}

/// Extract import/include-declaration info from an AST for Document Links.
fn collect_import_decl_info(ast: &graphcal_compiler::syntax::ast::File) -> Vec<ImportDeclInfo> {
    ast.declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Import(u) => Some(ImportDeclInfo {
                path: u.path.clone(),
            }),
            DeclKind::Include(u) => Some(ImportDeclInfo {
                path: u.path.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Build structured function signatures for Signature Help.
///
/// Returns builtin function signatures (always available).
fn build_fn_signatures() -> HashMap<String, FnSignatureInfo> {
    let mut sigs = HashMap::new();

    // Builtin functions — always available.
    for (name, f) in builtin_functions() {
        let (params, ret) = builtin_signature_parts(&f.dim_sig);
        let params_str = params.join(", ");
        let label = format!("fn {name}({params_str}) -> {ret}");
        sigs.insert(
            (*name).to_string(),
            FnSignatureInfo {
                label,
                parameters: params,
            },
        );
    }

    sigs
}

/// Format a dimension for display in builtin signatures (no registry needed).
fn format_dim_display(dim: &graphcal_compiler::syntax::dimension::Dimension) -> String {
    if dim.is_dimensionless() {
        return "Dimensionless".to_string();
    }
    let parts: Vec<String> = dim
        .iter()
        .map(|(id, exp)| {
            let name = id.fallback_symbol();
            if *exp == graphcal_compiler::syntax::dimension::Rational::ONE {
                name
            } else {
                format!("{name}^{exp}")
            }
        })
        .collect();
    parts.join(" * ")
}

/// Generate human-readable parameter and return type strings for a builtin function.
fn builtin_signature_parts(sig: &DimSignature) -> (Vec<String>, String) {
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| {
            let type_str = match &p.dim {
                ParamDim::Fixed(dim) => format_dim_display(dim),
                ParamDim::Bind(var) | ParamDim::Ref(var) => var.clone(),
            };
            format!("{}: {type_str}", p.name)
        })
        .collect();

    let ret = match &sig.result {
        ResultDim::Fixed(dim) => format_dim_display(dim),
        ResultDim::Var(name) => name.clone(),
        ResultDim::VarPow(name, power) => format!("{name}^({power})"),
    };

    (params, ret)
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

/// Maximum character length for inlay hint display strings.
/// When the formatted value exceeds this, entries are truncated with `...`.
const INLAY_HINT_MAX_LEN: usize = 80;

/// Format a single `Value` as a compact inline string for inlay hints.
///
/// - Scalar: `"9.81 [m/s^2]"` or `"3.14159"` (dimensionless)
/// - Bool: `"true"` / `"false"`
/// - Int: `"42"`
/// - Struct: `"LowThrust { thrust: 0.5 [N], duration: 3600 [s] }"`
/// - Indexed: `"{ Departure: 4.92 [km/s], Correction: 0.24 [km/s], ... }"`
fn format_value_inline(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
) -> String {
    format_value_inline_with_budget(value, symbols, INLAY_HINT_MAX_LEN)
}

/// Format a `Value` with a character budget. When the formatted entries would
/// exceed `max_len`, remaining entries are replaced with `...`.
fn format_value_inline_with_budget(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    match value {
        // Leaf types: delegate to the shared `format_display` on `Value`.
        Value::Scalar { .. }
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Label { .. }
        | Value::Datetime { .. } => value.format_display(Some(symbols)),
        Value::Struct {
            type_name, fields, ..
        } => {
            if fields.is_empty() {
                return format!("{type_name} {{}}");
            }
            format_braced_entries(
                &format!("{type_name} "),
                fields
                    .iter()
                    .map(|(k, v)| (k.as_str(), v))
                    .collect::<Vec<_>>(),
                symbols,
                max_len,
            )
        }
        Value::Indexed { entries, .. } => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            // For multi-indexed maps (nested Indexed values), flatten into
            // tuple-keyed form: `{ (A, X): 1, (A, Y): 2, (B, X): 3 }` instead
            // of nested braces: `{ A: { X: 1, Y: 2 }, B: { X: 3 } }`.
            let mut flat: Vec<(Vec<&str>, &Value)> = Vec::new();
            flatten_indexed_entries(entries, &mut Vec::new(), &mut flat);
            let is_multi = flat.first().is_some_and(|(keys, _)| keys.len() > 1);
            if is_multi {
                format_tuple_keyed_entries("", &flat, symbols, max_len)
            } else {
                format_braced_entries(
                    "",
                    entries
                        .iter()
                        .map(|(k, v)| (k.as_str(), v))
                        .collect::<Vec<_>>(),
                    symbols,
                    max_len,
                )
            }
        }
    }
}

/// Format a list of key-value pairs as `{prefix}{ k1: v1, k2: v2, ... }`,
/// truncating with `...` when the result would exceed `max_len`.
fn format_braced_entries(
    prefix: &str,
    entries: Vec<(&str, &Value)>,
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    let mut result = format!("{prefix}{{ ");
    let suffix = " }";
    let ellipsis = "... }";
    let total = entries.len();

    for (i, (key, val)) in entries.into_iter().enumerate() {
        let remaining_budget = max_len.saturating_sub(result.len() + suffix.len());
        let entry_str = format!(
            "{key}: {}",
            format_value_inline_with_budget(val, symbols, remaining_budget)
        );

        // Check if adding this entry (plus separator and closing) would exceed budget
        let separator = if i + 1 < total { ", " } else { "" };
        let needed = entry_str.len() + separator.len();

        if i > 0 && result.len() + needed + suffix.len() > max_len {
            // Truncate: replace with ellipsis
            result.push_str(ellipsis);
            return result;
        }

        result.push_str(&entry_str);
        if i + 1 < total {
            result.push_str(", ");
        }
    }

    result.push_str(suffix);
    result
}

/// Recursively flatten nested `Indexed` values into a list of `(key_path, leaf_value)` pairs.
///
/// For a single-level `Indexed { A: 1, B: 2 }`, produces `[([A], 1), ([B], 2)]`.
/// For a nested `Indexed { A: Indexed { X: 1, Y: 2 }, B: Indexed { X: 3 } }`,
/// produces `[([A, X], 1), ([A, Y], 2), ([B, X], 3)]`.
fn flatten_indexed_entries<'a>(
    entries: &'a IndexMap<VariantName, Value>,
    prefix: &mut Vec<&'a str>,
    out: &mut Vec<(Vec<&'a str>, &'a Value)>,
) {
    for (key, val) in entries {
        prefix.push(key.as_str());
        if let Value::Indexed { entries: inner, .. } = val {
            flatten_indexed_entries(inner, prefix, out);
        } else {
            out.push((prefix.clone(), val));
        }
        prefix.pop();
    }
}

/// Format flattened tuple-keyed entries as `{ (A, X): v1, (A, Y): v2, ... }`,
/// truncating with `...` when the result would exceed `max_len`.
fn format_tuple_keyed_entries(
    prefix: &str,
    entries: &[(Vec<&str>, &Value)],
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    let mut result = format!("{prefix}{{ ");
    let suffix = " }";
    let ellipsis = "... }";
    let total = entries.len();

    for (i, (keys, val)) in entries.iter().enumerate() {
        let remaining_budget = max_len.saturating_sub(result.len() + suffix.len());
        let key_str = format!("({})", keys.join(", "));
        let entry_str = format!(
            "{key_str}: {}",
            format_value_inline_with_budget(val, symbols, remaining_budget)
        );

        let separator = if i + 1 < total { ", " } else { "" };
        let needed = entry_str.len() + separator.len();

        if i > 0 && result.len() + needed + suffix.len() > max_len {
            result.push_str(ellipsis);
            return result;
        }

        result.push_str(&entry_str);
        if i + 1 < total {
            result.push_str(", ");
        }
    }

    result.push_str(suffix);
    result
}

/// Collect imported definitions from a loaded project.
///
/// For each `import` declaration in the root file, uses the loader-resolved
/// canonical paths to look up the imported file in the project, and builds a
/// symbol table from the imported file's AST to extract the definition info.
///
/// This uses `LoadedFile::imports_with_paths()` to consume the same canonical
/// import paths that the loader established, avoiding re-resolution and
/// supporting both file-path and module-path imports.
fn collect_imported_definitions(
    root_uri: &Url,
    project: &graphcal_eval::loader::LoadedProject,
    tir: Option<&graphcal_eval::tir::TIR>,
) -> HashMap<SymbolKey, ImportedDefinition> {
    use std::path::Path;

    let mut result = HashMap::new();

    let Some(root_file) = project.files.get(&project.root) else {
        return result;
    };

    // Cache symbol tables per canonical path to avoid re-building for files imported
    // by multiple `import` declarations.
    let mut table_cache: HashMap<&Path, (SymbolTable, Url, String)> = HashMap::new();

    for (_decl, import_decl, canonical) in root_file.imports_with_paths() {
        let Some(loaded_file) = project.files.get(canonical) else {
            continue;
        };

        let (imported_table, imported_uri, source) =
            table_cache.entry(canonical).or_insert_with(|| {
                let mut table = symbol_table::build_from_ast(&loaded_file.ast);
                if let Some(tir) = tir {
                    symbol_table::enrich_from_tir(&mut table, tir);
                }
                // Url::from_file_path handles percent-encoding correctly (spaces, special chars).
                // It only fails for non-absolute paths, which should not occur for loaded files.
                let uri =
                    Url::from_file_path(&loaded_file.path).unwrap_or_else(|()| root_uri.clone());
                let src = loaded_file.source.to_string();
                (table, uri, src)
            });

        match &import_decl.kind {
            graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
                for import_item in names {
                    let key = SymbolKey::TopLevel(import_item.name.name.clone());
                    if let Some(def) = imported_table.definitions.get(&key) {
                        result.insert(
                            key,
                            ImportedDefinition {
                                uri: imported_uri.clone(),
                                source: source.clone(),
                                definition: def.clone(),
                            },
                        );
                    }
                }
            }
            graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
                for (key, def) in &imported_table.definitions {
                    result.insert(
                        key.clone(),
                        ImportedDefinition {
                            uri: imported_uri.clone(),
                            source: source.clone(),
                            definition: def.clone(),
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
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["@".to_string(), ":".to_string()]),
                    resolve_provider: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    all_commit_characters: None,
                    completion_item: None,
                }),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
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
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let uri = params.text_document.uri;
        if !Self::is_graphcal_file(&uri) {
            return;
        }

        // Bump the generation counter for this URI.
        let generation = *self
            .change_generations
            .write()
            .await
            .entry(uri.clone())
            .and_modify(|v| *v += 1)
            .or_insert(1);

        // Spawn a delayed analysis task. If another change arrives within the
        // debounce window, its generation will be higher and this task will
        // skip analysis.
        let client = self.client.clone();
        let documents = self.documents.clone();
        let generations = self.change_generations.clone();
        let text = change.text;

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_DELAY_MS)).await;

            // Check if a newer change has superseded this one.
            let current = *generations.read().await.get(&uri).unwrap_or(&0);
            if current != generation {
                return;
            }

            let analysis = run_analysis(&uri, &text);
            let diagnostics = analysis.diagnostics.clone();
            documents.write().await.insert(uri.clone(), analysis);
            client.publish_diagnostics(uri, diagnostics, None).await;
            let _ = client.inlay_hint_refresh().await;
        });
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.analyze_and_publish(params.text_document.uri, text)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        self.change_generations.write().await.remove(&uri);
        // Clear diagnostics for the closed document so stale errors don't linger.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.with_analysis(&params.text_document.uri, |analysis| {
            Some(DocumentSymbolResponse::Nested(
                crate::document_symbols::build_document_symbols(analysis),
            ))
        })
        .await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::goto_definition::goto_definition(analysis, &uri, offset)
        })
        .await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::hover::hover(analysis, offset)
        })
        .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::references::references(analysis, &uri, offset, include_declaration)
        })
        .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        self.with_analysis(&uri, |analysis| {
            crate::inlay_hints::inlay_hints(analysis, range)
        })
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::signature_help::signature_help(analysis, offset)
        })
        .await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::completion::completion(analysis, offset).map(CompletionResponse::Array)
        })
        .await
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        self.with_analysis(&uri, |analysis| {
            crate::code_actions::code_actions(&params, &analysis.source)
        })
        .await
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::rename::rename(analysis, &uri, offset, &new_name)
        })
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        self.with_analysis(&uri, |analysis| {
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::rename::prepare_rename(analysis, offset)
        })
        .await
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        self.with_analysis(&uri, |analysis| {
            crate::document_links::document_links(analysis, &uri)
        })
        .await
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        self.with_analysis(&uri, |analysis| {
            crate::formatting::format_document(&analysis.source)
        })
        .await
    }
}

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(RwLock::new(HashMap::new())),
        change_generations: Arc::new(RwLock::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
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

    use std::collections::BTreeMap;

    use graphcal_compiler::syntax::dimension::Dimension;
    use graphcal_compiler::syntax::names::{FieldName, IndexName, StructTypeName, VariantName};
    use graphcal_eval::eval::Value;
    use indexmap::IndexMap;

    use super::*;

    fn empty_symbols() -> BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String> {
        BTreeMap::new()
    }

    fn scalar(si_value: f64) -> Value {
        Value::Scalar {
            si_value,
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    #[test]
    fn format_scalar_dimensionless() {
        let symbols = empty_symbols();
        assert_eq!(format_value_inline(&scalar(2.72), &symbols), "2.72");
        assert_eq!(format_value_inline(&scalar(42.0), &symbols), "42");
    }

    #[test]
    fn format_bool() {
        let symbols = empty_symbols();
        assert_eq!(format_value_inline(&Value::Bool(true), &symbols), "true");
        assert_eq!(format_value_inline(&Value::Bool(false), &symbols), "false");
    }

    #[test]
    fn format_int() {
        let symbols = empty_symbols();
        assert_eq!(format_value_inline(&Value::Int(7), &symbols), "7");
    }

    #[test]
    fn format_struct_with_fields() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("dv1"), scalar(100.0));
        fields.insert(FieldName::new("dv2"), scalar(200.0));
        let val = Value::Struct {
            type_name: StructTypeName::new("TransferResult"),
            fields,
        };
        assert_eq!(
            format_value_inline(&val, &symbols),
            "TransferResult { dv1: 100, dv2: 200 }"
        );
    }

    #[test]
    fn format_struct_empty_fields() {
        let symbols = empty_symbols();
        let val = Value::Struct {
            type_name: StructTypeName::new("Nominal"),
            fields: IndexMap::new(),
        };
        assert_eq!(format_value_inline(&val, &symbols), "Nominal {}");
    }

    #[test]
    fn format_struct_multi_variant() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("thrust"), scalar(0.5));
        fields.insert(FieldName::new("duration"), scalar(3600.0));
        let val = Value::Struct {
            type_name: StructTypeName::new("ManeuverKind"),
            fields,
        };
        assert_eq!(
            format_value_inline(&val, &symbols),
            "ManeuverKind { thrust: 0.5, duration: 3600 }"
        );
    }

    #[test]
    fn format_indexed() {
        let symbols = empty_symbols();
        let mut entries = IndexMap::new();
        entries.insert(VariantName::new("A"), scalar(1.0));
        entries.insert(VariantName::new("B"), scalar(2.0));
        entries.insert(VariantName::new("C"), scalar(3.0));
        let val = Value::Indexed {
            index_name: IndexName::new("Phase"),
            entries,
        };
        assert_eq!(format_value_inline(&val, &symbols), "{ A: 1, B: 2, C: 3 }");
    }

    #[test]
    fn format_indexed_empty() {
        let symbols = empty_symbols();
        let val = Value::Indexed {
            index_name: IndexName::new("Phase"),
            entries: IndexMap::new(),
        };
        assert_eq!(format_value_inline(&val, &symbols), "{}");
    }

    #[test]
    fn format_indexed_truncation() {
        let symbols = empty_symbols();
        let mut entries = IndexMap::new();
        // Create entries with long names to trigger truncation at 80 chars
        entries.insert(VariantName::new("LongVariantAlpha"), scalar(1.23456));
        entries.insert(VariantName::new("LongVariantBeta"), scalar(2.34567));
        entries.insert(VariantName::new("LongVariantGamma"), scalar(3.45678));
        entries.insert(VariantName::new("LongVariantDelta"), scalar(4.56789));
        let val = Value::Indexed {
            index_name: IndexName::new("Idx"),
            entries,
        };
        let result = format_value_inline(&val, &symbols);
        assert!(
            result.len() <= INLAY_HINT_MAX_LEN + 10,
            "result too long: {result}"
        );
        assert!(result.ends_with("... }"), "expected truncation: {result}");
    }

    #[test]
    fn format_struct_inside_indexed() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("x"), scalar(1.0));
        let struct_val = Value::Struct {
            type_name: StructTypeName::new("Point"),
            fields,
        };
        let mut entries = IndexMap::new();
        entries.insert(VariantName::new("A"), struct_val);
        let val = Value::Indexed {
            index_name: IndexName::new("Idx"),
            entries,
        };
        assert_eq!(format_value_inline(&val, &symbols), "{ A: Point { x: 1 } }");
    }

    #[test]
    fn format_nested_indexed_tuple_keyed() {
        let symbols = empty_symbols();
        let mut inner_a = IndexMap::new();
        inner_a.insert(VariantName::new("X"), scalar(1.0));
        inner_a.insert(VariantName::new("Y"), scalar(2.0));
        let mut inner_b = IndexMap::new();
        inner_b.insert(VariantName::new("X"), scalar(3.0));
        inner_b.insert(VariantName::new("Y"), scalar(4.0));
        let mut entries = IndexMap::new();
        entries.insert(
            VariantName::new("A"),
            Value::Indexed {
                index_name: IndexName::new("Col"),
                entries: inner_a,
            },
        );
        entries.insert(
            VariantName::new("B"),
            Value::Indexed {
                index_name: IndexName::new("Col"),
                entries: inner_b,
            },
        );
        let val = Value::Indexed {
            index_name: IndexName::new("Row"),
            entries,
        };
        assert_eq!(
            format_value_inline(&val, &symbols),
            "{ (A, X): 1, (A, Y): 2, (B, X): 3, (B, Y): 4 }"
        );
    }

    #[test]
    fn format_triple_nested_indexed() {
        let symbols = empty_symbols();
        // 3-level nesting: Scenario[Phase[Maneuver[scalar]]]
        let mut inner_most = IndexMap::new();
        inner_most.insert(VariantName::new("Dep"), scalar(100.0));
        let mut mid = IndexMap::new();
        mid.insert(
            VariantName::new("Launch"),
            Value::Indexed {
                index_name: IndexName::new("Maneuver"),
                entries: inner_most,
            },
        );
        let mut outer = IndexMap::new();
        outer.insert(
            VariantName::new("Nom"),
            Value::Indexed {
                index_name: IndexName::new("Phase"),
                entries: mid,
            },
        );
        let val = Value::Indexed {
            index_name: IndexName::new("Scenario"),
            entries: outer,
        };
        assert_eq!(
            format_value_inline(&val, &symbols),
            "{ (Nom, Launch, Dep): 100 }"
        );
    }

    #[test]
    fn format_nested_indexed_truncation() {
        let symbols = empty_symbols();
        let mut inner_a = IndexMap::new();
        inner_a.insert(VariantName::new("LongNameAlpha"), scalar(1.23456));
        inner_a.insert(VariantName::new("LongNameBeta"), scalar(2.34567));
        inner_a.insert(VariantName::new("LongNameGamma"), scalar(3.45678));
        let mut inner_b = IndexMap::new();
        inner_b.insert(VariantName::new("LongNameAlpha"), scalar(4.56789));
        inner_b.insert(VariantName::new("LongNameBeta"), scalar(5.6789));
        inner_b.insert(VariantName::new("LongNameGamma"), scalar(6.7891));
        let mut entries = IndexMap::new();
        entries.insert(
            VariantName::new("LongOuter1"),
            Value::Indexed {
                index_name: IndexName::new("Inner"),
                entries: inner_a,
            },
        );
        entries.insert(
            VariantName::new("LongOuter2"),
            Value::Indexed {
                index_name: IndexName::new("Inner"),
                entries: inner_b,
            },
        );
        let val = Value::Indexed {
            index_name: IndexName::new("Outer"),
            entries,
        };
        let result = format_value_inline(&val, &symbols);
        assert!(
            result.len() <= INLAY_HINT_MAX_LEN + 10,
            "result too long: {result}"
        );
        assert!(result.ends_with("... }"), "expected truncation: {result}");
    }
}
