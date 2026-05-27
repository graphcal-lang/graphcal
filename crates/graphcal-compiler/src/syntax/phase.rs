//! AST phase parameter (issue: phased AST split).
//!
//! The AST is parameterized over a [`Phase`] type so that variants that exist
//! only at earlier stages of compilation are statically excluded from later
//! stages.
//!
//! Three phases:
//!
//! - [`Raw`] — produced by the parser. Carries every surface sugar via
//!   [`crate::syntax::ast::RawDeclSugar`] /
//!   [`crate::syntax::ast::RawExprSugar`], plus the unresolved-ref slot
//!   [`crate::syntax::ast::UnresolvedRef`]. Consumed by the formatter and any surface-aware
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

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Marker trait for AST phases.
///
/// Sealed: only [`Raw`], [`Desugared`], and [`Resolved`] implement it.
pub trait Phase: 'static + sealed::Sealed {
    /// Phase-specific declaration sugar variants.
    ///
    /// Carried by `DeclKind::Sugar(_)`. For [`Raw`] this is
    /// [`crate::syntax::ast::RawDeclSugar`];
    /// for [`Desugared`] and [`Resolved`] it is [`Infallible`] so the variant
    /// cannot be constructed.
    type DeclSugar: Debug + Clone;

    /// Phase-specific expression sugar variants.
    ///
    /// Carried by `ExprKind::Sugar(_)`. For [`Raw`] this is
    /// [`crate::syntax::ast::RawExprSugar`];
    /// for [`Desugared`] and [`Resolved`] it is [`Infallible`].
    type ExprSugar: Debug + Clone;

    /// Phase-specific unresolved-reference variants.
    ///
    /// Carried by `ExprKind::UnresolvedRef(_)`. For [`Raw`] and [`Desugared`]
    /// this is [`crate::syntax::ast::UnresolvedRef`] (the parser produces
    /// unresolved identifier paths); for [`Resolved`] it is [`Infallible`] so
    /// the variant cannot be constructed and the name-resolution pass is
    /// statically known to have eliminated every unresolved reference.
    type RefSugar: Debug + Clone;

    /// Phase-specific name carried by `TypeExprKind::TypeApplication`.
    type TypeApplicationName: Debug + Clone;

    /// Phase-specific name carried by dimension terms in type expressions.
    type DimTermName: Debug + Clone;

    /// Phase-specific name carried by `IndexExpr::Name`.
    type IndexExprName: Debug + Clone;
}

/// Pre-desugar phase: every surface sugar is representable.
///
/// Produced by the parser. The formatter and any other surface-aware
/// consumer reads this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw {}

impl sealed::Sealed for Raw {}

/// Post-desugar phase: surface-sugar variants are statically impossible.
///
/// Produced by [`crate::desugar`]. Unresolved identifier paths are still
/// representable until the name-resolution pass runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desugared {}

impl sealed::Sealed for Desugared {}

/// Post-name-resolution phase: every phase slot is [`Infallible`].
///
/// Produced by [`crate::syntax::name_resolve::resolve_name_refs`]. No
/// unresolved references can appear in the AST; every consumer downstream
/// of name resolution (IR lowering, TIR, evaluation) reads this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {}

impl sealed::Sealed for Resolved {}

/// Helper for matching against `Sugar(Infallible)` arms.
///
/// In post-desugar code, `match decl.kind { ..., Sugar(s) => never(s) }`
/// is the canonical way to handle the impossible case without runtime panic.
#[inline]
#[must_use]
pub const fn never<T>(x: Infallible) -> T {
    match x {}
}
