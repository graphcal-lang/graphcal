//! Inference of conditionals, unary and binary operators, and display conversions.

use crate::hir::expr::{Expr, ExprKind, ResolvedUnitExpr};
use crate::outcome::Outcome;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::{
    MismatchOperand, MismatchRule, OperandExpectation,
};
use crate::source_id::SourceId;

use crate::semantic_error::SemanticError;
use crate::syntax::ast::UnaryOp;

use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::tir::dim_check::helpers::expect_quantity;
use crate::tir::dim_check::infer::rules::{self, Operand};

use super::context::Infer;

impl Infer<'_> {
    pub(super) fn infer_hir_if(
        &self,
        condition: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let infer = |expr: &Expr| self.infer_hir_type(expr);
        let cond_type = infer(condition)?;
        let then_type = infer(then_branch)?;
        let else_type = infer(else_branch)?;
        rules::if_rule(
            &Operand {
                ty: cond_type,
                span: condition.span,
            },
            &Operand {
                ty: then_type,
                span: then_branch.span,
            },
            &Operand {
                ty: else_type,
                span: else_branch.span,
            },
            self.env.registry,
            self.env.src,
        )
        .map_err(Outcome::Failed)
    }

    pub(super) fn infer_hir_unary(
        &self,
        op: crate::desugar::desugared_ast::UnaryOp,
        operand: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let operand_type = self.infer_hir_type(operand)?;
        rules::unary_rule(
            op,
            &Operand {
                ty: operand_type,
                span: operand.span,
            },
            self.env.registry,
            self.env.src,
        )
        .map_err(Outcome::Failed)
    }
}

pub(super) fn try_const_int(expr: &Expr) -> Option<i64> {
    use crate::desugar::desugared_ast::BinOp;
    match expr.kind() {
        ExprKind::Integer(n) => Some(*n),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => try_const_int(operand)?.checked_neg(),
        ExprKind::BinOp { op, lhs, rhs } => {
            let l = try_const_int(lhs)?;
            let r = try_const_int(rhs)?;
            match op {
                BinOp::Add => l.checked_add(r),
                BinOp::Sub => l.checked_sub(r),
                BinOp::Mul => l.checked_mul(r),
                BinOp::Div if r != 0 => l.checked_div(r),
                BinOp::Mod if r == -1 => Some(0),
                BinOp::Mod if r != 0 => l.checked_rem(r),
                BinOp::Pow(_) if r >= 0 => u32::try_from(r).ok().and_then(|e| l.checked_pow(e)),
                _ => None,
            }
        }
        _ => None,
    }
}

impl Infer<'_> {
    pub(super) fn infer_hir_binop(
        &self,
        span: crate::syntax::span::Span,
        op: crate::desugar::desugared_ast::BinOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        use crate::desugar::desugared_ast::BinOp;
        let lhs_type = self.infer_hir_type(lhs)?;
        let rhs_type = self.infer_hir_type(rhs)?;
        // Exact exponent shape is carried by `BinOp::Pow`; constant folding is
        // needed for runtime-classified right-associated Int power chains and for
        // the additive Fin-key rule (`k + c` shifts the bound by a static Nat).
        let rhs_const_int = if matches!(op, BinOp::Pow(_) | BinOp::Add) {
            try_const_int(rhs)
        } else {
            None
        };
        rules::binop_rule(
            span,
            op,
            &Operand {
                ty: lhs_type,
                span: lhs.span,
            },
            &Operand {
                ty: rhs_type,
                span: rhs.span,
            },
            rhs_const_int,
            self.env.registry,
            self.env.src,
        )
        .map_err(Outcome::Failed)
    }
}

/// Reject `(expr -> u) -> v` and its timezone-display analogues (#648 B2).
///
/// The surface grammar already rejects bare chaining (`expr -> u -> v`), but
/// parentheses used to smuggle a nested conversion through; only the outermost
/// target ever took effect, so the inner one is either a typo or dead code.
/// Parens are flattened in HIR, so a direct nested `Convert`/`DisplayTimezone`
/// operand is exactly the parenthesized-chain shape.
fn reject_nested_conversion(inner: &Expr, src: SourceId) -> Result<(), SemanticError> {
    if matches!(
        inner.kind(),
        ExprKind::Convert { .. } | ExprKind::DisplayTimezone { .. }
    ) {
        return Err(SemanticError::located(
            src,
            inner.span,
            DimensionError::NestedConversion,
        ));
    }
    Ok(())
}

impl Infer<'_> {
    pub(super) fn infer_hir_convert(
        &self,
        inner: &Expr,
        target: &ResolvedUnitExpr,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        reject_nested_conversion(inner, self.env.src)?;
        let inner_type = self.infer_hir_type(inner)?;
        // `->` distributes element-wise over indexed values (#648 U1): the quantity
        // element dimension must match the target. Multi-axis values unwrap
        // through each nested Indexed layer.
        let mut element = &inner_type;
        while let CheckedType::Indexed {
            element: nested, ..
        } = element
        {
            element = nested;
        }
        let expr_dim = match element.complex_dimension() {
            Some(dimension) => dimension.clone(),
            None => expect_quantity(element, self.env.registry, self.env.src, inner.span)?,
        };
        let target_dim =
            rules::resolve_unit_dimension_or_diagnose(target, self.env.tir, self.env.src)?;

        if expr_dim != target_dim {
            return Err(SemanticError::located(
                self.env.src,
                target.span,
                DimensionError::ConversionDimensionMismatch {
                    target: self.env.registry.dimensions.dimension_spelling(&target_dim),
                    expr_dim: self.env.registry.dimensions.dimension_spelling(&expr_dim),
                },
            )
            .into());
        }

        Ok(inner_type)
    }

    pub(super) fn infer_hir_display_timezone(
        &self,
        inner: &Expr,
        timezone: &crate::semantic::time_zone::IanaTimeZoneId,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        reject_nested_conversion(inner, self.env.src)?;
        let inner_type = self.infer_hir_type(inner)?;
        if !matches!(&inner_type, CheckedType::Datetime(_)) {
            return Err(SemanticError::located(
                self.env.src,
                inner.span,
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Expected(OperandExpectation::Datetime)),
                    found: Box::new(MismatchOperand::Type(
                        inner_type.spelling(&self.env.registry.dimensions),
                    )),
                    help: Box::new(MismatchRule::TimezoneDisplayDatetime(timezone.clone())),
                },
            )
            .into());
        }
        Ok(inner_type)
    }
}
