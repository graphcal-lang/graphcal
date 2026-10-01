//! The declarations of a checked DAG as consumers outside the compiler see
//! them: identity and the checked facts of each kind, without HIR bodies.

use crate::declaration_category::{DeclCategory, ValueDeclCategory};
use crate::ir::entry::Decl;
use crate::plot_visibility::PlotVisibility;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::{Span, Spanned};

use super::model::{CheckedTypeAnnotation, Typed};

/// One declaration of a checked DAG.
///
/// It exposes what evaluation, project preparation, and tools need of a
/// declaration, never its HIR expressions: a declaration's checked trees are
/// selected only by identity, as evaluation units.
#[derive(Debug, Clone, Copy)]
pub struct DeclarationView<'d> {
    identity: &'d ResolvedDeclName,
    kind: DeclarationKind<'d>,
}

/// The kind of a [`DeclarationView`], with the checked facts of that kind.
#[derive(Debug, Clone, Copy)]
pub enum DeclarationKind<'d> {
    /// A value declaration.
    Value(ValueDeclaration<'d>),
    /// An assertion.
    Assert { span: Span },
    /// A plot, with its output visibility.
    Plot { visibility: PlotVisibility },
    /// A figure, with the plots it composes in source order.
    Figure {
        plot_names: &'d [Spanned<ScopedName>],
    },
    /// A layer, with the plots it composes in source order.
    Layer {
        plot_names: &'d [Spanned<ScopedName>],
    },
}

/// A value declaration of a [`DeclarationView`].
#[derive(Debug, Clone, Copy)]
pub struct ValueDeclaration<'d> {
    /// Whether it is a constant, a parameter, or a node.
    pub category: ValueDeclCategory,
    /// Its checked type annotation.
    pub annotation: &'d CheckedTypeAnnotation,
    /// Whether it is a parameter with a default value.
    pub has_default: bool,
    /// Its source span.
    pub span: Span,
}

impl<'d> DeclarationView<'d> {
    /// The view of one declaration entry.
    pub(super) fn of(decl: &'d Decl<Typed>) -> Self {
        let value = |category, entry_annotation, has_default, span| {
            DeclarationKind::Value(ValueDeclaration {
                category,
                annotation: entry_annotation,
                has_default,
                span,
            })
        };
        let (identity, kind) = match decl {
            Decl::Const(entry) => (
                &entry.identity,
                value(ValueDeclCategory::Const, &entry.type_ann, false, entry.span),
            ),
            Decl::Param(entry) => (
                &entry.identity,
                value(
                    ValueDeclCategory::Param,
                    &entry.type_ann,
                    entry.default.is_some(),
                    entry.span,
                ),
            ),
            Decl::Node(entry) => (
                &entry.identity,
                value(ValueDeclCategory::Node, &entry.type_ann, false, entry.span),
            ),
            Decl::Assert(entry) => (
                &entry.identity,
                DeclarationKind::Assert { span: entry.span },
            ),
            Decl::Plot(entry) => (
                &entry.identity,
                DeclarationKind::Plot {
                    visibility: entry.visibility,
                },
            ),
            Decl::Figure(entry) => (
                &entry.identity,
                DeclarationKind::Figure {
                    plot_names: &entry.plot_names,
                },
            ),
            Decl::Layer(entry) => (
                &entry.identity,
                DeclarationKind::Layer {
                    plot_names: &entry.plot_names,
                },
            ),
        };
        Self { identity, kind }
    }

    /// The declaration's canonical identity.
    #[must_use]
    pub const fn identity(self) -> &'d ResolvedDeclName {
        self.identity
    }

    /// The declaration's local name in its owning DAG.
    #[must_use]
    pub const fn name(self) -> &'d DeclName {
        self.identity.leaf()
    }

    /// The declaration's kind, with its checked facts.
    #[must_use]
    pub const fn kind(self) -> DeclarationKind<'d> {
        self.kind
    }

    /// The declaration's category.
    #[must_use]
    pub const fn category(self) -> DeclCategory {
        match self.kind {
            DeclarationKind::Value(value) => DeclCategory::Value(value.category),
            DeclarationKind::Assert { .. } => DeclCategory::Assert,
            DeclarationKind::Plot { .. } => DeclCategory::Plot,
            DeclarationKind::Figure { .. } => DeclCategory::Figure,
            DeclarationKind::Layer { .. } => DeclCategory::Layer,
        }
    }

    /// The declaration as a value declaration, if it is one.
    #[must_use]
    pub const fn value(self) -> Option<ValueDeclaration<'d>> {
        match self.kind {
            DeclarationKind::Value(value) => Some(value),
            _ => None,
        }
    }
}
