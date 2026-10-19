//! Direct source-declaration provenance retained at the HIR boundary.

use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
};
use crate::static_interface::{StaticInputKind, StaticRole};
use crate::syntax::span::Span;

/// One runtime-interface-relevant declaration authored directly in a DAG.
///
/// Include elaboration can merge additional declarations into a [`HirDag`](crate::ir::model::HirDag).
/// This record deliberately excludes those merged declarations, preserving the
/// distinction between an entry DAG's own ports and its internal instances.
/// A node alias a selective include item materializes is authored by the
/// including DAG, so it is a [`Self::Node`] at its include's position.
///
/// Each record carries the canonical identity the resolver declared for it,
/// so the checked interface is attached by identity, not by spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDeclaration {
    Parameter {
        identity: ResolvedDeclName,
        span: Span,
    },
    Node {
        identity: ResolvedDeclName,
        span: Span,
    },
    Index {
        identity: ResolvedIndexName,
        span: Span,
    },
}

impl SourceDeclaration {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Parameter { span, .. } | Self::Node { span, .. } | Self::Index { span, .. } => {
                *span
            }
        }
    }
}

/// Canonical identity of one Static input declaration authored by a template.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StaticPortIdentity {
    Type(ResolvedStructTypeName),
    Dimension(ResolvedDimName),
    Index(ResolvedIndexName),
}

impl StaticPortIdentity {
    #[must_use]
    pub const fn kind(&self) -> StaticInputKind {
        match self {
            Self::Type(_) => StaticInputKind::Type,
            Self::Dimension(_) => StaticInputKind::Dimension,
            Self::Index(_) => StaticInputKind::Index,
        }
    }

    #[must_use]
    pub const fn name(&self) -> &crate::syntax::names::NameAtom {
        match self {
            Self::Type(name) => name.atom(),
            Self::Dimension(name) => name.atom(),
            Self::Index(name) => name.atom(),
        }
    }
}

/// One typed Static input port retained at the HIR boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPort {
    pub identity: StaticPortIdentity,
    pub role: StaticRole,
    pub span: Span,
}
