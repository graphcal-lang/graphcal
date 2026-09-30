//! Evaluation units: the source of one declaration, unit scale, or nominal
//! type, bound to the scope of the DAG that owns it.
//!
//! A checked body names declarations and units by DAG-relative handles, and
//! the instances of one template share their bodies. Which declaration a
//! handle denotes therefore depends on the DAG that runs the body. Outside the
//! compiler, the DAG is never chosen by the code evaluating a body: it is
//! selected here, from the owner of the typed identity whose unit is looked
//! up, and every tree of the unit is handed out together with that DAG's
//! scope ([`Scoped`], [`ScopedTree`]). A `BodyScope` exists only inside such
//! a value, and resolves a handle only for the scoped traversal of the tree
//! that holds it ([`ScopedNode`]): no public API resolves
//! a handle in a scope chosen apart from its tree.

use std::borrow::Borrow;

use crate::hir::expr::{AssertBody, Expr};
use crate::ir::entry::Decl;
use crate::resolved_name::{ResolvedDeclName, ResolvedStructTypeName, ResolvedUnitName};
use crate::tir::texpr::{CheckedBody, ContextualLiteral, ExecutableBodyError, TBody, TExpr};

use super::body_scope::{BodyScope, Scoped};
use super::checked::CheckedTir;
use super::model::{
    ResolvedDomainBound, ResolvedStructFieldSemantics, ResolvedStructFieldTypeKey, Typed,
    TypedAssertEntry, TypedFigureEntry, TypedLayerEntry, TypedPlotEntry,
};
use super::scoped_node::ScopedNode;

/// Why a root checked as a contextual literal is not a string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckedStringError {
    #[error("expected checked contextual operand String")]
    NotString,
    #[error("missing checked expression: {0:?}")]
    Missing(crate::expression_id::ExprId),
}

impl<'t> Scoped<'t, Expr> {
    /// The executable tree of this expression root, with its scope.
    ///
    /// # Errors
    ///
    /// Returns an [`ExecutableBodyError`] when the root is unknown to its
    /// DAG's checked bodies or is not an executable value.
    pub fn executable(self) -> Result<ScopedTree<'t, &'t TExpr>, ExecutableBodyError> {
        self.scope()
            .dag()
            .bodies()
            .executable_value(self.get().id())
            .map(|tree| ScopedTree::new(self.scope(), tree))
    }

    /// The text of this root, checked as a contextual string.
    ///
    /// # Errors
    ///
    /// Returns a [`CheckedStringError`] when the root was not checked as a
    /// contextual string.
    pub fn checked_string(self) -> Result<&'t str, CheckedStringError> {
        match self.scope().dag().bodies().get(self.get().id()) {
            Some(CheckedBody::Executable(TBody::Contextual(literal))) => match literal.literal() {
                ContextualLiteral::String(text) => Ok(text),
                ContextualLiteral::OffsetDateTime(_)
                | ContextualLiteral::CivilDateTime(_)
                | ContextualLiteral::ZonedDateTime(_)
                | ContextualLiteral::TimeZone(_) => Err(CheckedStringError::NotString),
            },
            Some(_) => Err(CheckedStringError::NotString),
            None => Err(CheckedStringError::Missing(self.get().id().clone())),
        }
    }

    /// Every declaration this root references through `@name`, including
    /// unselected branches, resolved in its own scope, in handle order.
    #[must_use]
    pub fn graph_refs(self) -> Vec<ResolvedDeclName> {
        crate::hir::expr::collect_expr_dependencies(self.get())
            .graph_refs
            .iter()
            .map(|handle| self.scope().resolve(handle))
            .collect()
    }
}

/// The operands of an assertion body, each in the assertion's scope.
#[derive(Debug, Clone, Copy)]
pub enum AssertionOperands<'t> {
    /// A Boolean (or indexed Boolean) condition.
    Condition(Scoped<'t, Expr>),
    /// `actual ≈ expected ± tolerance`.
    Tolerance {
        actual: Scoped<'t, Expr>,
        expected: Scoped<'t, Expr>,
        tolerance: Scoped<'t, Expr>,
    },
}

impl<'t> Scoped<'t, AssertBody> {
    /// The operands of this assertion body, in its scope.
    #[must_use]
    pub fn operands(self) -> AssertionOperands<'t> {
        match self.get() {
            AssertBody::Expr(condition) => {
                AssertionOperands::Condition(Scoped::new(self.scope(), condition))
            }
            AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
            } => AssertionOperands::Tolerance {
                actual: Scoped::new(self.scope(), actual),
                expected: Scoped::new(self.scope(), expected),
                tolerance: Scoped::new(self.scope(), tolerance),
            },
        }
    }
}

/// An executable tree together with the scope that resolves its handles.
#[derive(Debug, Clone, Copy)]
pub struct ScopedTree<'t, T> {
    scope: BodyScope<'t>,
    tree: T,
}

impl<'t, T: Borrow<TExpr>> ScopedTree<'t, T> {
    /// Pair a tree with the scope of the DAG it was checked or specialized
    /// in, for the compiler's own selections.
    pub(crate) const fn new(scope: BodyScope<'t>, tree: T) -> Self {
        Self { scope, tree }
    }

    /// The root node of the tree, in the tree's scope.
    #[must_use]
    pub fn root(&self) -> ScopedNode<'_> {
        Scoped::new(self.scope, self.tree.borrow())
    }

    /// The tree, for the compiler's own inspection.
    #[cfg(test)]
    pub(crate) fn tree(&self) -> &TExpr {
        self.tree.borrow()
    }
}

/// The source of one declaration in the scope of the DAG that owns it.
///
/// Obtained only through [`CheckedTir::declaration_body`], keyed by the
/// declaration's identity.
#[derive(Debug, Clone, Copy)]
pub struct DeclarationBody<'t> {
    scope: BodyScope<'t>,
    identity: &'t ResolvedDeclName,
    declaration: &'t Decl<Typed>,
}

impl<'t> DeclarationBody<'t> {
    /// The declaration's identity.
    #[must_use]
    pub const fn identity(self) -> &'t ResolvedDeclName {
        self.identity
    }

    /// The expression of a constant.
    #[must_use]
    pub fn const_expression(self) -> Option<Scoped<'t, Expr>> {
        match self.declaration {
            Decl::Const(entry) => Some(Scoped::new(self.scope, &*entry.expr)),
            _ => None,
        }
    }

    /// The runtime expression of a param (its default) or a node (its
    /// formula).
    #[must_use]
    pub fn runtime_expression(self) -> Option<Scoped<'t, Expr>> {
        match self.declaration {
            Decl::Param(entry) => entry
                .default
                .as_deref()
                .map(|expr| Scoped::new(self.scope, expr)),
            Decl::Node(entry) => entry
                .definition
                .formula()
                .map(|expr| Scoped::new(self.scope, &**expr)),
            _ => None,
        }
    }

    /// Whether the declaration is an unfinished node.
    #[must_use]
    pub const fn is_todo(self) -> bool {
        matches!(self.declaration, Decl::Node(entry) if entry.definition.todo().is_some())
    }

    /// The domain bounds of the declaration's type annotation.
    #[must_use]
    pub fn domain_bounds(self) -> Option<Scoped<'t, [ResolvedDomainBound]>> {
        self.scope
            .dag()
            .semantic()
            .domain_bounds
            .get(self.identity)
            .map(|bounds| Scoped::new(self.scope, bounds.as_slice()))
    }

    /// The assertion this declaration is.
    #[must_use]
    pub const fn assertion(self) -> Option<Scoped<'t, TypedAssertEntry>> {
        match self.declaration {
            Decl::Assert(entry) => Some(Scoped::new(self.scope, entry)),
            _ => None,
        }
    }

    /// The assertion's resolved `#[expected_fail]` configuration.
    #[must_use]
    pub fn expected_fail(self) -> Option<&'t crate::assertion_expectation::ExpectedFail> {
        self.scope.dag().body().expected_fail(self.identity)
    }

    /// The plot this declaration is.
    #[must_use]
    pub const fn plot(self) -> Option<Scoped<'t, TypedPlotEntry>> {
        match self.declaration {
            Decl::Plot(entry) => Some(Scoped::new(self.scope, entry)),
            _ => None,
        }
    }

    /// The checked presentation of each channel of the plot.
    #[must_use]
    pub fn plot_channel_presentations(
        self,
    ) -> Option<
        &'t std::collections::HashMap<
            crate::syntax::ast::EncodingChannel,
            crate::plot_shape::PlotChannelShape,
        >,
    > {
        self.scope.dag().plot_channel_presentations(self.identity)
    }

    /// The figure this declaration is.
    #[must_use]
    pub const fn figure(self) -> Option<Scoped<'t, TypedFigureEntry>> {
        match self.declaration {
            Decl::Figure(entry) => Some(Scoped::new(self.scope, entry)),
            _ => None,
        }
    }

    /// The layer this declaration is.
    #[must_use]
    pub const fn layer(self) -> Option<Scoped<'t, TypedLayerEntry>> {
        match self.declaration {
            Decl::Layer(entry) => Some(Scoped::new(self.scope, entry)),
            _ => None,
        }
    }
}

/// The field contracts of one nominal type in the scope of the DAG that
/// defines it.
///
/// Obtained only through [`CheckedTir::nominal_type_body`], keyed by the
/// type's identity.
#[derive(Debug, Clone, Copy)]
pub struct NominalTypeBody<'t> {
    scope: BodyScope<'t>,
    identity: &'t ResolvedStructTypeName,
    definition: &'t crate::hir::nominal::NominalTypeDef,
}

impl<'t> NominalTypeBody<'t> {
    /// The type's definition.
    #[must_use]
    pub const fn definition(self) -> &'t crate::hir::nominal::NominalTypeDef {
        self.definition
    }

    /// Every field of the type that carries domain bounds.
    pub fn constrained_fields(
        self,
    ) -> impl Iterator<
        Item = (
            &'t ResolvedStructFieldTypeKey,
            Scoped<'t, ResolvedStructFieldSemantics>,
        ),
    > + use<'t> {
        let (scope, identity) = (self.scope, self.identity);
        scope
            .dag()
            .semantic()
            .type_defs
            .constrained_fields()
            .filter(move |(key, _)| key.owning_type == *identity)
            .map(move |(key, field)| (key, Scoped::new(scope, field)))
    }
}

/// The dynamic scale definition of one unit in the scope of the DAG that
/// defines the unit.
///
/// Obtained only through [`CheckedTir::unit_scale_body`], keyed by the
/// unit's identity.
#[derive(Debug, Clone, Copy)]
pub struct UnitScaleBody<'t> {
    scope: BodyScope<'t>,
    spelling: &'t crate::syntax::dimension::UnitRef,
    expression: &'t Expr,
    declared_dimension: &'t crate::dimension::Dimension,
    base_unit_dimension: &'t crate::dimension::Dimension,
    span: crate::syntax::span::Span,
    source: &'t miette::NamedSource<std::sync::Arc<String>>,
}

impl<'t> UnitScaleBody<'t> {
    /// The spelling under which the unit is registered in its module.
    #[must_use]
    pub const fn spelling(self) -> &'t crate::syntax::dimension::UnitRef {
        self.spelling
    }

    /// The scalar scale expression.
    #[must_use]
    pub const fn expression(self) -> Scoped<'t, Expr> {
        Scoped::new(self.scope, self.expression)
    }

    /// The dimension declared on the unit definition.
    #[must_use]
    pub const fn declared_dimension(self) -> &'t crate::dimension::Dimension {
        self.declared_dimension
    }

    /// The dimension proved from the base-unit expression.
    #[must_use]
    pub const fn base_unit_dimension(self) -> &'t crate::dimension::Dimension {
        self.base_unit_dimension
    }

    /// The span of the scale expression.
    #[must_use]
    pub const fn span(self) -> crate::syntax::span::Span {
        self.span
    }

    /// The source of the defining module, which the spans index.
    #[must_use]
    pub const fn source(self) -> &'t miette::NamedSource<std::sync::Arc<String>> {
        self.source
    }
}

impl CheckedTir {
    /// The source of `declaration` in the scope of its owner.
    #[must_use]
    pub fn declaration_body<'t>(
        &'t self,
        declaration: &ResolvedDeclName,
    ) -> Option<DeclarationBody<'t>> {
        let (position, dag) = self.dag_registry().get_positioned(declaration.owner())?;
        let entry = dag.decls().get(declaration)?;
        let identity = match entry {
            Decl::Const(entry) => &entry.identity,
            Decl::Param(entry) => &entry.identity,
            Decl::Node(entry) => &entry.identity,
            Decl::Assert(entry) => &entry.identity,
            Decl::Plot(entry) => &entry.identity,
            Decl::Figure(entry) => &entry.identity,
            Decl::Layer(entry) => &entry.identity,
        };
        Some(DeclarationBody {
            scope: BodyScope::of(position, dag),
            identity,
            declaration: entry,
        })
    }

    /// The dynamic scale definition of `unit` in the scope of its owner.
    #[must_use]
    pub fn unit_scale_body(&self, unit: &ResolvedUnitName) -> Option<UnitScaleBody<'_>> {
        let (position, dag) = self.dag_registry().get_positioned(unit.owner())?;
        dag.semantic()
            .dynamic_unit_scales
            .get(unit)
            .map(|scale| UnitScaleBody {
                scope: BodyScope::of(position, dag),
                spelling: &scale.spelling,
                expression: &scale.expr,
                declared_dimension: &scale.declared_dimension,
                base_unit_dimension: &scale.base_unit_dimension,
                span: scale.span,
                source: &scale.src,
            })
    }

    /// The field contracts of `nominal` in the scope of the DAG defining it.
    #[must_use]
    pub fn nominal_type_body<'t>(
        &'t self,
        nominal: &ResolvedStructTypeName,
    ) -> Option<NominalTypeBody<'t>> {
        let (position, dag) = self.dag_registry().get_positioned(nominal.owner())?;
        let (identity, definition) = dag
            .semantic()
            .type_defs
            .struct_types
            .get_key_value(nominal)?;
        Some(NominalTypeBody {
            scope: BodyScope::of(position, dag),
            identity,
            definition,
        })
    }

    /// A closed external value tree checked in the root module, in the
    /// root's scope.
    pub(crate) const fn external_value_tree(&self, tree: TExpr) -> ScopedTree<'_, TExpr> {
        ScopedTree::new(
            BodyScope::of(super::dag_position::DagPosition::ROOT, self.root()),
            tree,
        )
    }
}
