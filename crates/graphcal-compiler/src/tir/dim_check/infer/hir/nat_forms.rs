//! Nat-expression normalization and its diagnostics.

use crate::hir::types::NatExpr;
use std::sync::Arc;

use miette::NamedSource;

use crate::nat::NatOverflowError;
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::typed::NatPolyForm;

pub(in crate::tir::dim_check) fn hir_nat_to_linear_form(
    expr: &NatExpr,
) -> Result<NatPolyForm, NatOverflowError> {
    match expr {
        NatExpr::Literal(n, _) => Ok(NatPolyForm::from_constant(*n)),
        NatExpr::Param(param) => Ok(NatPolyForm::from_var(param.value.name.clone())),
        NatExpr::Add(operands, _) => operands
            .iter()
            .try_fold(NatPolyForm::from_constant(0), |sum, operand| {
                sum.add(&hir_nat_to_linear_form(operand)?)
            }),
        NatExpr::Mul(operands, _) => operands
            .iter()
            .try_fold(NatPolyForm::from_constant(1), |product, operand| {
                product.mul(&hir_nat_to_linear_form(operand)?)
            }),
    }
}

pub(super) fn resolve_hir_nat_form(
    expr: &NatExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<NatPolyForm, GraphcalError> {
    hir_nat_to_linear_form(expr).map_err(|error| nat_overflow_error(error, src, expr.span()))
}

fn nat_overflow_error(
    err: NatOverflowError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

pub(super) fn finite_index_error(
    err: crate::registry::types::IndexCardinalityError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.describe_finite_index(),
        src: src.clone(),
        span: span.into(),
    }
}
