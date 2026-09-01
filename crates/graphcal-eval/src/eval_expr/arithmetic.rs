use graphcal_compiler::desugar::desugared_ast::BinOp;
use graphcal_compiler::exact_rational::ExactRational;
use graphcal_compiler::registry::declared_type::StructTypeRef;
use graphcal_compiler::syntax::span::Span;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use super::EvalContext;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn struct_value_constructor_refs_equal(lhs: &StructTypeRef, rhs: &StructTypeRef) -> bool {
    lhs.matches_ref(rhs) && lhs.name().atom() == rhs.name().atom()
}

fn semantic_value_equals(lhs: &RuntimeValue, rhs: &RuntimeValue) -> bool {
    match lhs {
        #[expect(
            clippy::float_cmp,
            reason = "Graphcal equality uses exact IEEE quantity equality"
        )]
        RuntimeValue::Quantity(lhs) => {
            matches!(rhs, RuntimeValue::Quantity(rhs) if lhs == rhs)
        }
        RuntimeValue::Complex(lhs) => {
            matches!(rhs, RuntimeValue::Complex(rhs) if lhs == rhs)
        }
        RuntimeValue::Bool(lhs) => matches!(rhs, RuntimeValue::Bool(rhs) if lhs == rhs),
        RuntimeValue::Int(lhs) => matches!(rhs, RuntimeValue::Int(rhs) if lhs == rhs),
        RuntimeValue::Label {
            index_name: lhs_index,
            variant: lhs_variant,
        } => matches!(
            rhs,
            RuntimeValue::Label {
                index_name: rhs_index,
                variant: rhs_variant,
            } if lhs_index.matches_ref(rhs_index) && lhs_variant == rhs_variant
        ),
        RuntimeValue::Struct {
            type_name: lhs_type,
            generic_args: lhs_args,
            fields: lhs_fields,
        } => match rhs {
            RuntimeValue::Struct {
                type_name: rhs_type,
                generic_args: rhs_args,
                fields: rhs_fields,
            } => {
                struct_value_constructor_refs_equal(lhs_type, rhs_type)
                    && lhs_args == rhs_args
                    && lhs_fields.len() == rhs_fields.len()
                    && lhs_fields.iter().all(|(field, lhs_value)| {
                        rhs_fields
                            .get(field)
                            .is_some_and(|rhs_value| semantic_value_equals(lhs_value, rhs_value))
                    })
            }
            _ => false,
        },
        RuntimeValue::Indexed {
            index_name: lhs_index,
            entries: lhs_entries,
        } => match rhs {
            RuntimeValue::Indexed {
                index_name: rhs_index,
                entries: rhs_entries,
            } => {
                lhs_index.matches_ref(rhs_index)
                    && lhs_entries.len() == rhs_entries.len()
                    && lhs_entries.iter().zip(rhs_entries).all(
                        |((lhs_key, lhs_value), (rhs_key, rhs_value))| {
                            lhs_key == rhs_key && semantic_value_equals(lhs_value, rhs_value)
                        },
                    )
            }
            _ => false,
        },
        RuntimeValue::CoordinateLabel {
            index_name: lhs_index,
            position: lhs_position,
            ..
        } => matches!(
            rhs,
            RuntimeValue::CoordinateLabel {
                index_name: rhs_index,
                position: rhs_position,
                ..
            } if lhs_index.matches_ref(rhs_index) && lhs_position == rhs_position
        ),
        RuntimeValue::Datetime(lhs) => {
            matches!(rhs, RuntimeValue::Datetime(rhs) if lhs == rhs)
        }
    }
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
pub(super) fn check_finite(
    value: f64,
    context: &str,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<f64, GraphcalError> {
    super::numeric::computed_finite_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn check_nonzero(
    value: f64,
    context: &str,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<f64, GraphcalError> {
    super::numeric::computed_nonzero_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// Evaluate equality for same-typed, unindexed values.
///
/// Mismatched operand types are evaluation errors. Indexed values reaching
/// this function violate the dimension checker's no-broadcasting invariant.
pub(super) fn eval_equality_values(
    op: BinOp,
    l: &RuntimeValue,
    r: &RuntimeValue,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    let is_eq = op == BinOp::Eq;
    let eq = match (l, r) {
        (RuntimeValue::Indexed { .. }, _) | (_, RuntimeValue::Indexed { .. }) => {
            return Err(ctx.internal_error("indexed operand reached comparison evaluation", span));
        }
        (RuntimeValue::Bool(_), RuntimeValue::Bool(_))
        | (RuntimeValue::Int(_), RuntimeValue::Int(_))
        | (RuntimeValue::Complex(_), RuntimeValue::Complex(_))
        | (RuntimeValue::Label { .. }, RuntimeValue::Label { .. })
        | (RuntimeValue::Struct { .. }, RuntimeValue::Struct { .. })
        | (RuntimeValue::CoordinateLabel { .. }, RuntimeValue::CoordinateLabel { .. })
        | (RuntimeValue::Datetime(_), RuntimeValue::Datetime(_)) => semantic_value_equals(l, r),
        _ => {
            let lv = l
                .expect_quantity("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_quantity("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            return Ok(RuntimeValue::Bool(eval_comparison(op, lv, rv, ctx, span)?));
        }
    };
    Ok(RuntimeValue::Bool(eq == is_eq))
}

/// Evaluate ordering for unindexed Int, Datetime, or Quantity operands.
///
/// The typed [`OrderingOp`] subset avoids an "impossible operator" fallback.
/// Indexed values reaching this function violate the dimension checker's
/// no-broadcasting invariant.
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
        (RuntimeValue::Indexed { .. }, _) | (_, RuntimeValue::Indexed { .. }) => {
            Err(ctx.internal_error("indexed operand reached comparison evaluation", span))
        }
        (RuntimeValue::Int(li), RuntimeValue::Int(ri)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, li, ri)))
        }
        (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, le, re)))
        }
        _ => {
            let lv = l
                .expect_quantity("comparison operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_quantity("comparison operand")
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
            // The mathematical remainder is zero for every dividend when the
            // divisor is -1. `checked_rem` nevertheless returns `None` for
            // `i64::MIN % -1` because the corresponding machine instruction
            // traps, so handle this total case explicitly.
            if r == -1 { Some(0) } else { l.checked_rem(r) }
        }
        BinOp::Pow(_) => {
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

/// Evaluate an exact rational power without discarding the rational metadata.
///
/// Integer exponents use `powi` when possible. Negative bases have a real
/// result exactly when the reduced denominator is odd; preserving the exact
/// denominator makes that rule deterministic instead of relying on `powf`'s
/// treatment of a rounded exponent.
pub(super) fn eval_exact_quantity_power(
    base: f64,
    exponent: ExactRational,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<f64, GraphcalError> {
    let result = exponent
        .pow_f64(base)
        .map_err(|error| ctx.eval_error(error.to_string(), span))?;
    if base == 0.0 {
        check_finite(result, "power operation", ctx, span)
    } else {
        check_nonzero(result, "power operation", ctx, span)
    }
}

/// Evaluate an arithmetic binary operator on two f64 values.
///
/// The result must be finite. (The evaluators previously diverged here:
/// this path only rejected non-finite results when the *inputs* were
/// finite, which could mask an upstream non-finite value — the strict
/// policy wins, matching every value-construction site.)
pub(super) fn eval_quantity_binop(
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
        BinOp::Pow(_) => l.powf(r),
        _ => {
            return Err(
                ctx.internal_error(format!("unexpected operator {op:?} in arithmetic"), span)
            );
        }
    };
    if matches!(op, BinOp::Mul | BinOp::Div | BinOp::Pow(_)) && l != 0.0 && r != 0.0 {
        check_nonzero(result, "arithmetic operation", ctx, span)
    } else {
        check_finite(result, "arithmetic operation", ctx, span)
    }
}
