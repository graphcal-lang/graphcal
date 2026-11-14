//! The server state and the operations its protocol handlers share.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::RwLock;
use tower_lsp::Client;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{MessageType, Url};

use crate::analysis::AnalysisResult;
use crate::analysis_generations::AnalysisGenerations;
use crate::client_capabilities::ClientFeatureSupport;
use crate::feature_worker::{FeatureWorkError, run_feature};
use crate::file_identity::{document_identity, file_identity};
use crate::formatting_scheduler::FormattingScheduler;
use crate::workspace_revision::{
    CurrentRevisions, DependencyGraph, DocumentIdentity, DocumentRevision, RevisionClock,
    RevisionExhausted,
};
use graphcal_compiler::cancellation::CancellationToken;

use super::analysis_worker::{AnalysisWorker, analyze_store_publish};
#[cfg(test)]
use super::custom_protocol::BlockingFeatureProbe;
use super::documents::{
    CurrentAnalysis, OpenDocumentSnapshot, RecordedDocument, SemanticAnalysis, StoredDocument,
    is_current_analysis, is_semantic_analysis, open_revisions,
};
use super::revisions::RevisionObservers;
use super::scheduler::AnalysisScheduler;

/// Debounce delay for `did_change` notifications (milliseconds).
const DEBOUNCE_DELAY_MS: u64 = 300;

/// The LSP server backend.
#[cfg_attr(test, derive(Debug))]
pub struct Backend {
    pub(super) client: Client,
    /// Immutable interpretation of the initialize request's optional client
    /// capabilities. `OnceLock` mirrors the protocol's initialize-once rule.
    pub(super) client_features: Arc<OnceLock<ClientFeatureSupport>>,
    /// Per-document analysis results, keyed by URI.
    pub(super) documents: Arc<RwLock<HashMap<Url, StoredDocument>>>,
    /// Change generation of each open document and its latest finished
    /// pass. A delayed task only runs analysis if its generation is still
    /// current (no newer change arrived); requests wait for the current pass
    /// instead of answering a keystroke too early.
    pub(super) generations: Arc<AnalysisGenerations>,
    /// Latest document text as typed in the editor, updated synchronously on
    /// every `did_open`/`did_change`/`did_save`. May be newer than
    /// `AnalysisResult::source` (analysis is debounced and can fail or time
    /// out), so text-sensitive requests fired right after a keystroke —
    /// trigger-character completion, signature help, formatting — must read
    /// this instead of the analyzed snapshot.
    pub(super) latest_text: Arc<RwLock<HashMap<Url, OpenDocumentSnapshot>>>,
    /// Bounded worker gate for memory-intensive synchronous formatting.
    pub(super) formatting_scheduler: Arc<FormattingScheduler>,
    /// Monotonic revision allocator shared by all document identities.
    revision_clock: Arc<RevisionClock>,
    /// Latest client-reported revision for each closed filesystem input.
    /// Open-buffer revisions override these entries when snapshots are built.
    pub(super) filesystem_revisions: Arc<RwLock<HashMap<DocumentIdentity, DocumentRevision>>>,
    /// Reverse dependency edges from the latest accepted analyses.
    pub(super) dependency_graph: Arc<RwLock<DependencyGraph>>,
    /// Passes whose observed revisions a prune must not invalidate.
    pub(super) revision_observers: Arc<RevisionObservers>,
    /// WASM plugin host shared across analysis passes, so re-analysis on
    /// every debounced keystroke hits the content-hash module cache instead
    /// of recompiling unchanged plugins.
    pub(super) plugin_host: Arc<graphcal_plugin_host::PluginHost>,
    /// Per-document single-flight and global concurrency control.
    pub(super) analysis_scheduler: Arc<AnalysisScheduler>,
    /// Controls the test-only slow request [`BlockingFeatureRequest`].
    #[cfg(test)]
    pub(crate) blocking_feature_probe: Arc<BlockingFeatureProbe>,
}

/// Map a failed feature worker onto the JSON-RPC response.
fn feature_response<R>(
    result: std::result::Result<Option<R>, FeatureWorkError>,
) -> Result<Option<R>> {
    result.map_err(|error| {
        let mut response = tower_lsp::jsonrpc::Error::internal_error();
        response.message = error.to_string().into();
        response
    })
}

impl Backend {
    /// A backend with empty state that talks to `client`.
    pub(super) fn new(client: Client) -> Self {
        Self {
            client,
            client_features: Arc::new(OnceLock::new()),
            documents: Arc::new(RwLock::new(HashMap::new())),
            generations: Arc::new(AnalysisGenerations::default()),
            latest_text: Arc::new(RwLock::new(HashMap::new())),
            formatting_scheduler: Arc::new(FormattingScheduler::new()),
            revision_clock: Arc::new(RevisionClock::default()),
            filesystem_revisions: Arc::new(RwLock::new(HashMap::new())),
            dependency_graph: Arc::new(RwLock::new(DependencyGraph::default())),
            revision_observers: Arc::new(RevisionObservers::default()),
            plugin_host: Arc::new(graphcal_plugin_host::PluginHost::new()),
            analysis_scheduler: Arc::new(AnalysisScheduler::new()),
            #[cfg(test)]
            blocking_feature_probe: Arc::new(BlockingFeatureProbe::default()),
        }
    }

    /// Shared handles an analysis worker needs, cloned for one pass.
    fn analysis_worker(&self) -> AnalysisWorker {
        AnalysisWorker {
            client: self.client.clone(),
            documents: Arc::clone(&self.documents),
            generations: Arc::clone(&self.generations),
            latest_text: Arc::clone(&self.latest_text),
            filesystem_revisions: Arc::clone(&self.filesystem_revisions),
            dependency_graph: Arc::clone(&self.dependency_graph),
            revision_observers: Arc::clone(&self.revision_observers),
            client_features: self.client_features(),
            plugin_host: Arc::clone(&self.plugin_host),
            scheduler: Arc::clone(&self.analysis_scheduler),
        }
    }

    pub(super) fn client_features(&self) -> ClientFeatureSupport {
        self.client_features.get().copied().unwrap_or_default()
    }

    pub(super) fn request_inlay_hint_refresh(&self) {
        if !self.client_features().inlay_hint_refresh.is_supported() {
            return;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.inlay_hint_refresh().await;
        });
    }

    /// Whether a document the client opens is Graphcal: its language
    /// identifier says so, or its path ends in `.gcl`. An untitled buffer
    /// has no suffix, so the language identifier is all that marks it.
    pub(super) fn opens_as_graphcal(uri: &Url, language_id: &str) -> bool {
        language_id.eq_ignore_ascii_case("graphcal")
            || std::path::Path::new(uri.path())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gcl"))
    }

    /// Whether `uri` is a document opened as Graphcal and not yet closed.
    pub(super) async fn is_open_graphcal_document(&self, uri: &Url) -> bool {
        self.latest_text.read().await.contains_key(uri)
    }

    /// The cached analysis of `uri` when `accept` admits it against the
    /// current workspace revisions, with the document's latest editor text.
    ///
    /// Both come from one consistent snapshot: the revision locks are held
    /// until the analysis is selected.
    async fn cached_analysis(
        &self,
        uri: &Url,
        accept: fn(&AnalysisResult, &CurrentRevisions<'_>) -> bool,
    ) -> Option<(Arc<AnalysisResult>, Option<Arc<String>>)> {
        let snapshots = self.latest_text.read().await;
        let disk_revisions = self.filesystem_revisions.read().await;
        let revisions = CurrentRevisions::new(&disk_revisions, open_revisions(&snapshots));
        let current_source = snapshots
            .get(uri)
            .map(|snapshot| Arc::clone(&snapshot.text));
        let docs = self.documents.read().await;
        let analysis = docs
            .get(uri)
            .map(|stored| Arc::clone(&stored.analysis))
            .filter(|analysis| accept(analysis, &revisions));
        drop(docs);
        drop(disk_revisions);
        drop(snapshots);
        Some((analysis?, current_source))
    }

    /// Apply an exact-coordinate feature only to a fully current analysis.
    ///
    /// The feature runs off the serving task, so it neither blocks other
    /// requests nor ignores `$/cancelRequest`.
    pub(super) async fn with_current_analysis<F, R>(&self, uri: &Url, f: F) -> Result<Option<R>>
    where
        F: FnOnce(CurrentAnalysis) -> Option<R> + Send + 'static,
        R: Send + 'static,
    {
        let Some((analysis, _)) = self.cached_analysis(uri, is_current_analysis).await else {
            return Ok(None);
        };
        feature_response(run_feature(move |_| f(CurrentAnalysis(analysis))).await)
    }

    /// [`Self::with_current_analysis`] for work that is long enough to
    /// observe the request's cancellation.
    pub(super) async fn with_current_analysis_cancellable<F, R>(
        &self,
        uri: &Url,
        f: F,
    ) -> Result<Option<R>>
    where
        F: FnOnce(CurrentAnalysis, &CancellationToken) -> Option<R> + Send + 'static,
        R: Send + 'static,
    {
        let Some((analysis, _)) = self.cached_analysis(uri, is_current_analysis).await else {
            return Ok(None);
        };
        feature_response(
            run_feature(move |cancellation| f(CurrentAnalysis(analysis), cancellation)).await,
        )
    }

    /// Apply a coordinate-free candidate feature when dependencies are still
    /// current. The root text may be newer than the cached semantic snapshot.
    pub(super) async fn with_semantic_analysis<F, R>(&self, uri: &Url, f: F) -> Result<Option<R>>
    where
        F: FnOnce(SemanticAnalysis, &str) -> Option<R> + Send + 'static,
        R: Send + 'static,
    {
        let Some((analysis, Some(current_source))) =
            self.cached_analysis(uri, is_semantic_analysis).await
        else {
            return Ok(None);
        };
        feature_response(run_feature(move |_| f(SemanticAnalysis(analysis), &current_source)).await)
    }

    /// Record the latest editor text for `uri`, synchronously with the
    /// notification that delivered it.
    pub(super) async fn record_latest_text(
        &self,
        uri: &Url,
        text: Arc<String>,
        version: Option<i32>,
    ) -> std::result::Result<RecordedDocument, RevisionExhausted> {
        use std::collections::hash_map::Entry;
        let revision = self.revision_clock.next()?;
        let identity = match self.latest_text.write().await.entry(uri.clone()) {
            Entry::Occupied(mut entry) => {
                let snapshot = entry.get_mut();
                snapshot.revision = revision;
                snapshot.text = text;
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
                    text,
                    version: version.unwrap_or_default(),
                });
                identity
            }
        };
        Ok(RecordedDocument { identity, revision })
    }

    /// Cancel and debounce the open roots identified by an invalidation plan.
    pub(super) async fn schedule_open_roots(&self, affected: &HashSet<DocumentIdentity>) {
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
            // A document closed meanwhile is not resurrected.
            let Some(generation) = self.generations.advance(&uri) else {
                continue;
            };
            self.analysis_scheduler.cancel_document(&uri);
            self.spawn_debounced_analysis(uri, text, revision, generation);
        }
    }

    /// Cancel and debounce every open analysis that consumes `changed`.
    pub(super) async fn schedule_transitive_importers(&self, changed: &DocumentIdentity) {
        let mut importers = self
            .dependency_graph
            .read()
            .await
            .transitive_importers(changed);
        importers.remove(changed);
        self.schedule_open_roots(&importers).await;
    }

    /// Record one client-reported external filesystem change and return the
    /// open roots it affects. Scheduling is left to the caller, so a batch of
    /// changes schedules each affected root once.
    pub(super) async fn record_filesystem_change(&self, uri: &Url) -> HashSet<DocumentIdentity> {
        let Ok(path) = uri.to_file_path() else {
            return HashSet::new();
        };
        let Some(kind) = crate::filesystem_events::WatchedArtifactKind::from_path(&path) else {
            return HashSet::new();
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
            return HashSet::new();
        }
        let revision = match self.revision_clock.next() {
            Ok(revision) => revision,
            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
                return HashSet::new();
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
        affected
    }

    /// Latest editor text for `uri`, falling back to the analyzed snapshot.
    pub(super) async fn current_text(&self, uri: &Url) -> Option<Arc<String>> {
        if let Some(snapshot) = self.latest_text.read().await.get(uri) {
            return Some(Arc::clone(&snapshot.text));
        }
        self.documents
            .read()
            .await
            .get(uri)
            .map(|stored| Arc::clone(&stored.analysis.source))
    }

    /// Record `text` as the latest editor text of `uri` and start its
    /// analysis at once (no debounce).
    ///
    /// The analysis runs on its own task: a notification handler that awaited
    /// it would hold one of tower-lsp's few handler slots for the whole pass.
    pub(super) async fn record_and_analyze(
        &self,
        uri: Url,
        text: Arc<String>,
        version: Option<i32>,
    ) {
        self.analysis_scheduler.open_document(&uri);
        let recorded = match self
            .record_latest_text(&uri, Arc::clone(&text), version)
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
        self.schedule_transitive_importers(&recorded.identity).await;

        // Bump the generation so any in-flight debounced analysis for this URI
        // becomes stale and refuses to overwrite fresh results.
        let generation = self.bump_generation(&uri);

        let worker = self.analysis_worker();
        tokio::spawn(async move {
            analyze_store_publish(&worker, uri, text, recorded.revision, generation).await;
        });
    }

    /// Spawn a debounced analysis task for a `did_change` event.
    ///
    /// Waits [`DEBOUNCE_DELAY_MS`] and bails out if a newer change has
    /// arrived (generation mismatch). Any `JoinError` from the blocking
    /// analysis task is logged to the client rather than silently swallowed.
    pub(super) fn spawn_debounced_analysis(
        &self,
        uri: Url,
        text: Arc<String>,
        root_revision: DocumentRevision,
        generation: u64,
    ) {
        let worker = self.analysis_worker();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DEBOUNCE_DELAY_MS)).await;

            // Check if a newer change has superseded this one.
            if !worker.generations.is_current(&uri, generation) {
                return;
            }

            analyze_store_publish(&worker, uri, text, root_revision, generation).await;
        });
    }

    /// Start and return a new generation for `uri`, cancelling older work.
    pub(super) fn bump_generation(&self, uri: &Url) -> u64 {
        let generation = self.generations.bump(uri);
        self.analysis_scheduler.cancel_document(uri);
        generation
    }
}
