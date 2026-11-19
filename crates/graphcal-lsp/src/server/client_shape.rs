//! Adapters from analysis results to the shapes the client negotiated.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeActionOrCommand, CodeActionResponse, Diagnostic, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, Url, WorkspaceEdit,
};

use crate::client_capabilities::{ClientFeatureSupport, WorkspaceEditShape};
use crate::project_symbols::ProjectSymbolIndex;

use super::documents::OpenDocumentSnapshot;

/// An explicit rename refusal as the descriptive JSON-RPC error clients show
/// to the user; a `null` answer would read as "nothing to rename here".
pub(super) fn rename_refusal_error(
    refusal: &crate::rename::RenameRefusal,
) -> tower_lsp::jsonrpc::Error {
    tower_lsp::jsonrpc::Error::invalid_params(refusal.to_string())
}

pub(super) fn workspace_edit_matches_open_snapshots(
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

pub(super) fn workspace_edit_for_client(
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

pub(super) fn code_actions_for_client(
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

pub(super) fn diagnostics_for_client(
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
