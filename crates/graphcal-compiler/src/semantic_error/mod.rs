//! Typed semantic diagnostics, grouped by diagnostic family.
//!
//! Every family is a closed enum of typed payloads whose stable codes share a
//! prefix (`C` for domain constraints, `D` for dimensions, …). A
//! [`SemanticErrorKind`] composes the families; a
//! [`Diagnostic<SemanticErrorKind>`](crate::diagnostic::Diagnostic) locates
//! one by source id and primary span. Rendering happens only at the shell,
//! through the source registry that issued the id.

use thiserror::Error;

use crate::diagnostic::{Diagnostic, DiagnosticKind, SecondaryLabel};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::internal_error::InternalError;
use crate::outcome::Outcome;
use crate::source_id::SourceId;
use crate::syntax::span::Span;

pub mod attribute;
pub mod dimension;
pub mod domain;
pub mod evaluation;
pub mod graph;
pub mod index;
pub mod module;
pub mod name;
pub mod plugin;
pub mod rendered;
pub mod structure;
pub mod visibility;
// MODULES

#[cfg(test)]
mod tests;

/// A semantic diagnostic: a typed family payload located in one source, or
/// a violated compiler invariant.
#[derive(Debug, Clone, Error)]
pub enum SemanticError {
    /// A diagnostic of a typed family, located by source id and span.
    #[error("{}", .0.kind)]
    Located(Diagnostic<SemanticErrorKind>),
    /// An internal invariant violation that should never be reached if earlier
    /// compiler phases (parsing, resolution, `dim_check`) are correct.
    #[error(transparent)]
    Internal(InternalError),
}

/// A cancellable operation that fails with a [`SemanticError`] reports it as
/// [`Outcome::Failed`]; cancellation only ever
/// comes from [`Cancelled`](crate::cancellation::Cancelled).
impl From<SemanticError> for Outcome<SemanticError> {
    fn from(error: SemanticError) -> Self {
        Self::Failed(error)
    }
}

impl SemanticError {
    /// Locate a typed family diagnostic at `primary` in `src`.
    #[must_use]
    pub fn located(src: SourceId, primary: Span, kind: impl Into<SemanticErrorKind>) -> Self {
        Self::Located(Diagnostic::new(src, primary, kind.into()))
    }

    /// Construct an internal diagnostic with an explicit source-anchor policy.
    #[must_use]
    #[cold]
    pub fn internal_error(
        message: impl Into<String>,
        src: SourceId,
        anchor: DiagnosticAnchor,
    ) -> Self {
        Self::Internal(InternalError::new(message, src, anchor))
    }

    /// The source this error's spans index into.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        match self {
            Self::Located(diagnostic) => diagnostic.src,
            Self::Internal(internal) => internal.src(),
        }
    }
}

/// The payload of a semantic diagnostic, by family.
#[derive(Debug, Clone)]
pub enum SemanticErrorKind {
    Domain(domain::DomainError),
    Attribute(attribute::AttributeError),
    Graph(graph::GraphError),
    Struct(structure::StructError),
    Visibility(visibility::VisibilityError),
    Name(name::NameError),
    Index(index::IndexError),
    Plugin(plugin::PluginError),
    Dimension(dimension::DimensionError),
    Module(module::ModuleError),
    Evaluation(evaluation::EvaluationError),
    // KINDS
}

impl SemanticErrorKind {
    /// The family payload, described for rendering.
    fn family(&self) -> &dyn DiagnosticKind {
        match self {
            Self::Domain(kind) => kind,
            Self::Attribute(kind) => kind,
            Self::Graph(kind) => kind,
            Self::Struct(kind) => kind,
            Self::Visibility(kind) => kind,
            Self::Name(kind) => kind,
            Self::Index(kind) => kind,
            Self::Plugin(kind) => kind,
            Self::Dimension(kind) => kind,
            Self::Module(kind) => kind,
            Self::Evaluation(kind) => kind,
            // DELEGATE
        }
    }
}

impl std::fmt::Display for SemanticErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.family().fmt(f)
    }
}

impl DiagnosticKind for SemanticErrorKind {
    fn code(&self) -> &'static str {
        self.family().code()
    }

    fn primary_label(&self) -> Option<String> {
        self.family().primary_label()
    }

    fn help(&self) -> Option<String> {
        self.family().help()
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        self.family().secondary_labels()
    }
}

impl From<domain::DomainError> for SemanticErrorKind {
    fn from(kind: domain::DomainError) -> Self {
        Self::Domain(kind)
    }
}

impl From<attribute::AttributeError> for SemanticErrorKind {
    fn from(kind: attribute::AttributeError) -> Self {
        Self::Attribute(kind)
    }
}

impl From<graph::GraphError> for SemanticErrorKind {
    fn from(kind: graph::GraphError) -> Self {
        Self::Graph(kind)
    }
}

impl From<structure::StructError> for SemanticErrorKind {
    fn from(kind: structure::StructError) -> Self {
        Self::Struct(kind)
    }
}

impl From<visibility::VisibilityError> for SemanticErrorKind {
    fn from(kind: visibility::VisibilityError) -> Self {
        Self::Visibility(kind)
    }
}

impl From<name::NameError> for SemanticErrorKind {
    fn from(kind: name::NameError) -> Self {
        Self::Name(kind)
    }
}

impl From<index::IndexError> for SemanticErrorKind {
    fn from(kind: index::IndexError) -> Self {
        Self::Index(kind)
    }
}

impl From<plugin::PluginError> for SemanticErrorKind {
    fn from(kind: plugin::PluginError) -> Self {
        Self::Plugin(kind)
    }
}

impl From<dimension::DimensionError> for SemanticErrorKind {
    fn from(kind: dimension::DimensionError) -> Self {
        Self::Dimension(kind)
    }
}

impl From<module::ModuleError> for SemanticErrorKind {
    fn from(kind: module::ModuleError) -> Self {
        Self::Module(kind)
    }
}

impl From<evaluation::EvaluationError> for SemanticErrorKind {
    fn from(kind: evaluation::EvaluationError) -> Self {
        Self::Evaluation(kind)
    }
}

// FROM
