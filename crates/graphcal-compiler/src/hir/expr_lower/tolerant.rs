//! Tolerant HIR: trees that keep going past unresolved references.
//!
//! Each error node owns the diagnostic that produced it, so a tolerant tree
//! is its own diagnostic list: IDE consumers read [`Expr::diagnostics`], and
//! strict lowering refines the tree into [`Draft`] HIR, rejecting it at the
//! first error node in source order.

use crate::hir::expr::{
    AssertBody, Completeness, CompletenessSealed, Draft, Expr, ExprKind, NoErrorNode, Refinement,
    refine_assert_body, refine_expr, visit_expr,
};

use super::error::ExprLowerError;
use crate::hir::expr::LocalDecl;
use crate::resolved_name::ResolvedDeclName;

/// Tolerant HIR: an unresolved reference becomes an error node that records
/// its diagnostic and keeps every independently lowerable child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tolerant {}

impl CompletenessSealed for Tolerant {}

impl Completeness for Tolerant {
    type Id = ();
    type Error = LoweringFailure;
    type DeclRef = crate::resolved_name::ResolvedDeclName;

    fn error_children(error: &Self::Error) -> &[Expr<Self>] {
        &error.children
    }

    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>] {
        &mut error.children
    }
}

/// The payload of a tolerant error node.
#[derive(Debug, Clone)]
pub struct LoweringFailure {
    error: ExprLowerError,
    children: Vec<Expr<Tolerant>>,
}

impl LoweringFailure {
    pub(super) const fn new(error: ExprLowerError, children: Vec<Expr<Tolerant>>) -> Self {
        Self { error, children }
    }

    /// The diagnostic reported for this node.
    #[must_use]
    pub const fn error(&self) -> &ExprLowerError {
        &self.error
    }

    /// Independently lowered expression children, in source order.
    #[must_use]
    pub fn children(&self) -> &[Expr<Tolerant>] {
        &self.children
    }
}

impl Expr<Tolerant> {
    /// Every lowering diagnostic in the tree, in source order.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<&ExprLowerError> {
        let mut diagnostics = Vec::new();
        visit_expr(self, &mut |node| {
            if let ExprKind::Error(failure) = node.kind() {
                diagnostics.push(failure.error());
            }
        });
        diagnostics
    }
}

/// Refines a tolerant tree into draft HIR, failing on its first error node.
struct RejectErrorNodes;

impl Refinement<Tolerant, Draft> for RejectErrorNodes {
    type Failure = ExprLowerError;

    fn id(&mut self, (): ()) -> Result<(), ExprLowerError> {
        Ok(())
    }

    fn error_node(&mut self, error: LoweringFailure) -> Result<NoErrorNode, ExprLowerError> {
        Err(error.error)
    }

    fn decl_ref(&mut self, definition: ResolvedDeclName) -> LocalDecl {
        LocalDecl::new(definition)
    }
}

/// Refine a tolerant expression into complete, unnumbered HIR.
pub(super) fn into_draft(expr: Expr<Tolerant>) -> Result<Expr<Draft>, ExprLowerError> {
    refine_expr(expr, &mut RejectErrorNodes)
}

/// Refine a tolerant assertion body into complete, unnumbered HIR.
pub(super) fn assert_body_into_draft(
    body: AssertBody<Tolerant>,
) -> Result<AssertBody<Draft>, ExprLowerError> {
    refine_assert_body(body, &mut RejectErrorNodes)
}
