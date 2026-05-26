//! Type aliases pinning every phase-parameterized AST node to
//! [`crate::syntax::phase::Desugared`].
//!
//! Post-desugar consumers — name resolution, IR lowering, TIR, evaluation —
//! `use graphcal_compiler::desugar::desugared_ast as ast` (or `use … ast::*`)
//! instead of `crate::syntax::ast`. Bare type names like `File` or
//! `Declaration` then resolve to their `<Desugared>` variants without each
//! signature having to spell out the phase.
//!
//! Pre-desugar consumers (parser, formatter, LSP surface features) keep
//! using `crate::syntax::ast` — that module defaults to
//! [`Raw`](crate::syntax::phase::Raw).
//!
//! Phase-invariant types (e.g. `Attribute`, `Ident`, `ModulePath`, `BinOp`)
//! are re-exported as-is — they have no `<P>` parameter and behave the same
//! in both phases.

use crate::syntax::phase::Desugared;

// ---------------------------------------------------------------------------
// Phase-parameterized aliases (pinned to Desugared)
// ---------------------------------------------------------------------------

pub type File = crate::syntax::ast::File<Desugared>;
pub type Declaration = crate::syntax::ast::Declaration<Desugared>;
pub type DeclKind = crate::syntax::ast::DeclKind<Desugared>;
pub type AssertDecl = crate::syntax::ast::AssertDecl<Desugared>;
pub type AssertBody = crate::syntax::ast::AssertBody<Desugared>;
pub type Encoding = crate::syntax::ast::Encoding<Desugared>;
pub type PlotField = crate::syntax::ast::PlotField<Desugared>;
pub type MarkSpec = crate::syntax::ast::MarkSpec<Desugared>;
pub type PlotDecl = crate::syntax::ast::PlotDecl<Desugared>;
pub type FigureDecl = crate::syntax::ast::FigureDecl<Desugared>;
pub type LayerDecl = crate::syntax::ast::LayerDecl<Desugared>;
pub type ParamBinding = crate::syntax::ast::ParamBinding<Desugared>;
pub type IncludeDecl = crate::syntax::ast::IncludeDecl<Desugared>;
pub type DagDecl = crate::syntax::ast::DagDecl<Desugared>;
pub type ParamDecl = crate::syntax::ast::ParamDecl<Desugared>;
pub type NodeDecl = crate::syntax::ast::NodeDecl<Desugared>;
pub type ConstNodeDecl = crate::syntax::ast::ConstNodeDecl<Desugared>;
pub type DimDecl = crate::syntax::ast::DimDecl<Desugared>;
pub type UnitDecl = crate::syntax::ast::UnitDecl<Desugared>;
pub type UnitDef = crate::syntax::ast::UnitDef<Desugared>;
pub type DomainBound = crate::syntax::ast::DomainBound<Desugared>;
pub type TypeExpr = crate::syntax::ast::TypeExpr<Desugared>;
pub type TypeExprKind = crate::syntax::ast::TypeExprKind<Desugared>;
pub type DimExpr = crate::syntax::ast::DimExpr<Desugared>;
pub type DimExprItem = crate::syntax::ast::DimExprItem<Desugared>;
pub type DimTerm = crate::syntax::ast::DimTerm<Desugared>;
pub type IndexExpr = crate::syntax::ast::IndexExpr<Desugared>;
pub type IndexDecl = crate::syntax::ast::IndexDecl<Desugared>;
pub type IndexDeclKind = crate::syntax::ast::IndexDeclKind<Desugared>;
pub type Expr = crate::syntax::ast::Expr<Desugared>;
pub type ExprKind = crate::syntax::ast::ExprKind<Desugared>;
pub type MapEntry = crate::syntax::ast::MapEntry<Desugared>;
pub type IndexArg = crate::syntax::ast::IndexArg<Desugared>;
pub type FieldInit = crate::syntax::ast::FieldInit<Desugared>;
pub type MatchArm = crate::syntax::ast::MatchArm<Desugared>;
pub type TupleMatchArm = crate::syntax::ast::TupleMatchArm<Desugared>;
pub type GenericArg = crate::syntax::ast::GenericArg<Desugared>;
pub type GenericParam = crate::syntax::ast::GenericParam<Desugared>;
pub type TypeDecl = crate::syntax::ast::TypeDecl<Desugared>;
pub type TypeDeclBody = crate::syntax::ast::TypeDeclBody<Desugared>;
pub type UnionMember = crate::syntax::ast::UnionMember<Desugared>;
pub type FieldDecl = crate::syntax::ast::FieldDecl<Desugared>;

// ---------------------------------------------------------------------------
// Phase-invariant re-exports (no `<P>` to pin)
// ---------------------------------------------------------------------------

pub use crate::syntax::ast::{
    Attribute, AttributeArg, BaseDimDecl, BinOp, BindableVisibility, DomainBoundKind,
    EncodingChannel, ForBinding, ForBindingIndex, GenericConstraint, Ident, ImportDecl, ImportItem,
    ImportKind, MapEntryKey, MarkType, MatchPattern, ModulePath, MulDivOp, MultiDataRow, MultiDecl,
    MultiDeclSlice, MultiDeclSlot, MultiHeaderCell, MultiSlotAxis, MultiSlotColumnSpan,
    MultiSlotKind, NatExpr, PatternBinding, TableIndexSpec, UnaryOp, UnitExpr, UnitExprItem,
    Visibility,
};
