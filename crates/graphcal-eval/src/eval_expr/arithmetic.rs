use graphcal_compiler::desugar::desugared_ast::BinOp;
use graphcal_compiler::exact_rational::ExactRational;
use graphcal_compiler::finite_value::{FiniteArithmeticError, FiniteQuantity};
use graphcal_compiler::syntax::span::Span;

use crate::runtime_value::RuntimeValue;
use graphcal_compiler::registry::error::GraphcalError;

use super::EvalContext;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::type_name::{ConstructorName, StructTypeName};

    #[test]
    fn nominal_equality_compares_owner_and_constructor_independently() {
        let value = |module, constructor| {
            RuntimeValue::Struct(crate::runtime_value::StructValue::for_test(
                graphcal_compiler::resolved_name::ResolvedStructTypeName::for_test(
                    DagId::root_in_package("test", module),
                    StructTypeName::expect_valid("Phase"),
                ),
                ConstructorName::expect_valid(constructor),
                indexmap::IndexMap::new(),
            ))
        };
        let idle = value("main", "Idle");
        assert_eq!(idle, value("main", "Idle"));
        assert_ne!(idle, value("main", "Running"));
        assert_ne!(idle, value("other", "Idle"));
    }

    #[test]
    fn orderings_follow_the_operator() {
        let cases = [
            (OrderingOp::Lt, [true, false, false]),
            (OrderingOp::Gt, [false, false, true]),
            (OrderingOp::Le, [true, true, false]),
            (OrderingOp::Ge, [false, true, true]),
        ];
        for (op, expected) in cases {
            let actual = [(1, 2), (2, 2), (3, 2)].map(|(lhs, rhs)| apply_ordering(op, &lhs, &rhs));
            assert_eq!(actual, expected, "{op:?}");
        }
    }
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
pub(super) fn check_finite(
    value: f64,
    context: &str,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    super::numeric::computed_finite_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn check_nonzero(
    value: f64,
    context: &str,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    super::numeric::computed_nonzero_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// A comparison operator: equality, inequality, or one of the four orderings.
///
/// The evaluator narrows [`BinOp`] to this type at its dispatch site, so the
/// comparison helpers match exhaustively instead of rejecting "impossible"
/// operators at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Comparison {
    Eq,
    Ne,
    Ord(OrderingOp),
}

/// The four ordering comparison operators (`<`, `>`, `<=`, `>=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OrderingOp {
    Lt,
    Gt,
    Le,
    Ge,
}

/// Evaluate a comparison on same-typed, unindexed values.
///
/// Equality accepts every unindexed value kind and is the values' structural
/// equality; ordering accepts Int, Datetime, and Quantity operands. Mismatched
/// operand types are evaluation errors. Indexed values reaching this function
/// violate the dimension checker's no-broadcasting invariant.
pub(super) fn eval_comparison_values(
    op: Comparison,
    l: &RuntimeValue,
    r: &RuntimeValue,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    match (op, l, r) {
        (_, RuntimeValue::Indexed(_), _) | (_, _, RuntimeValue::Indexed(_)) => {
            Err(ctx.internal_error("indexed operand reached comparison evaluation", span))
        }
        (Comparison::Eq | Comparison::Ne, RuntimeValue::Quantity(_), RuntimeValue::Quantity(_))
        | (Comparison::Eq | Comparison::Ne, RuntimeValue::Complex(_), RuntimeValue::Complex(_))
        | (Comparison::Eq | Comparison::Ne, RuntimeValue::Bool(_), RuntimeValue::Bool(_))
        | (Comparison::Eq | Comparison::Ne, RuntimeValue::Int(_), RuntimeValue::Int(_))
        | (Comparison::Eq | Comparison::Ne, RuntimeValue::Key(_), RuntimeValue::Key(_))
        | (Comparison::Eq | Comparison::Ne, RuntimeValue::Struct(_), RuntimeValue::Struct(_))
        | (Comparison::Eq | Comparison::Ne, RuntimeValue::Datetime(_), RuntimeValue::Datetime(_)) => {
            Ok(RuntimeValue::Bool((l == r) == (op == Comparison::Eq)))
        }
        (Comparison::Ord(ord_op), RuntimeValue::Quantity(lq), RuntimeValue::Quantity(rq)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, lq, rq)))
        }
        (Comparison::Ord(ord_op), RuntimeValue::Int(li), RuntimeValue::Int(ri)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, li, ri)))
        }
        (Comparison::Ord(ord_op), RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) => {
            Ok(RuntimeValue::Bool(apply_ordering(ord_op, le, re)))
        }
        _ => Err(ctx.eval_error(
            format!("cannot compare {} with {}", l.describe(), r.describe()),
            span,
        )),
    }
}

/// Dispatch an ordering operator (`<`, `>`, `<=`, `>=`) to the
/// `PartialOrd` comparison on any two homogeneous operands.
fn apply_ordering<T: PartialOrd + ?Sized>(op: OrderingOp, lhs: &T, rhs: &T) -> bool {
    match op {
        OrderingOp::Lt => lhs < rhs,
        OrderingOp::Gt => lhs > rhs,
        OrderingOp::Le => lhs <= rhs,
        OrderingOp::Ge => lhs >= rhs,
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
    base: FiniteQuantity,
    exponent: ExactRational,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    let result = exponent
        .pow_f64(base.get())
        .map_err(|error| ctx.eval_error(error.to_string(), span))?;
    if base.get() == 0.0 {
        check_finite(result, "power operation", ctx, span)
    } else {
        check_nonzero(result, "power operation", ctx, span)
    }
}

/// Evaluate an arithmetic binary operator on two finite quantities.
///
/// Sums, differences, products, and quotients use the checked finite
/// arithmetic of [`FiniteQuantity`]; a power of nonzero operands must also
/// not underflow to zero.
pub(super) fn eval_quantity_binop(
    op: BinOp,
    l: FiniteQuantity,
    r: FiniteQuantity,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    let context = "arithmetic operation";
    let result = match op {
        BinOp::Add => l.checked_add(r),
        BinOp::Sub => l.checked_sub(r),
        BinOp::Mul => l.checked_mul(r),
        BinOp::Div => l.checked_div(r),
        BinOp::Pow(_) => {
            let result = l.get().powf(r.get());
            return if l.get() != 0.0 && r.get() != 0.0 {
                check_nonzero(result, context, ctx, span)
            } else {
                check_finite(result, context, ctx, span)
            };
        }
        _ => {
            return Err(
                ctx.internal_error(format!("unexpected operator {op:?} in arithmetic"), span)
            );
        }
    };
    result.map_err(|error| {
        let message = match error {
            FiniteArithmeticError::DivisionByZero => "division by zero".to_string(),
            FiniteArithmeticError::Infinite => {
                super::numeric::QuantityValidationError::InfiniteResult {
                    context: context.to_string(),
                }
                .to_string()
            }
            FiniteArithmeticError::Underflow => {
                super::numeric::QuantityValidationError::UnderflowToZero {
                    context: context.to_string(),
                }
                .to_string()
            }
        };
        ctx.eval_error(message, span)
    })
}
