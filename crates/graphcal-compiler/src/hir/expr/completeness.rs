//! Completeness parameter of HIR expression trees.
//!
//! HIR lowering is diagnostic-accumulating: an IDE must keep a tree for code
//! that does not resolve. The batch pipeline must never observe such a tree,
//! and it keys checked facts by per-node occurrence identities.
//! [`Expr<C>`](super::Expr) makes both distinctions types, mirroring the AST
//! [`Phase`](crate::syntax::phase::Phase) technique:
//!
//! | Completeness | error node               | node identity | declaration reference | unit reference        |
//! |--------------|--------------------------|---------------|-----------------------|-----------------------|
//! | `Tolerant`   | diagnostic + children    | none          | source definition     | source definition     |
//! | [`Draft`]    | [`NoErrorNode`]          | none          | [`LocalDecl`]         | [`LocalUnit`]         |
//! | [`Strict`]   | [`NoErrorNode`]          | `ExprId`      | [`LocalDecl`]         | [`LocalUnit`]         |
//!
//! `Tolerant` is defined beside the lowerer, because its error node carries
//! a lowering diagnostic. Strict lowering refines `Tolerant` into [`Draft`];
//! finishing a body ([`super::CheckedExpr`]) numbers a [`Draft`] into
//! [`Strict`]. Consumers of checked HIR discharge the error-node arm with
//! [`NoErrorNode::absurd`] instead of a runtime fallback, and read identities
//! without a fallible lookup.

use core::fmt::Debug;
use core::hash::Hash;

use super::local_decl::LocalDecl;
use super::local_unit::LocalUnit;
use super::model::Expr;
use crate::expression_id::ExprId;

pub mod sealed {
    pub trait Sealed {}
}

/// Marker trait for HIR expression completeness.
///
/// Sealed: only the compiler's completeness markers implement it.
pub trait Completeness: 'static + Debug + Clone + Copy + sealed::Sealed + Sized {
    /// Occurrence identity stored on every node.
    type Id: Debug + Clone;

    /// Payload of [`ExprKind::Error`](super::ExprKind::Error).
    type Error: Debug + Clone;

    /// How a declaration reference names its target: the source definition
    /// in an IDE tree, a frame-relative [`LocalDecl`] in a complete tree.
    type DeclRef: Debug + Clone + PartialEq + Eq + Hash + Ord;

    /// How a unit reference names its unit: the source definition in an IDE
    /// tree, a frame-relative [`LocalUnit`] in a complete tree.
    type UnitRef: Debug + Clone + PartialEq + Eq;

    /// Expression children retained under an error node, in source order.
    fn error_children(error: &Self::Error) -> &[Expr<Self>];

    /// Mutable access to the children retained under an error node.
    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>];
}

/// The error-node payload of a complete tree: no value exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoErrorNode {}

impl NoErrorNode {
    /// Discharge the impossible error-node arm of a complete tree.
    #[must_use]
    pub const fn absurd<T>(self) -> T {
        match self {}
    }
}

/// Complete but unnumbered HIR: every reference resolved, no identities yet.
///
/// Produced by strict lowering and by programmatic construction of
/// already-resolved nodes; finishing a body numbers it into [`Strict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draft {}

impl sealed::Sealed for Draft {}

impl Completeness for Draft {
    type Id = ();
    type Error = NoErrorNode;
    type DeclRef = LocalDecl;
    type UnitRef = LocalUnit;

    fn error_children(error: &Self::Error) -> &[Expr<Self>] {
        error.absurd()
    }

    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>] {
        error.absurd()
    }
}

/// Finished HIR: complete, and every node carries an occurrence identity.
///
/// Produced only by finishing a [`Draft`] body, so identities are always
/// fresh and unique within their revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strict {}

impl sealed::Sealed for Strict {}

impl Completeness for Strict {
    type Id = ExprId;
    type Error = NoErrorNode;
    type DeclRef = LocalDecl;
    type UnitRef = LocalUnit;

    fn error_children(error: &Self::Error) -> &[Expr<Self>] {
        error.absurd()
    }

    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>] {
        error.absurd()
    }
}
