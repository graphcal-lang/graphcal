//! Evaluation diagnostics: failed and unavailable values.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code and label
//! are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::DiagnosticKind;
use crate::node_unavailable::NodeUnavailable;

/// Evaluation diagnostics: failed and unavailable values.
#[derive(Debug, Clone, Error)]
pub enum EvaluationError {
    /// Runtime propagation of an unavailable projected value. This is not a
    /// static checking error; evaluator boundaries retain the typed reason.
    #[error("{reason}")]
    Unavailable { reason: NodeUnavailable },
    /// An evaluation failure described only by its message.
    #[error("{message}")]
    Failed { message: String },
}

impl DiagnosticKind for EvaluationError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "graphcal::E050",
            Self::Failed { .. } => "graphcal::E001",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::Unavailable { .. } => Some("value unavailable here".to_owned()),
            Self::Failed { .. } => Some("error here".to_owned()),
        }
    }
}
