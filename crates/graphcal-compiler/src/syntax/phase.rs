//! AST phase parameter (issue: phased AST split).
//!
//! The AST is parameterized over a [`Phase`] type so that variants that exist
//! only at earlier stages of compilation are statically excluded from later
//! stages.
//!
//! Two phases:
//!
//! - [`Raw`] — produced by the parser. Carries every surface sugar via
//!   [`crate::syntax::ast::RawDeclSugar`] /
//!   [`crate::syntax::ast::RawExprSugar`]. Consumed by the formatter and any
//!   surface-aware tooling.
//! - [`Desugared`] — produced by [`crate::desugar`]. Surface-sugar slots
//!   are [`core::convert::Infallible`], so `match` arms over them are
//!   unreachable. Reference paths stay syntactic
//!   ([`crate::syntax::ast::UnresolvedRef`]); HIR lowering is the single
//!   stage that classifies and resolves them.
//!
//! ```text
//! parser → File<Raw> → desugar → File<Desugared> → HIR/IR → TIR → eval
//!                   ↘ formatter
//! ```

use core::convert::Infallible;
use core::fmt::Debug;

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Marker trait for AST phases.
///
/// Sealed: only [`Raw`] and [`Desugared`] implement it.
pub trait Phase: 'static + sealed::Sealed {
    /// Phase-specific declaration sugar variants.
    ///
    /// Carried by `DeclKind::Sugar(_)`. For [`Raw`] this is
    /// [`crate::syntax::ast::RawDeclSugar`]; for [`Desugared`] it is
    /// [`Infallible`] so the variant cannot be constructed.
    type DeclSugar: Debug + Clone;

    /// Phase-specific expression sugar variants.
    ///
    /// Carried by `ExprKind::Sugar(_)`. For [`Raw`] this is
    /// [`crate::syntax::ast::RawExprSugar`]; for [`Desugared`] it is
    /// [`Infallible`].
    type ExprSugar: Debug + Clone;
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
/// representable; HIR lowering resolves them. This is the final syntax-AST
/// phase — every downstream consumer reads HIR, not a further AST phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desugared {}

impl sealed::Sealed for Desugared {}

/// Helper for matching against `Sugar(Infallible)` arms.
///
/// In post-desugar code, `match decl.kind { ..., Sugar(s) => never(s) }`
/// is the canonical way to handle the impossible case without runtime panic.
#[inline]
#[must_use]
pub(crate) const fn never<T>(x: Infallible) -> T {
    match x {}
}
