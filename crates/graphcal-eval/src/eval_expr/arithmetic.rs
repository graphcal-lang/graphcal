use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::syntax::ast::{BinOp, Expr, UnaryOp};
use graphcal_compiler::syntax::names::{FieldName, StructTypeName};
use graphcal_compiler::syntax::span::Span;

use crate::error::GraphcalError;
use crate::runtime_value::RuntimeValue;

use super::EvalContext;
use super::eval_expr;

/// Evaluate a `BinOp` expression.
///
/// Dispatches to logical, equality, comparison, and arithmetic arms.
#[expect(
    clippy::too_many_lines,
    reason = "single match over all BinOp variants"
)]
pub(super) fn eval_binop_expr(
    expr: &Expr,
    op: &BinOp,
    lhs: &Expr,
    rhs: &Expr,
    values: &std::collections::HashMap<String, RuntimeValue>,
    local_values: &std::collections::HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match op {
        // Logical operators: Bool operands, Bool result
        BinOp::And => {
            let l = eval_expr(lhs, values, local_values, ctx)?
                .expect_bool("AND operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            let r = eval_expr(rhs, values, local_values, ctx)?
                .expect_bool("AND operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            Ok(RuntimeValue::Bool(l && r))
        }
        BinOp::Or => {
            let l = eval_expr(lhs, values, local_values, ctx)?
                .expect_bool("OR operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            let r = eval_expr(rhs, values, local_values, ctx)?
                .expect_bool("OR operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            Ok(RuntimeValue::Bool(l || r))
        }
        // Equality: compare same-typed value-level entities.
        BinOp::Eq | BinOp::Ne => {
            let l = eval_expr(lhs, values, local_values, ctx)?;
            let r = eval_expr(rhs, values, local_values, ctx)?;
            let is_eq = *op == BinOp::Eq;
            match (&l, &r) {
                (RuntimeValue::Bool(lb), RuntimeValue::Bool(rb)) => {
                    Ok(RuntimeValue::Bool(if is_eq { lb == rb } else { lb != rb }))
                }
                (RuntimeValue::Int(li), RuntimeValue::Int(ri)) => {
                    Ok(RuntimeValue::Bool(if is_eq { li == ri } else { li != ri }))
                }
                (
                    RuntimeValue::Label {
                        index_name: li,
                        variant: lv,
                    },
                    RuntimeValue::Label {
                        index_name: ri,
                        variant: rv,
                    },
                ) => {
                    let eq = li == ri && lv == rv;
                    Ok(RuntimeValue::Bool(if is_eq { eq } else { !eq }))
                }
                (RuntimeValue::Struct { .. }, RuntimeValue::Struct { .. }) => {
                    let eq = runtime_value_equals(&l, &r);
                    Ok(RuntimeValue::Bool(if is_eq { eq } else { !eq }))
                }
                (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => {
                    let eq = le == re;
                    Ok(RuntimeValue::Bool(if is_eq { eq } else { !eq }))
                }
                _ => {
                    let lv = l
                        .expect_scalar("comparison operand")
                        .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
                    let rv = r
                        .expect_scalar("comparison operand")
                        .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
                    Ok(RuntimeValue::Bool(eval_comparison(
                        *op, lv, rv, ctx.src, expr.span,
                    )?))
                }
            }
        }
        // Ordering comparisons: Int or Scalar operands, Bool result
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            let l = eval_expr(lhs, values, local_values, ctx)?;
            let r = eval_expr(rhs, values, local_values, ctx)?;
            if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                let result = match op {
                    BinOp::Lt => li < ri,
                    BinOp::Gt => li > ri,
                    BinOp::Le => li <= ri,
                    BinOp::Ge => li >= ri,
                    _ => {
                        return Err(ctx.internal_error(
                            format!("unexpected operator {op:?} in integer comparison"),
                            expr.span,
                        ));
                    }
                };
                return Ok(RuntimeValue::Bool(result));
            }
            if let (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) = (&l, &r) {
                let result = match op {
                    BinOp::Lt => le < re,
                    BinOp::Gt => le > re,
                    BinOp::Le => le <= re,
                    BinOp::Ge => le >= re,
                    _ => {
                        return Err(ctx.internal_error(
                            format!("unexpected operator {op:?} in datetime comparison"),
                            expr.span,
                        ));
                    }
                };
                return Ok(RuntimeValue::Bool(result));
            }
            let lv = l
                .expect_scalar("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            let rv = r
                .expect_scalar("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            Ok(RuntimeValue::Bool(eval_comparison(
                *op, lv, rv, ctx.src, expr.span,
            )?))
        }
        // Arithmetic operators: Int, Scalar, or derived struct operands
        _ => {
            let l = eval_expr(lhs, values, local_values, ctx)?;
            let r = eval_expr(rhs, values, local_values, ctx)?;
            if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                return Ok(RuntimeValue::Int(eval_int_binop(
                    *op, *li, *ri, ctx.src, expr.span,
                )?));
            }
            // Component-wise struct operations for derive(Add)/derive(Sub)
            if let (
                RuntimeValue::Struct {
                    type_name,
                    fields: lhs_fields,
                },
                RuntimeValue::Struct {
                    fields: rhs_fields, ..
                },
            ) = (&l, &r)
            {
                return eval_struct_binop(
                    *op, type_name, lhs_fields, rhs_fields, ctx.src, expr.span,
                );
            }
            // Datetime point-vs-vector arithmetic
            match (&l, &r) {
                (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => {
                    // Datetime - Datetime -> Scalar(Time in seconds)
                    if *op == BinOp::Sub {
                        return Ok(RuntimeValue::Scalar((*le - *re).to_seconds()));
                    }
                    return Err(ctx.eval_error("cannot add two datetimes", expr.span));
                }
                (RuntimeValue::Datetime(e), RuntimeValue::Scalar(secs)) => {
                    // Datetime +/- Scalar(Time) -> Datetime
                    let duration = hifitime::Duration::from_seconds(*secs);
                    return match op {
                        BinOp::Add => Ok(RuntimeValue::Datetime(*e + duration)),
                        BinOp::Sub => Ok(RuntimeValue::Datetime(*e - duration)),
                        _ => Err(ctx.eval_error(
                            format!("unsupported operator {op:?} for Datetime and scalar"),
                            expr.span,
                        )),
                    };
                }
                (RuntimeValue::Scalar(secs), RuntimeValue::Datetime(e)) => {
                    // Scalar(Time) + Datetime -> Datetime
                    if *op == BinOp::Add {
                        let duration = hifitime::Duration::from_seconds(*secs);
                        return Ok(RuntimeValue::Datetime(*e + duration));
                    }
                    return Err(ctx.eval_error(
                        "cannot subtract a Datetime from a scalar",
                        expr.span,
                    ));
                }
                _ => {}
            }
            let lv = l
                .expect_scalar("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            let rv = r
                .expect_scalar("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            Ok(RuntimeValue::Scalar(eval_scalar_binop(
                *op, lv, rv, ctx.src, expr.span,
            )?))
        }
    }
}

/// Evaluate a `UnaryOp` expression.
pub(super) fn eval_unaryop_expr(
    expr: &Expr,
    op: &UnaryOp,
    operand: &Expr,
    values: &std::collections::HashMap<String, RuntimeValue>,
    local_values: &std::collections::HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match op {
        UnaryOp::Neg => {
            let v = eval_expr(operand, values, local_values, ctx)?;
            match v {
                RuntimeValue::Int(i) => {
                    let negated = i
                        .checked_neg()
                        .ok_or_else(|| ctx.eval_error("integer negation overflow", expr.span))?;
                    Ok(RuntimeValue::Int(negated))
                }
                RuntimeValue::Struct { type_name, fields } => {
                    eval_struct_neg(&type_name, &fields, ctx.src, expr.span)
                }
                _ => Ok(RuntimeValue::Scalar(
                    -v.expect_scalar("unary negation")
                        .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?,
                )),
            }
        }
        UnaryOp::Not => {
            let v = eval_expr(operand, values, local_values, ctx)?
                .expect_bool("logical NOT")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            Ok(RuntimeValue::Bool(!v))
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

pub(super) fn runtime_value_equals(lhs: &RuntimeValue, rhs: &RuntimeValue) -> bool {
    match (lhs, rhs) {
        (
            RuntimeValue::Struct {
                type_name: lt,
                fields: lf,
            },
            RuntimeValue::Struct {
                type_name: rt,
                fields: rf,
            },
        ) => {
            lt == rt
                && lf.len() == rf.len()
                && lf
                    .iter()
                    .all(|(k, lvf)| rf.get(k).is_some_and(|rvf| runtime_value_equals(lvf, rvf)))
        }
        _ => false,
    }
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
pub(super) fn check_finite(
    value: f64,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, GraphcalError> {
    if value.is_nan() {
        Err(GraphcalError::EvalError {
            message: format!("invalid argument for {context} (result is NaN)"),
            src: src.clone(),
            span: span.into(),
        })
    } else if value.is_infinite() {
        Err(GraphcalError::EvalError {
            message: format!("{context} produced infinite result"),
            src: src.clone(),
            span: span.into(),
        })
    } else {
        Ok(value)
    }
}

/// Evaluate a comparison operator on two f64 values.
#[expect(clippy::float_cmp, reason = "DSL equality uses exact comparison")]
fn eval_comparison(
    op: BinOp,
    l: f64,
    r: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<bool, GraphcalError> {
    match op {
        BinOp::Eq => Ok(l == r),
        BinOp::Ne => Ok(l != r),
        BinOp::Lt => Ok(l < r),
        BinOp::Gt => Ok(l > r),
        BinOp::Le => Ok(l <= r),
        BinOp::Ge => Ok(l >= r),
        _ => Err(GraphcalError::InternalError {
            message: format!("unexpected operator {op:?} in comparison"),
            src: src.clone(),
            span: span.into(),
        }),
    }
}

/// Evaluate an arithmetic binary operator on two i64 values with checked arithmetic.
fn eval_int_binop(
    op: BinOp,
    l: i64,
    r: i64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<i64, GraphcalError> {
    match op {
        BinOp::Add => l.checked_add(r),
        BinOp::Sub => l.checked_sub(r),
        BinOp::Mul => l.checked_mul(r),
        BinOp::Div => {
            if r == 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l.checked_div(r)
        }
        BinOp::Mod => {
            if r == 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer modulo by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l.checked_rem(r)
        }
        BinOp::Pow => {
            if r < 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer exponent must be non-negative".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let exp = u32::try_from(r).map_err(|_| GraphcalError::EvalError {
                message: "integer exponent too large".to_string(),
                src: src.clone(),
                span: span.into(),
            })?;
            l.checked_pow(exp)
        }
        _ => {
            return Err(GraphcalError::InternalError {
                message: format!("unexpected operator {op:?} in integer arithmetic"),
                src: src.clone(),
                span: span.into(),
            });
        }
    }
    .ok_or_else(|| GraphcalError::EvalError {
        message: "integer arithmetic overflow".to_string(),
        src: src.clone(),
        span: span.into(),
    })
}

/// Evaluate an arithmetic binary operator on two f64 values.
fn eval_scalar_binop(
    op: BinOp,
    l: f64,
    r: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, GraphcalError> {
    let result = match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => {
            if r == 0.0 {
                return Err(GraphcalError::EvalError {
                    message: "division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l / r
        }
        BinOp::Pow => l.powf(r),
        _ => {
            return Err(GraphcalError::InternalError {
                message: format!("unexpected operator {op:?} in arithmetic"),
                src: src.clone(),
                span: span.into(),
            });
        }
    };

    // Post-check: if inputs were finite but result is not, report an error.
    if l.is_finite() && r.is_finite() && !result.is_finite() {
        if result.is_nan() {
            Err(GraphcalError::EvalError {
                message: "invalid arithmetic operation (result is NaN)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        } else {
            Err(GraphcalError::EvalError {
                message: "arithmetic overflow (result is infinite)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        }
    } else {
        Ok(result)
    }
}

/// Component-wise binary operation on two struct values (for derive(Add)/derive(Sub)).
/// Type checking has already verified that both operands are the same struct type
/// and that the struct derives the appropriate operator.
fn eval_struct_binop(
    op: BinOp,
    type_name: &StructTypeName,
    lhs_fields: &IndexMap<FieldName, RuntimeValue>,
    rhs_fields: &IndexMap<FieldName, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let mut result_fields = IndexMap::with_capacity(lhs_fields.len());
    for (field_name, lhs_val) in lhs_fields {
        let rhs_val = &rhs_fields[field_name];
        let result_val = match (lhs_val, rhs_val) {
            (RuntimeValue::Scalar(l), RuntimeValue::Scalar(r)) => {
                RuntimeValue::Scalar(eval_scalar_binop(op, *l, *r, src, span)?)
            }
            (RuntimeValue::Int(l), RuntimeValue::Int(r)) => {
                RuntimeValue::Int(eval_int_binop(op, *l, *r, src, span)?)
            }
            _ => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "field `{field_name}` of struct `{type_name}` has unsupported type for component-wise operation"
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            }
        };
        result_fields.insert(field_name.clone(), result_val);
    }
    Ok(RuntimeValue::Struct {
        type_name: type_name.clone(),
        fields: result_fields,
    })
}

/// Component-wise negation of a struct value (for derive(Neg)).
fn eval_struct_neg(
    type_name: &StructTypeName,
    fields: &IndexMap<FieldName, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let mut result_fields = IndexMap::with_capacity(fields.len());
    for (field_name, val) in fields {
        let result_val = match val {
            RuntimeValue::Scalar(v) => RuntimeValue::Scalar(-v),
            RuntimeValue::Int(i) => {
                RuntimeValue::Int(i.checked_neg().ok_or_else(|| GraphcalError::EvalError {
                    message: "integer negation overflow".to_string(),
                    src: src.clone(),
                    span: span.into(),
                })?)
            }
            _ => {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "field `{field_name}` of struct `{type_name}` has unsupported type for component-wise negation"
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            }
        };
        result_fields.insert(field_name.clone(), result_val);
    }
    Ok(RuntimeValue::Struct {
        type_name: type_name.clone(),
        fields: result_fields,
    })
}
