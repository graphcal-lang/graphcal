//! Type inference for scalar operations: BinOp, UnaryOp, Convert, DisplayTimezone.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{BinOp, Expr, ExprKind};
use crate::syntax::ast::UnaryOp;
use crate::syntax::names::ScopedName;

use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;

use super::super::helpers::{expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// A compile-time-known numeric exponent extracted from an `^` rhs.
///
/// Captures both the bare-literal forms (`2`, `2.0`) and a leading unary `-`
/// applied to a literal (`-2`, `-2.0`) — the parser builds the negated form as
/// `UnaryOp(Neg, IntLit/Number)`, but for D005 purposes both shapes denote
/// the same compile-time constant.
use super::rules::{self, LiteralExponent, Operand};

fn literal_exponent(expr: &Expr) -> Option<LiteralExponent> {
    match &expr.kind {
        ExprKind::Integer(n) => Some(LiteralExponent::Int(*n)),
        ExprKind::Number(n) => Some(LiteralExponent::Float(*n)),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Integer(n) => Some(LiteralExponent::Int(n.wrapping_neg())),
            ExprKind::Number(n) => Some(LiteralExponent::Float(-*n)),
            _ => None,
        },
        _ => None,
    }
}

/// Constant-fold an `Int`-valued exponent expression to an `i64`.
///
/// Symmetrizes the `^` chain behavior between Int and Float: `2.0 ^ 3.0 ^ 2.0`
/// already type-checks because the inner Pow infers to `Scalar(Dimensionless)`,
/// but the Int branch of the dim-check only inspects the rhs's syntactic shape
/// and so rejected `2 ^ 3 ^ 2` as a non-literal exponent (issue #578).
///
/// Mirrors the runtime rules in `eval_int_binop` (checked arithmetic, no
/// negative `^` exponents, no integer overflow), so a fold succeeding here is
/// sufficient to guarantee the runtime evaluation also succeeds with the same
/// value.
fn try_const_int(expr: &Expr) -> Option<i64> {
    match &expr.kind {
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
                BinOp::Mod if r != 0 => l.checked_rem(r),
                BinOp::Pow if r >= 0 => u32::try_from(r).ok().and_then(|e| l.checked_pow(e)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Infer the type of a binary operation expression.
///
/// Operand types come from this engine's walker; the typing rule itself is
/// shared with the HIR engine via [`rules::binop_rule`].
#[expect(clippy::too_many_arguments, reason = "binary expression context")]
pub(super) fn infer_binop(
    expr: &Expr,
    op: &BinOp,
    lhs: &Expr,
    rhs: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let lhs_type = infer_type(
        lhs,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let rhs_type = infer_type(
        rhs,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // Only the `^` rule reads the exponent shape; `x ^ -2` is structurally
    // `Unary(Neg, IntLit(2))`, which is still compile-time-known.
    let (rhs_lit, rhs_const_int) = if matches!(op, BinOp::Pow) {
        (literal_exponent(rhs), try_const_int(rhs))
    } else {
        (None, None)
    };
    rules::binop_rule(
        expr.span,
        *op,
        &Operand {
            ty: lhs_type,
            span: lhs.span,
        },
        &Operand {
            ty: rhs_type,
            span: rhs.span,
        },
        rhs_lit,
        rhs_const_int,
        registry,
        src,
    )
}

/// Infer the type of a unary operation expression.
pub(super) fn infer_unary(
    op: &crate::desugar::resolved_ast::UnaryOp,
    operand: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let operand_type = infer_type(
        operand,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    rules::unary_rule(
        *op,
        &Operand {
            ty: operand_type,
            span: operand.span,
        },
        registry,
        src,
    )
}

/// Infer the type of a unit conversion expression.
pub(super) fn infer_convert(
    inner: &Expr,
    target: &crate::desugar::resolved_ast::UnitExpr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let expr_dim = expect_scalar(&inner_type, registry, src, inner.span)?;
    let target_dim = rules::resolve_unit_dimension_or_diagnose(target, registry, src)?;

    if expr_dim != target_dim {
        return Err(GraphcalError::ConversionDimensionMismatch {
            target: registry.dimensions.format_dimension(&target_dim),
            expr_dim: registry.dimensions.format_dimension(&expr_dim),
            src: src.clone(),
            span: target.span.into(),
        });
    }

    Ok(InferredType::Scalar(expr_dim))
}

/// Infer the type of a display timezone expression.
pub(super) fn infer_display_timezone(
    expr: &Expr,
    inner: &Expr,
    timezone: &str,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if !matches!(&inner_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&inner_type, registry),
            help: format!("timezone display `-> \"{timezone}\"` requires a Datetime expression"),
            src: src.clone(),
            span: inner.span.into(),
        });
    }
    // Validate timezone string is a valid IANA timezone
    if jiff::tz::TimeZone::get(timezone).is_err() {
        return Err(GraphcalError::InvalidTimezone {
            timezone: timezone.to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    Ok(inner_type)
}
