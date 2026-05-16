//! LSP server backend: state management and `LanguageServer` trait implementation.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeActionParams, CodeActionProviderCapability, CodeActionResponse, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentFormattingParams, DocumentLink, DocumentLinkOptions,
    DocumentLinkParams, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, InlayHint, InlayHintParams, Location, MessageType, OneOf,
    PrepareRenameResponse, Range, ReferenceParams, RenameOptions, RenameParams, SaveOptions,
    ServerCapabilities, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions,
    WorkspaceEdit,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use graphcal_compiler::registry::builtins::{DimSignature, ParamDim, ResultDim, builtin_functions};
use graphcal_compiler::syntax::names::{DeclName, VariantName};
use graphcal_eval::eval::{
    CompileError, EvalResult, Value, compile_and_eval_from_project, compile_to_tir_from_project,
};
use graphcal_eval::loader::LoadedProject;
use indexmap::IndexMap;

use crate::convert::position_to_byte_offset;
use crate::diagnostics::{compile_error_to_diagnostics_grouped, eval_result_to_diagnostics};
use crate::symbol_table::{self, DefinitionInfo, SymbolKey, SymbolTable};

/// A definition from an imported file, for cross-file go-to-definition and hover.
pub(crate) struct ImportedDefinition {
    /// URI of the file containing the definition.
    pub(crate) uri: Url,
    /// Source text of the imported file (needed for span-to-range conversion).
    /// Shared via `Arc` to avoid cloning the full source per imported symbol.
    pub(crate) source: Arc<String>,
    /// The definition info (name, category, spans, type description).
    pub(crate) definition: DefinitionInfo,
}

/// A loader-resolved import link for Document Links.
///
/// Pairs the source-text span of the import path with the loader-resolved
/// target URI, so `document_links` doesn't need to re-resolve paths.
pub(crate) struct ResolvedImportLink {
    /// Span of the import path in the source text.
    pub(crate) path_span: graphcal_compiler::syntax::span::Span,
    /// Loader-resolved target URI.
    pub(crate) target_uri: Url,
}

/// Structured function signature for Signature Help.
pub(crate) struct FnSignatureInfo {
    /// Full signature label, e.g. `"fn sqrt(x: D) -> D^(1/2)"`.
    pub(crate) label: String,
    /// Individual parameter labels, e.g. `["x: D"]`.
    pub(crate) parameters: Vec<String>,
}

/// Cached analysis result for a document.
pub(crate) struct AnalysisResult {
    /// The raw source text. Shared via `Arc` so hover, inlay-hint, and
    /// formatting handlers can borrow without cloning the full buffer.
    pub(crate) source: Arc<String>,
    /// The symbol table (built from AST, enriched from TIR if available).
    pub(crate) symbol_table: SymbolTable,
    /// Definitions from imported files, keyed by symbol key.
    pub(crate) imported_definitions: HashMap<SymbolKey, ImportedDefinition>,
    /// Diagnostics to publish, grouped by the URI they belong to. The active
    /// document's URI is always present (with an empty Vec when clean) so a
    /// previously-published diagnostic can be cleared. Shared via `Arc` so
    /// `store_and_publish` can hand a snapshot to the publish loop without
    /// deep-cloning the map on every analysis cycle.
    pub(crate) diagnostics: Arc<HashMap<Url, Vec<Diagnostic>>>,
    /// Computed values from evaluation, keyed by declaration name.
    /// Each value is a formatted display string (e.g., `"9.81 [m/s^2]"`).
    pub(crate) eval_values: HashMap<DeclName, String>,
    /// Structured function signatures, keyed by function name.
    /// Points to a lazily-initialized static map (builtins never change).
    pub(crate) fn_signatures: &'static HashMap<String, FnSignatureInfo>,
    /// Loader-resolved import links (for Document Links).
    pub(crate) import_links: Vec<ResolvedImportLink>,
}

/// Debounce delay for `did_change` notifications (milliseconds).
const DEBOUNCE_DELAY_MS: u64 = 300;

/// Wall-clock cap on a single `run_analysis` pass. When exceeded, the LSP
/// publishes a timeout diagnostic and leaves any prior cached analysis in
/// place so queries (hover, goto, etc.) still answer from the last good
/// state. The blocking thread keeps running until it returns — `spawn_blocking`
/// is not cancellable — but its result is discarded.
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(10);

/// The LSP server backend.
#[cfg_attr(test, derive(Debug))]
pub struct Backend {
    client: Client,
    /// Per-document analysis results, keyed by URI.
    documents: Arc<RwLock<HashMap<Url, AnalysisResult>>>,
    /// Generation counter per URI, used for debouncing `did_change`.
    /// Each change increments the counter; a delayed task only runs analysis
    /// if its generation matches the current counter (no newer change arrived).
    change_generations: Arc<RwLock<HashMap<Url, u64>>>,
}

#[cfg(test)]
impl AnalysisResult {
    /// True when no diagnostics are present across any URI.
    pub(crate) fn has_no_diagnostics(&self) -> bool {
        self.diagnostics.values().all(Vec::is_empty)
    }
}

// `AnalysisResult`'s custom `Debug` shape (counts, not contents) is useful only
// inside test assertion messages; gating it behind `cfg(test)` keeps the
// release binary from carrying an impl no production code path can call.
#[cfg(test)]
impl std::fmt::Debug for AnalysisResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisResult")
            .field("source_len", &self.source.len())
            .field("symbol_table_defs", &self.symbol_table.definitions.len())
            .field("imported_defs", &self.imported_definitions.len())
            .field(
                "diagnostics_count",
                &self.diagnostics.values().map(Vec::len).sum::<usize>(),
            )
            .field("eval_values_count", &self.eval_values.len())
            .field("fn_signatures_count", &self.fn_signatures.len())
            .field("import_links_count", &self.import_links.len())
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

        // Bump the generation so any in-flight debounced analysis for this URI
        // becomes stale and refuses to overwrite fresh results.
        let generation = self.bump_generation(&uri).await;

        let uri_clone = uri.clone();
        let task = tokio::task::spawn_blocking(move || run_analysis(&uri_clone, &text));
        let analysis = match tokio::time::timeout(ANALYSIS_TIMEOUT, task).await {
            Ok(Ok(a)) => a,
            Ok(Err(e)) => {
                self.client
                    .log_message(MessageType::ERROR, format!("analysis task panicked: {e}"))
                    .await;
                return;
            }
            Err(_elapsed) => {
                publish_analysis_timeout(&self.client, &uri).await;
                return;
            }
        };

        self.store_and_publish(uri, analysis, generation).await;
    }

    /// Spawn a debounced analysis task for a `did_change` event.
    ///
    /// Waits [`DEBOUNCE_DELAY_MS`] and bails out if a newer change has
    /// arrived (generation mismatch). Any `JoinError` from the blocking
    /// analysis task is logged to the client rather than silently swallowed.
    fn spawn_debounced_analysis(&self, uri: Url, text: String, generation: u64) {
        let client = self.client.clone();
        let documents = self.documents.clone();
        let generations = self.change_generations.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DEBOUNCE_DELAY_MS)).await;

            // Check if a newer change has superseded this one.
            if !is_generation_current(&generations, &uri, generation).await {
                return;
            }

            let uri_for_analysis = uri.clone();
            let task = tokio::task::spawn_blocking(move || run_analysis(&uri_for_analysis, &text));
            let analysis = match tokio::time::timeout(ANALYSIS_TIMEOUT, task).await {
                Ok(Ok(a)) => a,
                Ok(Err(e)) => {
                    client
                        .log_message(MessageType::ERROR, format!("analysis task panicked: {e}"))
                        .await;
                    return;
                }
                Err(_elapsed) => {
                    publish_analysis_timeout(&client, &uri).await;
                    return;
                }
            };

            // A `did_save` or a later `did_change` may have fired while analysis
            // was running. Re-check before writing so stale results never clobber
            // fresh ones.
            if !is_generation_current(&generations, &uri, generation).await {
                return;
            }

            let new_diags = Arc::clone(&analysis.diagnostics);
            let stale_uris = collect_stale_uris(&documents, &uri, &new_diags).await;
            documents.write().await.insert(uri.clone(), analysis);
            for stale in stale_uris {
                client.publish_diagnostics(stale, Vec::new(), None).await;
            }
            for (target_uri, diags) in new_diags.iter() {
                client
                    .publish_diagnostics(target_uri.clone(), diags.clone(), None)
                    .await;
            }
            // Best-effort: the client may not support inlay-hint refresh.
            let _ = client.inlay_hint_refresh().await;
        });
    }

    /// Increment and return the current generation for `uri`.
    async fn bump_generation(&self, uri: &Url) -> u64 {
        *self
            .change_generations
            .write()
            .await
            .entry(uri.clone())
            .and_modify(|v| *v += 1)
            .or_insert(1)
    }

    /// Write `analysis` into `documents` and publish diagnostics, but only if
    /// `generation` is still the latest for `uri`. Otherwise the write is a
    /// no-op — a newer change has superseded this analysis.
    async fn store_and_publish(&self, uri: Url, analysis: AnalysisResult, generation: u64) {
        if !is_generation_current(&self.change_generations, &uri, generation).await {
            return;
        }
        let new_diags = Arc::clone(&analysis.diagnostics);
        let stale_uris = collect_stale_uris(&self.documents, &uri, &new_diags).await;
        self.documents.write().await.insert(uri.clone(), analysis);
        for stale in stale_uris {
            self.client
                .publish_diagnostics(stale, Vec::new(), None)
                .await;
        }
        for (target_uri, diags) in new_diags.iter() {
            self.client
                .publish_diagnostics(target_uri.clone(), diags.clone(), None)
                .await;
        }
        // Best-effort: the client may not support inlay-hint refresh.
        let _ = self.client.inlay_hint_refresh().await;
    }
}

/// Publish a single error diagnostic on `uri` indicating that the analysis
/// pipeline exceeded [`ANALYSIS_TIMEOUT`], and log the event to the client.
///
/// The cached `AnalysisResult` for `uri` (if any) is intentionally left
/// untouched so symbol queries continue to answer from the last good state.
/// Other URIs are not affected — `publish_diagnostics` is per-URI.
async fn publish_analysis_timeout(client: &Client, uri: &Url) {
    let secs = ANALYSIS_TIMEOUT.as_secs();
    client
        .log_message(
            MessageType::ERROR,
            format!("analysis for {uri} timed out after {secs}s"),
        )
        .await;
    let diag = Diagnostic {
        range: Range::default(),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("graphcal".to_string()),
        message: format!("graphcal-lsp: analysis timed out after {secs}s"),
        ..Default::default()
    };
    client
        .publish_diagnostics(uri.clone(), vec![diag], None)
        .await;
}

/// Compute the set of URIs that previously had diagnostics published from
/// `active_uri`'s analysis but are absent from the new diagnostic map. The
/// caller publishes empty Vecs to those URIs to clear stale markers.
async fn collect_stale_uris(
    documents: &Arc<RwLock<HashMap<Url, AnalysisResult>>>,
    active_uri: &Url,
    new_diags: &HashMap<Url, Vec<Diagnostic>>,
) -> Vec<Url> {
    let stale = {
        let docs = documents.read().await;
        docs.get(active_uri).map(|prev| {
            prev.diagnostics
                .keys()
                .filter(|u| !new_diags.contains_key(*u))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    stale.unwrap_or_default()
}

/// Check whether `generation` is still the current generation stored for `uri`.
async fn is_generation_current(
    generations: &Arc<RwLock<HashMap<Url, u64>>>,
    uri: &Url,
    generation: u64,
) -> bool {
    *generations.read().await.get(uri).unwrap_or(&0) == generation
}

/// Build a `LoadedProject` from a URI and in-memory text.
///
/// For file-backed URIs, loads the project from disk with the in-memory text
/// overlaid on the root file via [`graphcal_io::OverlayFileSystem`]. The base
/// reader is sandboxed to the discovered project root (when a `graphcal.toml`
/// is reachable from the buffer's directory) — keeping the LSP's filesystem
/// access in lockstep with the CLI's. For untitled/non-file URIs, builds a
/// single-file project from the in-memory text alone.
fn build_project(uri: &Url, text: &str) -> std::result::Result<LoadedProject, Box<CompileError>> {
    let name = uri.as_str();
    match uri.to_file_path() {
        Ok(path) => {
            // OverlayFileSystem canonicalizes internally and falls back to the
            // raw path when the overlay file is not yet on disk — this makes
            // unsaved LSP buffers work without a preflight `FileNotFound`.
            let base = graphcal_eval::loader::build_rooted_filesystem(&path, None);
            let fs = graphcal_io::OverlayFileSystem::new(base, path.clone(), text.to_string());
            graphcal_eval::loader::load_project(&path, None, &fs).map_err(Box::new)
        }
        Err(()) => LoadedProject::from_source(text, name).map_err(Box::new),
    }
}

/// Wrap a single-URI diagnostic vec into the per-URI map shape so the active
/// document's URI is always present (even when empty) and so eval diagnostics
/// — which always belong to the active file — sit alongside any cross-file
/// parse/TIR diagnostics.
fn diagnostics_for_active_uri(uri: &Url, diags: Vec<Diagnostic>) -> HashMap<Url, Vec<Diagnostic>> {
    let mut out = HashMap::new();
    out.insert(uri.clone(), diags);
    out
}

/// Run the analysis pipeline, producing an `AnalysisResult`.
///
/// The pipeline has two stages:
/// 1. Build a `LoadedProject` from the in-memory text (+ disk imports).
/// 2. Compile TIR from the project.
///
/// Both stages use the same source text, eliminating data provenance mismatches.
#[cfg(test)]
pub(crate) fn run_analysis_for_test(uri: &Url, text: &str) -> AnalysisResult {
    run_analysis(uri, text)
}

fn run_analysis(uri: &Url, text: &str) -> AnalysisResult {
    // Stage 1: Build project (parse + load imports).
    // If this fails, no AST is available for the multi-file pipeline. Fall
    // back to parsing just the active buffer so hover/goto-def on the active
    // file's own symbols still answer — the imported-file error remains
    // visible, but local LSP features degrade gracefully.
    let project = match build_project(uri, text) {
        Ok(project) => project,
        Err(e) => {
            let mut diagnostics = compile_error_to_diagnostics_grouped(&e, uri);
            diagnostics.entry(uri.clone()).or_default();
            let symbol_table = LoadedProject::from_source(text, uri.as_str())
                .ok()
                .map(|single| {
                    let root_ast = &single.files[&single.root].ast;
                    symbol_table::build_from_ast(root_ast, text)
                })
                .unwrap_or_default();
            return AnalysisResult {
                source: Arc::new(text.to_string()),
                symbol_table,
                imported_definitions: HashMap::new(),
                diagnostics: Arc::new(diagnostics),
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(),
                import_links: Vec::new(),
            };
        }
    };

    let root_ast = &project.files[&project.root].ast;
    let import_links = collect_import_links(&project);

    // Stage 2: Compile TIR from the project.
    match compile_to_tir_from_project(&project) {
        Ok(tir) => {
            // Full success: symbol table from AST + TIR enrichment.
            let mut symbol_table = symbol_table::build_from_ast(root_ast, text);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir);

            let imported_definitions = collect_imported_definitions(uri, &project, Some(&tir));
            let fn_signatures = build_fn_signatures();
            // Library files (required param/index not yet bound) cannot be evaluated
            // standalone. Skip the eval pipeline so editors don't surface false-positive
            // `RequiredIndexNotBound` / `RequiredParamNotProvided` diagnostics when the
            // user opens such a file for editing.
            let (mut diagnostics, eval_values) = if tir.is_library() {
                (HashMap::new(), HashMap::new())
            } else {
                run_eval_from_project(&project, uri, text, &symbol_table)
            };
            diagnostics.entry(uri.clone()).or_default();

            AnalysisResult {
                source: Arc::new(text.to_string()),
                symbol_table,
                imported_definitions,
                diagnostics: Arc::new(diagnostics),
                eval_values,
                fn_signatures,
                import_links,
            }
        }
        Err(e) => {
            // TIR failed (type/dim error) but parse succeeded — use AST for partial info.
            let symbol_table = symbol_table::build_from_ast(root_ast, text);
            let imported_definitions = collect_imported_definitions(uri, &project, None);
            let mut diagnostics = compile_error_to_diagnostics_grouped(&e, uri);
            diagnostics.entry(uri.clone()).or_default();

            AnalysisResult {
                source: Arc::new(text.to_string()),
                symbol_table,
                imported_definitions,
                diagnostics: Arc::new(diagnostics),
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(),
                import_links,
            }
        }
    }
}

/// Run evaluation from a loaded project and extract diagnostics and formatted values.
fn run_eval_from_project(
    project: &LoadedProject,
    uri: &Url,
    text: &str,
    symbol_table: &SymbolTable,
) -> (HashMap<Url, Vec<Diagnostic>>, HashMap<DeclName, String>) {
    match compile_and_eval_from_project(project, &HashMap::new()) {
        Ok(result) => {
            let diagnostics = eval_result_to_diagnostics(&result, text, symbol_table);
            let values = format_eval_values(&result);
            (diagnostics_for_active_uri(uri, diagnostics), values)
        }
        Err(e) => {
            let mut diagnostics = compile_error_to_diagnostics_grouped(&e, uri);
            diagnostics.entry(uri.clone()).or_default();
            (diagnostics, HashMap::new())
        }
    }
}

/// Collect loader-resolved import links from the project for Document Links.
///
/// Uses `imports_with_paths()` and `includes_with_paths()` from the loader,
/// so document links agree with actual compilation behavior.
fn collect_import_links(project: &LoadedProject) -> Vec<ResolvedImportLink> {
    let Some(root_file) = project.files.get(&project.root) else {
        return Vec::new();
    };

    let import_links = root_file
        .imports_with_dag_ids()
        .map(|(_, import_decl, dag_id)| (import_decl.path.span(), dag_id));
    let include_links = root_file
        .includes_with_dag_ids()
        .map(|(_, include_decl, dag_id)| (include_decl.path.span(), dag_id));

    import_links
        .chain(include_links)
        .filter_map(|(span, dag_id)| {
            let loaded = project.files.get(dag_id)?;
            let uri = Url::from_file_path(&loaded.path).ok()?;
            Some(ResolvedImportLink {
                path_span: span,
                target_uri: uri,
            })
        })
        .collect()
}

/// Get builtin function signatures for Signature Help.
///
/// Computed once and cached in a static. Builtins never change at runtime.
pub(crate) fn build_fn_signatures() -> &'static HashMap<String, FnSignatureInfo> {
    static FN_SIGS: LazyLock<HashMap<String, FnSignatureInfo>> = LazyLock::new(|| {
        let mut sigs = HashMap::new();
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
    });
    &FN_SIGS
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
fn format_eval_values(result: &EvalResult) -> HashMap<DeclName, String> {
    let mut map = HashMap::new();
    for (name, value_result, _decl_type) in &result.all {
        if let Ok(value) = value_result {
            map.insert(
                name.clone(),
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
            let entries: Vec<(&str, &Value)> =
                fields.iter().map(|(k, v)| (k.as_str(), v)).collect();
            format_braced_entries(&format!("{type_name} "), &entries, symbols, max_len)
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
                let single: Vec<(&str, &Value)> =
                    entries.iter().map(|(k, v)| (k.as_str(), v)).collect();
                format_braced_entries("", &single, symbols, max_len)
            }
        }
    }
}

/// Format a list of key-value pairs as `{prefix}{ k1: v1, k2: v2, ... }`,
/// truncating with `...` when the result would exceed `max_len`.
///
/// `render_key` shapes each entry's key — e.g., `|k| k.to_string()` for
/// single-axis variants or `|keys| format!("({})", keys.join(", "))` for
/// tuple-keyed multi-axis entries.
fn format_entries<K>(
    prefix: &str,
    entries: &[(K, &Value)],
    render_key: impl Fn(&K) -> String,
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    let mut result = format!("{prefix}{{ ");
    let suffix = " }";
    let ellipsis = "... }";
    let total = entries.len();

    for (i, (key, val)) in entries.iter().enumerate() {
        let remaining_budget = max_len.saturating_sub(result.len() + suffix.len());
        let entry_str = format!(
            "{}: {}",
            render_key(key),
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

fn format_braced_entries(
    prefix: &str,
    entries: &[(&str, &Value)],
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    format_entries(prefix, entries, |k| (*k).to_string(), symbols, max_len)
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

fn format_tuple_keyed_entries(
    prefix: &str,
    entries: &[(Vec<&str>, &Value)],
    symbols: &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    format_entries(
        prefix,
        entries,
        |keys| format!("({})", keys.join(", ")),
        symbols,
        max_len,
    )
}

/// Collect imported definitions from a loaded project.
///
/// For each `import` and `include` declaration in the root file, uses the
/// loader-resolved DAG ids to look up the dependency in the project, and
/// builds a symbol table from the imported file's AST to extract the
/// definition info.
///
/// Selective items are keyed by their **local name** (alias if present,
/// otherwise the original name), so that references using the alias resolve
/// correctly in LSP features.
fn collect_imported_definitions(
    root_uri: &Url,
    project: &graphcal_eval::loader::LoadedProject,
    tir: Option<&graphcal_compiler::tir::typed::TIR>,
) -> HashMap<SymbolKey, ImportedDefinition> {
    let mut result = HashMap::new();

    let Some(root_file) = project.files.get(&project.root) else {
        return result;
    };

    // Cache symbol tables per dag_id to avoid re-building for files referenced
    // by multiple import/include declarations.
    let mut table_cache: HashMap<
        &graphcal_compiler::syntax::dag_id::DagId,
        (SymbolTable, Url, Arc<String>),
    > = HashMap::new();

    let imports = root_file
        .imports_with_dag_ids()
        .map(|(_, decl, dag_id)| (decl.path.display_path(), &decl.kind, dag_id));
    let includes = root_file
        .includes_with_dag_ids()
        .map(|(_, decl, dag_id)| (decl.path.display_path(), &decl.kind, dag_id));

    for (path_display, kind, dag_id) in imports.chain(includes) {
        let Some(loaded_file) = project.files.get(dag_id) else {
            continue;
        };

        let (imported_table, imported_uri, source) =
            table_cache.entry(dag_id).or_insert_with(|| {
                let mut table = symbol_table::build_from_ast(&loaded_file.ast, &loaded_file.source);
                if let Some(tir) = tir {
                    symbol_table::enrich_from_tir(&mut table, tir);
                }
                let uri = Url::from_file_path(&loaded_file.path).unwrap_or_else(|()| {
                    // Url::from_file_path only fails for non-absolute paths.
                    // The loader canonicalizes, so this should not happen — but
                    // emit to stderr (LSP clients surface this) rather than
                    // silently misattribute go-to-definition.
                    #[expect(
                        clippy::print_stderr,
                        clippy::unnecessary_debug_formatting,
                        reason = "developer-visible warning for an unreachable fallback"
                    )]
                    {
                        eprintln!(
                            "graphcal-lsp: Url::from_file_path failed for {:?}; falling back to root URI",
                            loaded_file.path,
                        );
                    }
                    root_uri.clone()
                });
                let src = Arc::clone(&loaded_file.source);
                (table, uri, src)
            });

        match kind {
            graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(items) => {
                for import_item in items {
                    let original_key = SymbolKey::TopLevel(import_item.name.name.clone());
                    let Some(def) = imported_table.definitions.get(&original_key) else {
                        continue;
                    };
                    let local_key = SymbolKey::TopLevel(import_item.local_name().to_string());
                    insert_imported_def(&mut result, local_key, imported_uri, source, def);
                }
            }
            graphcal_compiler::desugar::resolved_ast::ImportKind::Module { alias } => {
                let module_name = alias.as_ref().map_or_else(
                    || {
                        graphcal_eval::loader::derive_module_name(&path_display)
                            .unwrap_or_else(|stem| stem)
                    },
                    |alias_ident| alias_ident.name.clone(),
                );
                for (key, def) in &imported_table.definitions {
                    let qualified_key = match key {
                        SymbolKey::TopLevel(name) => SymbolKey::Qualified {
                            // The module alias here is a single identifier
                            // segment; lift it into the structured form to
                            // match other Qualified construction sites.
                            module: vec![module_name.clone()],
                            name: name.clone(),
                        },
                        other => other.clone(),
                    };
                    insert_imported_def(&mut result, qualified_key, imported_uri, source, def);
                }
            }
        }
    }

    result
}

/// Record a symbol from an imported file as visible in the current file under
/// `key`. Both `ImportKind` branches use this so the insertion semantics stay
/// identical — only the key derivation differs between them.
fn insert_imported_def(
    result: &mut HashMap<SymbolKey, ImportedDefinition>,
    key: SymbolKey,
    uri: &Url,
    source: &Arc<String>,
    def: &DefinitionInfo,
) {
    result.insert(
        key,
        ImportedDefinition {
            uri: uri.clone(),
            source: Arc::clone(source),
            definition: def.clone(),
        },
    );
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

        let generation = self.bump_generation(&uri).await;
        self.spawn_debounced_analysis(uri, change.text, generation);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.analyze_and_publish(params.text_document.uri, text)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        // Pull the previous diagnostic URI set out of the cached analysis so
        // we can clear cross-file diagnostics this document had published.
        let stale_uris: Vec<Url> = self
            .documents
            .read()
            .await
            .get(&uri)
            .map(|prev| prev.diagnostics.keys().cloned().collect())
            .unwrap_or_default();
        self.documents.write().await.remove(&uri);
        self.change_generations.write().await.remove(&uri);
        for stale in stale_uris {
            self.client
                .publish_diagnostics(stale, Vec::new(), None)
                .await;
        }
        // Always clear the closed document's URI even if it was never analyzed.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
        // No `inlay_hint_refresh` here — the document is gone; there's
        // nothing for the client to re-fetch hints for. The refresh call
        // at `analyze_and_publish` and `spawn_debounced_analysis` sites
        // covers the only cases where hints could meaningfully change.
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
            crate::document_links::document_links(analysis)
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

    /// Use an `untitled:` URI so `build_project` falls through to the in-memory
    /// `LoadedProject::from_source` branch (disk-less analysis).
    fn untitled_uri() -> Url {
        Url::parse("untitled:test.gcl").unwrap()
    }

    #[test]
    fn library_file_with_required_index_has_no_diagnostics() {
        let text = "\
pub(bind) dim Velocity = Length / Time;
pub(bind) dim Acceleration = Length / Time^2;

pub(bind) index Phase;
pub(bind) index Step: Time;
pub(bind) index Accel: Length / Time^2;
";
        let analysis = run_analysis(&untitled_uri(), text);
        assert!(
            analysis.has_no_diagnostics(),
            "library file should have no diagnostics, got: {:?}",
            analysis.diagnostics
        );
    }

    #[test]
    fn library_file_with_required_param_has_no_diagnostics() {
        let text = "\
param mass: Mass;
";
        let analysis = run_analysis(&untitled_uri(), text);
        assert!(
            analysis.has_no_diagnostics(),
            "file with required param should have no diagnostics, got: {:?}",
            analysis.diagnostics
        );
    }

    #[test]
    fn executable_file_with_dim_mismatch_still_reports_diagnostic() {
        // Not a library (no required params/indexes) — real dim errors must still
        // surface, i.e., the library bypass must not swallow genuine diagnostics.
        let text = "\
param mass: Mass = 1.0 kg;
param length: Length = 1.0 m;
node bad: Mass = mass + length;
";
        let analysis = run_analysis(&untitled_uri(), text);
        assert!(
            !analysis.has_no_diagnostics(),
            "dim mismatch in executable file must still produce a diagnostic",
        );
    }

    fn write_project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }
        dir
    }

    #[test]
    fn imported_definitions_collected_for_selective_includes() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"lib\"\n"),
            (
                "src/lib/lib.gcl",
                "param limit: Dimensionless = 100.0;\npub node doubled: Dimensionless = @limit * 2.0;\n",
            ),
            (
                "src/lib/main.gcl",
                "include lib.lib().{ doubled };\nnode result: Dimensionless = @doubled + 1.0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        let doubled_key = crate::symbol_table::SymbolKey::TopLevel("doubled".to_string());
        assert!(
            analysis.imported_definitions.contains_key(&doubled_key),
            "expected imported definition for selective include `doubled`, got: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn parse_error_in_imported_file_routes_to_imported_uri() {
        // lib.gcl has a syntax error; main.gcl is fine. The diagnostic must
        // surface on lib.gcl's URI, not main.gcl's URI.
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/lib.gcl", "this is not valid graphcal"),
            (
                "src/helper/main.gcl",
                "import helper.lib.{y};\nnode z: Dimensionless = 1.0;",
            ),
        ]);
        let main_path = dir.path().join("src/helper/main.gcl");
        let helper_path = dir.path().join("src/helper/lib.gcl");
        // The loader stores the canonical path on every NamedSource, so the
        // diagnostic's URI is built from the canonical form too. Canonicalize
        // here so the test matches what the LSP actually publishes (on macOS,
        // this turns `/var/folders/...` into `/private/var/folders/...`).
        let main_uri = Url::from_file_path(main_path.canonicalize().unwrap()).unwrap();
        let helper_uri = Url::from_file_path(helper_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text);

        let helper_diags = analysis
            .diagnostics
            .get(&helper_uri)
            .cloned()
            .unwrap_or_default();
        let main_diags = analysis
            .diagnostics
            .get(&main_uri)
            .cloned()
            .unwrap_or_default();
        assert!(
            !helper_diags.is_empty(),
            "expected parse error in helper.gcl to surface on its own URI; full map: {:?}",
            analysis.diagnostics,
        );
        assert!(
            main_diags.is_empty(),
            "main.gcl should not carry the parse error from the imported file; got: {main_diags:?}",
        );
        // The diagnostic's range must come from helper.gcl's offsets indexed
        // against helper.gcl's text — not main.gcl's. helper.gcl's content is
        // "this is not valid graphcal" (one line); the parse error fires on
        // the first token, so the range must start at line 0 with a small
        // column. If the bug regressed (offsets indexed against main.gcl),
        // either the column would diverge or the start position would be off
        // entirely.
        let primary = &helper_diags[0];
        assert_eq!(primary.range.start.line, 0);
        assert!(
            primary.range.end.character <= u32::try_from("this".len()).unwrap(),
            "expected range bounded by the first token of helper.gcl, got: {:?}",
            primary.range,
        );
    }

    #[test]
    fn parse_error_in_non_sibling_imported_file_routes_to_imported_uri() {
        // Regression guard for the structural-source/offset bug: when the
        // failing import is *not* a sibling of the active buffer (here, an
        // extra `util/` directory between main.gcl and lib.gcl), the
        // diagnostic must still land on lib.gcl's URI with offsets indexed
        // against lib.gcl's text. The pre-fix code's sibling resolver would
        // fail to find lib.gcl from main.gcl's directory, leak the error
        // onto main.gcl's URI, and clamp the offset against main.gcl's line
        // index — producing a misleading squiggle on innocent code.
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/util/lib.gcl", "this is not valid graphcal"),
            (
                "src/helper/main.gcl",
                "import helper.util.lib.{y};\nnode z: Dimensionless = 1.0;",
            ),
        ]);
        let main_path = dir.path().join("src/helper/main.gcl");
        let lib_path = dir.path().join("src/helper/util/lib.gcl");
        let main_uri = Url::from_file_path(main_path.canonicalize().unwrap()).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text);

        let lib_diags = analysis
            .diagnostics
            .get(&lib_uri)
            .cloned()
            .unwrap_or_default();
        let main_diags = analysis
            .diagnostics
            .get(&main_uri)
            .cloned()
            .unwrap_or_default();
        assert!(
            !lib_diags.is_empty(),
            "expected parse error in util/lib.gcl to surface on its own URI even when not a sibling of main.gcl; full map: {:?}",
            analysis.diagnostics,
        );
        assert!(
            main_diags.is_empty(),
            "main.gcl must not carry the parse error from a non-sibling import; got: {main_diags:?}",
        );
    }

    #[test]
    fn imported_definitions_keyed_by_local_alias_for_selective_imports() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            (
                "src/helper/lib.gcl",
                "pub const node y: Dimensionless = 2.0;",
            ),
            (
                "src/helper/main.gcl",
                "import helper.lib.{y as renamed};\nnode z: Dimensionless = @renamed + 1.0;",
            ),
        ]);
        let main_path = dir.path().join("src/helper/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        let renamed_key = crate::symbol_table::SymbolKey::TopLevel("renamed".to_string());
        assert!(
            analysis.imported_definitions.contains_key(&renamed_key),
            "expected imported definition keyed by local alias `renamed`, got: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>(),
        );
    }
}
