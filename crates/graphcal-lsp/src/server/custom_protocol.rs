//! Graphcal-specific LSP requests and notifications.

use tower_lsp::lsp_types::Url;

/// Test-only request whose feature work blocks until the test releases it,
/// standing in for a slow handler such as `references` on a huge project.
#[cfg(test)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only item shared with protocol_tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) enum BlockingFeatureRequest {}

#[cfg(test)]
impl BlockingFeatureRequest {
    pub(crate) const METHOD: &'static str = "graphcal/test/blockingFeature";
}

/// Observation and release points of [`BlockingFeatureRequest`].
#[cfg(test)]
#[derive(Debug, Default)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only item shared with protocol_tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) struct BlockingFeatureProbe {
    pub(crate) started: tokio::sync::Notify,
    pub(crate) released: std::sync::atomic::AtomicBool,
    pub(crate) finished: tokio::sync::Notify,
}

/// `graphcal/viewModel`: a view model of a document, as of its latest
/// analysis: the document's own, or (with `target`) that of one include
/// instance its evaluation materializes. `null` until the document has been
/// analyzed, when its project did not check, when the evaluation has no such
/// instance, or when the client did not declare
/// `experimental.graphcal.viewModelChanges` (analysis then skips the view).
pub(super) enum ViewModelRequest {}

impl ViewModelRequest {
    pub(super) const METHOD: &'static str = "graphcal/viewModel";
}

/// Parameters of [`ViewModelRequest`].
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ViewModelParams {
    pub(super) text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
    /// What to project of the document: `{"kind": "module"}` (the default)
    /// or `{"kind": "instance", "instance": "<id>"}`.
    #[serde(default = "module_target")]
    pub(super) target: graphcal_project::view_model::ViewTarget,
}

const fn module_target() -> graphcal_project::view_model::ViewTarget {
    graphcal_project::view_model::ViewTarget::Module
}

/// `graphcal/viewModelDidChange`: a document's analysis completed, so a
/// view showing it should request the model again.
pub(super) enum ViewModelDidChange {}

impl tower_lsp::lsp_types::notification::Notification for ViewModelDidChange {
    const METHOD: &'static str = "graphcal/viewModelDidChange";
    type Params = ViewModelDidChangeParams;
}

/// Parameters of [`ViewModelDidChange`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct ViewModelDidChangeParams {
    pub(super) uri: Url,
}
