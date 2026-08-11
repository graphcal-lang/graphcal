//! Typed source anchors for compiler diagnostics.

use crate::syntax::span::Span;

/// Where a diagnostic should point when rendered against its associated source.
///
/// Keeping source absence explicit prevents an internal or synthetic failure
/// from fabricating a zero-length caret at byte zero of an unrelated user file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticAnchor {
    /// A precise source range.
    Source(Span),
    /// The failure concerns the source as a whole, with no precise range.
    WholeFile,
    /// The failure belongs to a synthetic built-in with no source location.
    Builtin,
}

impl DiagnosticAnchor {
    /// Resolve this semantic anchor to the optional span expected by `miette`.
    #[must_use]
    pub const fn resolve(self, source_len: usize) -> Option<Span> {
        match self {
            Self::Source(span) => Some(span),
            Self::WholeFile => Some(Span::new(0, source_len)),
            Self::Builtin => None,
        }
    }
}

impl From<Span> for DiagnosticAnchor {
    fn from(span: Span) -> Self {
        Self::Source(span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_distinguish_source_whole_file_and_builtin_locations() {
        let source = Span::new(4, 2);
        assert_eq!(DiagnosticAnchor::Source(source).resolve(10), Some(source));
        assert_eq!(
            DiagnosticAnchor::WholeFile.resolve(10),
            Some(Span::new(0, 10))
        );
        assert_eq!(DiagnosticAnchor::Builtin.resolve(10), None);
    }
}
