//! Type aliases pinning every phase-parameterized AST node to
//! [`crate::syntax::phase::Resolved`].
//!
//! Post-name-resolution consumers — IR lowering, TIR, evaluation —
//! `use graphcal_compiler::desugar::resolved_ast as ast` (or `use … ast::*`)
//! instead of [`crate::desugar::desugared_ast`]. Bare type names like `File`
//! or `Declaration` then resolve to their `<Resolved>` variants without each
//! signature having to spell out the phase.
//!
//! Pre-resolution consumers (parser, formatter, LSP symbol-table walk,
//! `desugar_tuple_matches`, `resolve_name_refs`) keep using
//! [`crate::desugar::desugared_ast`] — those operate on the AST before name
//! resolution has eliminated [`crate::syntax::ast::UnresolvedRef`].
//!
//! Phase-invariant types (e.g. `Attribute`, `Ident`, `ModulePath`, `BinOp`)
//! are re-exported as-is — they have no `<P>` parameter and behave the same
//! in every phase.

use crate::syntax::phase::Resolved;

// ---------------------------------------------------------------------------
// Phase-parameterized aliases (pinned to Resolved)
// ---------------------------------------------------------------------------

pub type File = crate::syntax::ast::File<Resolved>;
pub type Declaration = crate::syntax::ast::Declaration<Resolved>;
pub type DeclKind = crate::syntax::ast::DeclKind<Resolved>;
pub type AssertDecl = crate::syntax::ast::AssertDecl<Resolved>;
pub type AssertBody = crate::syntax::ast::AssertBody<Resolved>;
pub type Encoding = crate::syntax::ast::Encoding<Resolved>;
pub type PlotField = crate::syntax::ast::PlotField<Resolved>;
pub type MarkSpec = crate::syntax::ast::MarkSpec<Resolved>;
pub type PlotDecl = crate::syntax::ast::PlotDecl<Resolved>;
pub type FigureDecl = crate::syntax::ast::FigureDecl<Resolved>;
pub type LayerDecl = crate::syntax::ast::LayerDecl<Resolved>;
pub type ParamBinding = crate::syntax::ast::ParamBinding<Resolved>;
pub type IncludeDecl = crate::syntax::ast::IncludeDecl<Resolved>;
pub type DagDecl = crate::syntax::ast::DagDecl<Resolved>;
pub type ParamDecl = crate::syntax::ast::ParamDecl<Resolved>;
pub type NodeDecl = crate::syntax::ast::NodeDecl<Resolved>;
pub type ConstNodeDecl = crate::syntax::ast::ConstNodeDecl<Resolved>;
pub type DimDecl = crate::syntax::ast::DimDecl<Resolved>;
pub type UnitDecl = crate::syntax::ast::UnitDecl<Resolved>;
pub type UnitDef = crate::syntax::ast::UnitDef<Resolved>;
pub type DomainBound = crate::syntax::ast::DomainBound<Resolved>;
pub type TypeExpr = crate::syntax::ast::TypeExpr<Resolved>;
pub type TypeExprKind = crate::syntax::ast::TypeExprKind<Resolved>;
pub type DimExpr = crate::syntax::ast::DimExpr<Resolved>;
pub type DimExprItem = crate::syntax::ast::DimExprItem<Resolved>;
pub type DimTerm = crate::syntax::ast::DimTerm<Resolved>;
pub type IndexExpr = crate::syntax::ast::IndexExpr<Resolved>;
pub type IndexDecl = crate::syntax::ast::IndexDecl<Resolved>;
pub type IndexDeclKind = crate::syntax::ast::IndexDeclKind<Resolved>;
pub type Expr = crate::syntax::ast::Expr<Resolved>;
pub type ExprKind = crate::syntax::ast::ExprKind<Resolved>;
pub type MapEntry = crate::syntax::ast::MapEntry<Resolved>;
pub type IndexArg = crate::syntax::ast::IndexArg<Resolved>;
pub type FieldInit = crate::syntax::ast::FieldInit<Resolved>;
pub type MatchArm = crate::syntax::ast::MatchArm<Resolved>;
pub type TupleMatchArm = crate::syntax::ast::TupleMatchArm<Resolved>;
pub type GenericArg = crate::syntax::ast::GenericArg<Resolved>;
pub type GenericParam = crate::syntax::ast::GenericParam<Resolved>;
pub type TypeDecl = crate::syntax::ast::TypeDecl<Resolved>;
pub type TypeDeclBody = crate::syntax::ast::TypeDeclBody<Resolved>;
pub type UnionMember = crate::syntax::ast::UnionMember<Resolved>;
pub type FieldDecl = crate::syntax::ast::FieldDecl<Resolved>;

// ---------------------------------------------------------------------------
// Phase-invariant re-exports (no `<P>` to pin)
// ---------------------------------------------------------------------------

pub use crate::syntax::ast::{
    Attribute, AttributeArg, BaseDimDecl, BinOp, BindableVisibility, DomainBoundKind,
    EncodingChannel, ForBinding, ForBindingIndex, GenericConstraint, Ident, ImportDecl, ImportItem,
    ImportItemNamespace, ImportKind, MapEntryKey, MarkType, MatchPattern, ModulePath, MulDivOp,
    MultiDataRow, MultiDecl, MultiDeclSlice, MultiDeclSlot, MultiHeaderCell, MultiSlotAxis,
    MultiSlotColumnSpan, MultiSlotKind, NatExpr, PatternBinding, TableIndexSpec, UnaryOp, UnitExpr,
    UnitExprItem, Visibility,
};
