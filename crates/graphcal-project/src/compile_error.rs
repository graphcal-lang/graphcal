//! Top-level project compilation error.

use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::diagnostic_render::RenderableDiagnostic;

use crate::binding_error::BindingError;
use crate::load_error::LoadError;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::parser::{ParseError, ParseErrorKind};

/// Top-level compile error, composed of the failures of each project phase:
/// loading, parsing, semantic evaluation, and external parameter binding.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    /// A source file failed to parse; rendered against the text it indexes.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(RenderableDiagnostic<ParseErrorKind>),

    /// The project's files or manifests could not be read or resolved.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Load(#[from] LoadError),

    /// An external parameter binding names no bindable entry parameter, or a
    /// required parameter is left unbound.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Binding(#[from] BindingError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(#[from] graphcal_compiler::graphcal_error::GraphcalError),

    /// A value supplied through an external binding format failed semantic
    /// validation. The boundary source and parameter span deliberately replace
    /// synthetic expression offsets in the underlying error.
    #[error("invalid binding for `{name}`: {reason}")]
    #[diagnostic(code(graphcal::O004))]
    ExternalBinding {
        /// Entry-DAG parameter receiving the value.
        name: DeclName,
        /// Original compiler/evaluator error rendered at the boundary.
        reason: String,
        /// External parameter source, such as inline JSON, stdin, or a file.
        #[source_code]
        src: NamedSource<Arc<String>>,
        /// Span identifying the parameter in the external document.
        #[label("value for parameter `{name}`")]
        span: SourceSpan,
    },
}

/// A cancellable project operation that fails with a [`CompileError`]
/// reports it as [`Outcome::Failed`].
impl From<CompileError> for Outcome<CompileError> {
    fn from(error: CompileError) -> Self {
        Self::Failed(error)
    }
}

impl CompileError {
    /// Attach the named source a parse error was produced from.
    #[must_use]
    pub fn parse(error: ParseError, source: NamedSource<Arc<String>>) -> Self {
        Self::Parse(RenderableDiagnostic::in_source(
            error.kind, error.span, source,
        ))
    }

    /// Return the `NamedSource` embedded in this error, if any.
    ///
    /// Forwards to the parse diagnostic's attached source or
    /// [`GraphcalError::named_source`](graphcal_compiler::graphcal_error::GraphcalError::named_source).
    /// When present, the returned
    /// `NamedSource` pairs the file's name with the exact source text whose
    /// byte offsets the error's labels index into — so diagnostic emitters
    /// can build a line index over the right text without having to look it
    /// up by name.
    ///
    /// Parse, semantic, and external-binding diagnostics always carry a
    /// source; loader and binding failures that precede any source (e.g.
    /// `FileNotFound`, `CircularImport`) do not.
    #[must_use]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        match self {
            Self::Parse(e) => Some(e.named_source()),
            Self::Load(e) => e.named_source(),
            Self::Binding(e) => e.named_source(),
            Self::Eval(e) => Some(e.named_source()),
            Self::ExternalBinding { src, .. } => Some(src),
        }
    }
}
