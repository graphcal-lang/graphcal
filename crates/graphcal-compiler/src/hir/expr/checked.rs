//! Finished HIR bodies: error-free trees with assigned occurrence identities.

#[cfg(test)]
use super::model::ExprKind;
use super::model::{AssertBody, Expr};
use super::visit::{visit_expr, visit_expr_children_mut};
#[cfg(test)]
use crate::syntax::span::Span;

/// An HIR assertion body proven not to contain tolerant-lowering error nodes.
#[derive(Debug, Clone)]
pub struct CheckedAssertBody {
    data: std::sync::Arc<FinishedAssertion>,
}

#[derive(Debug)]
struct FinishedAssertion {
    body: AssertBody,
    source_map: crate::expression_source::ExpressionSourceMap,
}

impl std::ops::Deref for CheckedAssertBody {
    type Target = AssertBody;

    fn deref(&self) -> &Self::Target {
        &self.data.body
    }
}

#[cfg(test)]
impl CheckedAssertBody {
    pub(crate) fn from_assert_body_for_test(body: AssertBody) -> Self {
        Self::finish(body).unwrap()
    }
}

/// An HIR expression proven not to contain tolerant-lowering error nodes.
#[derive(Debug, Clone)]
pub struct CheckedExpr {
    data: std::sync::Arc<FinishedExpression>,
}

/// Immutable expression and its diagnostic projection travel together. Inherited
/// defaults can share this source product while specializing independent facts.
#[derive(Debug)]
struct FinishedExpression {
    expr: Expr,
    source_map: crate::expression_source::ExpressionSourceMap,
}

impl std::ops::Deref for CheckedExpr {
    type Target = Expr;

    fn deref(&self) -> &Self::Target {
        &self.data.expr
    }
}

#[cfg(test)]
impl CheckedExpr {
    pub(crate) fn into_expr_for_test(self) -> Expr {
        self.data.expr.clone()
    }

    pub(crate) fn replace_kind_for_test(&mut self, kind: ExprKind) {
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
    pub(in crate::hir) fn finish(
        mut expr: Expr,
    ) -> Result<Self, crate::expression_source::ExpressionSourceError> {
        assign_expression_ids(&mut expr, &mut crate::expression_id::ExprIds::default())?;
        let source_map = expression_source_map(std::iter::once(&expr))?;
        Ok(Self {
            data: std::sync::Arc::new(FinishedExpression { expr, source_map }),
        })
    }

    #[must_use]
    pub fn source_map(&self) -> &crate::expression_source::ExpressionSourceMap {
        &self.data.source_map
    }
}

impl CheckedAssertBody {
    pub(in crate::hir) fn finish(
        mut body: AssertBody,
    ) -> Result<Self, crate::expression_source::ExpressionSourceError> {
        let mut ids = crate::expression_id::ExprIds::default();
        body.expressions_mut()
            .try_for_each(|expr| assign_expression_ids(expr, &mut ids))?;
        let source_map = expression_source_map(body.expressions())?;
        Ok(Self {
            data: std::sync::Arc::new(FinishedAssertion { body, source_map }),
        })
    }

    #[must_use]
    pub fn source_map(&self) -> &crate::expression_source::ExpressionSourceMap {
        &self.data.source_map
    }
}

pub(super) fn expression_source_map<'a>(
    roots: impl Iterator<Item = &'a Expr>,
) -> Result<
    crate::expression_source::ExpressionSourceMap,
    crate::expression_source::ExpressionSourceError,
> {
    let mut entries = Vec::new();
    roots.for_each(|root| {
        visit_expr(root, &mut |expr| {
            entries.push(expr.id().map(|id| (id.clone(), expr.span)));
        });
    });
    crate::expression_source::ExpressionSourceMap::try_new(
        entries.into_iter().collect::<Result<Vec<_>, _>>()?,
    )
}

pub(super) fn assign_expression_ids(
    expr: &mut Expr,
    ids: &mut crate::expression_id::ExprIds,
) -> Result<(), crate::expression_id::ExprIdExhausted> {
    crate::stack::with_stack_growth(|| {
        expr.id = Some(ids.allocate()?);
        let mut result = Ok(());
        visit_expr_children_mut(expr, &mut |child| {
            if result.is_ok() {
                result = assign_expression_ids(child, ids);
            }
        });
        result
    })
}
