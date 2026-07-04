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

use crate::convert::position_to_byte_offset;
use crate::diagnostics::{compile_error_to_diagnostics_grouped, eval_result_to_diagnostics};
use crate::symbol_table::{self, DefinitionInfo, SymbolCategory, SymbolKey, SymbolTable};
use graphcal_compiler::dimension::{BaseDimId, Dimension, Rational};
use graphcal_compiler::function_signature::{DimMonomial, FunctionSignature, ValueKind};
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_eval::eval::{
    CompileError, EvalResult, Value, compile_and_eval_from_project, compile_to_tir_from_project,
};
use graphcal_eval::loader::LoadedProject;

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
    pub(crate) eval_values: HashMap<ScopedName, String>,
    /// Structured function signatures, keyed by function name.
    /// Points to a lazily-initialized static map (builtins never change).
    pub(crate) fn_signatures: &'static HashMap<String, FnSignatureInfo>,
    /// Extern (plugin) function signatures from this file's `import plugin`
    /// blocks, keyed by the qualified `alias.name` call spelling. Per-file,
    /// unlike the static builtin map.
    pub(crate) extern_fn_signatures: HashMap<String, FnSignatureInfo>,
    /// Loader-resolved import links (for Document Links).
    pub(crate) import_links: Vec<ResolvedImportLink>,
    /// `false` when this result is a parse-failure fallback: the buffer did
    /// not parse, so the symbol-dependent fields are empty placeholders and
    /// only `diagnostics` is meaningful. [`store_analysis`] keeps the
    /// previous good symbol state in that case (#834) — mid-edit buffers are
    /// unparsable more often than not, and completion/hover/goto should keep
    /// answering from the last successfully analyzed state.
    pub(crate) buffer_parsed: bool,
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
    /// Latest document text as typed in the editor, updated synchronously on
    /// every `did_open`/`did_change`/`did_save`. May be newer than
    /// `AnalysisResult::source` (analysis is debounced and can fail or time
    /// out), so text-sensitive requests fired right after a keystroke —
    /// trigger-character completion, signature help, formatting — must read
    /// this instead of the analyzed snapshot.
    latest_text: Arc<RwLock<HashMap<Url, Arc<String>>>>,
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
            .field(
                "extern_fn_signatures_count",
                &self.extern_fn_signatures.len(),
            )
            .field("import_links_count", &self.import_links.len())
            .field("buffer_parsed", &self.buffer_parsed)
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

    /// Record the latest editor text for `uri`, synchronously with the
    /// notification that delivered it.
    async fn record_latest_text(&self, uri: &Url, text: &str) {
        self.latest_text
            .write()
            .await
            .insert(uri.clone(), Arc::new(text.to_string()));
    }

    /// Latest editor text for `uri`, falling back to the analyzed snapshot.
    async fn current_text(&self, uri: &Url) -> Option<Arc<String>> {
        if let Some(text) = self.latest_text.read().await.get(uri) {
            return Some(Arc::clone(text));
        }
        self.documents
            .read()
            .await
            .get(uri)
            .map(|analysis| Arc::clone(&analysis.source))
    }

    async fn analyze_and_publish(&self, uri: Url, text: String) {
        if !Self::is_graphcal_file(&uri) {
            return;
        }

        self.record_latest_text(&uri, &text).await;

        // Bump the generation so any in-flight debounced analysis for this URI
        // becomes stale and refuses to overwrite fresh results.
        let generation = self.bump_generation(&uri).await;

        analyze_store_publish(
            &self.client,
            &self.documents,
            &self.change_generations,
            &self.latest_text,
            uri,
            text,
            generation,
        )
        .await;
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
        let latest_text = self.latest_text.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DEBOUNCE_DELAY_MS)).await;

            // Check if a newer change has superseded this one.
            if !is_generation_current(&generations, &uri, generation).await {
                return;
            }

            analyze_store_publish(
                &client,
                &documents,
                &generations,
                &latest_text,
                uri,
                text,
                generation,
            )
            .await;
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
}

/// Run the (blocking, timeout-guarded) analysis for `uri` and store/publish
/// the result. Shared by the immediate (`did_open`/`did_save`) and debounced
/// (`did_change`) paths so the panic/timeout/store sequence lives once.
async fn analyze_store_publish(
    client: &Client,
    documents: &Arc<RwLock<HashMap<Url, AnalysisResult>>>,
    generations: &Arc<RwLock<HashMap<Url, u64>>>,
    latest_text: &Arc<RwLock<HashMap<Url, Arc<String>>>>,
    uri: Url,
    text: String,
    generation: u64,
) {
    // Snapshot every *other* open document's latest text: the analysis
    // overlays them onto the filesystem so imports of open-but-dirty files
    // see the editor state, not stale disk content. The analyzed document
    // itself uses `text` (this analysis pass's snapshot).
    let open_buffers: Vec<OpenBuffer> = latest_text
        .read()
        .await
        .iter()
        .filter(|(open_uri, _)| **open_uri != uri)
        .filter_map(|(open_uri, open_text)| {
            open_uri.to_file_path().ok().map(|path| OpenBuffer {
                path,
                text: Arc::clone(open_text),
            })
        })
        .collect();
    let uri_for_analysis = uri.clone();
    let task =
        tokio::task::spawn_blocking(move || run_analysis(&uri_for_analysis, &text, &open_buffers));
    let analysis = match tokio::time::timeout(ANALYSIS_TIMEOUT, task).await {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            client
                .log_message(MessageType::ERROR, format!("analysis task panicked: {e}"))
                .await;
            return;
        }
        Err(_elapsed) => {
            publish_analysis_timeout(client, &uri).await;
            return;
        }
    };

    // Hold the documents write lock across the generation re-check and
    // the insert: a newer-generation analysis completing between a
    // separate check and insert used to be clobbered by this older one.
    let mut docs = documents.write().await;
    if !is_generation_current(generations, &uri, generation).await {
        return;
    }
    // Every URI this analysis touches — newly reported plus previously
    // reported (which may need clearing) — is re-published from the
    // merged view of all open documents, so one document's analysis
    // cannot clobber diagnostics another open document owns.
    let mut affected: Vec<Url> = analysis.diagnostics.keys().cloned().collect();
    if let Some(prev) = docs.get(&uri) {
        for prev_uri in prev.diagnostics.keys() {
            if !affected.contains(prev_uri) {
                affected.push(prev_uri.clone());
            }
        }
    }
    store_analysis(&mut docs, &uri, analysis);
    let publish = merged_diagnostics_for(&docs, &affected);
    drop(docs);
    for (target_uri, diags) in publish {
        client.publish_diagnostics(target_uri, diags, None).await;
    }
    // Best-effort: the client may not support inlay-hint refresh.
    let _ = client.inlay_hint_refresh().await;
}

/// Store a fresh analysis for `uri`, keeping the previous symbol-dependent
/// state when the new result is a parse-failure fallback.
///
/// Mid-edit buffers fail to parse more often than not (the parser has no
/// error recovery), so replacing the cached analysis with an empty symbol
/// table would make completion/hover/goto go dark ~300 ms after every
/// keystroke (#834). Instead, only the diagnostics are refreshed; symbol
/// queries keep answering from the last successfully analyzed state (whose
/// `source` snapshot they remain consistent with).
fn store_analysis(docs: &mut HashMap<Url, AnalysisResult>, uri: &Url, analysis: AnalysisResult) {
    if !analysis.buffer_parsed
        && let Some(prev) = docs.get_mut(uri)
    {
        prev.diagnostics = analysis.diagnostics;
        return;
    }
    docs.insert(uri.clone(), analysis);
}

/// Compute the diagnostics to publish for each URI in `targets` from the
/// whole open-document set.
///
/// Ownership rule (#832): a URI that is itself an open, analyzed document is
/// authoritative for its own diagnostics — its analysis ran on the exact
/// buffer the user sees. For URIs that are not open (imported files on
/// disk), the contributions of every open document are merged, deduplicating
/// identical diagnostics reported by multiple importers. A URI no open
/// document reports on yields an empty list, clearing stale squiggles.
fn merged_diagnostics_for(
    docs: &HashMap<Url, AnalysisResult>,
    targets: &[Url],
) -> Vec<(Url, Vec<Diagnostic>)> {
    targets
        .iter()
        .map(|target| {
            if let Some(own) = docs.get(target) {
                let diags = own.diagnostics.get(target).cloned().unwrap_or_default();
                return (target.clone(), diags);
            }
            let mut merged: Vec<Diagnostic> = Vec::new();
            for analysis in docs.values() {
                for diag in analysis.diagnostics.get(target).into_iter().flatten() {
                    if !merged.contains(diag) {
                        merged.push(diag.clone());
                    }
                }
            }
            (target.clone(), merged)
        })
        .collect()
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

/// Check whether `generation` is still the current generation stored for `uri`.
async fn is_generation_current(
    generations: &Arc<RwLock<HashMap<Url, u64>>>,
    uri: &Url,
    generation: u64,
) -> bool {
    *generations.read().await.get(uri).unwrap_or(&0) == generation
}

/// Snapshot of another open editor buffer, overlaid onto the filesystem
/// during analysis so cross-file diagnostics, goto-definition targets, and
/// hover reflect what the user actually sees instead of stale disk content.
pub(crate) struct OpenBuffer {
    pub(crate) path: std::path::PathBuf,
    pub(crate) text: Arc<String>,
}

/// Build a `LoadedProject` from a URI and in-memory text.
///
/// For file-backed URIs, loads the project from disk with the in-memory text
/// overlaid on the root file — and every other open document's latest text
/// overlaid on its path — via [`graphcal_io::OverlayFileSystem`]. The base
/// reader is sandboxed to the discovered project root (when a `graphcal.toml`
/// is reachable from the buffer's directory) — keeping the LSP's filesystem
/// access in lockstep with the CLI's. For untitled/non-file URIs, builds a
/// single-file project from the in-memory text alone.
fn build_project(
    uri: &Url,
    text: &str,
    open_buffers: &[OpenBuffer],
) -> std::result::Result<LoadedProject, Box<CompileError>> {
    let name = uri.as_str();
    match uri.to_file_path() {
        Ok(path) => {
            // OverlayFileSystem canonicalizes internally and falls back to the
            // raw path when the overlay file is not yet on disk — this makes
            // unsaved LSP buffers work without a preflight `FileNotFound`.
            // The analyzed snapshot comes first so it wins over any (possibly
            // newer) latest-text entry for the same file: the produced
            // `AnalysisResult` must stay consistent with `text`.
            let base = graphcal_eval::loader::build_rooted_filesystem(&path, None);
            let overlays = std::iter::once((path.clone(), text.to_string())).chain(
                open_buffers
                    .iter()
                    .map(|buffer| (buffer.path.clone(), buffer.text.as_ref().clone())),
            );
            let fs = graphcal_io::OverlayFileSystem::with_overlays(base, overlays);
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
    run_analysis(uri, text, &[])
}

fn run_analysis(uri: &Url, text: &str, open_buffers: &[OpenBuffer]) -> AnalysisResult {
    // Stage 1: Build project (parse + load imports).
    // If this fails, no AST is available for the multi-file pipeline. Fall
    // back to parsing just the active buffer so hover/goto-def on the active
    // file's own symbols still answer — the imported-file error remains
    // visible, but local LSP features degrade gracefully.
    let project = match build_project(uri, text, open_buffers) {
        Ok(project) => project,
        Err(e) => {
            let mut diagnostics = compile_error_to_diagnostics_grouped(&e, uri);
            diagnostics.entry(uri.clone()).or_default();
            // `Some` when the failure was in an *import* (the buffer itself
            // parses): the buffer's own symbols are still fully usable.
            // `None` when the buffer doesn't parse — the result is then a
            // diagnostics-only fallback and `store_analysis` retains the
            // previous symbol state (#834).
            let parsed_buffer_table =
                LoadedProject::from_source(text, uri.as_str())
                    .ok()
                    .map(|single| {
                        let root_ast = &single.files[&single.root].ast;
                        symbol_table::build_for_buffer(root_ast, text)
                    });
            let buffer_parsed = parsed_buffer_table.is_some();
            return AnalysisResult {
                source: Arc::new(text.to_string()),
                symbol_table: parsed_buffer_table.unwrap_or_default(),
                imported_definitions: HashMap::new(),
                diagnostics: Arc::new(diagnostics),
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(),
                extern_fn_signatures: HashMap::new(),
                import_links: Vec::new(),
                buffer_parsed,
            };
        }
    };

    let root_ast = &project.files[&project.root].ast;
    let import_links = collect_import_links(&project);
    // The project resolver backs the symbol table's reference walk: bodies
    // are tolerantly lowered to HIR and references keyed from canonical
    // identities. A resolver failure (e.g. duplicate symbols) degrades to
    // an empty resolver — references then surface via the spelling fallback.
    let module_resolver = project.build_module_resolver().unwrap_or_default();

    // Stage 2: Compile TIR from the project.
    match compile_to_tir_from_project(&project) {
        Ok(tir) => {
            // Full success: symbol table from AST + TIR enrichment.
            let mut symbol_table =
                symbol_table::build_from_ast(root_ast, text, &project.root, &module_resolver);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir, &project.root);

            let imported_definitions =
                collect_imported_definitions(uri, &project, Some(&tir), &module_resolver);
            let fn_signatures = build_fn_signatures();
            let extern_fn_signatures = build_extern_fn_signatures(&tir);
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
                extern_fn_signatures,
                import_links,
                buffer_parsed: true,
            }
        }
        Err(e) => {
            // TIR failed (type/dim error) but parse succeeded — use AST for partial info.
            let symbol_table =
                symbol_table::build_from_ast(root_ast, text, &project.root, &module_resolver);
            let imported_definitions =
                collect_imported_definitions(uri, &project, None, &module_resolver);
            let mut diagnostics = compile_error_to_diagnostics_grouped(&e, uri);
            diagnostics.entry(uri.clone()).or_default();

            AnalysisResult {
                source: Arc::new(text.to_string()),
                symbol_table,
                imported_definitions,
                diagnostics: Arc::new(diagnostics),
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(),
                extern_fn_signatures: HashMap::new(),
                import_links,
                buffer_parsed: true,
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
) -> (HashMap<Url, Vec<Diagnostic>>, HashMap<ScopedName, String>) {
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

/// Build extern (plugin) function signatures for Signature Help, keyed by
/// the qualified `alias.name` call spelling.
///
/// Unlike builtins, extern signatures are per-file (they depend on the
/// file's `import plugin` blocks and its registry's dimension names).
fn build_extern_fn_signatures(
    tir: &graphcal_compiler::tir::typed::TIR,
) -> HashMap<String, FnSignatureInfo> {
    use graphcal_compiler::function_signature::ValueKind as ExternValueKind;

    let mut sigs = HashMap::new();
    for function in tir.extern_functions.values() {
        let format_kind = |kind: &ExternValueKind| match kind {
            ExternValueKind::Bool => "Bool".to_string(),
            ExternValueKind::Int => "Int".to_string(),
            ExternValueKind::Scalar(monomial) => {
                let mut parts: Vec<String> = monomial
                    .vars
                    .iter()
                    .map(|factor| {
                        if factor.power == Rational::ONE {
                            factor.var.to_string()
                        } else {
                            format!("{}^({})", factor.var, factor.power)
                        }
                    })
                    .collect();
                if !monomial.fixed.is_dimensionless() {
                    parts.push(tir.registry.dimensions.format_dimension(&monomial.fixed));
                }
                if parts.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    parts.join(" * ")
                }
            }
        };
        let parameters: Vec<String> = function
            .signature
            .params()
            .iter()
            .map(|param| format!("{}: {}", param.name, format_kind(&param.kind)))
            .collect();
        let binders = if function.signature.dim_vars().is_empty() {
            String::new()
        } else {
            let vars: Vec<&str> = function
                .signature
                .dim_vars()
                .iter()
                .map(graphcal_compiler::syntax::dimension::DimVarName::as_str)
                .collect();
            format!("<{}>", vars.join(", "))
        };
        let qualified = format!("{}.{}", function.alias, function.name);
        let label = format!(
            "fn {qualified}{binders}({}) -> {}",
            parameters.join(", "),
            format_kind(function.signature.result())
        );
        sigs.insert(
            qualified,
            FnSignatureInfo {
                label,
                parameters,
            },
        );
    }
    sigs
}

/// Get builtin function signatures for Signature Help.
///
/// Computed once and cached in a static. Builtins never change at runtime.
pub(crate) fn build_fn_signatures() -> &'static HashMap<String, FnSignatureInfo> {
    static FN_SIGS: LazyLock<HashMap<String, FnSignatureInfo>> = LazyLock::new(|| {
        let mut sigs = HashMap::new();
        for (name, f) in builtin_functions() {
            let (params, ret) = builtin_signature_parts(&f.signature).unwrap_or_else(|err| {
                (
                    vec![format!("<invalid builtin signature: {err}>")],
                    "<invalid>".to_string(),
                )
            });
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

/// Format a dimension for display in builtin signatures.
///
/// Builtin signatures are defined only in terms of prelude dimensions, so no
/// per-file registry is needed here. A user-defined base dimension in this path
/// would be an internal bug in builtin construction.
fn format_dim_display(dim: &Dimension) -> std::result::Result<String, String> {
    if dim.is_dimensionless() {
        return Ok("Dimensionless".to_string());
    }
    let parts = dim
        .iter()
        .map(|(id, exp)| {
            let name = builtin_base_dim_name(id)?;
            Ok(if *exp == Rational::ONE {
                name.to_string()
            } else {
                format!("{name}^{exp}")
            })
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;
    Ok(parts.join(" * "))
}

fn builtin_base_dim_name(id: &BaseDimId) -> std::result::Result<&str, String> {
    match id {
        BaseDimId::Prelude(name) => Ok(name.as_str()),
        BaseDimId::UserDefined { .. } => Err(format!(
            "builtin signature unexpectedly referenced user-defined dimension {id:?}"
        )),
    }
}

/// Generate human-readable parameter and return type strings for a builtin function.
fn builtin_signature_parts(
    sig: &FunctionSignature,
) -> std::result::Result<(Vec<String>, String), String> {
    let params: Vec<String> = sig
        .params()
        .iter()
        .map(|p| Ok(format!("{}: {}", p.name, value_kind_display(&p.kind)?)))
        .collect::<std::result::Result<_, String>>()?;

    let ret = value_kind_display(sig.result())?;

    Ok((params, ret))
}

fn value_kind_display(kind: &ValueKind) -> std::result::Result<String, String> {
    match kind {
        ValueKind::Bool => Ok("Bool".to_string()),
        ValueKind::Int => Ok("Int".to_string()),
        ValueKind::Scalar(monomial) => monomial_display(monomial),
    }
}

fn monomial_display(monomial: &DimMonomial) -> std::result::Result<String, String> {
    let mut parts: Vec<String> = monomial
        .vars
        .iter()
        .map(|factor| {
            if factor.power == Rational::ONE {
                factor.var.to_string()
            } else {
                format!("{}^({})", factor.var, factor.power)
            }
        })
        .collect();
    if !monomial.fixed.is_dimensionless() {
        parts.push(format_dim_display(&monomial.fixed)?);
    }
    if parts.is_empty() {
        return Ok("Dimensionless".to_string());
    }
    Ok(parts.join(" * "))
}

/// Format all successfully evaluated values into display strings.
fn format_eval_values(result: &EvalResult) -> HashMap<ScopedName, String> {
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
/// - Constructor value: `"LowThrust(thrust: 0.5 [N], duration: 3600 [s])"`
/// - Unit constructor value: `"Nominal"`
/// - Indexed: `"{ Departure: 4.92 [km/s], Correction: 0.24 [km/s], ... }"`
fn format_value_inline(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
) -> String {
    format_value_inline_with_budget(value, symbols, INLAY_HINT_MAX_LEN)
}

/// Format a `Value` with a character budget. When the formatted entries would
/// exceed `max_len`, remaining entries are replaced with `...`.
fn format_value_inline_with_budget(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
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
                return type_name.as_str().to_string();
            }
            let entries: Vec<(&str, &Value)> =
                fields.iter().map(|(k, v)| (k.as_str(), v)).collect();
            format_parenthesized_entries(type_name.as_str(), &entries, symbols, max_len)
        }
        Value::Indexed { entries, .. } => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            // For multi-indexed maps (nested Indexed values), flatten into
            // tuple-keyed form: `{ (A, X): 1, (A, Y): 2, (B, X): 3 }` instead
            // of nested braces: `{ A: { X: 1, Y: 2 }, B: { X: 3 } }`.
            let mut flat: Vec<(Vec<String>, &Value)> = Vec::new();
            flatten_indexed_entries(value, &mut Vec::new(), &mut flat);
            let is_multi = flat.first().is_some_and(|(keys, _)| keys.len() > 1);
            if is_multi {
                format_tuple_keyed_entries("", &flat, symbols, max_len)
            } else {
                let single: Vec<(String, &Value)> = entries
                    .iter()
                    .map(|(k, v)| (value.indexed_entry_display_name(k), v))
                    .collect();
                format_entries("", &single, Clone::clone, symbols, max_len)
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
    symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    format_delimited_entries(
        EntryListLayout {
            prefix,
            open: "{ ",
            close: " }",
            ellipsis: "... }",
        },
        entries,
        render_key,
        symbols,
        max_len,
    )
}

#[derive(Clone, Copy)]
struct EntryListLayout<'a> {
    prefix: &'a str,
    open: &'a str,
    close: &'a str,
    ellipsis: &'a str,
}

fn format_delimited_entries<K>(
    layout: EntryListLayout<'_>,
    entries: &[(K, &Value)],
    render_key: impl Fn(&K) -> String,
    symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    let mut result = format!("{}{}", layout.prefix, layout.open);
    let total = entries.len();

    for (i, (key, val)) in entries.iter().enumerate() {
        let remaining_budget = max_len.saturating_sub(result.len() + layout.close.len());
        let entry_str = format!(
            "{}: {}",
            render_key(key),
            format_value_inline_with_budget(val, symbols, remaining_budget)
        );

        let separator = if i + 1 < total { ", " } else { "" };
        let needed = entry_str.len() + separator.len();

        if i > 0 && result.len() + needed + layout.close.len() > max_len {
            result.push_str(layout.ellipsis);
            return result;
        }

        result.push_str(&entry_str);
        if i + 1 < total {
            result.push_str(", ");
        }
    }

    result.push_str(layout.close);
    result
}

fn format_parenthesized_entries(
    prefix: &str,
    entries: &[(&str, &Value)],
    symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    format_delimited_entries(
        EntryListLayout {
            prefix,
            open: "(",
            close: ")",
            ellipsis: "...)",
        },
        entries,
        |k| (*k).to_string(),
        symbols,
        max_len,
    )
}

/// Recursively flatten nested `Indexed` values into a list of `(key_path, leaf_value)` pairs.
///
/// For a single-level `Indexed { A: 1, B: 2 }`, produces `[([A], 1), ([B], 2)]`.
/// For a nested `Indexed { A: Indexed { X: 1, Y: 2 }, B: Indexed { X: 3 } }`,
/// produces `[([A, X], 1), ([A, Y], 2), ([B, X], 3)]`.
fn flatten_indexed_entries<'a>(
    value: &'a Value,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'a Value)>,
) {
    let Value::Indexed { entries, .. } = value else {
        return;
    };
    for (key, val) in entries {
        prefix.push(value.indexed_entry_display_name(key));
        if matches!(val, Value::Indexed { .. }) {
            flatten_indexed_entries(val, prefix, out);
        } else {
            out.push((prefix.clone(), val));
        }
        prefix.pop();
    }
}

fn format_tuple_keyed_entries(
    prefix: &str,
    entries: &[(Vec<String>, &Value)],
    symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
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
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
) -> HashMap<SymbolKey, ImportedDefinition> {
    let mut result = HashMap::new();

    let Some(root_file) = project.files.get(&project.root) else {
        return result;
    };

    // Cache symbol tables per dag_id to avoid re-building for files referenced
    // by multiple import/include declarations.
    let mut table_cache: HashMap<
        &graphcal_compiler::dag_id::DagId,
        (SymbolTable, Url, Arc<String>),
    > = HashMap::new();

    let imports = root_file
        .imports_with_dag_ids()
        .map(|(_, decl, dag_id)| (&decl.path, &decl.kind, dag_id));
    let includes = root_file
        .includes_with_dag_ids()
        .map(|(_, decl, dag_id)| (&decl.path, &decl.kind, dag_id));

    for (path, kind, dag_id) in imports.chain(includes) {
        let Some(loaded_file) = project.files.get(dag_id) else {
            continue;
        };

        let (imported_table, imported_uri, source) =
            table_cache.entry(dag_id).or_insert_with(|| {
                let mut table = symbol_table::build_from_ast(
                    &loaded_file.ast,
                    &loaded_file.source,
                    dag_id,
                    module_resolver,
                );
                if let Some(tir) = tir {
                    symbol_table::enrich_from_tir(&mut table, tir, dag_id);
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
            graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => {
                for import_item in items {
                    let original_name = import_item.name.name.clone();
                    let local_name = import_item.local_name().to_string();
                    // Bring across the named definition itself plus every
                    // related entry that "belongs to" that name in the
                    // imported file's table: variants of an index/union,
                    // fields of a struct, and qualified body members of a
                    // DAG. Without this, goto-def / hover / find-references
                    // on tokens like `Color.Red` (variant) or `@scale(...).out`
                    // (DAG body member) miss because their non-TopLevel keys
                    // would never travel with the selective import.
                    for (key, def) in &imported_table.definitions {
                        let Some(local_key) = rekey_selective_import(
                            key,
                            def.category,
                            import_item.namespace,
                            original_name.as_str(),
                            &local_name,
                        ) else {
                            continue;
                        };
                        insert_imported_def(&mut result, local_key, imported_uri, source, def);
                    }
                }
            }
            graphcal_compiler::desugar::desugared_ast::ImportKind::Module { alias } => {
                // The local module qualifier is the alias when written,
                // otherwise the import path's leaf segment (`lib` for
                // `import gotom.lib;`). The structured path is used directly:
                // feeding its dotted display form to a *filesystem*-path
                // helper used to truncate `gotom.lib` to `gotom`, keying
                // every imported definition under a qualifier no reference
                // could ever produce (#830).
                let module_name = alias.as_ref().map_or_else(
                    || path.leaf().name.to_string(),
                    |alias_ident| alias_ident.value.to_string(),
                );
                for (key, def) in &imported_table.definitions {
                    let qualified_key = rekey_module_import(key, &module_name);
                    insert_imported_def(&mut result, qualified_key, imported_uri, source, def);
                }
            }
        }
    }

    result
}

/// Re-key an imported-file table entry for a selective import (`import lib.{X};`).
///
/// Returns `Some(local_key)` if `key` denotes the imported name `original` or
/// something semantically attached to it (its variants, fields, qualified body
/// members) — with the parent / qualifier rewritten to the local alias `local`.
/// Returns `None` for unrelated entries or entries in a namespace the import
/// item did not request (for example, `import lib.{type Student}` must not also
/// import the `Student` constructor).
fn rekey_selective_import(
    key: &SymbolKey,
    category: SymbolCategory,
    namespace: graphcal_compiler::desugar::desugared_ast::ImportItemNamespace,
    original: &str,
    local: &str,
) -> Option<SymbolKey> {
    if !selective_import_allows_category(namespace, category) {
        return None;
    }

    match key {
        SymbolKey::TopLevel(name) if name == original => {
            Some(SymbolKey::TopLevel(local.to_string()))
        }
        SymbolKey::Constructor(path) => path
            .rekey_first_segment(original, local)
            .map(SymbolKey::Constructor),
        SymbolKey::Variant { parent, variant } => {
            parent
                .rekey_first_segment(original, local)
                .map(|parent| SymbolKey::Variant {
                    parent,
                    variant: variant.clone(),
                })
        }
        SymbolKey::Field { owner, field_name } => {
            owner
                .rekey_first_segment(original, local)
                .map(|owner| SymbolKey::Field {
                    owner,
                    field_name: field_name.clone(),
                })
        }
        SymbolKey::Qualified { module, name } if module.first().is_some_and(|m| m == original) => {
            let mut rekeyed = Vec::with_capacity(module.len());
            rekeyed.push(local.to_string());
            rekeyed.extend(module.iter().skip(1).cloned());
            Some(SymbolKey::Qualified {
                module: rekeyed,
                name: name.clone(),
            })
        }
        _ => None,
    }
}

const fn selective_import_allows_category(
    namespace: graphcal_compiler::desugar::desugared_ast::ImportItemNamespace,
    category: SymbolCategory,
) -> bool {
    match namespace {
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Type => {
            matches!(category, SymbolCategory::StructType | SymbolCategory::Field)
        }
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Default => {
            !matches!(category, SymbolCategory::StructType)
        }
    }
}

/// Re-key an imported-file table entry for a module import (`import lib as m;`).
///
/// `TopLevel(x)` becomes `Qualified { module: [m], name: x }`. `Qualified` keys
/// nest the module alias as a new outer segment so `Qualified { module: [dag], name: out }`
/// in the imported file becomes `Qualified { module: [m, dag], name: out }` here —
/// matching the call-site key produced for `@m.dag(args).out`. Variant,
/// constructor, and field parents are re-keyed structurally, so
/// `module.Index.Variant` and `module.Constructor(field: ...)` navigate to the
/// declaration that owns the imported module rather than a same-leaf local item.
fn rekey_module_import(key: &SymbolKey, module_name: &str) -> SymbolKey {
    match key {
        SymbolKey::TopLevel(name) => SymbolKey::Qualified {
            module: vec![module_name.to_string()],
            name: name.clone(),
        },
        SymbolKey::Qualified { module, name } => SymbolKey::Qualified {
            module: crate::symbol_table::prepend_segment(module_name, module),
            name: name.clone(),
        },
        SymbolKey::Constructor(path) => SymbolKey::Constructor(path.prepend_module(module_name)),
        SymbolKey::Variant { parent, variant } => SymbolKey::Variant {
            parent: parent.prepend_module(module_name),
            variant: variant.clone(),
        },
        SymbolKey::Field { owner, field_name } => SymbolKey::Field {
            owner: owner.prepend_module(module_name),
            field_name: field_name.clone(),
        },
        other @ SymbolKey::ExprScoped { .. } => other.clone(),
    }
}

/// Record a symbol from an imported file as visible in the current file under
/// `key`. Both `ImportKind` branches use this so the insertion semantics stay
/// identical — only the key derivation differs between them.
/// Render the *local* spelling of an imported symbol from its re-keyed
/// [`SymbolKey`], for completion labels and hover titles. The defining
/// file's spelling (`def.name`) is wrong once the import renames
/// (`import lib.{y as renamed};`) or module-qualifies (`import lib as m;`)
/// the symbol — offering `y` instead of `renamed` produces an identifier
/// that does not resolve in the importing file.
fn local_display_name(key: &SymbolKey) -> Option<String> {
    match key {
        SymbolKey::TopLevel(name) => Some(name.clone()),
        SymbolKey::Qualified { module, name } => {
            let mut rendered = module.join(".");
            rendered.push('.');
            rendered.push_str(name);
            Some(rendered)
        }
        // Constructors, variants, fields, and expression locals keep the
        // definition's spelling: their display contexts render the parent
        // path separately.
        SymbolKey::Constructor(_)
        | SymbolKey::Variant { .. }
        | SymbolKey::Field { .. }
        | SymbolKey::ExprScoped { .. } => None,
    }
}

fn insert_imported_def(
    result: &mut HashMap<SymbolKey, ImportedDefinition>,
    key: SymbolKey,
    uri: &Url,
    source: &Arc<String>,
    def: &DefinitionInfo,
) {
    let mut definition = def.clone();
    if let Some(local) = local_display_name(&key) {
        definition.name = local;
    }
    result.insert(
        key,
        ImportedDefinition {
            uri: uri.clone(),
            source: Arc::clone(source),
            definition,
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

        // Record the new text synchronously: completion/signature-help fire
        // on trigger characters milliseconds after this notification, long
        // before the debounced analysis lands.
        self.record_latest_text(&uri, &change.text).await;
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
        // Re-publish every URI the closing document reported on from the
        // *remaining* open documents' merged view: closing one document only
        // removes that document's contribution — a URI another open document
        // still reports on keeps its diagnostics (#832). The closed URI
        // itself is always included so its squiggles clear even if this
        // document was never analyzed.
        let publish = {
            let mut docs = self.documents.write().await;
            let mut affected: Vec<Url> = docs
                .get(&uri)
                .map(|prev| prev.diagnostics.keys().cloned().collect())
                .unwrap_or_default();
            if !affected.contains(&uri) {
                affected.push(uri.clone());
            }
            docs.remove(&uri);
            let publish = merged_diagnostics_for(&docs, &affected);
            drop(docs);
            publish
        };
        self.change_generations.write().await.remove(&uri);
        self.latest_text.write().await.remove(&uri);
        for (target_uri, diags) in publish {
            self.client
                .publish_diagnostics(target_uri, diags, None)
                .await;
        }
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
        // Signature help fires on `(`/`,` against the just-typed text,
        // which is ahead of the debounced analysis snapshot.
        let text = self.current_text(&uri).await;
        self.with_analysis(&uri, |analysis| {
            let source = text.as_deref().map_or(analysis.source.as_str(), |t| t);
            let offset = position_to_byte_offset(source, position);
            crate::signature_help::signature_help(analysis, source, offset)
        })
        .await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        // Trigger-character completion runs against the just-typed text,
        // which is ahead of the debounced analysis snapshot.
        let text = self.current_text(&uri).await;
        self.with_analysis(&uri, |analysis| {
            let source = text.as_deref().map_or(analysis.source.as_str(), |t| t);
            let offset = position_to_byte_offset(source, position);
            crate::completion::completion(analysis, source, offset).map(CompletionResponse::Array)
        })
        .await
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        let current_text = self.current_text(&uri).await;
        self.with_analysis(&uri, move |analysis| {
            let current_text = current_text.as_ref()?;
            if current_text.as_str() != analysis.source.as_str() {
                return None;
            }
            crate::code_actions::code_actions(&params, analysis, current_text)
        })
        .await
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        let current_text = self.current_text(&uri).await;
        let outcome = self
            .with_analysis(&uri, |analysis| {
                let current_text = current_text.as_ref()?;
                if current_text.as_str() != analysis.source.as_str() {
                    return None;
                }
                let offset = position_to_byte_offset(current_text, position);
                Some(crate::rename::rename(analysis, &uri, offset, &new_name))
            })
            .await?;
        match outcome {
            None | Some(Ok(None)) => Ok(None),
            Some(Ok(Some(edit))) => Ok(Some(edit)),
            // An explicit refusal (invalid identifier, name collision) is a
            // descriptive error the client shows to the user — silently
            // returning null would suggest "nothing to rename here".
            Some(Err(refusal)) => Err(tower_lsp::jsonrpc::Error::invalid_params(
                refusal.to_string(),
            )),
        }
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let current_text = self.current_text(&uri).await;
        self.with_analysis(&uri, |analysis| {
            let current_text = current_text.as_ref()?;
            if current_text.as_str() != analysis.source.as_str() {
                return None;
            }
            let offset = position_to_byte_offset(current_text, position);
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
        // Formatting edits must be computed against the client's current
        // buffer, not the (possibly older) analyzed snapshot.
        let text = self.current_text(&uri).await;
        self.with_analysis(&uri, |analysis| {
            let source = text.as_deref().map_or(analysis.source.as_str(), |t| t);
            crate::formatting::format_document(source)
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
        latest_text: Arc::new(RwLock::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::syntax::index_name::{IndexName, IndexVariantName};
    use graphcal_compiler::syntax::type_name::{FieldName, StructTypeName};
    use graphcal_eval::eval::Value;
    use indexmap::IndexMap;

    use super::*;

    fn empty_symbols() -> BTreeMap<graphcal_compiler::dimension::BaseDimId, String> {
        BTreeMap::new()
    }

    fn scalar(si_value: f64) -> Value {
        Value::Scalar {
            si_value,
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    fn test_owner() -> graphcal_compiler::dag_id::DagId {
        graphcal_compiler::dag_id::DagId::root_in_package("test", "<lsp-format-test>")
    }

    fn test_struct(type_name: StructTypeName, fields: IndexMap<FieldName, Value>) -> Value {
        Value::struct_with_owner(test_owner(), type_name, fields)
    }

    fn test_indexed(index_name: IndexName, entries: IndexMap<IndexVariantName, Value>) -> Value {
        Value::indexed_with_owner(test_owner(), index_name, entries)
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
        fields.insert(FieldName::expect_valid("dv1"), scalar(100.0));
        fields.insert(FieldName::expect_valid("dv2"), scalar(200.0));
        let val = test_struct(StructTypeName::expect_valid("TransferResult"), fields);
        assert_eq!(
            format_value_inline(&val, &symbols),
            "TransferResult(dv1: 100, dv2: 200)"
        );
    }

    #[test]
    fn format_struct_empty_fields() {
        let symbols = empty_symbols();
        let val = test_struct(StructTypeName::expect_valid("Nominal"), IndexMap::new());
        assert_eq!(format_value_inline(&val, &symbols), "Nominal");
    }

    #[test]
    fn format_struct_multi_variant() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::expect_valid("thrust"), scalar(0.5));
        fields.insert(FieldName::expect_valid("duration"), scalar(3600.0));
        let val = test_struct(StructTypeName::expect_valid("LowThrust"), fields);
        assert_eq!(
            format_value_inline(&val, &symbols),
            "LowThrust(thrust: 0.5, duration: 3600)"
        );
    }

    #[test]
    fn inlay_hints_use_constructor_call_syntax_for_algebraic_values() {
        let text = include_str!("../../../tests/fixtures/valid/tagged_union.gcl");
        let analysis = run_analysis(&untitled_uri(), text, &[]);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics
        );

        let hints = crate::inlay_hints::inlay_hints(
            &analysis,
            Range::new(
                tower_lsp::lsp_types::Position::new(0, 0),
                tower_lsp::lsp_types::Position::new(u32::MAX, u32::MAX),
            ),
        )
        .expect("expected inlay hints for tagged_union fixture");
        let labels: Vec<String> = hints
            .into_iter()
            .filter_map(|hint| match hint.label {
                tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label),
                tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
            })
            .collect();

        assert!(
            labels
                .iter()
                .any(|label| label.contains("= LowThrust(thrust: 0.5 [N], duration: 3600 [s])")),
            "expected constructor-call hint for `maneuver`, got: {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("= TransferResult(dv1: 100 [m/s], dv2: 200 [m/s])")),
            "expected constructor-call hint for `transfer`, got: {labels:?}"
        );
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("LowThrust {") || label.contains("TransferResult {")),
            "constructor hints must not use brace syntax: {labels:?}"
        );
    }

    #[test]
    fn format_indexed() {
        let symbols = empty_symbols();
        let mut entries = IndexMap::new();
        entries.insert(IndexVariantName::expect_valid("A"), scalar(1.0));
        entries.insert(IndexVariantName::expect_valid("B"), scalar(2.0));
        entries.insert(IndexVariantName::expect_valid("C"), scalar(3.0));
        let val = test_indexed(IndexName::expect_valid("Phase"), entries);
        assert_eq!(format_value_inline(&val, &symbols), "{ A: 1, B: 2, C: 3 }");
    }

    #[test]
    fn format_indexed_empty() {
        let symbols = empty_symbols();
        let val = test_indexed(IndexName::expect_valid("Phase"), IndexMap::new());
        assert_eq!(format_value_inline(&val, &symbols), "{}");
    }

    #[test]
    fn format_indexed_truncation() {
        let symbols = empty_symbols();
        let mut entries = IndexMap::new();
        // Create entries with long names to trigger truncation at 80 chars
        entries.insert(
            IndexVariantName::expect_valid("LongVariantAlpha"),
            scalar(1.23456),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongVariantBeta"),
            scalar(2.34567),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongVariantGamma"),
            scalar(3.45678),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongVariantDelta"),
            scalar(4.56789),
        );
        let val = test_indexed(IndexName::expect_valid("Idx"), entries);
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
        fields.insert(FieldName::expect_valid("x"), scalar(1.0));
        let struct_val = test_struct(StructTypeName::expect_valid("Point"), fields);
        let mut entries = IndexMap::new();
        entries.insert(IndexVariantName::expect_valid("A"), struct_val);
        let val = test_indexed(IndexName::expect_valid("Idx"), entries);
        assert_eq!(format_value_inline(&val, &symbols), "{ A: Point(x: 1) }");
    }

    #[test]
    fn format_nested_indexed_tuple_keyed() {
        let symbols = empty_symbols();
        let mut inner_a = IndexMap::new();
        inner_a.insert(IndexVariantName::expect_valid("X"), scalar(1.0));
        inner_a.insert(IndexVariantName::expect_valid("Y"), scalar(2.0));
        let mut inner_b = IndexMap::new();
        inner_b.insert(IndexVariantName::expect_valid("X"), scalar(3.0));
        inner_b.insert(IndexVariantName::expect_valid("Y"), scalar(4.0));
        let mut entries = IndexMap::new();
        entries.insert(
            IndexVariantName::expect_valid("A"),
            test_indexed(IndexName::expect_valid("Col"), inner_a),
        );
        entries.insert(
            IndexVariantName::expect_valid("B"),
            test_indexed(IndexName::expect_valid("Col"), inner_b),
        );
        let val = test_indexed(IndexName::expect_valid("Row"), entries);
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
        inner_most.insert(IndexVariantName::expect_valid("Dep"), scalar(100.0));
        let mut mid = IndexMap::new();
        mid.insert(
            IndexVariantName::expect_valid("Launch"),
            test_indexed(IndexName::expect_valid("Maneuver"), inner_most),
        );
        let mut outer = IndexMap::new();
        outer.insert(
            IndexVariantName::expect_valid("Nom"),
            test_indexed(IndexName::expect_valid("Phase"), mid),
        );
        let val = test_indexed(IndexName::expect_valid("Scenario"), outer);
        assert_eq!(
            format_value_inline(&val, &symbols),
            "{ (Nom, Launch, Dep): 100 }"
        );
    }

    #[test]
    fn format_nested_indexed_truncation() {
        let symbols = empty_symbols();
        let mut inner_a = IndexMap::new();
        inner_a.insert(
            IndexVariantName::expect_valid("LongNameAlpha"),
            scalar(1.23456),
        );
        inner_a.insert(
            IndexVariantName::expect_valid("LongNameBeta"),
            scalar(2.34567),
        );
        inner_a.insert(
            IndexVariantName::expect_valid("LongNameGamma"),
            scalar(3.45678),
        );
        let mut inner_b = IndexMap::new();
        inner_b.insert(
            IndexVariantName::expect_valid("LongNameAlpha"),
            scalar(4.56789),
        );
        inner_b.insert(
            IndexVariantName::expect_valid("LongNameBeta"),
            scalar(5.6789),
        );
        inner_b.insert(
            IndexVariantName::expect_valid("LongNameGamma"),
            scalar(6.7891),
        );
        let mut entries = IndexMap::new();
        entries.insert(
            IndexVariantName::expect_valid("LongOuter1"),
            test_indexed(IndexName::expect_valid("Inner"), inner_a),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongOuter2"),
            test_indexed(IndexName::expect_valid("Inner"), inner_b),
        );
        let val = test_indexed(IndexName::expect_valid("Outer"), entries);
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
pub(bind) dim Speed = Length / Time;
pub(bind) dim CustomAcceleration = Length / Time^2;

pub(bind) index Phase;
pub(bind) index Step: Time;
pub(bind) index Accel: Length / Time^2;
";
        let analysis = run_analysis(&untitled_uri(), text, &[]);
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
        let analysis = run_analysis(&untitled_uri(), text, &[]);
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
        let analysis = run_analysis(&untitled_uri(), text, &[]);
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
        let analysis = run_analysis(&uri, &text, &[]);
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
        let analysis = run_analysis(&main_uri, &text, &[]);

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
        let analysis = run_analysis(&main_uri, &text, &[]);

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
        let analysis = run_analysis(&uri, &text, &[]);
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

    /// Issue #631 case 1+2: `@module.dag(args).out` — both the DAG name and
    /// the output projection must resolve to the imported library file.
    #[test]
    fn goto_definition_resolves_cross_file_inline_dag_call() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"lib\"\n"),
            (
                "src/lib/lib.gcl",
                "pub dag scale {\n    \
                     param factor: Dimensionless;\n    \
                     param v: Velocity;\n    \
                     pub node result: Velocity = @v * @factor;\n\
                 }\n",
            ),
            (
                "src/lib/main.gcl",
                "import lib.lib as lib;\n\
                 param speed: Velocity = 10.0 m/s;\n\
                 node doubled: Velocity = @lib.scale(factor: 2.0, v: @speed).result;\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let lib_path = dir.path().join("src/lib/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[]);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        // Cursor on `scale` inside `@lib.scale(...)`.
        let scale_offset = text.find("scale").expect("scale token in main.gcl");
        let scale_resolved =
            resolve_symbol_at(&analysis, scale_offset + 1).expect("resolve `scale`");
        let SymbolLocation::Imported(scale_imported) = scale_resolved.location else {
            panic!("expected `scale` to resolve to an imported definition");
        };
        assert_eq!(scale_imported.uri, lib_uri, "scale should jump to lib.gcl");

        // Cursor on `result` (the output projection).
        let result_offset = text.find(").result").expect("`.result` projection") + 2;
        let result_resolved =
            resolve_symbol_at(&analysis, result_offset).expect("resolve `result`");
        let SymbolLocation::Imported(result_imported) = result_resolved.location else {
            panic!("expected `result` to resolve to an imported definition");
        };
        assert_eq!(
            result_imported.uri, lib_uri,
            "result should jump to lib.gcl"
        );
    }

    /// Issue #631 case 3: variants of a selectively-imported index must
    /// resolve back to the library file that declared them.
    #[test]
    fn goto_definition_resolves_variant_of_selectively_imported_index() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"lib\"\n"),
            (
                "src/lib/lib.gcl",
                "pub index Color = { Red, Green, Blue };\n\
                 param dummy: Dimensionless = 0.0;\n",
            ),
            (
                "src/lib/main.gcl",
                "import lib.lib.{ Color };\n\
                 param favorite: Dimensionless[Color] = {\n    \
                     Color.Red: 1.0,\n    \
                     Color.Green: 2.0,\n    \
                     Color.Blue: 3.0,\n\
                 };\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let lib_path = dir.path().join("src/lib/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[]);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        // Cursor on `Red` inside `Color.Red:`.
        let red_offset = text.find("Color.Red").expect("Color.Red token") + "Color.".len();
        let red_resolved = resolve_symbol_at(&analysis, red_offset + 1).expect("resolve `Red`");
        let SymbolLocation::Imported(red_imported) = red_resolved.location else {
            panic!("expected `Red` to resolve to an imported definition");
        };
        assert_eq!(red_imported.uri, lib_uri, "Red should jump to lib.gcl");
    }

    /// A selective-imported alias should also bring across the variants of the
    /// underlying index, keyed under the local alias.
    #[test]
    fn selectively_imported_index_brings_variants_under_local_alias() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"lib\"\n"),
            (
                "src/lib/lib.gcl",
                "pub index Color = { Red, Green, Blue };\n\
                 param dummy: Dimensionless = 0.0;\n",
            ),
            (
                "src/lib/main.gcl",
                "import lib.lib.{ Color as Palette };\n\
                 param favorite: Dimensionless[Palette] = {\n    \
                     Palette.Red: 1.0,\n    \
                     Palette.Green: 2.0,\n    \
                     Palette.Blue: 3.0,\n\
                 };\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text, &[]);

        let palette_red_key = crate::symbol_table::SymbolKey::Variant {
            parent: crate::symbol_table::SymbolPath::local("Palette"),
            variant: "Red".to_string(),
        };
        assert!(
            analysis.imported_definitions.contains_key(&palette_red_key),
            "expected imported variant under local alias `Palette.Red`, got: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>(),
        );
    }

    /// Issue #830: goto-definition and hover must resolve symbols brought in
    /// by module imports — bare (`import gotom.lib;` → `@lib.g0`) and aliased
    /// (`import gotom.lib as alias_lib;` → `@alias_lib.g0`).
    #[test]
    fn module_imported_symbols_resolve_for_goto_and_hover() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"gotom\"\n"),
            (
                "src/gotom/lib.gcl",
                "pub const node g0: Acceleration = 9.80665 m/s^2;\n",
            ),
            (
                "src/gotom/main.gcl",
                "import gotom.lib;\n\
                 import gotom.lib as alias_lib;\n\
                 node g: Acceleration = @lib.g0;\n\
                 node g2: Acceleration = @alias_lib.g0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/gotom/main.gcl");
        let lib_path = dir.path().join("src/gotom/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[]);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for (needle, prefix) in [("@lib.g0", "@lib."), ("@alias_lib.g0", "@alias_lib.")] {
            let offset = text.find(needle).expect("reference in main.gcl") + prefix.len();
            let resolved = resolve_symbol_at(&analysis, offset)
                .unwrap_or_else(|| panic!("expected `{needle}` to resolve"));
            let SymbolLocation::Imported(imported) = resolved.location else {
                panic!("expected `{needle}` to resolve to an imported definition");
            };
            assert_eq!(imported.uri, lib_uri, "`{needle}` should jump to lib.gcl");
        }
    }

    /// Build a bare `AnalysisResult` carrying only a diagnostics map, for
    /// exercising the per-URI ownership merge.
    fn analysis_with_diags(diags: HashMap<Url, Vec<Diagnostic>>) -> AnalysisResult {
        AnalysisResult {
            source: Arc::new(String::new()),
            symbol_table: SymbolTable::default(),
            imported_definitions: HashMap::new(),
            diagnostics: Arc::new(diags),
            eval_values: HashMap::new(),
            fn_signatures: build_fn_signatures(),
            extern_fn_signatures: HashMap::new(),
            import_links: Vec::new(),
            buffer_parsed: true,
        }
    }

    fn diag(message: &str) -> Diagnostic {
        Diagnostic {
            message: message.to_string(),
            ..Default::default()
        }
    }

    /// Issue #832: an open document's own analysis is authoritative for its
    /// own URI — an importer's (possibly stale) contribution must not
    /// overwrite or clear it.
    #[test]
    fn open_document_owns_its_own_diagnostics() {
        let lib_uri = Url::parse("file:///p/lib.gcl").unwrap();
        let main_uri = Url::parse("file:///p/main.gcl").unwrap();

        let mut docs = HashMap::new();
        // lib's own analysis reports its own error.
        docs.insert(
            lib_uri.clone(),
            analysis_with_diags(HashMap::from([(lib_uri.clone(), vec![diag("lib broken")])])),
        );
        // main's analysis reports nothing for lib (e.g. computed before
        // lib's latest edit).
        docs.insert(
            main_uri.clone(),
            analysis_with_diags(HashMap::from([
                (main_uri, vec![]),
                (lib_uri.clone(), vec![]),
            ])),
        );

        let published = merged_diagnostics_for(&docs, std::slice::from_ref(&lib_uri));
        assert_eq!(published.len(), 1);
        assert_eq!(
            published[0].1,
            vec![diag("lib broken")],
            "lib's own analysis must win for lib's URI"
        );
    }

    /// Issue #832 repro: after closing the importer, a still-open imported
    /// document keeps its own diagnostics instead of having them wiped.
    #[test]
    fn closing_importer_keeps_open_imported_documents_diagnostics() {
        let lib_uri = Url::parse("file:///p/lib.gcl").unwrap();
        let main_uri = Url::parse("file:///p/main.gcl").unwrap();

        let mut docs = HashMap::new();
        docs.insert(
            lib_uri.clone(),
            analysis_with_diags(HashMap::from([(lib_uri.clone(), vec![diag("lib broken")])])),
        );
        // The importer was just closed: only lib remains open. The closing
        // path re-publishes every URI from the remaining documents' view.
        let affected = vec![lib_uri.clone(), main_uri.clone()];
        let published = merged_diagnostics_for(&docs, &affected);
        let by_uri: HashMap<_, _> = published.into_iter().collect();
        assert_eq!(
            by_uri[&lib_uri],
            vec![diag("lib broken")],
            "still-open lib keeps its diagnostics"
        );
        assert!(
            by_uri[&main_uri].is_empty(),
            "the closed importer's URI clears"
        );
    }

    /// Diagnostics for a file that is not open merge across importers,
    /// deduplicating identical reports.
    #[test]
    fn unopened_uri_merges_and_dedups_contributions() {
        let disk_uri = Url::parse("file:///p/disk.gcl").unwrap();
        let a_uri = Url::parse("file:///p/a.gcl").unwrap();
        let b_uri = Url::parse("file:///p/b.gcl").unwrap();

        let mut docs = HashMap::new();
        docs.insert(
            a_uri,
            analysis_with_diags(HashMap::from([(disk_uri.clone(), vec![diag("broken")])])),
        );
        docs.insert(
            b_uri,
            analysis_with_diags(HashMap::from([(
                disk_uri.clone(),
                vec![diag("broken"), diag("also broken")],
            )])),
        );

        let published = merged_diagnostics_for(&docs, std::slice::from_ref(&disk_uri));
        let mut messages: Vec<_> = published[0].1.iter().map(|d| d.message.clone()).collect();
        messages.sort();
        assert_eq!(messages, vec!["also broken", "broken"]);
    }

    /// Issue #834: a parse-failure analysis (the normal state mid-keystroke)
    /// must not replace the cached symbol state with an empty table —
    /// completion/hover/goto keep answering from the last good analysis
    /// while only the diagnostics refresh.
    #[test]
    fn parse_failure_keeps_last_good_symbol_state() {
        let uri = untitled_uri();
        let good_text = "\
param mass: Mass = 100.0 kg;
param velocity: Velocity = 50.0 m/s;
node momentum: Force * Time = @mass * @velocity;
";
        let good = run_analysis(&uri, good_text, &[]);
        assert!(good.buffer_parsed);
        assert!(good.has_no_diagnostics());

        // Mid-edit: the buffer no longer parses.
        let broken_text = format!("{good_text}node next: Mass = @");
        let broken = run_analysis(&uri, &broken_text, &[]);
        assert!(!broken.buffer_parsed, "broken buffer must be flagged");
        assert!(
            !broken.has_no_diagnostics(),
            "the parse error must still be reported"
        );

        let mut docs = HashMap::new();
        store_analysis(&mut docs, &uri, good);
        store_analysis(&mut docs, &uri, broken);

        let cached = &docs[&uri];
        for name in ["mass", "velocity", "momentum"] {
            assert!(
                cached
                    .symbol_table
                    .definitions
                    .contains_key(&SymbolKey::TopLevel(name.to_string())),
                "symbol `{name}` must survive the parse failure"
            );
        }
        assert!(
            !cached.has_no_diagnostics(),
            "diagnostics must come from the failed analysis"
        );
        assert_eq!(
            cached.source.as_str(),
            good_text,
            "the kept source must stay consistent with the kept symbol table"
        );
    }

    /// Issue #833: analysis must overlay the latest text of *all* open
    /// documents, not just the analyzed one — otherwise an importer's
    /// analysis sees stale disk content of an open-but-unsaved imported file
    /// and publishes phantom diagnostics on the imported file's URI.
    #[test]
    fn analysis_overlays_other_open_documents() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"staleo\"\n"),
            (
                // Broken on disk; the open editor buffer has the fix.
                "src/staleo/lib.gcl",
                "pub const node g0: Acceleration = @broken_ref;\n",
            ),
            (
                "src/staleo/main.gcl",
                "import staleo.lib;\nnode x: Acceleration = @lib.g0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/staleo/main.gcl");
        let lib_path = dir.path().join("src/staleo/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();

        // Control: without the overlay, the stale disk content produces
        // diagnostics (on lib.gcl's URI).
        let stale = run_analysis(&main_uri, &text, &[]);
        assert!(
            !stale.has_no_diagnostics(),
            "expected diagnostics from the broken on-disk lib.gcl"
        );

        let open_buffers = vec![OpenBuffer {
            path: lib_path,
            text: Arc::new("pub const node g0: Acceleration = 9.80665 m/s^2;\n".to_string()),
        }];
        let analysis = run_analysis(&main_uri, &text, &open_buffers);
        assert!(
            analysis.has_no_diagnostics(),
            "the fixed editor buffer must shadow the broken disk content, got: {:?}",
            analysis.diagnostics,
        );
    }

    /// Issue #831: imported symbols hover with the same type fidelity as
    /// local ones — the project TIR is already computed; the type must be
    /// read off the dependency's own `DagTIR`, not the root's.
    #[test]
    fn imported_definitions_carry_resolved_types() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"gotom\"\n"),
            (
                "src/gotom/lib.gcl",
                "pub const node g0: Acceleration = 9.80665 m/s^2;\n",
            ),
            (
                "src/gotom/main.gcl",
                "import gotom.lib.{g0};\n\
                 import gotom.lib as m;\n\
                 node g: Acceleration = @g0;\n\
                 node g2: Acceleration = @m.g0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/gotom/main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[]);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for key in [
            SymbolKey::TopLevel("g0".to_string()),
            SymbolKey::Qualified {
                module: vec!["m".to_string()],
                name: "g0".to_string(),
            },
        ] {
            let imported = analysis
                .imported_definitions
                .get(&key)
                .unwrap_or_else(|| panic!("expected imported definition for {key:?}"));
            let type_description = imported
                .definition
                .type_description
                .as_deref()
                .unwrap_or_else(|| panic!("expected a type description for {key:?}"));
            assert!(
                type_description.contains("Acceleration"),
                "expected resolved type for {key:?}, got `{type_description}`"
            );
        }
    }

    #[test]
    fn goto_definition_uses_qualified_same_leaf_import_identity() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"proj\"\n"),
            (
                "src/proj/a.gcl",
                "pub index Phase = { Burn, Coast };\n\
                 pub type Item { Pick(distance: Dimensionless), Idle }\n\
                 pub const node bias: Dimensionless = 1.0;\n",
            ),
            (
                "src/proj/b.gcl",
                "pub index Phase = { Burn, Coast };\n\
                 pub type Item { Pick(distance: Dimensionless), Idle }\n\
                 pub const node bias: Dimensionless = 2.0;\n",
            ),
            (
                "src/proj/main.gcl",
                "import proj.a as a;\n\
                 import proj.b as b;\n\n\
                 const node from_a: Dimensionless = a.bias;\n\
                 node phase_score: Dimensionless[a.Phase] = for phase: a.Phase {\n\
                     match phase {\n\
                         a.Phase.Burn => a.bias,\n\
                         a.Phase.Coast => b.bias,\n\
                     }\n\
                 };\n\
                 node item: a.Item = a.Pick(distance: a.bias);\n",
            ),
        ]);
        let main_path = dir.path().join("src/proj/main.gcl");
        let a_path = dir.path().join("src/proj/a.gcl");
        let b_path = dir.path().join("src/proj/b.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let a_uri = Url::from_file_path(a_path.canonicalize().unwrap()).unwrap();
        let b_uri = Url::from_file_path(b_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[]);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for (needle, token_prefix) in [
            ("a.bias", "a."),
            ("a.Phase]", "a."),
            ("a.Phase.Burn", "a.Phase."),
            ("a.Item", "a."),
            ("a.Pick", "a."),
        ] {
            let offset = text.find(needle).expect("token in main.gcl") + token_prefix.len();
            let resolved = resolve_symbol_at(&analysis, offset + 1)
                .unwrap_or_else(|| panic!("resolve `{needle}`"));
            let SymbolLocation::Imported(imported) = resolved.location else {
                panic!("expected `{needle}` to resolve to an imported definition");
            };
            assert_eq!(imported.uri, a_uri, "`{needle}` should jump to a.gcl");
            assert_ne!(imported.uri, b_uri, "`{needle}` must not jump to b.gcl");
        }
    }
}
