//! Completeness parameter of HIR expression trees.
//!
//! HIR lowering is diagnostic-accumulating: an IDE must keep a tree for code
//! that does not resolve. The batch pipeline must never observe such a tree.
//! [`Expr<C>`](super::Expr) makes the distinction a type, mirroring the AST
//! [`Phase`](crate::syntax::phase::Phase) technique:
//!
//! - [`Strict`]: the error-node slot is the uninhabited [`NoErrorNode`], so
//!   every consumer of checked HIR handles it with
//!   [`NoErrorNode::absurd`] instead of a runtime fallback.
//! - `Tolerant` (defined beside the lowerer, because its error node carries
//!   a lowering diagnostic): the error-node slot holds the diagnostic and the
//!   independently lowerable children.

use core::fmt::Debug;

use super::model::Expr;

pub mod sealed {
    pub trait Sealed {}
}

/// Marker trait for HIR expression completeness.
///
/// Sealed: only the compiler's completeness markers implement it.
pub trait Completeness: 'static + Debug + Clone + Copy + sealed::Sealed + Sized {
    /// Payload of [`ExprKind::Error`](super::ExprKind::Error).
    type Error: Debug + Clone;

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

/// Complete HIR: every reference resolved, no error node representable.
///
/// Produced only by strict lowering (which rejects a tree with any
/// diagnostic) and by programmatic construction of already-resolved nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strict {}

impl sealed::Sealed for Strict {}

impl Completeness for Strict {
    type Error = NoErrorNode;

    fn error_children(error: &Self::Error) -> &[Expr<Self>] {
        error.absurd()
    }

    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>] {
        error.absurd()
    }
}
