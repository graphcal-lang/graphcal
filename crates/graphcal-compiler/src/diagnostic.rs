//! Source-located diagnostics as plain core data.
//!
//! A [`Diagnostic`] pairs a typed, phase-specific payload (`kind`) with the
//! [`SourceId`] and primary [`Span`] it points at. It deliberately holds no
//! file name, source text, or rendering-library types: the shell attaches the
//! source through a [`SourceRegistry`](crate::source_registry::SourceRegistry)
//! and renders it with
//! [`RenderableDiagnostic`](crate::diagnostic_render::RenderableDiagnostic).
//!
//! Each phase error enum implements [`DiagnosticKind`] to describe its stable
//! code, message, help, and labels. The description is the only place a typed
//! payload becomes text, and it happens at the rendering boundary.

use std::fmt;

use crate::source_id::SourceId;
use crate::syntax::span::Span;

/// A typed diagnostic located in one registered source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic<K> {
    /// Source text the spans index into.
    pub src: SourceId,
    /// Span the diagnostic is primarily about; carries
    /// [`DiagnosticKind::primary_label`].
    pub primary: Span,
    /// Phase-specific typed payload.
    pub kind: K,
}

impl<K> Diagnostic<K> {
    /// Locate `kind` at `primary` in `src`.
    pub const fn new(src: SourceId, primary: Span, kind: K) -> Self {
        Self { src, primary, kind }
    }

    /// Re-type the payload, keeping the location.
    pub fn map_kind<L>(self, map: impl FnOnce(K) -> L) -> Diagnostic<L> {
        Diagnostic {
            src: self.src,
            primary: self.primary,
            kind: map(self.kind),
        }
    }
}

/// An additional labelled span in the same source as the primary span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecondaryLabel {
    pub span: Span,
    pub text: String,
}

/// Rendering description of a diagnostic payload.
///
/// `Display` is the one-line message. The remaining methods describe the
/// stable diagnostic code and the optional help and label texts.
pub trait DiagnosticKind: fmt::Display {
    /// Stable diagnostic code, such as `graphcal::P001`. Codes never change
    /// once published.
    fn code(&self) -> &'static str;

    /// Text attached to the primary span, if any.
    fn primary_label(&self) -> Option<String>;

    /// Follow-up advice shown after the message.
    fn help(&self) -> Option<String> {
        None
    }

    /// Further labelled spans, in the same source as the primary span.
    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        Vec::new()
    }
}
