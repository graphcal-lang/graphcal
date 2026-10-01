//! Typed semantic diagnostics, grouped by diagnostic family.
//!
//! Every family is a closed enum of typed payloads whose stable codes share a
//! prefix (`C` for domain constraints, `D` for dimensions, …). A
//! [`SemanticErrorKind`] composes the families; a
//! [`Diagnostic<SemanticErrorKind>`](crate::diagnostic::Diagnostic) locates
//! one by source id and primary span. Rendering happens only at the shell,
//! through the source registry that issued the id.

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};

pub mod attribute;
pub mod domain;
pub mod graph;
pub mod structure;
pub mod visibility;
// MODULES

#[cfg(test)]
pub(crate) mod tests;

/// The payload of a semantic diagnostic, by family.
#[derive(Debug, Clone)]
pub enum SemanticErrorKind {
    Domain(domain::DomainError),
    Attribute(attribute::AttributeError),
    Graph(graph::GraphError),
    Struct(structure::StructError),
    Visibility(visibility::VisibilityError),
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

// FROM
