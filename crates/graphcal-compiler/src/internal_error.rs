//! Violated compiler invariants, reported as internal errors (`X001`).
//!
//! An internal error is a bug in Graphcal, not a problem with the program
//! being checked. [`InternalError::new`] is its only constructor, so every
//! construction point is one countable call.

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::source_id::SourceId;

/// A violated invariant, located by an explicit anchor policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("internal error: {message}")]
pub struct InternalError {
    message: String,
    src: SourceId,
    anchor: DiagnosticAnchor,
}

impl InternalError {
    /// Stable diagnostic code of every internal error.
    pub const CODE: &'static str = "graphcal::X001";
    /// Follow-up advice shown with every internal error.
    pub const HELP: &'static str = "this is a compiler bug — please report it";
    /// Text attached to the anchor, when it resolves to a span.
    pub const LABEL: &'static str = "unexpected state here";

    /// Record a violated invariant reported at `anchor` in `src`.
    #[must_use]
    #[cold]
    pub fn new(message: impl Into<String>, src: SourceId, anchor: DiagnosticAnchor) -> Self {
        Self {
            message: message.into(),
            src,
            anchor,
        }
    }

    /// What was violated.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The source the anchor resolves against.
    #[must_use]
    pub const fn src(&self) -> SourceId {
        self.src
    }

    /// Where the violation is reported; resolved against the source text
    /// only when rendered.
    #[must_use]
    pub const fn anchor(&self) -> DiagnosticAnchor {
        self.anchor
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source_registry::SourceRegistry;

    #[test]
    fn internal_errors_keep_their_message_source_and_anchor() {
        let src = SourceRegistry::new().register("a.gcl", Arc::new("x".to_string()));
        let error = InternalError::new("broken", src, DiagnosticAnchor::Builtin);
        assert_eq!(error.message(), "broken");
        assert_eq!(error.src(), src);
        assert_eq!(error.anchor(), DiagnosticAnchor::Builtin);
        assert_eq!(error.to_string(), "internal error: broken");
    }
}
