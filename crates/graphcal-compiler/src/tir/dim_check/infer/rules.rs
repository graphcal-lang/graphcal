//! Pure dimension/type rules shared by the syntax-AST and HIR inference
//! engines.
//!
//! Both engines walk different expression representations but must apply
//! identical typing rules. Keeping the rules here as pure functions over
//! [`InferredType`] operands means a rule change lands once — the engines
//! had already drifted (HIR accepted `-` on Bool) when each carried its own
//! copy.

use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{BinOp, UnaryOp, UnitExpr};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::{Dimension, Rational};
use crate::syntax::span::Span;

use super::super::InferredType;
use super::super::helpers::{expect_scalar, format_inferred_type};

/// A typed operand with the span diagnostics should point at.
pub(super) struct Operand {
    pub ty: InferredType,
    pub span: Span,
}

/// A compile-time-known exponent literal (possibly behind a unary minus).
#[derive(Clone, Copy)]
pub(super) enum LiteralExponent {
    Int(i64),
    Float(f64),
}

/// Typing rule for a binary operation, given already-inferred operands.
///
/// `rhs_lit` is the literal exponent of the right operand when it is
/// compile-time-known, and `rhs_const_int` is its constant-folded Int value
/// (for `Int ^ Int` chains, issue #578) — both computed by the calling
/// engine from its own expression representation; only the `^` arm reads
/// them.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive match over all BinOp variants"
)]
pub(super) fn binop_rule(
    expr_span: Span,
    op: BinOp,
    lhs: &Operand,
    rhs: &Operand,
    rhs_lit: Option<LiteralExponent>,
    rhs_const_int: Option<i64>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let lhs_type = &lhs.ty;
    let rhs_type = &rhs.ty;
    match op {
        // Logical operators: require Bool operands, return Bool
        BinOp::And | BinOp::Or => {
            if *lhs_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(lhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: lhs.span.into(),
                });
            }
            if *rhs_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(rhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        // Equality: operands must have the same ValueType.
        // Int and Fin(N) are compatible for equality comparison.
        BinOp::Eq | BinOp::Ne => {
            if lhs_type == rhs_type || (lhs_type.is_int_like() && rhs_type.is_int_like()) {
                return Ok(InferredType::Bool);
            }
            Err(GraphcalError::DimensionMismatch {
                expected: format_inferred_type(lhs_type, registry),
                found: format_inferred_type(rhs_type, registry),
                help: "equality operands must have the same type".to_string(),
                src: src.clone(),
                span: rhs.span.into(),
            })
        }
        // Ordering comparisons: require same-type scalar or Int/Fin operands, return Bool
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if lhs_type.is_int_like() || rhs_type.is_int_like() {
                if !lhs_type.is_int_like() || !rhs_type.is_int_like() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(lhs_type, registry),
                        found: format_inferred_type(rhs_type, registry),
                        help: "comparison operands must have the same type".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Bool);
            }
            // Datetime comparisons: same time scale required
            if let InferredType::Datetime(ls) = lhs_type
                && let InferredType::Datetime(rs) = rhs_type
            {
                if ls != rs {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(lhs_type, registry),
                        found: format_inferred_type(rhs_type, registry),
                        help: "cannot compare datetimes with different time scales".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Bool);
            }
            let lhs_dim = expect_scalar(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "comparison operands must have the same dimension".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        // Arithmetic operators: require matching numeric operands (Int or Scalar)
        BinOp::Add | BinOp::Sub => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            // Point-vs-vector rules for Datetime
            if let InferredType::Datetime(ls) = lhs_type {
                let time_dim = Dimension::base(crate::syntax::dimension::BaseDimId::Prelude(
                    "Time".to_string(),
                ));
                if let InferredType::Datetime(rs) = rhs_type {
                    // Datetime - Datetime -> Scalar(Time)
                    if op == BinOp::Sub {
                        if ls != rs {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format_inferred_type(lhs_type, registry),
                                found: format_inferred_type(rhs_type, registry),
                                help: "cannot subtract datetimes with different time scales"
                                    .to_string(),
                                src: src.clone(),
                                span: rhs.span.into(),
                            });
                        }
                        return Ok(InferredType::Scalar(time_dim));
                    }
                    // Datetime + Datetime -> error
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Scalar(Time)".to_string(),
                        found: format_inferred_type(rhs_type, registry),
                        help: "cannot add two datetimes; did you mean to subtract?".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                // Datetime +/- Scalar(Time) -> Datetime
                let rhs_dim = expect_scalar(rhs_type, registry, src, rhs.span)?;
                if rhs_dim != time_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Time".to_string(),
                        found: registry.dimensions.format_dimension(&rhs_dim),
                        help: "can only add/subtract a Time duration to/from a Datetime"
                            .to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Datetime(*ls));
            }
            if let InferredType::Datetime(rs) = rhs_type {
                // Scalar(Time) + Datetime -> Datetime (only for Add)
                if op == BinOp::Add {
                    let time_dim = Dimension::base(crate::syntax::dimension::BaseDimId::Prelude(
                        "Time".to_string(),
                    ));
                    let lhs_dim = expect_scalar(lhs_type, registry, src, lhs.span)?;
                    if lhs_dim != time_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Time".to_string(),
                            found: registry.dimensions.format_dimension(&lhs_dim),
                            help: "can only add a Time duration to a Datetime".to_string(),
                            src: src.clone(),
                            span: lhs.span.into(),
                        });
                    }
                    return Ok(InferredType::Datetime(*rs));
                }
                // Scalar - Datetime -> error
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(lhs_type, registry),
                    found: format_inferred_type(rhs_type, registry),
                    help: "cannot subtract a Datetime from a scalar".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            let lhs_dim = expect_scalar(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "operands of addition and subtraction must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Scalar(lhs_dim))
        }
        BinOp::Mul => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let lhs_dim = expect_scalar(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(rhs_type, registry, src, rhs.span)?;
            let dim = (lhs_dim * rhs_dim).map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: expr_span.into(),
            })?;
            Ok(InferredType::Scalar(dim))
        }
        BinOp::Div => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let lhs_dim = expect_scalar(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(rhs_type, registry, src, rhs.span)?;
            let dim = (lhs_dim / rhs_dim).map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: expr_span.into(),
            })?;
            Ok(InferredType::Scalar(dim))
        }
        BinOp::Mod => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            Err(GraphcalError::DimensionMismatch {
                expected: "Int".to_string(),
                found: format!(
                    "{} % {}",
                    format_inferred_type(lhs_type, registry),
                    format_inferred_type(rhs_type, registry)
                ),
                help: "modulo operator requires Int operands".to_string(),
                src: src.clone(),
                span: expr_span.into(),
            })
        }
        BinOp::Pow => {
            // Int/Fin ^ Int (literal or constant-foldable, non-negative) -> Int
            if lhs_type.is_int_like() {
                let int_exp = match rhs_lit {
                    Some(LiteralExponent::Int(n)) => Some(n),
                    // Constant-fold Int^Int chains so `2 ^ 3 ^ 2` symmetrizes
                    // with the de facto Float behavior (issue #578).
                    _ => rhs_const_int,
                };
                if let Some(n) = int_exp {
                    if n >= 0 {
                        return Ok(InferredType::Int);
                    }
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "non-negative Int exponent".to_string(),
                        found: format!("{n}"),
                        help: "integer power requires a non-negative exponent".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Err(GraphcalError::NonLiteralExponent {
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            // Scalar ^ literal exponent
            let lhs_dim = expect_scalar(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(rhs_type, registry, src, rhs.span)?;
            match rhs_lit {
                Some(LiteralExponent::Float(n)) => {
                    if n.fract() == 0.0 {
                        // `as i32` saturates for out-of-range floats, which
                        // would silently produce a wrong dimension exponent;
                        // reject instead.
                        if n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {
                            return Err(GraphcalError::DimensionOverflow {
                                src: src.clone(),
                                span: expr_span.into(),
                            });
                        }
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "guarded by fract() == 0.0 and range checks"
                        )]
                        let exp = n as i32;
                        let dim = lhs_dim.pow(Rational::from_int(exp)).map_err(|_| {
                            GraphcalError::DimensionOverflow {
                                src: src.clone(),
                                span: expr_span.into(),
                            }
                        })?;
                        Ok(InferredType::Scalar(dim))
                    } else {
                        #[expect(
                            clippy::float_cmp,
                            reason = "checking exact 0.5 literal for square-root exponent"
                        )]
                        if n == 0.5 {
                            let dim = lhs_dim.pow(Rational::HALF).map_err(|_| {
                                GraphcalError::DimensionOverflow {
                                    src: src.clone(),
                                    span: expr_span.into(),
                                }
                            })?;
                            Ok(InferredType::Scalar(dim))
                        } else {
                            Err(GraphcalError::NonLiteralExponent {
                                src: src.clone(),
                                span: rhs.span.into(),
                            })
                        }
                    }
                }
                Some(LiteralExponent::Int(n)) => {
                    // `as i32` would wrap: `x ^ 4294967296` (2^32) used to
                    // truncate to exponent 0 and silently infer Dimensionless.
                    let exp = i32::try_from(n).map_err(|_| GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: expr_span.into(),
                    })?;
                    let dim = lhs_dim.pow(Rational::from_int(exp)).map_err(|_| {
                        GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: expr_span.into(),
                        }
                    })?;
                    Ok(InferredType::Scalar(dim))
                }
                None => {
                    if rhs_dim.is_dimensionless() && lhs_dim.is_dimensionless() {
                        Ok(InferredType::Scalar(Dimension::dimensionless()))
                    } else {
                        Err(GraphcalError::NonLiteralExponent {
                            src: src.clone(),
                            span: rhs.span.into(),
                        })
                    }
                }
            }
        }
    }
}

/// Typing rule for a unary operation, given the already-inferred operand.
pub(super) fn unary_rule(
    op: UnaryOp,
    operand: &Operand,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match op {
        UnaryOp::Not => {
            if operand.ty != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&operand.ty, registry),
                    help: "logical NOT requires a Bool operand".to_string(),
                    src: src.clone(),
                    span: operand.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        UnaryOp::Neg => {
            if operand.ty == InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "numeric type".to_string(),
                    found: "Bool".to_string(),
                    help: "negation requires a numeric operand, not Bool".to_string(),
                    src: src.clone(),
                    span: operand.span.into(),
                });
            }
            // Negation preserves the type (Scalar or Int)
            Ok(operand.ty.clone())
        }
    }
}

/// Typing rule for an `if`/`else` expression, given inferred parts.
pub(super) fn if_rule(
    cond: &Operand,
    then_branch: &Operand,
    else_branch: &Operand,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if cond.ty != InferredType::Bool {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Bool".to_string(),
            found: format_inferred_type(&cond.ty, registry),
            help: "if/else condition must be Bool".to_string(),
            src: src.clone(),
            span: cond.span.into(),
        });
    }
    if then_branch.ty != else_branch.ty {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&then_branch.ty, registry),
            found: format_inferred_type(&else_branch.ty, registry),
            help: "both branches of if/else must have the same dimension".to_string(),
            src: src.clone(),
            span: else_branch.span.into(),
        });
    }
    Ok(then_branch.ty.clone())
}

/// Resolve a unit expression's dimension, with a precise diagnostic
/// pointing at the first unknown unit term (previously copied at four
/// sites across both engines).
pub(super) fn resolve_unit_dimension_or_diagnose(
    unit: &UnitExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    use crate::registry::types::UnitResolveError;
    registry
        .units
        .resolve_unit_dimension(unit)
        .map_err(|err| match err {
            UnitResolveError::UnknownUnit(name) => {
                // Point at the failing term's own span when we can find it.
                let span = unit
                    .terms
                    .iter()
                    .find(|item| item.name.value == name)
                    .map_or(unit.span, |item| item.name.span);
                GraphcalError::UnknownUnit {
                    name,
                    src: src.clone(),
                    span: span.into(),
                }
            }
            UnitResolveError::DynamicScale(name) => GraphcalError::EvalError {
                message: format!("unit `{name}` has a dynamic scale"),
                src: src.clone(),
                span: unit.span.into(),
            },
            UnitResolveError::Overflow(_) => GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: unit.span.into(),
            },
        })
}

/// Typing rule for `match` arms: all arms must have the same type, and at
/// least one arm must exist. `arm_body_span` maps an arm index to the span
/// of its body for diagnostics (the two engines carry different arm types).
pub(in crate::tir::dim_check) fn match_arms_rule(
    arm_types: &[InferredType],
    arm_body_span: impl Fn(usize) -> Span,
    expr_span: Span,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(first) = arm_types.first() else {
        return Err(GraphcalError::EvalError {
            message: "match expression has no arms".to_string(),
            src: src.clone(),
            span: expr_span.into(),
        });
    };
    for (i, arm_type) in arm_types.iter().enumerate().skip(1) {
        if arm_type != first {
            return Err(GraphcalError::DimensionMismatch {
                expected: format_inferred_type(first, registry),
                found: format_inferred_type(arm_type, registry),
                help: "all match arms must return the same type".to_string(),
                src: src.clone(),
                span: arm_body_span(i).into(),
            });
        }
    }
    Ok(first.clone())
}
