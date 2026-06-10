use graphcal_compiler::desugar::resolved_ast::{BinOp, Expr, UnaryOp};
use graphcal_compiler::registry::declared_type::StructTypeRef;
use graphcal_compiler::syntax::span::Span;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use super::EvalContext;
use super::RuntimeValueMap;
use super::eval_expr;

/// Evaluate a `BinOp` expression.
///
/// Dispatches to logical, equality, comparison, and arithmetic arms.
pub(super) fn eval_binop_expr(
    expr: &Expr,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    values: &RuntimeValueMap,
    local_values: &std::collections::HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match op {
        // Logical AND/OR: both operands are always evaluated (no short-circuit).
        // This is intentional — in a reactive calculation graph, every sub-expression
        // should be valid regardless of control flow. Silently skipping a broken operand
        // via short-circuit would hide errors. Users who need conditional evaluation
        // should use `if-then-else` expressions instead.
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
            eval_equality_values(op, &l, &r, ctx, expr.span)
        }
        // Ordering comparisons: Int, Datetime, or Scalar operands, Bool result.
        // Int/Int and Datetime/Datetime dispatch identically via `Ord`; anything
        // else falls through to the scalar path which handles Scalar/Int
        // mixing and dimension checks.
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            let l = eval_expr(lhs, values, local_values, ctx)?;
            let r = eval_expr(rhs, values, local_values, ctx)?;
            eval_ordering_values(op, &l, &r, ctx, expr.span)
        }
        // Arithmetic operators: Int or Scalar operands
        _ => {
            let l = eval_expr(lhs, values, local_values, ctx)?;
            let r = eval_expr(rhs, values, local_values, ctx)?;
            if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                return Ok(RuntimeValue::Int(eval_int_binop(
                    op, *li, *ri, ctx, expr.span,
                )?));
            }
            // Datetime point-vs-vector arithmetic
            match (&l, &r) {
                (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => {
                    // Datetime - Datetime -> Scalar(Time in seconds)
                    if op == BinOp::Sub {
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
                    if op == BinOp::Add {
                        let duration = hifitime::Duration::from_seconds(*secs);
                        return Ok(RuntimeValue::Datetime(*e + duration));
                    }
                    return Err(
                        ctx.eval_error("cannot subtract a Datetime from a scalar", expr.span)
                    );
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
                op, lv, rv, ctx, expr.span,
            )?))
        }
    }
}

/// Evaluate a `UnaryOp` expression.
pub(super) fn eval_unaryop_expr(
    expr: &Expr,
    op: UnaryOp,
    operand: &Expr,
    values: &RuntimeValueMap,
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

fn struct_value_type_refs_equal(lhs: &StructTypeRef, rhs: &StructTypeRef) -> bool {
    lhs.matches_ref(rhs)
}

pub(super) fn runtime_value_equals(lhs: &RuntimeValue, rhs: &RuntimeValue) -> bool {
    match (lhs, rhs) {
        #[expect(
            clippy::float_cmp,
            reason = "Graphcal equality uses exact IEEE scalar equality"
        )]
        (RuntimeValue::Scalar(lhs), RuntimeValue::Scalar(rhs)) => lhs == rhs,
        (RuntimeValue::Bool(lhs), RuntimeValue::Bool(rhs)) => lhs == rhs,
        (RuntimeValue::Int(lhs), RuntimeValue::Int(rhs)) => lhs == rhs,
        (
            RuntimeValue::Label {
                index_name: lhs_index,
                variant: lhs_variant,
            },
            RuntimeValue::Label {
                index_name: rhs_index,
                variant: rhs_variant,
            },
        ) => lhs_index.matches_ref(rhs_index) && lhs_variant == rhs_variant,
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
            struct_value_type_refs_equal(lt, rt)
                && lf.len() == rf.len()
                && lf
                    .iter()
                    .all(|(k, lvf)| rf.get(k).is_some_and(|rvf| runtime_value_equals(lvf, rvf)))
        }
        (
            RuntimeValue::Indexed {
                index_name: lhs_index,
                entries: lhs_entries,
            },
            RuntimeValue::Indexed {
                index_name: rhs_index,
                entries: rhs_entries,
            },
        ) => {
            lhs_index.matches_ref(rhs_index)
                && lhs_entries.len() == rhs_entries.len()
                && lhs_entries.iter().all(|(variant, lhs_value)| {
                    rhs_entries
                        .get(variant)
                        .is_some_and(|rhs_value| runtime_value_equals(lhs_value, rhs_value))
                })
        }
        (RuntimeValue::Datetime(lhs), RuntimeValue::Datetime(rhs)) => lhs == rhs,
        _ => false,
    }
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
pub(super) fn check_finite(
    value: f64,
    context: &str,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<f64, GraphcalError> {
    super::numeric::computed_finite_scalar(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// Equality kernel shared by both evaluators: same-typed value-level
/// entities compare; mismatched operand types are an evaluation error.
/// (The HIR evaluator previously returned `false` for mismatched types via
/// a permissive structural comparison — the strict policy wins.)
pub(super) fn eval_equality_values(
    op: BinOp,
    l: &RuntimeValue,
    r: &RuntimeValue,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let is_eq = op == BinOp::Eq;
    let eq = match (l, r) {
        (RuntimeValue::Bool(lb), RuntimeValue::Bool(rb)) => lb == rb,
        (RuntimeValue::Int(li), RuntimeValue::Int(ri)) => li == ri,
        (
            RuntimeValue::Label {
                index_name: li,
                variant: lv,
            },
            RuntimeValue::Label {
                index_name: ri,
                variant: rv,
            },
        ) => li.matches_ref(ri) && lv == rv,
        (RuntimeValue::Struct { .. }, RuntimeValue::Struct { .. }) => runtime_value_equals(l, r),
        (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => le == re,
        _ => {
            let lv = l
                .expect_scalar("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_scalar("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            return Ok(RuntimeValue::Bool(eval_comparison(op, lv, rv, ctx, span)?));
        }
    };
    Ok(RuntimeValue::Bool(eq == is_eq))
}

/// Ordering kernel shared by both evaluators: Int, Datetime, or Scalar
/// operands, dispatched through the typed [`OrderingOp`] subset so there is
/// no "impossible operator" fallback.
pub(super) fn eval_ordering_values(
    op: BinOp,
    l: &RuntimeValue,
    r: &RuntimeValue,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let ord_op = OrderingOp::from_binop(op)
        .ok_or_else(|| ctx.internal_error(format!("non-ordering op {op:?}"), span))?;
    match (l, r) {
        (RuntimeValue::Int(li), RuntimeValue::Int(ri)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, li, ri)))
        }
        (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, le, re)))
        }
        _ => {
            let lv = l
                .expect_scalar("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_scalar("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(eval_comparison(op, lv, rv, ctx, span)?))
        }
    }
}

/// Restriction of [`BinOp`] to the four ordering comparison operators.
///
/// Carrying this typed subset lets [`apply_ordering`] dispatch without an
/// "impossible" arm — the type system forbids non-ordering ops at the call
/// site rather than checking at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderingOp {
    Lt,
    Gt,
    Le,
    Ge,
}

impl OrderingOp {
    /// Narrow a [`BinOp`] to an ordering operator, or `None` for the other variants.
    const fn from_binop(op: BinOp) -> Option<Self> {
        match op {
            BinOp::Lt => Some(Self::Lt),
            BinOp::Gt => Some(Self::Gt),
            BinOp::Le => Some(Self::Le),
            BinOp::Ge => Some(Self::Ge),
            _ => None,
        }
    }
}

/// Dispatch an ordering operator (`<`, `>`, `<=`, `>=`) to the `Ord`-derived
/// comparison on any two homogeneous operands.
fn apply_ordering<T: Ord + ?Sized>(op: OrderingOp, lhs: &T, rhs: &T) -> bool {
    match op {
        OrderingOp::Lt => lhs < rhs,
        OrderingOp::Gt => lhs > rhs,
        OrderingOp::Le => lhs <= rhs,
        OrderingOp::Ge => lhs >= rhs,
    }
}

/// Evaluate a comparison operator on two f64 values.
#[expect(clippy::float_cmp, reason = "DSL equality uses exact comparison")]
fn eval_comparison(
    op: BinOp,
    l: f64,
    r: f64,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<bool, GraphcalError> {
    match op {
        BinOp::Eq => Ok(l == r),
        BinOp::Ne => Ok(l != r),
        BinOp::Lt => Ok(l < r),
        BinOp::Gt => Ok(l > r),
        BinOp::Le => Ok(l <= r),
        BinOp::Ge => Ok(l >= r),
        _ => Err(ctx.internal_error(format!("unexpected operator {op:?} in comparison"), span)),
    }
}

/// Evaluate an arithmetic binary operator on two i64 values with checked arithmetic.
pub(super) fn eval_int_binop(
    op: BinOp,
    l: i64,
    r: i64,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<i64, GraphcalError> {
    match op {
        BinOp::Add => l.checked_add(r),
        BinOp::Sub => l.checked_sub(r),
        BinOp::Mul => l.checked_mul(r),
        BinOp::Div => {
            if r == 0 {
                return Err(ctx.eval_error("integer division by zero", span));
            }
            l.checked_div(r)
        }
        BinOp::Mod => {
            if r == 0 {
                return Err(ctx.eval_error("integer modulo by zero", span));
            }
            l.checked_rem(r)
        }
        BinOp::Pow => {
            if r < 0 {
                return Err(ctx.eval_error("integer exponent must be non-negative", span));
            }
            let exp =
                u32::try_from(r).map_err(|_| ctx.eval_error("integer exponent too large", span))?;
            l.checked_pow(exp)
        }
        _ => {
            return Err(ctx.internal_error(
                format!("unexpected operator {op:?} in integer arithmetic"),
                span,
            ));
        }
    }
    .ok_or_else(|| ctx.eval_error("integer arithmetic overflow", span))
}

/// Evaluate an arithmetic binary operator on two f64 values.
///
/// The result must be finite. (The evaluators previously diverged here:
/// this path only rejected non-finite results when the *inputs* were
/// finite, which could mask an upstream non-finite value — the strict
/// policy wins, matching every value-construction site.)
pub(super) fn eval_scalar_binop(
    op: BinOp,
    l: f64,
    r: f64,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<f64, GraphcalError> {
    let result = match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => {
            if r == 0.0 {
                return Err(ctx.eval_error("division by zero", span));
            }
            l / r
        }
        BinOp::Pow => l.powf(r),
        _ => {
            return Err(
                ctx.internal_error(format!("unexpected operator {op:?} in arithmetic"), span)
            );
        }
    };
    check_finite(result, "arithmetic operation", ctx, span)
}
