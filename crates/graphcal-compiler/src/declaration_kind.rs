//! Broad semantic categories of source declarations and the source contexts
//! attributes attach to.
//!
//! These are diagnostic and declaration-policy data shared by declaration
//! collection and [`SemanticError`](crate::semantic_error::SemanticError).

use crate::desugar::desugared_ast::DeclKind;
use crate::syntax::attribute::AttributeName;
use crate::syntax::names::NameAtom;
use crate::syntax::phase::never;

/// Semantic category of a source declaration.
///
/// This broad category is for diagnostics and declaration-level policy. It is
/// distinct from [`DeclCategory`](crate::declaration_category::DeclCategory), whose variants are intentionally limited to
/// declarations that can appear in evaluation source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclarationKind {
    ConstNode,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
    Dimension,
    Unit,
    Type,
    Index,
    Import,
    Include,
    Dag,
}

impl DeclarationKind {
    /// Classify one post-desugar declaration.
    #[must_use]
    pub const fn from_decl_kind(kind: &DeclKind) -> Self {
        match kind {
            DeclKind::ConstNode(_) => Self::ConstNode,
            DeclKind::Param(_) => Self::Param,
            DeclKind::Node(_) => Self::Node,
            DeclKind::Assert(_) => Self::Assert,
            DeclKind::Plot(_) => Self::Plot,
            DeclKind::Figure(_) => Self::Figure,
            DeclKind::Layer(_) => Self::Layer,
            DeclKind::BaseDimension(_) | DeclKind::Dimension(_) => Self::Dimension,
            DeclKind::Unit(_) => Self::Unit,
            DeclKind::Type(_) => Self::Type,
            DeclKind::Index(_) => Self::Index,
            DeclKind::Import(_) | DeclKind::PluginImport(_) => Self::Import,
            DeclKind::Include(_) => Self::Include,
            DeclKind::Dag(_) => Self::Dag,
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(s) => never(*s),
        }
    }
}

impl std::fmt::Display for DeclarationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ConstNode => "const node",
            Self::Param => "param",
            Self::Node => "node",
            Self::Assert => "assert",
            Self::Plot => "plot",
            Self::Figure => "figure",
            Self::Layer => "layer",
            Self::Dimension => "dim",
            Self::Unit => "unit",
            Self::Type => "type",
            Self::Index => "cat/range",
            Self::Import => "import",
            Self::Include => "include",
            Self::Dag => "dag",
        })
    }
}

/// Source context in which an attribute is attached.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttributeTarget {
    Declaration(DeclarationKind),
    IncludeItem {
        /// Producer category when the include item names an assertion or plot.
        producer: Option<DeclarationKind>,
        /// Producer spelling written on the include item.
        name: NameAtom,
    },
}

impl AttributeTarget {
    #[must_use]
    pub const fn declaration(kind: DeclarationKind) -> Self {
        Self::Declaration(kind)
    }

    #[must_use]
    pub const fn include_item(producer: Option<DeclarationKind>, name: NameAtom) -> Self {
        Self::IncludeItem { producer, name }
    }

    /// Whether this target accepts the attribute's semantic role.
    #[must_use]
    pub const fn accepts(&self, attribute: AttributeName) -> bool {
        match (self, attribute) {
            (
                &Self::Declaration(DeclarationKind::Param | DeclarationKind::Node),
                AttributeName::Assumes,
            )
            | (
                &Self::Declaration(DeclarationKind::Assert)
                | &Self::IncludeItem {
                    producer: Some(DeclarationKind::Assert),
                    ..
                },
                AttributeName::ExpectedFail,
            )
            | (
                &Self::Declaration(DeclarationKind::Plot)
                | &Self::IncludeItem {
                    producer: Some(DeclarationKind::Plot),
                    ..
                },
                AttributeName::Hidden,
            ) => true,
            (
                _,
                AttributeName::Assumes
                | AttributeName::ExpectedFail
                | AttributeName::Hidden
                | AttributeName::Lazy,
            ) => false,
        }
    }
}

impl std::fmt::Display for AttributeTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declaration(kind) => kind.fmt(f),
            Self::IncludeItem { .. } => f.write_str("include/import item"),
        }
    }
}
