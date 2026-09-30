//! LSP server backend: state management and `LanguageServer` trait implementation.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{RwLock, Semaphore};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CompletionOptions, CompletionParams,
    CompletionResponse, Diagnostic, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentChanges, DocumentFormattingParams, DocumentLink, DocumentLinkOptions,
    DocumentLinkParams, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, InlayHint, InlayHintParams, Location, MessageType, OneOf,
    OptionalVersionedTextDocumentIdentifier, PositionEncodingKind, PrepareRenameResponse,
    ReferenceParams, RenameOptions, RenameParams, SaveOptions, ServerCapabilities, SignatureHelp,
    SignatureHelpOptions, SignatureHelpParams, TextDocumentEdit, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions, WorkspaceEdit,
};
use tower_lsp::{Client, ClientSocket, LanguageServer, LspService, Server};

use crate::analysis::AnalysisResult;
use crate::analysis_pipeline::{AnalysisRun, OpenBuffer, run_analysis_with_cancellation};
use crate::analysis_schedule_state::AnalysisScheduleState;
use crate::client_capabilities::{ClientFeatureSupport, DocumentSymbolShape, WorkspaceEditShape};
use crate::convert::position_to_byte_offset;
use crate::file_identity::{document_identity, file_identity};
use crate::formatting_scheduler::{FormattingScheduler, FormattingTaskError};
use crate::project_symbols::ProjectSymbolIndex;
use crate::workspace_revision::{
    AnalysisFreshness, AnalysisInputSnapshot, DependencyGraph, DocumentIdentity, DocumentRevision,
    RevisionClock, RevisionExhausted, current_revisions,
};
use graphcal_compiler::cancellation::{CancellationSource, CancellationToken, Cancelled};

/// Debounce delay for `did_change` notifications (milliseconds).
const DEBOUNCE_DELAY_MS: u64 = 300;

/// Wall-clock cap on a single `run_analysis` pass. When exceeded, the LSP
/// requests cooperative cancellation and leaves prior cached analysis in
/// place so queries still answer from the last good state.
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(10);

/// Global cap for CPU-bound analyses. Permits are owned by the blocking
/// closures, not their async waiters, so a timed-out but not-yet-cooperating
/// operation continues to consume its permit instead of allowing runaway
/// replacement work to accumulate.
const MAX_CONCURRENT_ANALYSES: usize = 2;

/// Bounded, per-document single-flight coordination for blocking analysis.
#[derive(Debug)]
struct AnalysisScheduler {
    /// Closing entries remain only while old work is unwinding. A quick reopen
    /// reuses that gate; quiescent closed entries are removed.
    states: Mutex<HashMap<Url, ScheduledDocumentState>>,
    global_gate: Arc<Semaphore>,
}

#[derive(Debug)]
struct ScheduledDocumentState {
    core: AnalysisScheduleState,
    document_gate: Arc<Semaphore>,
}

impl ScheduledDocumentState {
    fn new(is_open: bool) -> Self {
        Self {
            core: AnalysisScheduleState::new(is_open),
            document_gate: Arc::new(Semaphore::new(1)),
        }
    }
}

impl AnalysisScheduler {
    fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            global_gate: Arc::new(Semaphore::new(MAX_CONCURRENT_ANALYSES)),
        }
    }

    fn open_document(&self, uri: &Url) {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(uri.clone())
            .and_modify(|state| state.core.open())
            .or_insert_with(|| ScheduledDocumentState::new(true));
    }

    /// Register one revision before it waits for a worker permit. A later edit
    /// can therefore cancel queued as well as running work.
    fn register(&self, uri: &Url, generation: u64) -> ScheduledAnalysis {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A missing entry here belongs to a stale task racing with
        // `did_close`; genuine open/change/save paths call `open_document`
        // first. Keep it closed so `finish` can remove it.
        let state = states
            .entry(uri.clone())
            .or_insert_with(|| ScheduledDocumentState::new(false));
        let source = state.core.register(generation);
        let document_gate = Arc::clone(&state.document_gate);
        drop(states);
        ScheduledAnalysis {
            source,
            document_gate,
            global_gate: Arc::clone(&self.global_gate),
        }
    }

    fn cancel_document(&self, uri: &Url) {
        let states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = states.get(uri) {
            state.core.cancel_all();
        }
    }

    fn close_document(&self, uri: &Url) {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = states.get_mut(uri).is_some_and(|state| state.core.close());
        if remove {
            states.remove(uri);
        }
    }

    fn cancel_all(&self) {
        let states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for state in states.values() {
            state.core.cancel_all();
        }
    }

    fn finish(&self, uri: &Url, generation: u64) {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = states
            .get_mut(uri)
            .is_some_and(|state| state.core.finish(generation));
        if remove {
            states.remove(uri);
        }
    }
}

#[derive(Debug)]
struct ScheduledAnalysis {
    source: CancellationSource,
    document_gate: Arc<Semaphore>,
    global_gate: Arc<Semaphore>,
}

/// Wait for a worker permit while periodically observing the core's std-only
/// cancellation token. Dropping and retrying the acquire future removes stale
/// revisions from Tokio's waiter queue, so repeated edits cannot accumulate an
/// unbounded pending backlog behind one slow analysis.
async fn acquire_analysis_permit(
    gate: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> std::result::Result<Option<tokio::sync::OwnedSemaphorePermit>, tokio::sync::AcquireError> {
    const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

    loop {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        match tokio::time::timeout(
            CANCELLATION_POLL_INTERVAL,
            Arc::clone(&gate).acquire_owned(),
        )
        .await
        {
            Ok(result) => return result.map(Some),
            Err(_elapsed) => {}
        }
    }
}

/// Latest text and LSP version for one open editor document.
#[derive(Debug, Clone)]
struct OpenDocumentSnapshot {
    identity: DocumentIdentity,
    revision: DocumentRevision,
    text: Arc<String>,
    version: i32,
}

#[derive(Debug)]
struct RecordedDocument {
    identity: DocumentIdentity,
    revision: DocumentRevision,
}

/// The LSP server backend.
#[cfg_attr(test, derive(Debug))]
pub struct Backend {
    client: Client,
    /// Immutable interpretation of the initialize request's optional client
    /// capabilities. `OnceLock` mirrors the protocol's initialize-once rule.
    client_features: Arc<OnceLock<ClientFeatureSupport>>,
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
    latest_text: Arc<RwLock<HashMap<Url, OpenDocumentSnapshot>>>,
    /// Bounded worker gate for memory-intensive synchronous formatting.
    formatting_scheduler: Arc<FormattingScheduler>,
    /// Monotonic revision allocator shared by all document identities.
    revision_clock: Arc<RevisionClock>,
    /// Latest client-reported revision for each closed filesystem input.
    /// Open-buffer revisions override these entries when snapshots are built.
    filesystem_revisions: Arc<RwLock<HashMap<DocumentIdentity, DocumentRevision>>>,
    /// Reverse dependency edges from the latest accepted analyses.
    dependency_graph: Arc<RwLock<DependencyGraph>>,
    /// WASM plugin host shared across analysis passes, so re-analysis on
    /// every debounced keystroke hits the content-hash module cache instead
    /// of recompiling unchanged plugins.
    plugin_host: Arc<graphcal_plugin_host::PluginHost>,
    /// Per-document single-flight and global concurrency control.
    analysis_scheduler: Arc<AnalysisScheduler>,
}

#[derive(Clone, Copy)]
struct CurrentAnalysis<'a>(&'a AnalysisResult);

impl<'a> CurrentAnalysis<'a> {
    const fn get(self) -> &'a AnalysisResult {
        self.0
    }
}

#[derive(Clone, Copy)]
struct SemanticAnalysis<'a>(&'a AnalysisResult);

impl<'a> SemanticAnalysis<'a> {
    const fn get(self) -> &'a AnalysisResult {
        self.0
    }
}

fn select_current_analysis<'a>(
    analysis: &'a AnalysisResult,
    revisions: &HashMap<DocumentIdentity, DocumentRevision>,
) -> Option<CurrentAnalysis<'a>> {
    (analysis.inputs.freshness(revisions) == AnalysisFreshness::Current)
        .then_some(CurrentAnalysis(analysis))
}

fn select_semantic_analysis<'a>(
    analysis: &'a AnalysisResult,
    revisions: &HashMap<DocumentIdentity, DocumentRevision>,
) -> Option<SemanticAnalysis<'a>> {
    (analysis.inputs.freshness(revisions) != AnalysisFreshness::DependencyChanged)
        .then_some(SemanticAnalysis(analysis))
}

fn open_revisions(
    snapshots: &HashMap<Url, OpenDocumentSnapshot>,
) -> impl Iterator<Item = (DocumentIdentity, DocumentRevision)> + '_ {
    snapshots
        .values()
        .map(|snapshot| (snapshot.identity.clone(), snapshot.revision))
}

impl Backend {
    fn client_features(&self) -> ClientFeatureSupport {
        self.client_features.get().copied().unwrap_or_default()
    }

    fn request_inlay_hint_refresh(&self) {
        if !self.client_features().inlay_hint_refresh.is_supported() {
            return;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.inlay_hint_refresh().await;
        });
    }

    fn is_graphcal_file(uri: &Url) -> bool {
        std::path::Path::new(uri.path())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gcl"))
    }

    /// Apply an exact-coordinate feature only to a fully current analysis.
    async fn with_current_analysis<F, R>(&self, uri: &Url, f: F) -> Result<Option<R>>
    where
        F: FnOnce(CurrentAnalysis<'_>) -> Option<R>,
    {
        let snapshots = self.latest_text.read().await;
        let disk_revisions = self.filesystem_revisions.read().await;
        let revisions = current_revisions(&disk_revisions, open_revisions(&snapshots));
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(current) = select_current_analysis(analysis, &revisions) else {
            return Ok(None);
        };
        drop(snapshots);
        drop(disk_revisions);
        let result = f(current);
        drop(docs);
        Ok(result)
    }

    /// Apply a coordinate-free candidate feature when dependencies are still
    /// current. The root text may be newer than the cached semantic snapshot.
    async fn with_semantic_analysis<F, R>(&self, uri: &Url, f: F) -> Result<Option<R>>
    where
        F: FnOnce(SemanticAnalysis<'_>, &str) -> Option<R>,
    {
        let snapshots = self.latest_text.read().await;
        let disk_revisions = self.filesystem_revisions.read().await;
        let revisions = current_revisions(&disk_revisions, open_revisions(&snapshots));
        let Some(current_source) = snapshots
            .get(uri)
            .map(|snapshot| Arc::clone(&snapshot.text))
        else {
            return Ok(None);
        };
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(semantic) = select_semantic_analysis(analysis, &revisions) else {
            return Ok(None);
        };
        drop(snapshots);
        drop(disk_revisions);
        let result = f(semantic, &current_source);
        drop(docs);
        Ok(result)
    }

    /// Record the latest editor text for `uri`, synchronously with the
    /// notification that delivered it.
    async fn record_latest_text(
        &self,
        uri: &Url,
        text: &str,
        version: Option<i32>,
    ) -> std::result::Result<RecordedDocument, RevisionExhausted> {
        use std::collections::hash_map::Entry;
        let revision = self.revision_clock.next()?;
        let identity = match self.latest_text.write().await.entry(uri.clone()) {
            Entry::Occupied(mut entry) => {
                let snapshot = entry.get_mut();
                snapshot.revision = revision;
                snapshot.text = Arc::new(text.to_string());
                if let Some(version) = version {
                    snapshot.version = version;
                }
                snapshot.identity.clone()
            }
            Entry::Vacant(entry) => {
                let identity = document_identity(uri);
                entry.insert(OpenDocumentSnapshot {
                    identity: identity.clone(),
                    revision,
                    text: Arc::new(text.to_string()),
                    version: version.unwrap_or_default(),
                });
                identity
            }
        };
        Ok(RecordedDocument { identity, revision })
    }

    /// Cancel and debounce the open roots identified by an invalidation plan.
    async fn schedule_open_roots(&self, affected: &HashSet<DocumentIdentity>) {
        let pending: Vec<_> = self
            .latest_text
            .read()
            .await
            .iter()
            .filter(|(_, snapshot)| affected.contains(&snapshot.identity))
            .map(|(uri, snapshot)| (uri.clone(), Arc::clone(&snapshot.text), snapshot.revision))
            .collect();
        if !pending.is_empty() {
            self.request_inlay_hint_refresh();
        }
        for (uri, text, revision) in pending {
            let generation = self.bump_generation(&uri).await;
            self.spawn_debounced_analysis(uri, text.as_ref().clone(), revision, generation);
        }
    }

    /// Cancel and debounce every open analysis that consumes `changed`.
    async fn schedule_transitive_importers(&self, changed: &DocumentIdentity) {
        let mut importers = self
            .dependency_graph
            .read()
            .await
            .transitive_importers(changed);
        importers.remove(changed);
        self.schedule_open_roots(&importers).await;
    }

    /// Record and schedule one client-reported external filesystem change.
    async fn handle_filesystem_change(&self, uri: Url) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let Some(kind) = crate::filesystem_events::WatchedArtifactKind::from_path(&path) else {
            return;
        };
        let identity = file_identity(&path);
        if self
            .latest_text
            .read()
            .await
            .values()
            .any(|snapshot| snapshot.identity == identity)
        {
            // The editor snapshot is authoritative. This also coalesces the
            // watcher event commonly emitted for an LSP didSave notification.
            return;
        }
        let revision = match self.revision_clock.next() {
            Ok(revision) => revision,
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
                return;
            }
        };
        self.filesystem_revisions
            .write()
            .await
            .insert(identity.clone(), revision);

        let mut affected = self
            .dependency_graph
            .read()
            .await
            .transitive_importers(&identity);
        if kind == crate::filesystem_events::WatchedArtifactKind::Manifest
            && let Some(parent) = identity.file_path().and_then(std::path::Path::parent)
        {
            affected.extend(
                self.latest_text
                    .read()
                    .await
                    .values()
                    .filter(|snapshot| {
                        snapshot
                            .identity
                            .file_path()
                            .is_some_and(|root| root.starts_with(parent))
                    })
                    .map(|snapshot| snapshot.identity.clone()),
            );
        }
        self.schedule_open_roots(&affected).await;
    }

    /// Latest editor text for `uri`, falling back to the analyzed snapshot.
    async fn current_text(&self, uri: &Url) -> Option<Arc<String>> {
        if let Some(snapshot) = self.latest_text.read().await.get(uri) {
            return Some(Arc::clone(&snapshot.text));
        }
        self.documents
            .read()
            .await
            .get(uri)
            .map(|analysis| Arc::clone(&analysis.source))
    }

    async fn analyze_and_publish(&self, uri: Url, text: String, version: Option<i32>) {
        if !Self::is_graphcal_file(&uri) {
            return;
        }

        self.analysis_scheduler.open_document(&uri);
        let recorded = match self.record_latest_text(&uri, &text, version).await {
            Ok(recorded) => recorded,
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
                return;
            }
        };
        self.schedule_transitive_importers(&recorded.identity).await;

        // Bump the generation so any in-flight debounced analysis for this URI
        // becomes stale and refuses to overwrite fresh results.
        let generation = self.bump_generation(&uri).await;

        analyze_store_publish(
            &self.client,
            &self.documents,
            &self.change_generations,
            &self.latest_text,
            &self.filesystem_revisions,
            &self.dependency_graph,
            self.client_features(),
            Arc::clone(&self.plugin_host),
            Arc::clone(&self.analysis_scheduler),
            uri,
            text,
            recorded.revision,
            generation,
        )
        .await;
    }

    /// Spawn a debounced analysis task for a `did_change` event.
    ///
    /// Waits [`DEBOUNCE_DELAY_MS`] and bails out if a newer change has
    /// arrived (generation mismatch). Any `JoinError` from the blocking
    /// analysis task is logged to the client rather than silently swallowed.
    fn spawn_debounced_analysis(
        &self,
        uri: Url,
        text: String,
        root_revision: DocumentRevision,
        generation: u64,
    ) {
        let client = self.client.clone();
        let documents = self.documents.clone();
        let generations = self.change_generations.clone();
        let latest_text = self.latest_text.clone();
        let filesystem_revisions = Arc::clone(&self.filesystem_revisions);
        let dependency_graph = Arc::clone(&self.dependency_graph);
        let plugin_host = Arc::clone(&self.plugin_host);
        let analysis_scheduler = Arc::clone(&self.analysis_scheduler);
        let client_features = self.client_features();

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
                &filesystem_revisions,
                &dependency_graph,
                client_features,
                plugin_host,
                analysis_scheduler,
                uri,
                text,
                root_revision,
                generation,
            )
            .await;
        });
    }

    /// Increment and return the current generation for `uri`.
    async fn bump_generation(&self, uri: &Url) -> u64 {
        let generation = bump_generation_value(&self.change_generations, uri).await;
        self.analysis_scheduler.cancel_document(uri);
        generation
    }
}

#[derive(Debug)]
enum AnalysisCompletion {
    Done,
    Retry(OpenDocumentSnapshot),
}

/// Run analysis until it either stores a revision-coherent result or reaches
/// a terminal cancellation/error. A dependency edit racing the worker returns
/// the current root snapshot for another bounded pass.
#[expect(
    clippy::too_many_arguments,
    reason = "the arguments are the Backend's shared state"
)]
async fn analyze_store_publish(
    client: &Client,
    documents: &Arc<RwLock<HashMap<Url, AnalysisResult>>>,
    generations: &Arc<RwLock<HashMap<Url, u64>>>,
    latest_text: &Arc<RwLock<HashMap<Url, OpenDocumentSnapshot>>>,
    filesystem_revisions: &Arc<RwLock<HashMap<DocumentIdentity, DocumentRevision>>>,
    dependency_graph: &Arc<RwLock<DependencyGraph>>,
    client_features: ClientFeatureSupport,
    plugin_host: Arc<graphcal_plugin_host::PluginHost>,
    scheduler: Arc<AnalysisScheduler>,
    uri: Url,
    mut text: String,
    mut root_revision: DocumentRevision,
    mut generation: u64,
) {
    loop {
        match analyze_store_publish_once(
            client,
            documents,
            generations,
            latest_text,
            filesystem_revisions,
            dependency_graph,
            client_features,
            Arc::clone(&plugin_host),
            Arc::clone(&scheduler),
            uri.clone(),
            text,
            root_revision,
            generation,
        )
        .await
        {
            AnalysisCompletion::Done => return,
            AnalysisCompletion::Retry(snapshot) => {
                text = snapshot.text.as_ref().clone();
                root_revision = snapshot.revision;
                generation = bump_generation_value(generations, &uri).await;
                scheduler.cancel_document(&uri);
            }
        }
    }
}

/// Run one blocking, timeout-guarded analysis pass and store/publish it only
/// when every consumed open-document revision is still current.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one call site per trigger; the arguments are the Backend's shared state"
)]
async fn analyze_store_publish_once(
    client: &Client,
    documents: &Arc<RwLock<HashMap<Url, AnalysisResult>>>,
    generations: &Arc<RwLock<HashMap<Url, u64>>>,
    latest_text: &Arc<RwLock<HashMap<Url, OpenDocumentSnapshot>>>,
    filesystem_revisions: &Arc<RwLock<HashMap<DocumentIdentity, DocumentRevision>>>,
    dependency_graph: &Arc<RwLock<DependencyGraph>>,
    client_features: ClientFeatureSupport,
    plugin_host: Arc<graphcal_plugin_host::PluginHost>,
    scheduler: Arc<AnalysisScheduler>,
    uri: Url,
    text: String,
    root_revision: DocumentRevision,
    generation: u64,
) -> AnalysisCompletion {
    let registration = scheduler.register(&uri, generation);
    let cancellation = registration.source.token();
    if !is_generation_current(generations, &uri, generation).await {
        registration.source.cancel();
        scheduler.finish(&uri, generation);
        return AnalysisCompletion::Done;
    }
    let document_permit =
        match acquire_analysis_permit(registration.document_gate, &cancellation).await {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                scheduler.finish(&uri, generation);
                return AnalysisCompletion::Done;
            }
            Err(error) => {
                scheduler.finish(&uri, generation);
                client
                    .log_message(
                        MessageType::ERROR,
                        format!("document analysis gate closed unexpectedly: {error}"),
                    )
                    .await;
                return AnalysisCompletion::Done;
            }
        };
    if cancellation.is_cancelled() || !is_generation_current(generations, &uri, generation).await {
        scheduler.finish(&uri, generation);
        return AnalysisCompletion::Done;
    }

    let global_permit = match acquire_analysis_permit(registration.global_gate, &cancellation).await
    {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            scheduler.finish(&uri, generation);
            return AnalysisCompletion::Done;
        }
        Err(error) => {
            scheduler.finish(&uri, generation);
            client
                .log_message(
                    MessageType::ERROR,
                    format!("global analysis gate closed unexpectedly: {error}"),
                )
                .await;
            return AnalysisCompletion::Done;
        }
    };
    if cancellation.is_cancelled() || !is_generation_current(generations, &uri, generation).await {
        scheduler.finish(&uri, generation);
        return AnalysisCompletion::Done;
    }

    // Snapshot every *other* open document's latest text: the analysis
    // overlays them onto the filesystem so imports of open-but-dirty files
    // see the editor state, not stale disk content. The analyzed document
    // itself uses `text` (this analysis pass's snapshot).
    let snapshots = latest_text.read().await;
    let disk_revisions = filesystem_revisions.read().await;
    let root_identity = snapshots.get(&uri).map_or_else(
        || document_identity(&uri),
        |snapshot| snapshot.identity.clone(),
    );
    let observed_revisions = current_revisions(&disk_revisions, open_revisions(&snapshots));
    let input_snapshot =
        AnalysisInputSnapshot::new(root_identity, root_revision, observed_revisions);
    let open_buffers: Vec<OpenBuffer> = snapshots
        .iter()
        .filter(|(open_uri, _)| **open_uri != uri)
        .filter_map(|(open_uri, snapshot)| {
            open_uri.to_file_path().ok().map(|path| OpenBuffer {
                path,
                text: Arc::clone(&snapshot.text),
            })
        })
        .collect();
    drop(snapshots);
    drop(disk_revisions);
    if cancellation.is_cancelled() || !is_generation_current(generations, &uri, generation).await {
        scheduler.finish(&uri, generation);
        return AnalysisCompletion::Done;
    }

    let uri_for_analysis = uri.clone();
    let source = registration.source;
    let worker_plugin_host = Arc::clone(&plugin_host);
    let mut task = tokio::task::spawn_blocking(move || {
        // Keep both permits until the synchronous worker truly exits. Dropping
        // the async waiter on timeout must not admit replacement work while a
        // non-cooperating blocking closure still consumes CPU.
        let _permits = (document_permit, global_permit);
        run_analysis_with_cancellation(
            &uri_for_analysis,
            &text,
            &open_buffers,
            &worker_plugin_host,
            &input_snapshot,
            &cancellation,
        )
    });
    let analysis_run = match tokio::time::timeout(ANALYSIS_TIMEOUT, &mut task).await {
        Ok(Ok(Ok(analysis_run))) => analysis_run,
        Ok(Ok(Err(Cancelled))) => {
            scheduler.finish(&uri, generation);
            return AnalysisCompletion::Done;
        }
        Ok(Err(error)) => {
            scheduler.finish(&uri, generation);
            client
                .log_message(
                    MessageType::ERROR,
                    format!("analysis task panicked: {error}"),
                )
                .await;
            return AnalysisCompletion::Done;
        }
        Err(_elapsed) => {
            source.cancel();
            task.abort();
            let cleanup_scheduler = Arc::clone(&scheduler);
            let cleanup_uri = uri.clone();
            tokio::spawn(async move {
                let _ = task.await;
                cleanup_scheduler.finish(&cleanup_uri, generation);
            });
            if is_generation_current(generations, &uri, generation).await {
                publish_analysis_timeout(client, &uri).await;
            }
            return AnalysisCompletion::Done;
        }
    };
    scheduler.finish(&uri, generation);
    let AnalysisRun {
        analysis,
        degradations,
    } = analysis_run;
    let latest_guard = latest_text.read().await;
    let filesystem_guard = filesystem_revisions.read().await;
    let current_revisions = current_revisions(&filesystem_guard, open_revisions(&latest_guard));
    if !analysis.inputs.is_current(&current_revisions) {
        let root_is_current = analysis.inputs.root_is_current(&current_revisions);
        if analysis.inputs.has_complete_dependencies() || analysis.buffer_parsed {
            dependency_graph.write().await.update(
                analysis.inputs.root().clone(),
                analysis.inputs.dependencies().cloned().collect(),
            );
        }
        let retry = root_is_current
            .then(|| latest_guard.get(&uri).cloned())
            .flatten();
        drop(latest_guard);
        return retry.map_or(AnalysisCompletion::Done, AnalysisCompletion::Retry);
    }
    let graph_update = (analysis.inputs.has_complete_dependencies() || analysis.buffer_parsed)
        .then(|| {
            (
                analysis.inputs.root().clone(),
                analysis.inputs.dependencies().cloned().collect(),
            )
        });

    // Hold the documents write lock across the generation re-check and
    // the insert: a newer-generation analysis completing between a
    // separate check and insert used to be clobbered by this older one.
    let mut docs = documents.write().await;
    if !is_generation_current(generations, &uri, generation).await {
        return AnalysisCompletion::Done;
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
    // Publish dependency edges before releasing the revision snapshot lock.
    // A concurrent dependency edit cannot become visible and miss this new
    // importer between accepting its result and registering its edges.
    if let Some((root, dependencies)) = graph_update {
        dependency_graph.write().await.update(root, dependencies);
    }
    drop(latest_guard);
    drop(filesystem_guard);
    for degradation in degradations {
        client
            .log_message(MessageType::WARNING, degradation.message(&uri))
            .await;
    }
    for (target_uri, diags) in publish {
        client
            .publish_diagnostics(
                target_uri,
                diagnostics_for_client(diags, client_features),
                None,
            )
            .await;
    }
    if client_features.inlay_hint_refresh.is_supported() {
        let _ = client.inlay_hint_refresh().await;
    }
    AnalysisCompletion::Done
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

fn workspace_edit_matches_open_snapshots(
    edit: &WorkspaceEdit,
    project: &ProjectSymbolIndex,
    snapshots: &HashMap<Url, OpenDocumentSnapshot>,
) -> bool {
    edit.changes.as_ref().is_none_or(|changes| {
        changes.keys().all(|uri| {
            snapshots.get(uri).is_none_or(|snapshot| {
                project
                    .document(uri)
                    .is_some_and(|document| document.source == snapshot.text)
            })
        })
    })
}

fn workspace_edit_for_client(
    mut edit: WorkspaceEdit,
    snapshots: &HashMap<Url, OpenDocumentSnapshot>,
    shape: WorkspaceEditShape,
) -> WorkspaceEdit {
    if shape == WorkspaceEditShape::Changes {
        return edit;
    }
    let Some(changes) = edit.changes.take() else {
        return edit;
    };
    let mut changes: Vec<_> = changes.into_iter().collect();
    changes.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
    edit.document_changes = Some(DocumentChanges::Edits(
        changes
            .into_iter()
            .map(|(uri, edits)| TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    version: snapshots.get(&uri).map(|snapshot| snapshot.version),
                    uri,
                },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            })
            .collect(),
    ));
    edit
}

fn code_actions_for_client(
    actions: CodeActionResponse,
    snapshots: &HashMap<Url, OpenDocumentSnapshot>,
    features: ClientFeatureSupport,
) -> CodeActionResponse {
    actions
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::Command(command) => Some(CodeActionOrCommand::Command(command)),
            CodeActionOrCommand::CodeAction(mut action) => {
                if !features.code_action_literals.is_supported() {
                    return None;
                }
                if !features.code_action_is_preferred.is_supported() {
                    action.is_preferred = None;
                }
                action.edit = action.edit.map(|edit| {
                    workspace_edit_for_client(edit, snapshots, features.workspace_edit_shape)
                });
                Some(CodeActionOrCommand::CodeAction(action))
            }
        })
        .collect()
}

fn diagnostics_for_client(
    diagnostics: Vec<Diagnostic>,
    features: ClientFeatureSupport,
) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|mut diagnostic| {
            if !features.diagnostic_related_information.is_supported() {
                diagnostic.related_information = None;
            }
            if !features.diagnostic_data.is_supported() {
                diagnostic.data = None;
            }
            diagnostic
        })
        .collect()
}

/// Log a current-revision analysis timeout without replacing source
/// diagnostics. A timeout is a server condition, not a Graphcal program error;
/// the last completed diagnostics remain visible while the worker cooperatively
/// shuts down.
async fn publish_analysis_timeout(client: &Client, uri: &Url) {
    let secs = ANALYSIS_TIMEOUT.as_secs();
    client
        .log_message(
            MessageType::ERROR,
            format!("analysis for {uri} timed out after {secs}s and was cancelled"),
        )
        .await;
}

async fn bump_generation_value(generations: &Arc<RwLock<HashMap<Url, u64>>>, uri: &Url) -> u64 {
    *generations
        .write()
        .await
        .entry(uri.clone())
        .and_modify(|value| *value = value.saturating_add(1))
        .or_insert(1)
}

/// Check whether `generation` is still the current generation stored for `uri`.
async fn is_generation_current(
    generations: &Arc<RwLock<HashMap<Url, u64>>>,
    uri: &Url,
    generation: u64,
) -> bool {
    *generations.read().await.get(uri).unwrap_or(&0) == generation
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let client_features = ClientFeatureSupport::from_client(&params.capabilities);
        let _ = self.client_features.set(client_features);
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
                code_action_provider: client_features.code_action_literals.is_supported().then(
                    || {
                        CodeActionProviderCapability::Options(CodeActionOptions {
                            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                            resolve_provider: Some(false),
                        })
                    },
                ),
                document_formatting_provider: Some(OneOf::Left(true)),
                position_encoding: Some(PositionEncodingKind::UTF16),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "graphcal-lsp initialized")
            .await;
        if self
            .client_features()
            .watched_files_dynamic_registration
            .is_supported()
        {
            let client = self.client.clone();
            tokio::spawn(async move {
                let registration = match crate::filesystem_events::watcher_registration() {
                    Ok(registration) => registration,
                    Err(error) => {
                        client
                            .log_message(
                                MessageType::ERROR,
                                format!(
                                    "could not serialize filesystem watcher registration: {error}"
                                ),
                            )
                            .await;
                        return;
                    }
                };
                if let Err(error) = client.register_capability(vec![registration]).await {
                    client
                        .log_message(
                            MessageType::WARNING,
                            format!("could not register filesystem watchers: {error}"),
                        )
                        .await;
                }
            });
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.analysis_scheduler.cancel_all();
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.analyze_and_publish(
            params.text_document.uri,
            params.text_document.text,
            Some(params.text_document.version),
        )
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

        self.analysis_scheduler.open_document(&uri);
        // Record the new text synchronously: completion/signature-help fire
        // on trigger characters milliseconds after this notification, long
        // before the debounced analysis lands.
        let recorded = match self
            .record_latest_text(&uri, &change.text, Some(params.text_document.version))
            .await
        {
            Ok(recorded) => recorded,
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
                return;
            }
        };
        // Cached hint coordinates became stale as soon as the root revision
        // changed; ask capable clients to clear/re-request them before debounce.
        self.request_inlay_hint_refresh();
        self.schedule_transitive_importers(&recorded.identity).await;
        let generation = self.bump_generation(&uri).await;
        self.spawn_debounced_analysis(uri, change.text, recorded.revision, generation);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.analyze_and_publish(params.text_document.uri, text, None)
                .await;
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut seen = HashSet::new();
        for change in params.changes {
            if seen.insert(change.uri.clone()) {
                self.handle_filesystem_change(change.uri).await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let identity = self.latest_text.read().await.get(&uri).map_or_else(
            || document_identity(&uri),
            |snapshot| snapshot.identity.clone(),
        );
        // Invalidate and cancel queued/running work before clearing state. Keep
        // the generation entry to avoid an ABA collision if the URI is reopened
        // while an old blocking worker is still winding down.
        self.bump_generation(&uri).await;
        self.analysis_scheduler.close_document(&uri);
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
        self.latest_text.write().await.remove(&uri);
        self.dependency_graph.write().await.remove(&identity);
        self.schedule_transitive_importers(&identity).await;
        let features = self.client_features();
        for (target_uri, diags) in publish {
            self.client
                .publish_diagnostics(target_uri, diagnostics_for_client(diags, features), None)
                .await;
        }
        // The closed document needs no hint refresh. If it had open
        // importers, `schedule_transitive_importers` already requested one
        // globally so their stale evaluated hints are cleared immediately.
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let shape = self.client_features().document_symbol_shape;
        self.with_current_analysis(&uri, |current| {
            let symbols = crate::document_symbols::build_document_symbols(current.get());
            Some(match shape {
                DocumentSymbolShape::Flat => DocumentSymbolResponse::Flat(
                    crate::document_symbols::flatten_document_symbols(&uri, symbols),
                ),
                DocumentSymbolShape::Hierarchical => DocumentSymbolResponse::Nested(symbols),
            })
        })
        .await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        self.with_current_analysis(&uri, |current| {
            let analysis = current.get();
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::goto_definition::goto_definition(analysis, &uri, offset)
        })
        .await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let format = self.client_features().hover_format;
        self.with_current_analysis(&uri, |current| {
            let analysis = current.get();
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::hover::hover_with_format(analysis, offset, format)
        })
        .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;
        self.with_current_analysis(&uri, |current| {
            let analysis = current.get();
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::references::references(analysis, &uri, offset, include_declaration)
        })
        .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        self.with_current_analysis(&uri, |current| {
            crate::inlay_hints::inlay_hints(current.get(), range)
        })
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        // Signature help parses the atomically captured current text while
        // reading coordinate-free signatures from the cached analysis.
        self.with_semantic_analysis(&uri, |semantic, current_source| {
            let analysis = semantic.get();
            let offset = position_to_byte_offset(current_source, position);
            crate::signature_help::signature_help(analysis, current_source, offset)
        })
        .await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        // Completion derives context and DAG scope from the atomically
        // captured current text, never from stale declaration spans.
        self.with_semantic_analysis(&uri, |semantic, current_source| {
            let analysis = semantic.get();
            let offset = position_to_byte_offset(current_source, position);
            crate::completion::completion(analysis, current_source, offset)
                .map(CompletionResponse::Array)
        })
        .await
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let features = self.client_features();
        if !features.code_action_literals.is_supported() {
            return Ok(None);
        }
        let uri = params.text_document.uri.clone();
        let current_text = self.current_text(&uri).await;
        let snapshots = self.latest_text.read().await.clone();
        self.with_current_analysis(&uri, move |current| {
            let analysis = current.get();
            let current_text = current_text.as_ref()?;
            if current_text.as_str() != analysis.source.as_str() {
                return None;
            }
            crate::code_actions::code_actions(&params, analysis, current_text)
                .map(|actions| code_actions_for_client(actions, &snapshots, features))
                .filter(|actions| !actions.is_empty())
        })
        .await
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        let current_text = self.current_text(&uri).await;
        let snapshots = self.latest_text.read().await.clone();
        let workspace_edit_shape = self.client_features().workspace_edit_shape;
        let outcome = self
            .with_current_analysis(&uri, |current| {
                let analysis = current.get();
                let current_text = current_text.as_ref()?;
                if current_text.as_str() != analysis.source.as_str() {
                    return None;
                }
                let offset = position_to_byte_offset(current_text, position);
                let result = crate::rename::rename(analysis, &uri, offset, &new_name);
                Some(match result {
                    Ok(Some(edit)) => {
                        if let Some(project) = analysis.project_symbols.complete()
                            && !workspace_edit_matches_open_snapshots(&edit, project, &snapshots)
                        {
                            Err(crate::rename::RenameRefusal::StaleProjectSnapshot)
                        } else {
                            Ok(Some(workspace_edit_for_client(
                                edit,
                                &snapshots,
                                workspace_edit_shape,
                            )))
                        }
                    }
                    other => other,
                })
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
        self.with_current_analysis(&uri, |current| {
            let analysis = current.get();
            let current_text = current_text.as_ref()?;
            if current_text.as_str() != analysis.source.as_str() {
                return None;
            }
            let offset = position_to_byte_offset(current_text, position);
            crate::rename::prepare_rename(analysis, &uri, offset)
        })
        .await
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        self.with_current_analysis(&uri, |current| {
            crate::document_links::document_links(current.get())
        })
        .await
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        // Snapshot only the current Arc and release the text lock before
        // waiting for the bounded blocking worker.
        let text = {
            let snapshots = self.latest_text.read().await;
            snapshots
                .get(&uri)
                .map(|snapshot| Arc::clone(&snapshot.text))
        };
        let Some(text) = text else {
            return Ok(None);
        };
        match self.formatting_scheduler.format(text).await {
            Ok(edits) => Ok(edits),
            Err(FormattingTaskError::Formatter(error))
                if matches!(*error, graphcal_fmt::FormatError::Parse(_)) =>
            {
                Ok(None)
            }
            Err(error @ FormattingTaskError::SourceTooLarge { .. }) => {
                let message = error.to_string();
                self.client
                    .log_message(MessageType::WARNING, message.clone())
                    .await;
                Err(tower_lsp::jsonrpc::Error::invalid_params(message))
            }
            Err(FormattingTaskError::Cancelled) => {
                Err(tower_lsp::jsonrpc::Error::request_cancelled())
            }
            Err(error) => {
                let message = error.to_string();
                self.client
                    .log_message(MessageType::ERROR, message.clone())
                    .await;
                let mut response = tower_lsp::jsonrpc::Error::internal_error();
                response.message = message.into();
                Err(response)
            }
        }
    }
}

pub(crate) fn service() -> (LspService<Backend>, ClientSocket) {
    LspService::new(|client| Backend {
        client,
        client_features: Arc::new(OnceLock::new()),
        documents: Arc::new(RwLock::new(HashMap::new())),
        change_generations: Arc::new(RwLock::new(HashMap::new())),
        latest_text: Arc::new(RwLock::new(HashMap::new())),
        formatting_scheduler: Arc::new(FormattingScheduler::new()),
        revision_clock: Arc::new(RevisionClock::default()),
        filesystem_revisions: Arc::new(RwLock::new(HashMap::new())),
        dependency_graph: Arc::new(RwLock::new(DependencyGraph::default())),
        plugin_host: Arc::new(graphcal_plugin_host::PluginHost::new()),
        analysis_scheduler: Arc::new(AnalysisScheduler::new()),
    })
}

/// Start the LSP server, reading from stdin and writing to stdout.
pub(crate) async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = service();
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use graphcal_compiler::builtin::BuiltinFn;
    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::function_signature::{
        FunctionParam, FunctionSignature, ParamKind, ScalarValueKind,
    };
    use graphcal_compiler::syntax::function_name::FnParamName;
    use graphcal_compiler::syntax::index_name::{IndexName, IndexVarName, IndexVariantName};
    use graphcal_compiler::syntax::non_empty::NonEmpty;
    use graphcal_compiler::syntax::type_name::{FieldName, StructTypeName};
    use graphcal_eval::eval::Value;
    use indexmap::IndexMap;
    use tower_lsp::lsp_types::{InlayHintLabel, Position, Range};

    use super::*;

    use crate::analysis_pipeline::{
        AnalysisDegradation, dependency_artifact_identities, run_analysis, run_analysis_for_test,
        run_analysis_run, test_plugin_host,
    };
    use crate::fn_signatures::build_fn_signatures;
    use crate::project_symbols::{ProjectDocumentSymbols, ProjectSymbols};
    use crate::symbol_identity::VisibleBinding;
    use crate::symbol_table::{SymbolKey, SymbolTable};
    use crate::value_format::*;
    use crate::workspace_revision::AnalysisInputs;
    use crate::{completion, goto_definition, inlay_hints};

    fn imported_target_for_spelling<'a>(
        analysis: &'a AnalysisResult,
        spelling: &str,
    ) -> Option<&'a SymbolKey> {
        analysis
            .imported_bindings
            .iter()
            .find(|binding| binding.spelling().to_string() == spelling)
            .map(VisibleBinding::target)
    }

    #[test]
    fn extern_signature_labels_use_the_shared_signature_renderer() {
        let uri = Url::parse("file:///extern-labels.gcl").unwrap();
        let source = concat!(
            "type DvRange { DvRange(min: Velocity, max: Velocity) }\n",
            "import plugin \"graphcal:demo\" as demo {\n",
            "    fn inverse<D: Dim>(x: D) -> D^-1;\n",
            "    fn geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);\n",
            "    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D^2 / Time;\n",
            "    fn dv_range<I: Index>(xs: Velocity[I]) -> DvRange;\n",
            "}\n",
            "node y: Dimensionless = 1.0;\n",
        );
        let analysis = run_analysis(&uri, source, &[], test_plugin_host());
        let signature = |name: &str| {
            analysis
                .extern_fn_signatures
                .get(name)
                .map(|info| (info.label.as_str(), info.parameters.join("; ")))
        };
        assert_eq!(
            signature("demo::inverse"),
            Some(("fn demo::inverse<D: Dim>(x: D) -> D^-1", "x: D".to_string()))
        );
        assert_eq!(
            signature("demo::geometric_mean"),
            Some((
                "fn demo::geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2)",
                "x: D1; y: D2".to_string()
            ))
        );
        assert_eq!(
            signature("demo::lerp"),
            Some((
                "fn demo::lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D^2 * Frequency",
                "a: D; b: D; t: Dimensionless".to_string()
            ))
        );
        assert_eq!(
            signature("demo::dv_range"),
            Some((
                "fn demo::dv_range<I: Index>(xs: Velocity[I]) -> DvRange",
                "xs: Velocity[I]".to_string()
            ))
        );
    }

    #[test]
    fn lsp_signature_display_preserves_bool_and_int_array_kinds() {
        let index = IndexVarName::expect_valid("I");
        for (element, expected) in [
            (ScalarValueKind::Bool, "Bool[I]"),
            (ScalarValueKind::Int, "Int[I]"),
        ] {
            let kind = ParamKind::Indexed {
                element: element.clone(),
                indexes: NonEmpty::singleton(index.clone()),
            };
            let signature = FunctionSignature::try_new(
                Vec::new(),
                vec![index.clone()],
                vec![FunctionParam {
                    name: FnParamName::expect_valid("values"),
                    kind: kind.clone(),
                }],
                kind.into(),
            )
            .unwrap();
            assert_eq!(
                signature.params()[0].format_with(&mut |_| String::new()),
                format!("values: {expected}")
            );
            assert!(signature.format_with(|_| String::new()).contains(expected));
        }
    }

    #[test]
    fn scheduler_cancels_registered_document_work() {
        let scheduler = AnalysisScheduler::new();
        let uri = Url::parse("file:///cancel.gcl").unwrap();
        scheduler.open_document(&uri);
        let registration = scheduler.register(&uri, 1);
        let cancellation = registration.source.token();

        assert!(!cancellation.is_cancelled());
        scheduler.cancel_document(&uri);
        assert!(cancellation.is_cancelled());

        scheduler.finish(&uri, 1);
        let next = scheduler.register(&uri, 2);
        assert!(!next.source.is_cancelled());
        scheduler.close_document(&uri);
        scheduler.finish(&uri, 2);
        assert!(
            !scheduler
                .states
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&uri)
        );
    }

    #[test]
    fn complex_signature_help_uses_dimension_aware_signatures() {
        let signatures = build_fn_signatures();
        assert_eq!(
            signatures
                .get("complex")
                .map(|signature| signature.label.as_str()),
            Some("fn complex<D: Dim>(re: D, im: D) -> Complex<D>")
        );
        assert_eq!(
            signatures
                .get("polar")
                .map(|signature| signature.parameters.as_slice()),
            Some(["magnitude: D".to_string(), "phase: Angle".to_string()].as_slice())
        );
        assert_eq!(
            signatures
                .get("abs")
                .map(|signature| signature.label.as_str()),
            Some("fn abs<D: Dim>(x: D | Complex<D>) -> D")
        );
    }

    #[test]
    fn aggregation_signature_help_includes_product_and_rss() {
        let signatures = build_fn_signatures();
        assert_eq!(
            signatures
                .get("product")
                .map(|signature| signature.label.as_str()),
            Some("fn product<D: Dim, I: Index>(values: D[I]) -> D^|I|")
        );
        assert_eq!(
            signatures
                .get("rss")
                .map(|signature| signature.label.as_str()),
            Some("fn rss<D: Dim, I: Index>(values: D[I]) -> D")
        );
    }

    #[test]
    fn signature_help_covers_exactly_the_documented_builtins() {
        let signatures = build_fn_signatures();
        let documented = BuiltinFn::all()
            .filter(|function| function.entry().documented_arity().is_some())
            .map(BuiltinFn::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let keys = signatures
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(keys, documented);
        for function in BuiltinFn::all() {
            let Some(arity) = function.entry().documented_arity() else {
                continue;
            };
            assert_eq!(
                signatures[function.as_str()].parameters.len(),
                arity,
                "`{function}`"
            );
        }
        assert!(!signatures.contains_key("datetime"));
        assert!(!signatures.contains_key("to_float"));
        let label = |name: &str| signatures[name].label.as_str();
        assert_eq!(label("sqrt"), "fn sqrt<D: Dim>(x: D) -> D^(1/2)");
        assert_eq!(label("atan2"), "fn atan2<D: Dim>(y: D, x: D) -> Angle");
        assert_eq!(label("abs"), "fn abs<D: Dim>(x: D | Complex<D>) -> D");
        assert_eq!(
            label("cross"),
            "fn cross<D1: Dim, D2: Dim, I: Index>(a: D1[I], b: D2[I]) -> (D1 * D2)[I] where |I| = 3"
        );
    }

    #[test]
    fn parse_fallback_records_empty_symbol_table_degradation() {
        let uri = Url::parse("file:///degraded-symbols.gcl").unwrap();
        let run = run_analysis_run(
            &uri,
            "node broken: Dimensionless = @",
            &[],
            test_plugin_host(),
        );

        assert!(!run.analysis.buffer_parsed);
        let [degradation] = run.degradations.as_slice() else {
            panic!("expected one degradation, got {:?}", run.degradations);
        };
        assert!(matches!(
            degradation,
            AnalysisDegradation::EmptySymbolTable { .. }
        ));
        assert!(degradation.message(&uri).contains("fallback symbol table"));
    }

    #[test]
    fn failed_resolver_rebuild_records_file_local_resolver_degradation() {
        let uri = Url::parse("file:///degraded-resolver.gcl").unwrap();
        let run = run_analysis_run(
            &uri,
            "node duplicate: Dimensionless = 1.0;\nnode duplicate: Dimensionless = 2.0;",
            &[],
            test_plugin_host(),
        );

        assert!(run.analysis.buffer_parsed);
        let [degradation] = run.degradations.as_slice() else {
            panic!("expected one degradation, got {:?}", run.degradations);
        };
        assert!(matches!(
            degradation,
            AnalysisDegradation::FileLocalModuleResolver { .. }
        ));
        assert!(
            degradation
                .message(&uri)
                .contains("file-local module resolver")
        );
    }

    fn empty_render_context() -> graphcal_eval::eval::RenderContext {
        graphcal_eval::eval::RenderContext::new(
            BTreeMap::new(),
            graphcal_compiler::semantic::time_zone::TimeZoneRegistry::bundled(),
        )
    }

    fn quantity(si_value: f64) -> Value {
        Value::Quantity {
            si_value: graphcal_compiler::finite_value::FiniteQuantity::try_new(si_value).unwrap(),
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    fn test_owner() -> graphcal_compiler::dag_id::DagId {
        graphcal_compiler::dag_id::DagId::root_in_package("test", "<lsp-format-test>")
    }

    fn test_struct(type_name: StructTypeName, fields: IndexMap<FieldName, Value>) -> Value {
        let constructor = graphcal_compiler::syntax::type_name::record_constructor_name(&type_name);
        Value::struct_with_owner(test_owner(), type_name, constructor, fields)
    }

    fn test_indexed(index_name: IndexName, entries: IndexMap<IndexVariantName, Value>) -> Value {
        let entries = entries
            .into_iter()
            .map(|(variant, value)| {
                (
                    graphcal_compiler::syntax::index_name::IndexEntryKey::named(variant),
                    value,
                )
            })
            .collect();
        Value::indexed_with_owner(test_owner(), index_name, entries)
    }

    #[test]
    fn format_quantity_dimensionless() {
        let render = empty_render_context();
        assert_eq!(format_value_inline(&quantity(2.72), &render), "2.72");
        assert_eq!(format_value_inline(&quantity(42.0), &render), "42");
    }

    #[test]
    fn format_bool() {
        let render = empty_render_context();
        assert_eq!(format_value_inline(&Value::Bool(true), &render), "true");
        assert_eq!(format_value_inline(&Value::Bool(false), &render), "false");
    }

    #[test]
    fn format_int() {
        let render = empty_render_context();
        assert_eq!(format_value_inline(&Value::Int(7), &render), "7");
    }

    #[test]
    fn format_struct_with_fields() {
        let render = empty_render_context();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::expect_valid("dv1"), quantity(100.0));
        fields.insert(FieldName::expect_valid("dv2"), quantity(200.0));
        let val = test_struct(StructTypeName::expect_valid("TransferResult"), fields);
        assert_eq!(
            format_value_inline(&val, &render),
            "TransferResult(dv1: 100, dv2: 200)"
        );
    }

    #[test]
    fn format_struct_empty_fields() {
        let render = empty_render_context();
        let val = test_struct(StructTypeName::expect_valid("Nominal"), IndexMap::new());
        assert_eq!(format_value_inline(&val, &render), "Nominal");
    }

    #[test]
    fn format_struct_multi_variant() {
        let render = empty_render_context();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::expect_valid("thrust"), quantity(0.5));
        fields.insert(FieldName::expect_valid("duration"), quantity(3600.0));
        let val = test_struct(StructTypeName::expect_valid("LowThrust"), fields);
        assert_eq!(
            format_value_inline(&val, &render),
            "LowThrust(thrust: 0.5, duration: 3600)"
        );
    }

    #[test]
    fn inlay_hints_use_constructor_call_syntax_for_algebraic_values() {
        let text = include_str!("../../../tests/fixtures/valid/tagged_union.gcl");
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
    fn inlay_hints_render_coordinate_keys_with_axis_display_unit() {
        let text = "index TimeStep = range(1.0 h, 3.0 h, step: 1.0 h);\n\
                    node samples: Time[TimeStep] = for t: TimeStep { coord(t) };\n\
                    node peak: Key<TimeStep> = argmax(@samples);\n\
                    node copies: Key<TimeStep>[Fin(2)] = for i: Fin(2) { @peak };\n";
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
        .expect("expected inlay hints for coordinate keys");
        let labels = hints
            .into_iter()
            .filter_map(|hint| match hint.label {
                tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label),
                tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
            })
            .collect::<Vec<_>>();

        assert!(
            labels.iter().any(|label| label == " = 3 [h]"),
            "expected axis display unit on scalar coordinate key: {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|label| label == " = { #0: 3 [h], #1: 3 [h] }"),
            "expected axis display unit on indexed coordinate keys: {labels:?}"
        );
    }

    #[test]
    fn inlay_hints_keep_reordered_dynamic_dag_presentations() {
        let text = "index Rate = { Two, Three };\n\
                    index Order = { First, Second };\n\
                    dag units {\n\
                        param rate: Dimensionless;\n\
                        unit DynamicM: Length = (@rate) m;\n\
                        pub node distance: Length = 1.0 DynamicM;\n\
                    }\n\
                    node rates: Dimensionless[Rate] = {\n\
                        Rate#Two: 2.0,\n\
                        Rate#Three: 3.0,\n\
                    };\n\
                    node values: Length[Rate] = for rate: Rate {\n\
                        @units(rate: @rates[rate])::distance\n\
                    };\n\
                    node reversed: Length[Order] = {\n\
                        Order#First: @values[Rate#Three],\n\
                        Order#Second: @values[Rate#Two],\n\
                    };\n";
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
        .expect("expected inlay hints for reordered DAG output");
        let labels = hints
            .into_iter()
            .filter_map(|hint| match hint.label {
                tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label),
                tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
            })
            .collect::<Vec<_>>();

        assert!(
            labels
                .iter()
                .any(|label| label == " = { First: 1 [DynamicM], Second: 1 [DynamicM] }"),
            "expected exact invocation-aware presentation hint: {labels:?}"
        );
    }

    #[test]
    fn inlay_hints_use_declared_scale_datetime_extractors() {
        let text = "node tt: Datetime<TT> = epoch<TT>(\"2024-01-01T00:00:30\");\n\
                    node utc: Datetime = to_utc(@tt);\n\
                    node tt_hour: Int = hour(@tt);\n\
                    node utc_hour: Int = hour(@utc);\n";
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
        .expect("expected datetime inlay hints");
        let label_on_line = |line| {
            hints
                .iter()
                .find(|hint| hint.position.line == line)
                .and_then(|hint| match &hint.label {
                    tower_lsp::lsp_types::InlayHintLabel::String(label) => Some(label.as_str()),
                    tower_lsp::lsp_types::InlayHintLabel::LabelParts(_) => None,
                })
        };
        assert_eq!(label_on_line(2), Some(" = 0"));
        assert_eq!(label_on_line(3), Some(" = 23"));
    }

    #[test]
    fn inlay_hints_evaluate_valid_datetime_domain_constraints() {
        let text = "node event: Datetime<TT>(\
                    min: epoch<TT>(\"2024-01-01T00:00:00\"), \
                    max: epoch<TT>(\"2024-12-31T23:59:59\")) = \
                    epoch<TT>(\"2024-06-01T12:00:00\");\n";
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
        .expect("expected datetime domain inlay hint");
        assert!(hints.iter().any(|hint| {
            matches!(
                &hint.label,
                tower_lsp::lsp_types::InlayHintLabel::String(label)
                    if label.contains("2024-06-01T12:00:00 TT")
            )
        }));
    }

    #[test]
    fn format_indexed() {
        let render = empty_render_context();
        let mut entries = IndexMap::new();
        entries.insert(IndexVariantName::expect_valid("A"), quantity(1.0));
        entries.insert(IndexVariantName::expect_valid("B"), quantity(2.0));
        entries.insert(IndexVariantName::expect_valid("C"), quantity(3.0));
        let val = test_indexed(IndexName::expect_valid("Phase"), entries);
        assert_eq!(format_value_inline(&val, &render), "{ A: 1, B: 2, C: 3 }");
    }

    #[test]
    fn format_indexed_empty() {
        let render = empty_render_context();
        let val = test_indexed(IndexName::expect_valid("Phase"), IndexMap::new());
        assert_eq!(format_value_inline(&val, &render), "{}");
    }

    #[test]
    fn format_indexed_truncation() {
        let render = empty_render_context();
        let mut entries = IndexMap::new();
        // Create entries with long names to trigger truncation at 80 chars
        entries.insert(
            IndexVariantName::expect_valid("LongVariantAlpha"),
            quantity(1.23456),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongVariantBeta"),
            quantity(2.34567),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongVariantGamma"),
            quantity(3.45678),
        );
        entries.insert(
            IndexVariantName::expect_valid("LongVariantDelta"),
            quantity(4.56789),
        );
        let val = test_indexed(IndexName::expect_valid("Idx"), entries);
        let result = format_value_inline(&val, &render);
        assert!(
            result.len() <= INLAY_HINT_MAX_LEN + 10,
            "result too long: {result}"
        );
        assert!(result.ends_with("... }"), "expected truncation: {result}");
    }

    #[test]
    fn format_struct_inside_indexed() {
        let render = empty_render_context();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::expect_valid("x"), quantity(1.0));
        let struct_val = test_struct(StructTypeName::expect_valid("Point"), fields);
        let mut entries = IndexMap::new();
        entries.insert(IndexVariantName::expect_valid("A"), struct_val);
        let val = test_indexed(IndexName::expect_valid("Idx"), entries);
        assert_eq!(format_value_inline(&val, &render), "{ A: Point(x: 1) }");
    }

    #[test]
    fn format_nested_indexed_tuple_keyed() {
        let render = empty_render_context();
        let mut inner_a = IndexMap::new();
        inner_a.insert(IndexVariantName::expect_valid("X"), quantity(1.0));
        inner_a.insert(IndexVariantName::expect_valid("Y"), quantity(2.0));
        let mut inner_b = IndexMap::new();
        inner_b.insert(IndexVariantName::expect_valid("X"), quantity(3.0));
        inner_b.insert(IndexVariantName::expect_valid("Y"), quantity(4.0));
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
            format_value_inline(&val, &render),
            "{ (A, X): 1, (A, Y): 2, (B, X): 3, (B, Y): 4 }"
        );
    }

    #[test]
    fn format_triple_nested_indexed() {
        let render = empty_render_context();
        // 3-level nesting: Scenario[Phase[Maneuver[quantity]]]
        let mut inner_most = IndexMap::new();
        inner_most.insert(IndexVariantName::expect_valid("Dep"), quantity(100.0));
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
            format_value_inline(&val, &render),
            "{ (Nom, Launch, Dep): 100 }"
        );
    }

    #[test]
    fn format_nested_indexed_truncation() {
        let render = empty_render_context();
        let mut inner_a = IndexMap::new();
        inner_a.insert(
            IndexVariantName::expect_valid("LongNameAlpha"),
            quantity(1.23456),
        );
        inner_a.insert(
            IndexVariantName::expect_valid("LongNameBeta"),
            quantity(2.34567),
        );
        inner_a.insert(
            IndexVariantName::expect_valid("LongNameGamma"),
            quantity(3.45678),
        );
        let mut inner_b = IndexMap::new();
        inner_b.insert(
            IndexVariantName::expect_valid("LongNameAlpha"),
            quantity(4.56789),
        );
        inner_b.insert(
            IndexVariantName::expect_valid("LongNameBeta"),
            quantity(5.6789),
        );
        inner_b.insert(
            IndexVariantName::expect_valid("LongNameGamma"),
            quantity(6.7891),
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
        let result = format_value_inline(&val, &render);
        assert!(
            result.len() <= INLAY_HINT_MAX_LEN + 10,
            "result too long: {result}"
        );
        assert!(result.ends_with("... }"), "expected truncation: {result}");
    }

    #[test]
    fn diagnostic_ranges_use_lf_cr_and_crlf_lines_end_to_end() {
        let uri = untitled_uri();
        for line_ending in ["\n", "\r", "\r\n"] {
            let source = format!("node x: Dimensionless = 1.0;{line_ending}?");
            let analysis = run_analysis(&uri, &source, &[], test_plugin_host());
            let diagnostics = analysis
                .diagnostics
                .get(&uri)
                .expect("active document diagnostics must be present");
            let diagnostic = diagnostics
                .first()
                .unwrap_or_else(|| panic!("expected diagnostic for {line_ending:?}"));
            assert_eq!(
                diagnostic.range.start,
                Position::new(1, 0),
                "diagnostic position for {line_ending:?}"
            );
        }
    }

    /// Use an `untitled:` URI so `build_project` falls through to the in-memory
    /// `LoadedProject::from_source` branch (disk-less analysis).
    fn untitled_uri() -> Url {
        Url::parse("untitled:test.gcl").unwrap()
    }

    #[test]
    fn unfinished_markers_are_informational_in_executables_and_libraries() {
        let uri = untitled_uri();
        for prefix in ["", "param required: Length;"] {
            let source = format!(
                "{prefix} node missing: Length = todo {{}}; node blocked: Length = @missing;"
            );
            let analysis = run_analysis(&uri, &source, &[], test_plugin_host());
            let diagnostics = &analysis.diagnostics[&uri];
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert_eq!(
                diagnostics[0].severity,
                Some(tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION)
            );
            assert!(diagnostics[0].message.contains("TODO"));
            if prefix.is_empty() {
                assert!(
                    analysis
                        .eval_values
                        .values()
                        .any(|value| value.contains("BLOCKED"))
                );
            }
        }
        let analysis = run_analysis(
            &uri,
            "dag unused { node missing: Length = todo {}; }",
            &[],
            test_plugin_host(),
        );
        assert_eq!(analysis.diagnostics[&uri].len(), 1);
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
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
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
        let analysis = run_analysis(&untitled_uri(), text, &[], test_plugin_host());
        assert!(
            !analysis.has_no_diagnostics(),
            "dim mismatch in executable file must still produce a diagnostic",
        );
    }

    #[test]
    fn non_integer_const_to_int_reports_diagnostic() {
        let uri = untitled_uri();
        let text = "const node invalid: Int = to_int(3.7);\n";
        let analysis = run_analysis(&uri, text, &[], test_plugin_host());
        let diagnostics = analysis
            .diagnostics
            .get(&uri)
            .expect("the active URI should always have a diagnostic entry");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("is not integer-valued")),
            "expected exact-conversion diagnostic, got: {diagnostics:?}",
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
                "include lib.lib()::{ doubled };\nnode result: Dimensionless = @doubled + 1.0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        let doubled_key = imported_target_for_spelling(&analysis, "doubled")
            .expect("expected a visible binding for selective include `doubled`");
        assert!(
            analysis.imported_definitions.contains_key(doubled_key),
            "expected imported definition for selective include `doubled`, got: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn selective_includes_from_same_leaf_modules_have_no_diagnostics() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"app\"\n"),
            (
                "src/app/analysis/shared.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input + 1.0;\n",
            ),
            (
                "src/app/presentation/shared.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input * 2.0;\n",
            ),
            (
                "src/app/main.gcl",
                "include app.analysis.shared(input: 2.0)::{ output as analyzed };\n\
                 include app.presentation.shared(input: 5.0)::{ output as rendered };\n\
                 node combined: Dimensionless = @analyzed + @rendered;\n",
            ),
        ]);
        let main_path = dir.path().join("src/app/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text, &[], test_plugin_host());

        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );
    }

    #[test]
    fn same_leaf_imported_types_resolve_to_their_canonical_defining_files() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"app\"\n"),
            (
                "src/app/left.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\n",
            ),
            (
                "src/app/right.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\n",
            ),
            (
                "src/app/main.gcl",
                "import app.left as left;\n\
                 import app.right as right;\n\
                 node left_value: left::Measure = 1.0 left::u;\n\
                 node right_value: right::Measure = 2.0 right::u;\n",
            ),
        ]);
        let main_path = dir.path().join("src/app/main.gcl");
        let left_uri =
            Url::from_file_path(dir.path().join("src/app/left.gcl").canonicalize().unwrap())
                .unwrap();
        let right_uri =
            Url::from_file_path(dir.path().join("src/app/right.gcl").canonicalize().unwrap())
                .unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean canonical-owner analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for (spelling, expected_uri) in [("left::Measure", left_uri), ("right::Measure", right_uri)]
        {
            let offset = text.find(spelling).unwrap() + spelling.find("::").unwrap() + 3;
            let resolved = resolve_symbol_at(&analysis, offset)
                .unwrap_or_else(|| panic!("resolve `{spelling}`"));
            let SymbolLocation::Imported(imported) = resolved.location else {
                panic!("expected `{spelling}` to resolve to an imported definition");
            };
            assert_eq!(imported.uri, expected_uri);
        }
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
                "import helper.lib::{y};\nnode z: Dimensionless = 1.0;",
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
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());

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
                "import helper.util.lib::{y};\nnode z: Dimensionless = 1.0;",
            ),
        ]);
        let main_path = dir.path().join("src/helper/main.gcl");
        let lib_path = dir.path().join("src/helper/util/lib.gcl");
        let main_uri = Url::from_file_path(main_path.canonicalize().unwrap()).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());

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
                "import helper.lib::{y as renamed};\nnode z: Dimensionless = @renamed + 1.0;",
            ),
        ]);
        let main_path = dir.path().join("src/helper/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        let renamed_key = imported_target_for_spelling(&analysis, "renamed")
            .expect("expected imported binding under local alias `renamed`");
        assert!(
            analysis.imported_definitions.contains_key(renamed_key),
            "expected canonical imported definition for local alias `renamed`, got: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>(),
        );

        let cursor = text.find("@renamed").unwrap() + 1;
        let resolved = crate::resolve::resolve_symbol_at(&analysis, cursor)
            .expect("selective alias reference should resolve");
        assert_eq!(&resolved.key, renamed_key);
        assert!(matches!(
            resolved.location,
            crate::resolve::SymbolLocation::Imported(_)
        ));
    }

    #[test]
    fn multiple_aliases_preserve_each_visible_binding_for_one_identity() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            (
                "src/helper/lib.gcl",
                "pub const node y: Dimensionless = 2.0;",
            ),
            (
                "src/helper/main.gcl",
                "import helper.lib::{y as first};\n\
                 import helper.lib::{y as other};\n\
                 node z: Dimensionless = @first + @other;",
            ),
        ]);
        let main_path = dir.path().join("src/helper/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text, &[], test_plugin_host());
        assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);

        let first = imported_target_for_spelling(&analysis, "first").unwrap();
        let other = imported_target_for_spelling(&analysis, "other").unwrap();
        assert_eq!(first, other);
        for spelling in ["@first", "@other"] {
            let offset = text.find(spelling).unwrap() + 1;
            assert_eq!(
                crate::resolve::resolve_symbol_at(&analysis, offset).map(|resolved| resolved.key),
                Some(first.clone())
            );
        }
    }

    /// Issue #631 case 1+2: `@module.dag(args)::out` — both the DAG name and
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
                 node doubled: Velocity = @lib.scale(factor: 2.0, v: @speed)::result;\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let lib_path = dir.path().join("src/lib/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
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
        let result_offset = text.find(")::result").expect("`::result` projection") + 3;
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

    #[test]
    fn goto_definition_resolves_direct_file_and_inline_dag_alias_calls() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"lib\"\n"),
            (
                "src/lib/library.gcl",
                "param x: Dimensionless;\n\
                 pub node doubled: Dimensionless = @x * 2.0;\n\
                 pub dag helper {\n    \
                     param x: Dimensionless;\n    \
                     pub node tripled: Dimensionless = @x * 3.0;\n\
                 }\n",
            ),
            (
                "src/lib/main.gcl",
                "import lib.library as file_dag;\n\
                 import lib.library.helper as inline_dag;\n\
                 import lib.library::{helper as selected_dag};\n\
                 node a: Dimensionless = @file_dag(x: 2.0)::doubled;\n\
                 node b: Dimensionless = @inline_dag(x: 3.0)::tripled;\n\
                 node c: Dimensionless = @selected_dag(x: 4.0)::tripled;\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let library_path = dir.path().join("src/lib/library.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let library_uri = Url::from_file_path(library_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for token in ["doubled", "inline_dag", "selected_dag", "tripled"] {
            let offset = text.rfind(token).expect("call token in main.gcl");
            let resolved = resolve_symbol_at(&analysis, offset + 1)
                .unwrap_or_else(|| panic!("resolve `{token}`"));
            let SymbolLocation::Imported(imported) = resolved.location else {
                panic!("expected `{token}` to resolve to an imported definition");
            };
            assert_eq!(
                imported.uri, library_uri,
                "{token} should jump to library.gcl"
            );
        }
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
                "import lib.lib::{ index Color };\n\
                 param favorite: Dimensionless[Color] = {\n    \
                     Color#Red: 1.0,\n    \
                     Color#Green: 2.0,\n    \
                     Color#Blue: 3.0,\n\
                 };\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let lib_path = dir.path().join("src/lib/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        // Cursor on `Red` inside `Color#Red:`.
        let red_offset = text.find("Color#Red").expect("Color#Red token") + "Color#".len();
        let red_resolved = resolve_symbol_at(&analysis, red_offset + 1).expect("resolve `Red`");
        let SymbolLocation::Imported(red_imported) = red_resolved.location else {
            panic!("expected `Red` to resolve to an imported definition");
        };
        assert_eq!(red_imported.uri, lib_uri, "Red should jump to lib.gcl");
    }

    #[test]
    fn goto_definition_resolves_qualified_table_axes_and_slice_labels() {
        use crate::resolve::{SymbolLocation, resolve_symbol_at};

        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"lib\"\n"),
            (
                "src/lib/axes.gcl",
                "pub index Scenario = { Nominal };\n\
                 pub index Row = { A };\n\
                 pub index Column = { X };\n",
            ),
            (
                "src/lib/main.gcl",
                "import lib.axes as axes;\n\
                 param cube: Dimensionless[axes::Scenario, axes::Row, axes::Column] =\n    \
                     table[axes::Scenario, axes::Row, axes::Column] {\n        \
                         [axes::Scenario#Nominal]\n        \
                         : X;\n        \
                         A: 1.0;\n    \
                     };\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let axes_path = dir.path().join("src/lib/axes.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let axes_uri = Url::from_file_path(axes_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        let table_axis = text.find("table[axes::Scenario").unwrap() + "table[axes::".len();
        let slice_axis = text.find("[axes::Scenario#Nominal]").unwrap() + "[axes::".len();
        let slice_variant = text.find("axes::Scenario#Nominal").unwrap() + "axes::Scenario#".len();
        let column_variant = text.find(": X;").unwrap() + 2;
        let row_variant = text.find("A: 1.0;").unwrap();

        for (offset, spelling) in [
            (table_axis, "Scenario"),
            (slice_axis, "Scenario"),
            (slice_variant, "Nominal"),
            (column_variant, "X"),
            (row_variant, "A"),
        ] {
            let resolved = resolve_symbol_at(&analysis, offset)
                .unwrap_or_else(|| panic!("resolve table spelling `{spelling}`"));
            let SymbolLocation::Imported(imported) = resolved.location else {
                panic!("expected `{spelling}` to resolve to an imported definition");
            };
            assert_eq!(imported.uri, axes_uri, "wrong owner for `{spelling}`");
        }
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
                "import lib.lib::{ index Color as Palette };\n\
                 param favorite: Dimensionless[Palette] = {\n    \
                     Palette#Red: 1.0,\n    \
                     Palette#Green: 2.0,\n    \
                     Palette#Blue: 3.0,\n\
                 };\n",
            ),
        ]);
        let main_path = dir.path().join("src/lib/main.gcl");
        let uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&uri, &text, &[], test_plugin_host());

        let palette_red_key = imported_target_for_spelling(&analysis, "Palette#Red")
            .expect("expected imported binding under local alias `Palette#Red`");
        assert!(
            analysis.imported_definitions.contains_key(palette_red_key),
            "expected imported variant under local alias `Palette#Red`, got: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>(),
        );
    }

    /// Issue #830: goto-definition and hover must resolve symbols brought in
    /// by module imports — bare (`import gotom.lib;` → `@lib::g0`) and aliased
    /// (`import gotom.lib as alias_lib;` → `@alias_lib::g0`).
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
                 node g: Acceleration = @lib::g0;\n\
                 node g2: Acceleration = @alias_lib::g0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/gotom/main.gcl");
        let lib_path = dir.path().join("src/gotom/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for (needle, prefix) in [("@lib::g0", "@lib::"), ("@alias_lib::g0", "@alias_lib::")] {
            let offset = text.find(needle).expect("reference in main.gcl") + prefix.len();
            let resolved = resolve_symbol_at(&analysis, offset)
                .unwrap_or_else(|| panic!("expected `{needle}` to resolve"));
            let SymbolLocation::Imported(imported) = resolved.location else {
                panic!("expected `{needle}` to resolve to an imported definition");
            };
            assert_eq!(imported.uri, lib_uri, "`{needle}` should jump to lib.gcl");
        }
    }

    #[test]
    fn analysis_access_separates_exact_coordinates_from_stale_root_candidates() {
        let uri = Url::parse("file:///tmp/stale-position.gcl").unwrap();
        let root = document_identity(&uri);
        let dependency = DocumentIdentity::file(std::path::PathBuf::from("/tmp/lib.gcl"));
        let clock = RevisionClock::default();
        let root_revision = clock.next().unwrap();
        let dependency_revision = clock.next().unwrap();
        let snapshot = AnalysisInputSnapshot::new(
            root.clone(),
            root_revision,
            HashMap::from([(dependency.clone(), dependency_revision)]),
        );
        let mut analysis = run_analysis_for_test(
            &uri,
            "const node first: Dimensionless = 1.0;\r\n\
             node second: Dimensionless = @first;\r\n",
        );
        analysis.inputs = snapshot.finish_loaded_project(HashSet::from([dependency.clone()]));

        let current = HashMap::from([
            (root.clone(), root_revision),
            (dependency.clone(), dependency_revision),
        ]);
        assert!(select_current_analysis(&analysis, &current).is_some());
        assert!(select_semantic_analysis(&analysis, &current).is_some());

        // This revision represents a current buffer with deleted CRLF text and
        // an inserted astral character (a two-code-unit UTF-16 shift).
        let shifted_root = HashMap::from([
            (root.clone(), clock.next().unwrap()),
            (dependency.clone(), dependency_revision),
        ]);
        assert!(select_current_analysis(&analysis, &shifted_root).is_none());
        assert!(select_semantic_analysis(&analysis, &shifted_root).is_some());

        let changed_dependency =
            HashMap::from([(root, root_revision), (dependency, clock.next().unwrap())]);
        assert!(select_current_analysis(&analysis, &changed_dependency).is_none());
        assert!(select_semantic_analysis(&analysis, &changed_dependency).is_none());
    }

    #[test]
    fn workspace_rename_refuses_stale_open_project_document() {
        let uri = Url::parse("file:///open.gcl").unwrap();
        let project = ProjectSymbolIndex::loaded_dependency_closure(
            uri.clone(),
            [ProjectDocumentSymbols::new(
                uri.clone(),
                Arc::new("old".to_string()),
                SymbolTable::default(),
                Vec::new(),
            )],
        );
        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), Vec::new())])),
            document_changes: None,
            change_annotations: None,
        };
        let revision = RevisionClock::default().next().unwrap();
        let identity = document_identity(&uri);
        let snapshots = HashMap::from([(
            uri,
            OpenDocumentSnapshot {
                identity,
                revision,
                text: Arc::new("new".to_string()),
                version: 2,
            },
        )]);

        assert!(!workspace_edit_matches_open_snapshots(
            &edit, &project, &snapshots
        ));
    }

    #[test]
    fn workspace_rename_edits_carry_open_document_versions() {
        let open_uri = Url::parse("file:///open.gcl").unwrap();
        let closed_uri = Url::parse("file:///closed.gcl").unwrap();
        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([
                (
                    open_uri.clone(),
                    vec![TextEdit {
                        range: tower_lsp::lsp_types::Range::default(),
                        new_text: "renamed".to_string(),
                    }],
                ),
                (
                    closed_uri.clone(),
                    vec![TextEdit {
                        range: tower_lsp::lsp_types::Range::default(),
                        new_text: "renamed".to_string(),
                    }],
                ),
            ])),
            document_changes: None,
            change_annotations: None,
        };
        let revision = RevisionClock::default().next().unwrap();
        let snapshots = HashMap::from([(
            open_uri.clone(),
            OpenDocumentSnapshot {
                identity: document_identity(&open_uri),
                revision,
                text: Arc::new(String::new()),
                version: 7,
            },
        )]);

        let versioned =
            workspace_edit_for_client(edit, &snapshots, WorkspaceEditShape::DocumentChanges);
        assert!(versioned.changes.is_none());
        let Some(DocumentChanges::Edits(documents)) = versioned.document_changes else {
            panic!("expected versioned document edits");
        };
        assert_eq!(documents.len(), 2);
        let versions: HashMap<_, _> = documents
            .into_iter()
            .map(|document| (document.text_document.uri, document.text_document.version))
            .collect();
        assert_eq!(versions[&open_uri], Some(7));
        assert_eq!(versions[&closed_uri], None);
    }

    #[test]
    fn legacy_workspace_edits_keep_changes_map() {
        let uri = Url::parse("file:///open.gcl").unwrap();
        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(uri, Vec::new())])),
            ..Default::default()
        };

        let shaped = workspace_edit_for_client(edit, &HashMap::new(), WorkspaceEditShape::Changes);
        assert!(shaped.changes.is_some());
        assert!(shaped.document_changes.is_none());
    }

    /// Build a bare `AnalysisResult` carrying only a diagnostics map, for
    /// exercising the per-URI ownership merge.
    fn analysis_with_diags(diags: HashMap<Url, Vec<Diagnostic>>) -> AnalysisResult {
        AnalysisResult {
            inputs: AnalysisInputs::untracked_for_test(DocumentIdentity::virtual_uri(
                Url::parse("file:///diagnostics.gcl").unwrap(),
            )),
            source: Arc::new(String::new()),
            symbol_table: SymbolTable::default(),
            project_symbols: ProjectSymbols::Incomplete,
            imported_definitions: HashMap::new(),
            imported_bindings: Vec::new(),
            import_surfaces: HashMap::new(),
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
        let good = run_analysis(&uri, good_text, &[], test_plugin_host());
        assert!(good.buffer_parsed);
        assert!(good.has_no_diagnostics());

        // Mid-edit: the buffer no longer parses.
        let broken_text = format!("{good_text}node next: Mass = @");
        let broken = run_analysis(&uri, &broken_text, &[], test_plugin_host());
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
                    .values()
                    .any(|definition| definition.name == name),
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
                "import staleo.lib;\nnode x: Acceleration = @lib::g0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/staleo/main.gcl");
        let lib_path = dir.path().join("src/staleo/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();

        // Control: without the overlay, the stale disk content produces
        // diagnostics (on lib.gcl's URI).
        let stale = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            !stale.has_no_diagnostics(),
            "expected diagnostics from the broken on-disk lib.gcl"
        );

        let open_buffers = vec![OpenBuffer {
            path: lib_path,
            text: Arc::new("pub const node g0: Acceleration = 9.80665 m/s^2;\n".to_string()),
        }];
        let analysis = run_analysis(&main_uri, &text, &open_buffers, test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "the fixed editor buffer must shadow the broken disk content, got: {:?}",
            analysis.diagnostics,
        );
    }

    #[test]
    fn analysis_ignores_open_buffers_outside_the_active_root_capability() {
        let project = write_project(&[
            ("graphcal.toml", "[package]\nname = \"active\"\n"),
            ("src/active/main.gcl", "node x: Dimensionless = 1.0;\n"),
        ]);
        let unrelated = write_project(&[
            ("graphcal.toml", "[package]\nname = \"other\"\n"),
            ("src/other/main.gcl", "node outside: Dimensionless = 2.0;\n"),
        ]);
        let main_path = project.path().join("src/active/main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let open_buffers = vec![OpenBuffer {
            path: unrelated.path().join("src/other/main.gcl"),
            text: Arc::new("this unrelated buffer is mid-edit }{".to_string()),
        }];

        let analysis = run_analysis(&main_uri, &text, &open_buffers, test_plugin_host());

        assert!(
            analysis.has_no_diagnostics(),
            "an unrelated open buffer must not widen or invalidate the active project: {:?}",
            analysis.diagnostics
        );
    }

    #[test]
    fn failed_load_tracks_missing_import_and_manifest_inputs() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"missingdep\"\n"),
            (
                "src/missingdep/main.gcl",
                "import missingdep.lib::{value};\nnode result: Dimensionless = @value;\n",
            ),
        ]);
        let main_path = dir.path().join("src/missingdep/main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let source = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &source, &[], test_plugin_host());
        let dependencies: HashSet<_> = analysis.inputs.dependencies().cloned().collect();

        assert!(!analysis.has_no_diagnostics());
        assert!(!analysis.inputs.has_complete_dependencies());
        assert!(dependencies.contains(&file_identity(&dir.path().join("src/missingdep/lib.gcl"))));
        assert!(dependencies.contains(&file_identity(&dir.path().join("graphcal.toml"))));
    }

    #[test]
    fn dependency_watch_inputs_include_authenticated_out_of_source_plugins() {
        use graphcal_io::{
            InMemoryFileSystem, NeverCancel, SourceTreeHashLimits, VirtualAbsolutePath,
        };
        let root = std::path::Path::new("/dependency");
        let mut fs = InMemoryFileSystem::new();
        for (path, text) in [
            ("graphcal.toml", "manifest"),
            ("src/library.gcl", "source"),
            ("plugins/kernel.wasm", "binary"),
        ] {
            fs.add_file(
                VirtualAbsolutePath::new(root.join(path)).unwrap(),
                text.to_string(),
            )
            .unwrap();
        }
        let mut snapshot = graphcal_io::capture_source_tree(
            &fs,
            root,
            std::path::Path::new("src"),
            SourceTreeHashLimits::unbounded(),
            &NeverCancel,
        )
        .unwrap();
        snapshot
            .capture_artifact(
                &fs,
                root,
                std::path::Path::new("plugins/kernel.wasm"),
                SourceTreeHashLimits::unbounded(),
                &NeverCancel,
            )
            .unwrap();
        let closure = graphcal_project::loader::LoadedPackageClosure {
            lockfile: String::new(),
            dependencies: std::collections::BTreeMap::from([(
                graphcal_package::PackageInstanceId::new("dependency").unwrap(),
                graphcal_project::loader::LoadedDependency {
                    root: root.to_path_buf(),
                    snapshot,
                },
            )]),
        };
        let inputs: HashSet<_> = dependency_artifact_identities(&closure).collect();
        for path in ["graphcal.toml", "src/library.gcl", "plugins/kernel.wasm"] {
            assert!(inputs.contains(&DocumentIdentity::file(root.join(path))));
        }
    }

    #[test]
    fn analysis_tracks_manifest_lockfile_and_plugin_artifacts() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"plugininputs\"\n"),
            (
                "src/plugininputs/main.gcl",
                "import plugin \"plugin.wasm\" as plugin {\nfn value() -> Dimensionless;\n}\n",
            ),
            ("plugin.wasm", "not a wasm module"),
        ]);
        let main_path = dir.path().join("src/plugininputs/main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let source = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &source, &[], test_plugin_host());
        let dependencies: HashSet<_> = analysis.inputs.dependencies().cloned().collect();

        for input in ["graphcal.toml", "graphcal.lock", "plugin.wasm"] {
            assert!(
                dependencies.contains(&file_identity(&dir.path().join(input))),
                "missing tracked filesystem input {input}: {dependencies:?}"
            );
        }
    }

    #[test]
    fn dependency_revision_change_invalidates_importer_analysis() {
        let dir = write_project(&[
            ("graphcal.toml", "[package]\nname = \"fresh\"\n"),
            (
                "src/fresh/lib.gcl",
                "pub const node y: Dimensionless = 2.0;\n",
            ),
            (
                "src/fresh/main.gcl",
                "import fresh.lib::{y};\nnode z: Dimensionless = @y + 1.0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/fresh/main.gcl");
        let lib_path = dir.path().join("src/fresh/lib.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let main_text = std::fs::read_to_string(&main_path).unwrap();
        let root = document_identity(&main_uri);
        let dependency = DocumentIdentity::file(lib_path.canonicalize().unwrap());
        let clock = RevisionClock::default();
        let root_revision = clock.next().unwrap();
        let old_dependency_revision = clock.next().unwrap();
        let input = AnalysisInputSnapshot::new(
            root.clone(),
            root_revision,
            HashMap::from([(dependency.clone(), old_dependency_revision)]),
        );
        let overlay = vec![OpenBuffer {
            path: lib_path,
            text: Arc::new("pub const node y: Dimensionless = 2.0;\n".to_string()),
        }];
        let analysis = run_analysis_with_cancellation(
            &main_uri,
            &main_text,
            &overlay,
            test_plugin_host(),
            &input,
            &CancellationToken::unbounded(),
        )
        .unwrap()
        .analysis;
        assert!(analysis.has_no_diagnostics());
        assert!(
            analysis
                .inputs
                .dependencies()
                .any(|identity| identity == &dependency)
        );
        assert!(analysis.inputs.is_current(&HashMap::from([
            (root.clone(), root_revision),
            (dependency.clone(), old_dependency_revision),
        ])));
        let new_dependency_revision = clock.next().unwrap();
        assert!(!analysis.inputs.is_current(&HashMap::from([
            (root.clone(), root_revision),
            (dependency.clone(), new_dependency_revision),
        ])));
        let refreshed_input = AnalysisInputSnapshot::new(
            root,
            root_revision,
            HashMap::from([(dependency, new_dependency_revision)]),
        );
        let refreshed_overlay = vec![OpenBuffer {
            path: dir.path().join("src/fresh/lib.gcl"),
            text: Arc::new("pub const node q: Dimensionless = 4.0;\n".to_string()),
        }];
        let refreshed = run_analysis_with_cancellation(
            &main_uri,
            &main_text,
            &refreshed_overlay,
            test_plugin_host(),
            &refreshed_input,
            &CancellationToken::unbounded(),
        )
        .unwrap()
        .analysis;
        assert!(!refreshed.has_no_diagnostics());
        let use_offset = main_text.rfind('y').unwrap();
        assert!(goto_definition::goto_definition(&analysis, &main_uri, use_offset).is_some());
        assert!(goto_definition::goto_definition(&refreshed, &main_uri, use_offset).is_none());
        let old_labels: HashSet<_> = completion::completion(&analysis, &main_text, use_offset)
            .unwrap()
            .into_iter()
            .map(|item| item.label)
            .collect();
        let new_labels: HashSet<_> = completion::completion(&refreshed, &main_text, use_offset)
            .unwrap()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(old_labels.contains("y"));
        assert!(!new_labels.contains("y"));

        let range = tower_lsp::lsp_types::Range {
            start: Position::new(0, 0),
            end: Position::new(u32::MAX, u32::MAX),
        };
        let old_hints = inlay_hints::inlay_hints(&analysis, range).unwrap();
        let new_hints = inlay_hints::inlay_hints(&refreshed, range).unwrap_or_default();
        let is_old_value = |hint: &InlayHint| matches!(&hint.label, InlayHintLabel::String(label) if label == " = 3");
        assert!(old_hints.iter().any(is_old_value));
        assert!(!new_hints.iter().any(is_old_value));
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
                "import gotom.lib::{g0};\n\
                 import gotom.lib as m;\n\
                 node g: Acceleration = @g0;\n\
                 node g2: Acceleration = @m::g0;\n",
            ),
        ]);
        let main_path = dir.path().join("src/gotom/main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for spelling in ["g0", "m::g0"] {
            let key = imported_target_for_spelling(&analysis, spelling)
                .unwrap_or_else(|| panic!("expected imported binding for {spelling}"));
            let imported = analysis
                .imported_definitions
                .get(key)
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
                 const node from_a: Dimensionless = a::bias;\n\
                 node phase_score: Dimensionless[a::Phase] = for current_phase: a::Phase {\n\
                     match current_phase {\n\
                         a::Phase#Burn => a::bias,\n\
                         a::Phase#Coast => b::bias,\n\
                     }\n\
                 };\n\
                 node item: a::Item = a::Pick(distance: a::bias);\n",
            ),
        ]);
        let main_path = dir.path().join("src/proj/main.gcl");
        let a_path = dir.path().join("src/proj/a.gcl");
        let b_path = dir.path().join("src/proj/b.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let a_uri = Url::from_file_path(a_path.canonicalize().unwrap()).unwrap();
        let b_uri = Url::from_file_path(b_path.canonicalize().unwrap()).unwrap();
        let text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = run_analysis(&main_uri, &text, &[], test_plugin_host());
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics,
        );

        for (needle, token_prefix) in [
            ("a::bias", "a::"),
            ("a::Phase]", "a::"),
            ("a::Phase#Burn", "a::Phase#"),
            ("a::Item", "a::"),
            ("a::Pick", "a::"),
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
