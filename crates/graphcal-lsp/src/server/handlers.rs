//! `LanguageServer` protocol handlers and custom request handlers.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tower_lsp::LanguageServer;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionParams, CodeActionProviderCapability,
    CodeActionResponse, CompletionOptions, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams, DocumentLink,
    DocumentLinkOptions, DocumentLinkParams, DocumentSymbolParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, InlayHint, InlayHintParams, Location,
    MessageType, OneOf, PositionEncodingKind, PrepareRenameResponse, ReferenceParams,
    RenameOptions, RenameParams, SaveOptions, ServerCapabilities, SignatureHelp,
    SignatureHelpOptions, SignatureHelpParams, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions, WorkspaceEdit,
};

use crate::analysis_pipeline::OpenBuffer;
use crate::client_capabilities::{ClientFeatureSupport, DocumentSymbolShape};
use crate::convert::position_to_byte_offset;
use crate::file_identity::document_identity;
use crate::formatting_scheduler::FormattingTaskError;
use crate::rename_check::RenameCheck;
#[cfg(test)]
use crate::workspace_revision::DocumentIdentity;

use super::backend::Backend;
use super::client_shape::{
    code_actions_for_client, diagnostics_for_client, rename_refusal_error,
    workspace_edit_for_client, workspace_edit_matches_open_snapshots,
};
use super::custom_protocol::ViewModelParams;
use super::documents::merged_diagnostics_for;
use super::revisions::prune_filesystem_revisions;

/// How long an inlay-hint request waits for the analysis of the latest text
/// (debounce included) before answering from whatever is current.
const CURRENT_ANALYSIS_WAIT: Duration = Duration::from_secs(2);

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
        if !Self::opens_as_graphcal(&params.text_document.uri, &params.text_document.language_id) {
            return;
        }
        self.record_and_analyze(
            params.text_document.uri,
            Arc::new(params.text_document.text),
            Some(params.text_document.version),
        )
        .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let uri = params.text_document.uri;
        if !self.is_open_graphcal_document(&uri).await {
            return;
        }
        let text = Arc::new(change.text);

        self.analysis_scheduler.open_document(&uri);
        // Record the new text synchronously: completion/signature-help fire
        // on trigger characters milliseconds after this notification, long
        // before the debounced analysis lands.
        let recorded = match self
            .record_latest_text(&uri, Arc::clone(&text), Some(params.text_document.version))
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
        let generation = self.bump_generation(&uri);
        self.spawn_debounced_analysis(uri, text, recorded.revision, generation);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if !self
            .is_open_graphcal_document(&params.text_document.uri)
            .await
        {
            return;
        }
        if let Some(text) = params.text {
            self.record_and_analyze(params.text_document.uri, Arc::new(text), None)
                .await;
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // Record every revision before scheduling anything: a burst (such as
        // a `git checkout`) then schedules each affected root once, with one
        // inlay-hint refresh, instead of once per event.
        let mut seen = HashSet::new();
        let mut affected = HashSet::new();
        for change in params.changes {
            if seen.insert(change.uri.clone()) {
                affected.extend(self.record_filesystem_change(&change.uri).await);
            }
        }
        self.schedule_open_roots(&affected).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let identity = self.latest_text.read().await.get(&uri).map_or_else(
            || document_identity(&uri),
            |snapshot| snapshot.identity.clone(),
        );
        // Invalidate and cancel queued/running work before clearing state.
        // Generations are process-wide, so forgetting the document cannot
        // let a reopened URI reuse a generation an old worker still holds;
        // nothing waits for a pass of a closed document either.
        self.generations.forget(&uri);
        self.analysis_scheduler.cancel_document(&uri);
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
                .map_or_default(|prev| prev.diagnostics.keys().cloned().collect());
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
        prune_filesystem_revisions(
            &self.filesystem_revisions,
            &self.dependency_graph,
            &self.revision_observers,
        )
        .await;
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
        self.with_current_analysis(&uri.clone(), move |current| {
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
        self.with_current_analysis(&uri.clone(), move |current| {
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
        self.with_current_analysis(&uri.clone(), move |current| {
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
        self.with_current_analysis(&uri.clone(), move |current| {
            let analysis = current.get();
            let offset = position_to_byte_offset(&analysis.source, position);
            crate::references::references(analysis, &uri, offset, include_declaration)
        })
        .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        // A client without inlay-hint refresh (Helix) keeps an empty answer
        // until its view changes, so wait for the pass of the latest text.
        self.generations
            .wait_for_current(&uri, CURRENT_ANALYSIS_WAIT)
            .await;
        self.with_current_analysis(&uri.clone(), move |current| {
            crate::inlay_hints::inlay_hints(current.get(), range)
        })
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        // Signature help parses the atomically captured current text while
        // reading coordinate-free signatures from the cached analysis.
        self.with_semantic_analysis(&uri.clone(), move |semantic, current_source| {
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
        self.with_semantic_analysis(&uri.clone(), move |semantic, current_source| {
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
        let plugin_host = Arc::clone(&self.plugin_host);
        let outcome = self
            .with_current_analysis_cancellable(&uri.clone(), move |current, cancellation| {
                let analysis = current.get();
                let current_text = current_text.as_ref()?;
                if current_text.as_str() != analysis.source.as_str() {
                    return None;
                }
                let offset = position_to_byte_offset(current_text, position);
                let open_buffers = snapshots
                    .iter()
                    .filter(|(open_uri, _)| **open_uri != uri)
                    .filter_map(|(open_uri, snapshot)| {
                        OpenBuffer::new(open_uri.clone(), Arc::clone(&snapshot.text))
                    })
                    .collect();
                let check = RenameCheck::new(open_buffers, &plugin_host, cancellation);
                let result = crate::rename::rename(analysis, &uri, offset, &new_name, &check);
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
            Some(Err(refusal)) => Err(rename_refusal_error(&refusal)),
        }
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let current_text = self.current_text(&uri).await;
        let outcome = self
            .with_current_analysis(&uri.clone(), move |current| {
                let analysis = current.get();
                let current_text = current_text.as_ref()?;
                if current_text.as_str() != analysis.source.as_str() {
                    return None;
                }
                let offset = position_to_byte_offset(current_text, position);
                Some(crate::rename::prepare_rename(analysis, &uri, offset))
            })
            .await?;
        outcome.map_or(Ok(None), |response| {
            response.map_err(|refusal| rename_refusal_error(&refusal))
        })
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        self.with_current_analysis(&uri.clone(), move |current| {
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

#[cfg(test)]
impl Backend {
    /// The change generation of `uri`; it grows once per scheduled analysis.
    pub(crate) fn generation_for_test(&self, uri: &Url) -> u64 {
        self.generations.current(uri)
    }

    /// Wait until the pass of the current generation of `uri` finished.
    pub(crate) async fn wait_until_settled_for_test(&self, uri: &Url) {
        assert!(
            self.generations
                .wait_for_current(uri, Duration::from_secs(30))
                .await,
            "the analysis of {uri} did not settle"
        );
    }

    /// Identities that still have a recorded disk revision.
    pub(crate) async fn disk_revision_identities_for_test(&self) -> HashSet<DocumentIdentity> {
        self.filesystem_revisions
            .read()
            .await
            .keys()
            .cloned()
            .collect()
    }

    /// How many entries the per-document maps hold.
    pub(crate) async fn tracked_documents_for_test(&self) -> [usize; 3] {
        [
            self.documents.read().await.len(),
            self.latest_text.read().await.len(),
            self.generations.tracked(),
        ]
    }

    pub(super) async fn blocking_feature(
        &self,
        params: tower_lsp::lsp_types::TextDocumentIdentifier,
    ) -> Result<Option<()>> {
        /// Bound the block so a regression fails the test instead of hanging.
        const MAX_BLOCK: Duration = Duration::from_secs(60);
        let probe = Arc::clone(&self.blocking_feature_probe);
        self.with_current_analysis(&params.uri, move |_| {
            probe.started.notify_one();
            let started = std::time::Instant::now();
            while !probe.released.load(std::sync::atomic::Ordering::SeqCst)
                && started.elapsed() < MAX_BLOCK
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            probe.finished.notify_one();
            Some(())
        })
        .await
    }
}

impl Backend {
    /// Serve [`ViewModelRequest`] from the cached analysis.
    pub(super) async fn view_model(
        &self,
        params: ViewModelParams,
    ) -> Result<Option<graphcal_project::view_model::ViewModel>> {
        self.with_current_analysis(&params.text_document.uri.clone(), move |analysis| {
            analysis
                .get()
                .view
                .as_ref()
                .and_then(|views| views.get(&params.target))
                .cloned()
        })
        .await
    }
}
