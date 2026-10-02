//! Evaluation diagnostics: failed and unavailable values.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code and label
//! are described by its [`DiagnosticKind`] implementation.

use std::sync::Arc;

use thiserror::Error;

use crate::diagnostic::DiagnosticKind;
use crate::node_unavailable::RuntimeUnavailable;

/// Evaluation diagnostics: failed and unavailable values.
#[derive(Debug, Clone, Error)]
pub enum EvaluationError {
    /// Runtime propagation of an unavailable projected value. This is not a
    /// static checking error; evaluator boundaries retain the typed reason
    /// and rename its declarations before reporting it, so its own text
    /// names none.
    #[error("{}", .reason.summary())]
    Unavailable { reason: RuntimeUnavailable },
    /// A failure the evaluator reported at runtime, kept as its typed error.
    #[error("{0}")]
    Runtime(EvaluatorFailure),
}

/// A typed failure the evaluator reported while running a checked program.
///
/// The compiler does not know the evaluator's failure types, so the failure is
/// kept as the error value itself — inspectable by downcasting — and is
/// rendered only where a diagnostic is displayed.
#[derive(Clone)]
pub struct EvaluatorFailure(Arc<dyn std::error::Error + Send + Sync>);

impl EvaluatorFailure {
    /// Keep `failure`, an evaluator's typed error.
    #[must_use]
    pub fn new(failure: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Arc::new(failure))
    }

    /// The failure as `T`, when it is one.
    #[must_use]
    pub fn downcast_ref<T: std::error::Error + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl std::fmt::Display for EvaluatorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Debug for EvaluatorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl DiagnosticKind for EvaluationError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "graphcal::E050",
            Self::Runtime(_) => "graphcal::E001",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::Unavailable { .. } => Some("value unavailable here".to_owned()),
            Self::Runtime(_) => Some("error here".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EvaluationError, EvaluatorFailure};
    use crate::diagnostic::DiagnosticKind;

    #[derive(Debug, thiserror::Error)]
    #[error("sentinel failure")]
    struct Sentinel;

    #[test]
    fn runtime_failures_keep_their_type_and_render_as_e001() {
        let failure = EvaluatorFailure::new(Sentinel);
        assert!(failure.downcast_ref::<Sentinel>().is_some());
        assert_eq!(format!("{failure:?}"), "Sentinel");
        let error = EvaluationError::Runtime(failure);
        assert_eq!(error.to_string(), "sentinel failure");
        assert_eq!(error.code(), "graphcal::E001");
        assert_eq!(error.primary_label().as_deref(), Some("error here"));
    }

    #[test]
    fn unavailable_values_render_no_runtime_identity() {
        use crate::dag_id::DagId;
        use crate::node_unavailable::NodeUnavailable;
        use crate::resolved_name::ResolvedDeclName;
        use crate::syntax::decl_name::DeclName;
        use crate::syntax::non_empty::NonEmpty;

        let identity = ResolvedDeclName::for_test(
            DagId::root_in_package("test", "lib"),
            DeclName::expect_valid("hidden"),
        );
        let render = |reason| EvaluationError::Unavailable { reason }.to_string();
        assert_eq!(
            render(NodeUnavailable::DependencyFailed {
                failed_deps: NonEmpty::singleton(identity.clone()),
            }),
            "dependency failed"
        );
        assert_eq!(
            render(NodeUnavailable::Todo {
                declaration: identity.clone(),
            }),
            "TODO — formula unfinished"
        );
        assert_eq!(
            render(NodeUnavailable::Blocked {
                unfinished: NonEmpty::singleton(identity.clone()),
                failed_deps: vec![identity.clone()],
            }),
            "BLOCKED — unfinished dependencies; dependency failed"
        );
        assert_eq!(
            render(NodeUnavailable::Blocked {
                unfinished: NonEmpty::singleton(identity),
                failed_deps: Vec::new(),
            }),
            "BLOCKED — unfinished dependencies"
        );
        assert_eq!(
            render(NodeUnavailable::EvalFailed {
                message: "division by zero".to_owned(),
            }),
            "division by zero"
        );
    }
}
