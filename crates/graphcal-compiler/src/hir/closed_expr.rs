//! Finished, syntactically closed input literals. Type checking remains a separate phase.

use std::ops::Deref;

use thiserror::Error;

use crate::builtin::BuiltinFnName;
use crate::expression_source::{ExpressionSourceError, ExpressionSourceMap};
use crate::hir::expr::{CheckedExpr, ConstRef, Expr, ExprKind, FunctionRef, visit_expr};
use crate::syntax::ast::UnaryOp;

/// An input literal whose complete tree excludes value references and authored computations.
///
/// This is stronger than resolved HIR, but is not a type proof. Canonical type/unit
/// references still require checking against the selected environment.
#[derive(Debug, Clone)]
pub struct ClosedExpr {
    expression: CheckedExpr,
}

#[derive(Debug, Clone, Error)]
pub enum ClosedExpressionError {
    #[error("numeric values must be finite")]
    NonFiniteNumber,
    #[error("quantity values must be finite")]
    NonFiniteQuantity,
    #[error("an unresolved expression is not allowed")]
    Unresolved,
    #[error("references, computations, and control-flow expressions are not allowed")]
    NotLiteral,
    #[error(transparent)]
    Identity(#[from] ExpressionSourceError),
}

impl ClosedExpr {
    /// Validate the entire literal tree and finish a fresh immutable source revision.
    pub fn try_new(expression: Expr) -> Result<Self, ClosedExpressionError> {
        let mut validation = Ok(());
        visit_expr(&expression, &mut |node| {
            if validation.is_ok() {
                validation = validate_literal_node(node);
            }
        });
        validation?;
        Ok(Self {
            expression: CheckedExpr::finish(expression)?,
        })
    }

    #[must_use]
    pub const fn source_map(&self) -> &ExpressionSourceMap {
        self.expression.source_map()
    }
}

impl Deref for ClosedExpr {
    type Target = Expr;
    fn deref(&self) -> &Self::Target {
        &self.expression
    }
}

const fn validate_literal_node(expr: &Expr) -> Result<(), ClosedExpressionError> {
    match expr.kind() {
        ExprKind::Number(value) if value.is_finite() => Ok(()),
        ExprKind::Number(_) => Err(ClosedExpressionError::NonFiniteNumber),
        ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::OffsetDateTimeLiteral(_)
        | ExprKind::CivilDateTimeLiteral(_)
        | ExprKind::ZonedDateTimeLiteral(_)
        | ExprKind::IanaTimeZoneLiteral(_)
        | ExprKind::VariantLiteral(_)
        | ExprKind::DisplayTimezone { .. }
        | ExprKind::ConstructorCall { .. }
        | ExprKind::MapLiteral { .. }
        | ExprKind::KeyForm { .. }
        | ExprKind::UnaryOp {
            op: UnaryOp::Neg, ..
        } => Ok(()),
        ExprKind::QuantityLiteral { value, .. } if value.is_finite() => Ok(()),
        ExprKind::QuantityLiteral { .. } => Err(ClosedExpressionError::NonFiniteQuantity),
        ExprKind::ConstRef(reference) if matches!(reference.value, ConstRef::Constructor(_)) => {
            Ok(())
        }
        ExprKind::FnCall { callee, .. }
            if matches!(
                callee.value,
                FunctionRef::Builtin(BuiltinFnName::Complex | BuiltinFnName::Datetime)
                    | FunctionRef::Epoch { .. }
            ) =>
        {
            Ok(())
        }
        ExprKind::Error { .. } => Err(ClosedExpressionError::Unresolved),
        ExprKind::StringLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::BinOp { .. }
        | ExprKind::UnaryOp { .. }
        | ExprKind::FnCall { .. }
        | ExprKind::If { .. }
        | ExprKind::Convert { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::ForComp { .. }
        | ExprKind::IndexAccess { .. }
        | ExprKind::Scan { .. }
        | ExprKind::Unfold { .. }
        | ExprKind::Match { .. }
        | ExprKind::DagCall { .. } => Err(ClosedExpressionError::NotLiteral),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::span::Span;

    #[test]
    fn closed_literals_finish_every_child_and_reject_nested_non_literals() {
        let literal = || {
            Expr::new(
                ExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(Expr::new(ExprKind::Number(1.0), Span::new(0, 1))),
                },
                Span::new(0, 1),
            )
        };
        let first = ClosedExpr::try_new(literal()).unwrap();
        let second = ClosedExpr::try_new(literal()).unwrap();
        assert_ne!(first.id().unwrap(), second.id().unwrap());
        visit_expr(&first, &mut |node| {
            assert_eq!(
                first.source_map().span(node.id().unwrap()).unwrap(),
                node.span
            );
        });
        let nested = Expr::new(
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                operand: Box::new(Expr::new(
                    ExprKind::StringLiteral("not a number".to_owned()),
                    Span::new(0, 1),
                )),
            },
            Span::new(0, 1),
        );
        assert!(matches!(
            ClosedExpr::try_new(nested),
            Err(ClosedExpressionError::NotLiteral)
        ));
        assert!(matches!(
            ClosedExpr::try_new(Expr::new(ExprKind::Number(f64::NAN), Span::new(0, 1))),
            Err(ClosedExpressionError::NonFiniteNumber)
        ));
    }
}
