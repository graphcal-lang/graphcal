//! Browser WebAssembly adapter for Graphcal compilation and evaluation.
//!
//! The compiler and evaluator remain platform-neutral functional cores. This
//! crate validates an in-memory browser project, invokes those cores, and
//! converts their typed results into a JavaScript-facing transport model.
#![allow(
    clippy::disallowed_methods,
    reason = "the browser adapter is an imperative shell where absent transport metadata may intentionally render as empty"
)]

mod bindings;
mod browser_report;
mod diagnostics;
#[cfg(target_arch = "wasm32")]
mod js_request;
mod one_shot;
mod output;
mod package_layout;
mod prepared;
mod project;

pub use bindings::{BindingRequest, MAX_BINDING_EXPR_BYTES};
pub use diagnostics::{
    DiagnosticLabelView, DiagnosticSeverity, DiagnosticView, TextPosition, TextRange,
};
#[cfg(target_arch = "wasm32")]
pub use one_shot::evaluate_project_js;
pub use one_shot::{PlaygroundOutcome, evaluate};
pub use output::{
    AssertionOutcomeView, AssertionView, DeclarationKindView, DeclarationOutcomeView,
    DeclarationView, EvaluationView, FigureView, IndexEntryKeyView, IndexedEntryView,
    NodeUnavailableView, NoticeView, StructFieldView, ValueView,
};
#[cfg(target_arch = "wasm32")]
pub use package_layout::package_layout_js;
pub use package_layout::{PackageLayoutView, package_layout};
pub use prepared::{
    BindingErrorView, ControlView, EvaluateOutcome, EvaluateReportOutcome, ParameterPortView,
    PrepareOutcome, PreparedPlayground, prepare, prepare_bundle, prepare_view,
};
#[cfg(target_arch = "wasm32")]
pub use project::project_limits_js;
pub use project::{
    MAX_PLAYGROUND_CONTENT_BYTES, MAX_PLAYGROUND_FILE_BYTES, MAX_PLAYGROUND_FILES,
    MAX_PLAYGROUND_PATH_BYTES, MAX_PLAYGROUND_PATH_TOTAL_BYTES, PROJECT_LIMITS, PlaygroundFile,
    PlaygroundRequest, ProjectLimitsView, ProjectPathRole, ProjectValidationError,
    RequestErrorKind, RequestErrorView,
};
