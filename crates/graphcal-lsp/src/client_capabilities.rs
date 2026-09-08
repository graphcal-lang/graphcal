//! Typed interpretation of optional LSP client capabilities.
//!
//! Protocol payloads are reduced to the response shapes the server needs. The
//! rest of the LSP stays independent of the nested wire-format structures.

use tower_lsp::lsp_types::{ClientCapabilities, MarkupKind};

/// Shape used for `textDocument/documentSymbol` responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentSymbolShape {
    Flat,
    Hierarchical,
}

/// Markup format used for hover responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverFormat {
    PlainText,
    Markdown,
}

/// Workspace edit representation supported by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEditShape {
    /// The legacy `changes` map. It cannot carry document versions.
    Changes,
    /// Versioned `documentChanges` entries.
    DocumentChanges,
}

/// Support state for an optional protocol feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalFeatureSupport {
    Unsupported,
    Supported,
}

impl OptionalFeatureSupport {
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

impl From<bool> for OptionalFeatureSupport {
    fn from(supported: bool) -> Self {
        if supported {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }
}

/// Optional client features that affect Graphcal response shapes or callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientFeatureSupport {
    pub document_symbol_shape: DocumentSymbolShape,
    pub hover_format: HoverFormat,
    pub workspace_edit_shape: WorkspaceEditShape,
    pub code_action_literals: OptionalFeatureSupport,
    pub code_action_is_preferred: OptionalFeatureSupport,
    pub diagnostic_related_information: OptionalFeatureSupport,
    pub diagnostic_data: OptionalFeatureSupport,
    pub inlay_hint_refresh: OptionalFeatureSupport,
    pub watched_files_dynamic_registration: OptionalFeatureSupport,
}

impl Default for ClientFeatureSupport {
    fn default() -> Self {
        Self {
            document_symbol_shape: DocumentSymbolShape::Flat,
            hover_format: HoverFormat::PlainText,
            workspace_edit_shape: WorkspaceEditShape::Changes,
            code_action_literals: OptionalFeatureSupport::Unsupported,
            code_action_is_preferred: OptionalFeatureSupport::Unsupported,
            diagnostic_related_information: OptionalFeatureSupport::Unsupported,
            diagnostic_data: OptionalFeatureSupport::Unsupported,
            inlay_hint_refresh: OptionalFeatureSupport::Unsupported,
            watched_files_dynamic_registration: OptionalFeatureSupport::Unsupported,
        }
    }
}

impl ClientFeatureSupport {
    /// Interpret the capabilities that affect currently implemented features.
    pub fn from_client(capabilities: &ClientCapabilities) -> Self {
        let text_document = capabilities.text_document.as_ref();
        let workspace = capabilities.workspace.as_ref();
        let document_symbol_shape = text_document
            .and_then(|caps| caps.document_symbol.as_ref())
            .and_then(|caps| caps.hierarchical_document_symbol_support)
            .filter(|supported| *supported)
            .map_or(DocumentSymbolShape::Flat, |_| {
                DocumentSymbolShape::Hierarchical
            });
        let hover_format = text_document
            .and_then(|caps| caps.hover.as_ref())
            .and_then(|caps| caps.content_format.as_ref())
            .and_then(|formats| {
                formats.iter().find_map(|format| {
                    if *format == MarkupKind::Markdown {
                        Some(HoverFormat::Markdown)
                    } else if *format == MarkupKind::PlainText {
                        Some(HoverFormat::PlainText)
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(HoverFormat::PlainText);
        let code_action = text_document.and_then(|caps| caps.code_action.as_ref());
        let diagnostics = text_document.and_then(|caps| caps.publish_diagnostics.as_ref());

        Self {
            document_symbol_shape,
            hover_format,
            workspace_edit_shape: workspace
                .and_then(|caps| caps.workspace_edit.as_ref())
                .and_then(|caps| caps.document_changes)
                .filter(|supported| *supported)
                .map_or(WorkspaceEditShape::Changes, |_| {
                    WorkspaceEditShape::DocumentChanges
                }),
            code_action_literals: code_action
                .and_then(|caps| caps.code_action_literal_support.as_ref())
                .is_some()
                .into(),
            code_action_is_preferred: code_action
                .and_then(|caps| caps.is_preferred_support)
                .unwrap_or(false)
                .into(),
            diagnostic_related_information: diagnostics
                .and_then(|caps| caps.related_information)
                .unwrap_or(false)
                .into(),
            diagnostic_data: diagnostics
                .and_then(|caps| caps.data_support)
                .unwrap_or(false)
                .into(),
            inlay_hint_refresh: workspace
                .and_then(|caps| caps.inlay_hint.as_ref())
                .and_then(|caps| caps.refresh_support)
                .unwrap_or(false)
                .into(),
            watched_files_dynamic_registration: workspace
                .and_then(|caps| caps.did_change_watched_files.as_ref())
                .and_then(|caps| caps.dynamic_registration)
                .unwrap_or(false)
                .into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{
        CodeActionClientCapabilities, CodeActionKindLiteralSupport, CodeActionLiteralSupport,
        DidChangeWatchedFilesClientCapabilities, DocumentSymbolClientCapabilities,
        HoverClientCapabilities, InlayHintWorkspaceClientCapabilities,
        PublishDiagnosticsClientCapabilities, TextDocumentClientCapabilities,
        WorkspaceClientCapabilities, WorkspaceEditClientCapabilities,
    };

    use super::*;

    #[test]
    fn absent_optional_capabilities_use_legacy_safe_shapes() {
        assert_eq!(
            ClientFeatureSupport::from_client(&ClientCapabilities::default()),
            ClientFeatureSupport::default()
        );
    }

    #[test]
    fn supported_optional_capabilities_are_preserved() {
        let capabilities = ClientCapabilities {
            workspace: Some(WorkspaceClientCapabilities {
                workspace_edit: Some(WorkspaceEditClientCapabilities {
                    document_changes: Some(true),
                    ..Default::default()
                }),
                inlay_hint: Some(InlayHintWorkspaceClientCapabilities {
                    refresh_support: Some(true),
                }),
                did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: Some(false),
                }),
                ..Default::default()
            }),
            text_document: Some(TextDocumentClientCapabilities {
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                hover: Some(HoverClientCapabilities {
                    content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                    ..Default::default()
                }),
                code_action: Some(CodeActionClientCapabilities {
                    code_action_literal_support: Some(CodeActionLiteralSupport {
                        code_action_kind: CodeActionKindLiteralSupport {
                            value_set: vec!["quickfix".to_string()],
                        },
                    }),
                    is_preferred_support: Some(true),
                    ..Default::default()
                }),
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                    related_information: Some(true),
                    data_support: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            ClientFeatureSupport::from_client(&capabilities),
            ClientFeatureSupport {
                document_symbol_shape: DocumentSymbolShape::Hierarchical,
                hover_format: HoverFormat::Markdown,
                workspace_edit_shape: WorkspaceEditShape::DocumentChanges,
                code_action_literals: OptionalFeatureSupport::Supported,
                code_action_is_preferred: OptionalFeatureSupport::Supported,
                diagnostic_related_information: OptionalFeatureSupport::Supported,
                diagnostic_data: OptionalFeatureSupport::Supported,
                inlay_hint_refresh: OptionalFeatureSupport::Supported,
                watched_files_dynamic_registration: OptionalFeatureSupport::Supported,
            }
        );
    }

    #[test]
    fn hover_honors_client_format_preference_order() {
        let capabilities: ClientCapabilities = serde_json::from_value(serde_json::json!({
            "textDocument": {
                "hover": { "contentFormat": ["plaintext", "markdown"] }
            }
        }))
        .unwrap();

        assert_eq!(
            ClientFeatureSupport::from_client(&capabilities).hover_format,
            HoverFormat::PlainText
        );
    }
}
