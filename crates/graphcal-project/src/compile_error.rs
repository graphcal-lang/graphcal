//! Top-level project compilation error.

use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::diagnostic_render::RenderableDiagnostic;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::parser::{ParseError, ParseErrorKind};

/// Top-level compile error for parsing, semantic evaluation, and external
/// parameter binding.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    /// A source file failed to parse; rendered against the text it indexes.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(RenderableDiagnostic<ParseErrorKind>),

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
    /// Parse and external-binding diagnostics always carry a source;
    /// `GraphcalError` may return `None` for a few variants representing
    /// source-less errors (e.g. `FileNotFound`, `CircularImport`).
    #[must_use]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        match self {
            Self::Parse(e) => Some(e.named_source()),
            Self::Eval(e) => e.named_source(),
            Self::ExternalBinding { src, .. } => Some(src),
        }
    }
}
