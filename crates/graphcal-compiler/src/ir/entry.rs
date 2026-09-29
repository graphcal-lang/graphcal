//! Phase-indexed value and sink declaration entries.
//!
//! One entry type per declaration kind carries its complete signature from the
//! moment the declaration is collected: the type annotation travels with the
//! body instead of being re-joined by name later. The [`BodyPhase`] parameter
//! decides how bodies are represented:
//!
//! - [`Syntax`]: desugared AST bodies, each paired with the module scope that
//!   will resolve it ([`InScope`]). Include assembly rewrites these before the
//!   freeze boundary.
//! - The lowered phase (defined next to the frozen HIR DAG in `ir::lower`):
//!   strictly lowered HIR bodies whose references are already canonical.

use std::fmt;
use std::sync::Arc;

use miette::NamedSource;

use crate::dag_id::DagId;
use crate::declaration_category::{DeclCategory, ValueDeclCategory};
use crate::desugar::desugared_ast::{AssertBody, Encoding, Expr, PlotField, TypeExpr};
use crate::dimension::Dimension;
use crate::plot_visibility::PlotVisibility;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::ast::MarkType;
use crate::syntax::dimension::UnitRef;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::{Span, Spanned};

/// Representation of declaration bodies in one IR phase.
pub trait BodyPhase {
    /// A single value expression (const body, param default, unit scale).
    type Expr: fmt::Debug + Clone;
    /// A declaration's type annotation, including its domain bounds.
    type TypeAnnotation: fmt::Debug + Clone;
    /// A node's formula or explicit dependency interface.
    type NodeDefinition: fmt::Debug + Clone;
    /// An assertion body.
    type AssertBody: fmt::Debug + Clone;
    /// A plot's encodings and properties.
    type PlotBody: fmt::Debug + Clone;
    /// A figure's or layer's property fields.
    type CompositionFields: fmt::Debug + Clone;
    /// Identity of the unit defined by a dynamic unit scale.
    type UnitIdentity: fmt::Debug + Clone;
}

/// Pre-freeze phase: bodies are desugared syntax awaiting HIR lowering.
#[derive(Debug, Clone, Copy)]
pub enum Syntax {}

/// A syntactic body together with the module scope that resolves it.
///
/// Include assembly can move a body into another DAG (selective aliases,
/// specialized instances), so the resolution scope is part of the body rather
/// than implied by the DAG that currently holds the entry.
#[derive(Debug, Clone)]
pub struct InScope<T> {
    pub(crate) syntax: T,
    pub(crate) resolution_owner: DagId,
}

impl<T> InScope<T> {
    #[must_use]
    pub(crate) const fn new(syntax: T, resolution_owner: DagId) -> Self {
        Self {
            syntax,
            resolution_owner,
        }
    }
}

/// Syntactic plot body: every expression-bearing part of a plot declaration.
#[derive(Debug, Clone)]
pub struct PlotSyntax {
    pub(crate) encodings: Vec<Encoding>,
    pub(crate) mark_properties: Vec<PlotField>,
    pub(crate) properties: Vec<PlotField>,
}

impl BodyPhase for Syntax {
    type Expr = InScope<Expr>;
    type TypeAnnotation = InScope<TypeExpr>;
    type NodeDefinition =
        InScope<crate::node_definition::NodeDefinition<Expr, crate::syntax::ast::IdentPath>>;
    type AssertBody = InScope<AssertBody>;
    type PlotBody = InScope<PlotSyntax>;
    type CompositionFields = InScope<Vec<PlotField>>;
    /// The DAG that declares the unit; the canonical identity is resolved at
    /// the freeze boundary.
    type UnitIdentity = DagId;
}

/// A `const node` declaration.
#[derive(Debug, Clone)]
pub struct ConstEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    pub type_ann: P::TypeAnnotation,
    pub(crate) expr: P::Expr,
    pub span: Span,
}

/// A `param` declaration with its optional default.
#[derive(Debug, Clone)]
pub struct ParamEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    pub type_ann: P::TypeAnnotation,
    pub default: Option<P::Expr>,
    pub span: Span,
    /// Include overrides whose nominal dependencies must be checked after
    /// canonical type inference.
    pub(crate) override_reconciliations:
        Vec<crate::ir::override_reconciliation::PendingOverrideReconciliation>,
}

/// A `node` declaration.
#[derive(Debug, Clone)]
pub struct NodeEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    pub type_ann: P::TypeAnnotation,
    pub definition: P::NodeDefinition,
    pub span: Span,
}

/// An `assert` declaration.
#[derive(Debug, Clone)]
pub struct AssertEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    pub body: P::AssertBody,
    pub span: Span,
}

/// A `plot` declaration.
#[derive(Debug, Clone)]
pub struct PlotEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    /// Mark shape rendered for this plot.
    pub mark_type: MarkType,
    pub body: P::PlotBody,
    /// Whether this plot renders standalone when its file is the entry
    /// point; `#[hidden]` makes it composition-only (#847).
    pub visibility: PlotVisibility,
}

/// A `figure` declaration.
#[derive(Debug, Clone)]
pub struct FigureEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    /// Plots composed by this figure, in source order.
    pub plot_names: Vec<Spanned<ScopedName>>,
    pub fields: P::CompositionFields,
}

/// A `layer` declaration.
#[derive(Debug, Clone)]
pub struct LayerEntry<P: BodyPhase> {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: DagId,
    /// Plots composed by this layer, in source order.
    pub plot_names: Vec<Spanned<ScopedName>>,
    pub fields: P::CompositionFields,
}

/// A validated dynamic unit scale definition.
#[derive(Debug, Clone)]
pub struct DynamicUnitScaleEntry<P: BodyPhase> {
    /// Identity of the unit being defined.
    pub unit: P::UnitIdentity,
    /// Source spelling under which the unit is registered in this IR.
    pub spelling: UnitRef,
    /// Scalar scale expression.
    pub expr: P::Expr,
    /// Dimension declared on the unit definition.
    pub declared_dimension: Dimension,
    /// Dimension proved from the RHS base-unit expression.
    pub base_unit_dimension: Dimension,
    /// Span of the scalar expression.
    pub span: Span,
    /// Source of the owning DAG, whose bytes `expr` and `span` index. The
    /// evaluator needs it when it evaluates a unit scale owned by another DAG.
    pub src: NamedSource<Arc<String>>,
}

/// Canonical identity of a declaration entry owned by `owner`.
///
/// Every entry is authored under a local, unqualified spelling; qualified
/// names reach a DAG only as lexical bindings to other owners' declarations.
fn entry_identity(owner: &DagId, name: &ScopedName) -> ResolvedDeclName {
    ResolvedDeclName::from_def(owner.clone(), name.leaf().clone())
}

macro_rules! impl_entry_identity {
    ($($entry:ident),* $(,)?) => {$(
        impl<P: BodyPhase> $entry<P> {
            /// Canonical identity of this declaration.
            #[must_use]
            pub fn identity(&self) -> ResolvedDeclName {
                entry_identity(&self.declaration_owner, &self.name)
            }
        }
    )*};
}

impl_entry_identity!(
    ConstEntry,
    ParamEntry,
    NodeEntry,
    AssertEntry,
    PlotEntry,
    FigureEntry,
    LayerEntry,
);

/// One value, assertion, or visualization declaration.
#[derive(Debug, Clone)]
pub enum Decl<P: BodyPhase> {
    Const(ConstEntry<P>),
    Param(ParamEntry<P>),
    Node(NodeEntry<P>),
    Assert(AssertEntry<P>),
    Plot(PlotEntry<P>),
    Figure(FigureEntry<P>),
    Layer(LayerEntry<P>),
}

impl<P: BodyPhase> Decl<P> {
    /// Source-facing spelling of the declaration.
    #[must_use]
    pub const fn name(&self) -> &ScopedName {
        match self {
            Self::Const(entry) => &entry.name,
            Self::Param(entry) => &entry.name,
            Self::Node(entry) => &entry.name,
            Self::Assert(entry) => &entry.name,
            Self::Plot(entry) => &entry.name,
            Self::Figure(entry) => &entry.name,
            Self::Layer(entry) => &entry.name,
        }
    }

    /// Canonical owner of the declaration.
    #[must_use]
    pub const fn declaration_owner(&self) -> &DagId {
        match self {
            Self::Const(entry) => &entry.declaration_owner,
            Self::Param(entry) => &entry.declaration_owner,
            Self::Node(entry) => &entry.declaration_owner,
            Self::Assert(entry) => &entry.declaration_owner,
            Self::Plot(entry) => &entry.declaration_owner,
            Self::Figure(entry) => &entry.declaration_owner,
            Self::Layer(entry) => &entry.declaration_owner,
        }
    }

    /// Canonical identity of the declaration.
    #[must_use]
    pub fn identity(&self) -> ResolvedDeclName {
        entry_identity(self.declaration_owner(), self.name())
    }

    /// Evaluation source-order category of the declaration.
    #[must_use]
    pub const fn category(&self) -> DeclCategory {
        match self {
            Self::Const(_) => DeclCategory::Value(ValueDeclCategory::Const),
            Self::Param(_) => DeclCategory::Value(ValueDeclCategory::Param),
            Self::Node(_) => DeclCategory::Value(ValueDeclCategory::Node),
            Self::Assert(_) => DeclCategory::Assert,
            Self::Plot(_) => DeclCategory::Plot,
            Self::Figure(_) => DeclCategory::Figure,
            Self::Layer(_) => DeclCategory::Layer,
        }
    }
}
