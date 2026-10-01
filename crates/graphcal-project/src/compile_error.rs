//! Top-level project compilation error.

use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::diagnostic_render::RenderableDiagnostic;
use graphcal_compiler::graphcal_error::{GraphcalError, RenderedGraphcalError};
use graphcal_compiler::source_registry::SourceRegistry;

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

    /// A semantic or evaluation diagnostic, rendered against the project
    /// source it points into.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(RenderedGraphcalError),

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

/// A failure inside the project pipeline.
///
/// Semantic errors name their source only by id; they become a renderable
/// [`CompileError`] at the project's public boundary, where the registry that
/// issued the id is at hand ([`Self::render`]).
#[derive(Debug)]
pub(crate) enum PipelineError {
    /// A failure that is already renderable on its own.
    Compile(CompileError),
    /// A semantic or evaluation error about a registered source.
    Semantic(GraphcalError),
}

impl PipelineError {
    /// Render this failure against the registry that issued its source ids.
    #[must_use]
    pub(crate) fn render(self, sources: &SourceRegistry) -> CompileError {
        match self {
            Self::Compile(error) => error,
            Self::Semantic(error) => CompileError::semantic(error, sources),
        }
    }
}

impl From<CompileError> for PipelineError {
    fn from(error: CompileError) -> Self {
        Self::Compile(error)
    }
}

impl From<GraphcalError> for PipelineError {
    fn from(error: GraphcalError) -> Self {
        Self::Semantic(error)
    }
}

impl From<LoadError> for PipelineError {
    fn from(error: LoadError) -> Self {
        Self::Compile(CompileError::Load(error))
    }
}

impl From<BindingError> for PipelineError {
    fn from(error: BindingError) -> Self {
        Self::Compile(CompileError::Binding(error))
    }
}

/// A cancellable pipeline operation that fails with a [`PipelineError`]
/// reports it as [`Outcome::Failed`].
impl From<PipelineError> for Outcome<PipelineError> {
    fn from(error: PipelineError) -> Self {
        Self::Failed(error)
    }
}

impl CompileError {
    /// Render a semantic error against the project sources that issued its
    /// source id.
    #[must_use]
    pub fn semantic(error: GraphcalError, sources: &SourceRegistry) -> Self {
        Self::Eval(RenderedGraphcalError::new(error, sources))
    }

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
    /// [`RenderedGraphcalError::named_source`].
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
