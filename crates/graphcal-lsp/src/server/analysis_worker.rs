//! One debounced analysis pass: run, store, and publish its results.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tower_lsp::Client;
use tower_lsp::lsp_types::{MessageType, Url};

use crate::analysis::AnalysisResult;
use crate::analysis_generations::AnalysisGenerations;
use crate::analysis_pipeline::{
    AnalysisDemand, AnalysisRun, EvaluationDemand, OpenBuffer, ViewModelDemand,
    run_analysis_with_cancellation,
};
use crate::client_capabilities::ClientFeatureSupport;
use crate::file_identity::document_identity;
use crate::workspace_revision::{
    AnalysisInputSnapshot, CurrentRevisions, DependencyGraph, DocumentIdentity, DocumentRevision,
};
use graphcal_compiler::cancellation::Cancelled;

use super::client_shape::diagnostics_for_client;
use super::custom_protocol::{ViewModelDidChange, ViewModelDidChangeParams};
use super::documents::{
    OpenDocumentSnapshot, StoredDocument, merged_diagnostics_for, open_revisions,
};
use super::revisions::{RevisionObservers, prune_filesystem_revisions};
use super::scheduler::{AnalysisScheduler, acquire_analysis_permit};

/// Wall-clock cap on a single `run_analysis` pass. When exceeded, the LSP
/// requests cooperative cancellation and leaves prior cached analysis in
/// place so queries still answer from the last good state.
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
enum AnalysisCompletion {
    Done,
    Retry(OpenDocumentSnapshot),
}

/// The Backend state one analysis worker reads and publishes into.
///
/// Every field is a shared handle, so a debounced task owns a cheap clone
/// while the Backend keeps serving requests.
#[derive(Clone)]
pub(super) struct AnalysisWorker {
    pub(super) client: Client,
    pub(super) documents: Arc<RwLock<HashMap<Url, StoredDocument>>>,
    pub(super) generations: Arc<AnalysisGenerations>,
    pub(super) latest_text: Arc<RwLock<HashMap<Url, OpenDocumentSnapshot>>>,
    pub(super) filesystem_revisions: Arc<RwLock<HashMap<DocumentIdentity, DocumentRevision>>>,
    pub(super) dependency_graph: Arc<RwLock<DependencyGraph>>,
    pub(super) revision_observers: Arc<RevisionObservers>,
    pub(super) client_features: ClientFeatureSupport,
    pub(super) plugin_host: Arc<graphcal_plugin_host::PluginHost>,
    pub(super) scheduler: Arc<AnalysisScheduler>,
}

/// Run analysis until it either stores a revision-coherent result or reaches
/// a terminal cancellation/error. A dependency edit racing the worker returns
/// the current root snapshot for another bounded pass.
pub(super) async fn analyze_store_publish(
    worker: &AnalysisWorker,
    uri: Url,
    mut text: Arc<String>,
    mut root_revision: DocumentRevision,
    mut generation: u64,
) {
    loop {
        match analyze_store_publish_once(worker, uri.clone(), text, root_revision, generation).await
        {
            AnalysisCompletion::Done => {
                prune_filesystem_revisions(
                    &worker.filesystem_revisions,
                    &worker.dependency_graph,
                    &worker.revision_observers,
                )
                .await;
                worker.generations.settle(&uri, generation);
                return;
            }
            AnalysisCompletion::Retry(snapshot) => {
                text = snapshot.text;
                root_revision = snapshot.revision;
                // The pass ends here when its document closed meanwhile.
                let Some(next) = worker.generations.advance(&uri) else {
                    return;
                };
                generation = next;
                worker.scheduler.cancel_document(&uri);
            }
        }
    }
}

/// Run one blocking, timeout-guarded analysis pass and store/publish it only
/// when every consumed open-document revision is still current.
#[expect(
    clippy::too_many_lines,
    reason = "one call site per trigger; the pass is one gated sequence"
)]
async fn analyze_store_publish_once(
    worker: &AnalysisWorker,
    uri: Url,
    text: Arc<String>,
    root_revision: DocumentRevision,
    generation: u64,
) -> AnalysisCompletion {
    let AnalysisWorker {
        client,
        documents,
        generations,
        latest_text,
        filesystem_revisions,
        dependency_graph,
        revision_observers,
        client_features,
        plugin_host,
        scheduler,
    } = worker;
    let client_features = *client_features;
    let registration = scheduler.register(&uri, generation);
    let cancellation = registration.source.token();
    if !generations.is_current(&uri, generation) {
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
    if cancellation.is_cancelled() || !generations.is_current(&uri, generation) {
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
    if cancellation.is_cancelled() || !generations.is_current(&uri, generation) {
        scheduler.finish(&uri, generation);
        return AnalysisCompletion::Done;
    }

    // Snapshot every *other* open document's latest text: the analysis
    // overlays them onto the filesystem so imports of open-but-dirty files
    // see the editor state, not stale disk content. The analyzed document
    // itself uses `text` (this analysis pass's snapshot).
    let snapshots = latest_text.read().await;
    let disk_revisions = filesystem_revisions.read().await;
    // Registered under the read lock and held until this pass has checked
    // its observed revisions again (it ends with the function).
    let _observation = revision_observers.observe(&disk_revisions);
    let root_identity = snapshots.get(&uri).map_or_else(
        || document_identity(&uri),
        |snapshot| snapshot.identity.clone(),
    );
    let observed_revisions =
        CurrentRevisions::new(&disk_revisions, open_revisions(&snapshots)).to_map();
    let input_snapshot =
        AnalysisInputSnapshot::new(root_identity, root_revision, observed_revisions);
    let open_buffers: Vec<OpenBuffer> = snapshots
        .iter()
        .filter(|(open_uri, _)| **open_uri != uri)
        .filter_map(|(open_uri, snapshot)| {
            OpenBuffer::new(open_uri.clone(), Arc::clone(&snapshot.text))
        })
        .collect();
    drop(snapshots);
    drop(disk_revisions);
    if cancellation.is_cancelled() || !generations.is_current(&uri, generation) {
        scheduler.finish(&uri, generation);
        return AnalysisCompletion::Done;
    }

    let uri_for_analysis = uri.clone();
    let view = if client_features.view_model_changes.is_supported() {
        ViewModelDemand::Project
    } else {
        ViewModelDemand::Skip
    };
    let source = registration.source;
    let worker_plugin_host = Arc::clone(plugin_host);
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
            AnalysisDemand {
                view,
                evaluation: EvaluationDemand::Evaluate,
            },
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
            let cleanup_scheduler = Arc::clone(scheduler);
            let cleanup_uri = uri.clone();
            tokio::spawn(async move {
                let _ = task.await;
                cleanup_scheduler.finish(&cleanup_uri, generation);
            });
            if generations.is_current(&uri, generation) {
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
    let current_revisions = CurrentRevisions::new(&filesystem_guard, open_revisions(&latest_guard));
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
    if !generations.is_current(&uri, generation) {
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
    if client_features.view_model_changes.is_supported() {
        // Announce on a separate task: the client socket holds one message,
        // and a client that reads it only after this handler returns would
        // otherwise deadlock with an analysis waiting to send a second one.
        let announce = client.clone();
        let announced = uri.clone();
        tokio::spawn(async move {
            announce
                .send_notification::<ViewModelDidChange>(ViewModelDidChangeParams {
                    uri: announced,
                })
                .await;
        });
    }
    AnalysisCompletion::Done
}

/// Store a fresh analysis for `uri`, keeping the previous symbol-dependent
/// state when the new result is a parse-failure fallback.
///
/// Mid-edit buffers fail to parse more often than not (the parser has no
/// error recovery), so replacing the cached analysis with an empty symbol
/// table would make completion and signature help go dark ~300 ms after
/// every keystroke (#834). Instead, only the diagnostics are refreshed;
/// coordinate-free features (completion, signature help) keep answering
/// from the last successfully analyzed state. Coordinate features (hover,
/// goto, references, inlay hints) answer only for the text the analysis
/// covers, so they return nothing until the buffer parses again.
pub(super) fn store_analysis(
    docs: &mut HashMap<Url, StoredDocument>,
    uri: &Url,
    analysis: AnalysisResult,
) {
    if !analysis.buffer_parsed
        && let Some(prev) = docs.get_mut(uri)
    {
        prev.diagnostics = analysis.diagnostics;
        return;
    }
    docs.insert(uri.clone(), StoredDocument::new(analysis));
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
