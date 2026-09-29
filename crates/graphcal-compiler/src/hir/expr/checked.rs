//! Finished HIR bodies: strict trees whose every node carries an occurrence identity.

use super::completeness::{Draft, NoErrorNode, Strict};
use super::local_decl::LocalDecl;
#[cfg(test)]
use super::model::ExprKind;
use super::model::{AssertBody, Expr};
use super::refine::{Refinement, refine_assert_body, refine_expr};
use super::visit::visit_expr;
#[cfg(test)]
use super::visit::visit_expr_children_mut;
use crate::expression_id::{ExprId, ExprIdExhausted, ExprIds};
use crate::expression_source::{ExpressionSourceError, ExpressionSourceMap};
#[cfg(test)]
use crate::syntax::span::Span;

/// A finished HIR assertion body: strict operands numbered in one revision.
#[derive(Debug, Clone)]
pub struct CheckedAssertBody {
    data: std::sync::Arc<FinishedAssertion>,
}

#[derive(Debug)]
struct FinishedAssertion {
    body: AssertBody,
    source_map: ExpressionSourceMap,
}

impl std::ops::Deref for CheckedAssertBody {
    type Target = AssertBody;

    fn deref(&self) -> &Self::Target {
        &self.data.body
    }
}

#[cfg(test)]
impl CheckedAssertBody {
    pub(crate) fn from_assert_body_for_test(body: AssertBody<Draft>) -> Self {
        Self::finish(body).unwrap()
    }
}

/// A finished HIR expression: a strict tree numbered in a fresh revision.
#[derive(Debug, Clone)]
pub struct CheckedExpr {
    data: std::sync::Arc<FinishedExpression>,
}

/// Immutable expression and its diagnostic projection travel together. Inherited
/// defaults can share this source product while specializing independent facts.
#[derive(Debug)]
struct FinishedExpression {
    expr: Expr,
    source_map: ExpressionSourceMap,
}

impl std::ops::Deref for CheckedExpr {
    type Target = Expr;

    fn deref(&self) -> &Self::Target {
        &self.data.expr
    }
}

#[cfg(test)]
impl CheckedExpr {
    pub(crate) fn from_draft_for_test(expr: Expr<Draft>) -> Self {
        Self::finish(expr).unwrap()
    }

    pub(crate) fn into_expr_for_test(self) -> Expr {
        self.data.expr.clone()
    }

    pub(crate) fn replace_kind_for_test(&mut self, kind: ExprKind<Draft>) {
        *self = Self::finish(Expr::new(kind, self.data.expr.span)).unwrap();
    }

    /// Change diagnostic projections without changing any semantic node identity.
    pub(crate) fn map_spans_for_test(&mut self, project: impl Fn(Span) -> Span) {
        fn apply(expr: &mut Expr, project: &impl Fn(Span) -> Span) {
            crate::stack::with_stack_growth(|| {
                expr.span = project(expr.span);
                visit_expr_children_mut(expr, &mut |child| apply(child, project));
            });
        }
        let mut expr = self.data.expr.clone();
        apply(&mut expr, &project);
        let source_map = expression_source_map(std::iter::once(&expr)).unwrap();
        self.data = std::sync::Arc::new(FinishedExpression { expr, source_map });
    }
}

impl CheckedExpr {
    /// Number a complete tree in a fresh revision and seal its source map.
    pub(in crate::hir) fn finish(expr: Expr<Draft>) -> Result<Self, ExpressionSourceError> {
        let expr = refine_expr(expr, &mut NumberNodes(ExprIds::default()))?;
        let source_map = expression_source_map(std::iter::once(&expr))?;
        Ok(Self {
            data: std::sync::Arc::new(FinishedExpression { expr, source_map }),
        })
    }

    #[must_use]
    pub fn source_map(&self) -> &ExpressionSourceMap {
        &self.data.source_map
    }
}

impl CheckedAssertBody {
    /// Number every operand in one fresh revision and seal their source map.
    pub(in crate::hir) fn finish(body: AssertBody<Draft>) -> Result<Self, ExpressionSourceError> {
        let body = refine_assert_body(body, &mut NumberNodes(ExprIds::default()))?;
        let source_map = expression_source_map(body.expressions())?;
        Ok(Self {
            data: std::sync::Arc::new(FinishedAssertion { body, source_map }),
        })
    }

    #[must_use]
    pub fn source_map(&self) -> &ExpressionSourceMap {
        &self.data.source_map
    }
}

#[cfg(test)]
impl Expr {
    /// Drop occurrence identities so a finished tree can be finished again.
    pub(crate) fn into_draft_for_test(self) -> Expr<Draft> {
        struct ForgetIds;
        impl Refinement<Strict, Draft> for ForgetIds {
            type Failure = std::convert::Infallible;

            fn id(&mut self, _: ExprId) -> Result<(), Self::Failure> {
                Ok(())
            }

            fn error_node(&mut self, error: NoErrorNode) -> Result<NoErrorNode, Self::Failure> {
                error.absurd()
            }

            fn decl_ref(&mut self, reference: LocalDecl) -> LocalDecl {
                reference
            }
        }
        match refine_expr(self, &mut ForgetIds) {
            Ok(draft) => draft,
            Err(never) => match never {},
        }
    }
}

/// Assigns occurrence identities in pre-order while refining a draft to strict HIR.
struct NumberNodes(ExprIds);

impl Refinement<Draft, Strict> for NumberNodes {
    type Failure = ExprIdExhausted;

    fn id(&mut self, (): ()) -> Result<ExprId, ExprIdExhausted> {
        self.0.allocate()
    }

    fn error_node(&mut self, error: NoErrorNode) -> Result<NoErrorNode, ExprIdExhausted> {
        error.absurd()
    }

    fn decl_ref(&mut self, reference: LocalDecl) -> LocalDecl {
        reference
    }
}

fn expression_source_map<'a>(
    roots: impl Iterator<Item = &'a Expr>,
) -> Result<ExpressionSourceMap, ExpressionSourceError> {
    let mut entries = Vec::new();
    roots.for_each(|root| {
        visit_expr(root, &mut |expr| {
            entries.push((expr.id().clone(), expr.span));
        });
    });
    ExpressionSourceMap::try_new(entries)
}
