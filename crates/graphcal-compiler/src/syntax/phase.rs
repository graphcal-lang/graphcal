//! AST phase parameter (issue: phased AST split).
//!
//! The AST is parameterized over a [`Phase`] type so that variants that exist
//! only at earlier stages of compilation are statically excluded from later
//! stages.
//!
//! Three phases:
//!
//! - [`Raw`] — produced by the parser. Carries every surface sugar via
//!   [`RawDeclSugar`] / [`RawExprSugar`], plus the unresolved-ref slot
//!   [`UnresolvedRef`]. Consumed by the formatter and any surface-aware
//!   tooling.
//! - [`Desugared`] — produced by [`crate::desugar`]. Surface-sugar slots
//!   are [`core::convert::Infallible`], so `match` arms over them are
//!   unreachable. The unresolved-ref slot is still inhabited — name
//!   resolution has not yet run.
//! - [`Resolved`] — produced by
//!   [`crate::syntax::name_resolve::resolve_name_refs`]. Every phase slot
//!   ([`Phase::DeclSugar`], [`Phase::ExprSugar`], [`Phase::RefSugar`]) is
//!   [`Infallible`], so the AST is a strict subset of the desugared form
//!   with no unresolved references remaining.
//!
//! ```text
//! parser → File<Raw> → desugar → File<Desugared> → name_resolve → File<Resolved> → IR → TIR → eval
//!                   ↘ formatter
//! ```

use core::convert::Infallible;
use core::fmt::Debug;

use crate::syntax::ast::{Ident, MapEntry, MultiDecl, TableIndexSpec};
use crate::syntax::span::Span;

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for AST phases.
///
/// Sealed: only [`Raw`], [`Desugared`], and [`Resolved`] implement it.
pub trait Phase: 'static + sealed::Sealed {
    /// Phase-specific declaration sugar variants.
    ///
    /// Carried by `DeclKind::Sugar(_)`. For [`Raw`] this is [`RawDeclSugar`];
    /// for [`Desugared`] and [`Resolved`] it is [`Infallible`] so the variant
    /// cannot be constructed.
    type DeclSugar: Debug + Clone;

    /// Phase-specific expression sugar variants.
    ///
    /// Carried by `ExprKind::Sugar(_)`. For [`Raw`] this is [`RawExprSugar`];
    /// for [`Desugared`] and [`Resolved`] it is [`Infallible`].
    type ExprSugar: Debug + Clone;

    /// Phase-specific unresolved-reference variants.
    ///
    /// Carried by `ExprKind::UnresolvedRef(_)`. For [`Raw`] and [`Desugared`]
    /// this is [`UnresolvedRef`] (the parser produces `NameRef`/`QualifiedNameRef`
    /// variants); for [`Resolved`] it is [`Infallible`] so the variant cannot
    /// be constructed and the name-resolution pass is statically known to
    /// have eliminated every unresolved reference.
    type RefSugar: Debug + Clone;
}

/// Pre-desugar phase: every surface sugar is representable.
///
/// Produced by the parser. The formatter and any other surface-aware
/// consumer reads this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw {}

impl sealed::Sealed for Raw {}
impl Phase for Raw {
    type DeclSugar = RawDeclSugar;
    type ExprSugar = RawExprSugar;
    type RefSugar = UnresolvedRef;
}

/// Post-desugar phase: surface-sugar variants are statically impossible.
///
/// Produced by [`crate::desugar`]. Unresolved references (`NameRef`,
/// `QualifiedNameRef`) are still representable until the name-resolution
/// pass runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desugared {}

impl sealed::Sealed for Desugared {}
impl Phase for Desugared {
    type DeclSugar = Infallible;
    type ExprSugar = Infallible;
    type RefSugar = UnresolvedRef;
}

/// Post-name-resolution phase: every phase slot is [`Infallible`].
///
/// Produced by [`crate::syntax::name_resolve::resolve_name_refs`]. No
/// unresolved references can appear in the AST; every consumer downstream
/// of name resolution (IR lowering, TIR, evaluation) reads this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {}

impl sealed::Sealed for Resolved {}
impl Phase for Resolved {
    type DeclSugar = Infallible;
    type ExprSugar = Infallible;
    type RefSugar = Infallible;
}

/// Helper for matching against `Sugar(Infallible)` arms.
///
/// In post-desugar code, `match decl.kind { ..., Sugar(s) => never(s) }`
/// is the canonical way to handle the impossible case without runtime panic.
#[inline]
#[must_use]
pub const fn never<T>(x: Infallible) -> T {
    match x {}
}

// ---------------------------------------------------------------------------
// Raw-only sugar variants
// ---------------------------------------------------------------------------

/// Declaration-level sugar — only legal in [`Raw`].
///
/// Each variant corresponds to a surface declaration form that is rewritten
/// into ordinary `DeclKind` variants by [`crate::desugar`]. After desugaring,
/// `DeclKind::Sugar(_)` carries [`Infallible`] and these variants vanish from
/// the type system entirely.
#[derive(Debug, Clone)]
pub enum RawDeclSugar {
    /// Multi-declaration (issue #481): N parallel slots sharing one
    /// `table[…] {…}` initializer. Desugared into N separate
    /// `DeclKind::{Param, Node, ConstNode}` declarations.
    ///
    /// Pinned to `MultiDecl<Raw>` because multi-decl is by definition a
    /// raw-only construct — the desugar pass eliminates it.
    Multi(MultiDecl<Raw>),
}

impl RawDeclSugar {
    /// Returns the surface span of the sugar form.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Multi(m) => m.span,
        }
    }
}

/// Expression-level sugar — only legal in [`Raw`].
///
/// Each variant corresponds to a surface expression form that is rewritten
/// into ordinary `ExprKind` variants by [`crate::desugar::convert`]. In
/// `Desugared`, the `Sugar` slot is `Infallible` and these variants vanish.
#[derive(Debug, Clone)]
pub enum RawExprSugar {
    /// Table literal: `table[Phase, 3] { ... }`.
    ///
    /// Desugars to [`crate::syntax::ast::ExprKind::MapLiteral`] — the
    /// `indexes` metadata is dropped (entries already carry full
    /// `Index.Variant` keys), and the `table` keyword is purely surface
    /// syntax preserved by the formatter via the raw AST.
    TableLiteral {
        indexes: Vec<TableIndexSpec>,
        entries: Vec<MapEntry<Raw>>,
    },
}

// ---------------------------------------------------------------------------
// Unresolved-ref variants (legal in `Raw` and `Desugared`, not in `Resolved`)
// ---------------------------------------------------------------------------

/// Unresolved reference, produced by the parser before name resolution.
///
/// Carried by `ExprKind::UnresolvedRef(P::RefSugar)`. The parser emits these
/// when the meaning of a bare or dotted identifier cannot be determined from
/// syntax alone; the name-resolution pass rewrites them into concrete
/// `ConstRef` / `LocalRef` / `VariantLiteral` / `StructConstruction` variants
/// and produces a [`Resolved`] AST in which
/// `RefSugar = Infallible`.
#[derive(Debug, Clone)]
pub enum UnresolvedRef {
    /// Unresolved bare identifier reference.
    ///
    /// Resolved to one of `ConstRef`, `LocalRef`, or `StructConstruction`
    /// (bare variant) depending on context.
    NameRef(Ident),
    /// Unresolved qualified reference: `a.b`.
    ///
    /// Resolved to `VariantLiteral` (when `a` is a known index) or
    /// `ConstRef` (module-qualified constant) depending on context.
    QualifiedNameRef { qualifier: Ident, member: Ident },
}

impl UnresolvedRef {
    /// Returns the source span of the underlying identifier(s).
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::NameRef(ident) => ident.span,
            Self::QualifiedNameRef { qualifier, member } => qualifier.span.merge(member.span),
        }
    }
}
