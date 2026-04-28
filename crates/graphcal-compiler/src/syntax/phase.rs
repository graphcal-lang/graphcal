//! AST phase parameter (issue: phased AST split).
//!
//! The AST is parameterized over a [`Phase`] type so that sugar variants
//! (multi-decl, table-literal, future tuple-match) are statically excluded
//! from the post-desugar form.
//!
//! Two phases:
//!
//! - [`Raw`] — produced by the parser. Carries every surface sugar via
//!   [`RawDeclSugar`] / [`RawExprSugar`]. Consumed by the formatter and any
//!   surface-aware tooling.
//! - [`Desugared`] — produced by [`crate::desugar`]. Sugar slots are
//!   [`core::convert::Infallible`], so `match` arms over them are unreachable
//!   and the post-desugar AST is a strict subset of the raw AST.
//!
//! ```text
//! parser → File<Raw> → desugar → File<Desugared> → name_resolve → IR → TIR → eval
//!                  ↘ formatter
//! ```
//!
//! # Migration status (commit 1 of the phased-AST refactor)
//!
//! - The phase parameter exists and threads through the AST as
//!   `<P: Phase = Raw>`, so existing call sites that say `Expr` / `Declaration`
//!   continue to mean the raw form.
//! - [`RawDeclSugar`] and [`RawExprSugar`] are *empty* enums today —
//!   `MultiDecl` and `TableLiteral` still live in `DeclKind` / `ExprKind`.
//!   Commit 2 moves them into the sugar enums and switches downstream
//!   consumers to `Desugared`.
//!
//! Adding a new sugar customer is a three-step change: (1) add a variant to
//! [`RawDeclSugar`] or [`RawExprSugar`]; (2) add a `DesugarSugar` impl in
//! [`crate::desugar`]; (3) the desugared form remains untouched.

use core::convert::Infallible;
use core::fmt::Debug;

use crate::syntax::ast::{MapEntry, MultiDecl, TableIndexSpec};
use crate::syntax::span::Span;

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for AST phases.
///
/// Sealed: only [`Raw`] and [`Desugared`] implement it.
pub trait Phase: 'static + sealed::Sealed {
    /// Phase-specific declaration sugar variants.
    ///
    /// Carried by `DeclKind::Sugar(_)` (added in commit 2). For [`Raw`] this
    /// is [`RawDeclSugar`]; for [`Desugared`] it is [`Infallible`] so the
    /// variant cannot be constructed.
    type DeclSugar: Debug + Clone;

    /// Phase-specific expression sugar variants.
    ///
    /// Carried by `ExprKind::Sugar(_)` (added in commit 2). For [`Raw`] this
    /// is [`RawExprSugar`]; for [`Desugared`] it is [`Infallible`].
    type ExprSugar: Debug + Clone;
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
}

/// Post-desugar phase: sugar variants are statically impossible.
///
/// Produced by [`crate::desugar`]. Every consumer downstream of desugar
/// (name resolution, IR lowering, TIR, evaluation) reads this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desugared {}

impl sealed::Sealed for Desugared {}
impl Phase for Desugared {
    type DeclSugar = Infallible;
    type ExprSugar = Infallible;
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
