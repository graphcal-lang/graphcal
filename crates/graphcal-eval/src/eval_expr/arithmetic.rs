use graphcal_compiler::exact_rational::ExactRational;
use graphcal_compiler::finite_value::{FiniteArithmeticError, FiniteQuantity};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::operators::{ArithOp, IntArithOp, OrderingOp};

use super::EvalSession;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
pub(super) fn check_finite(
    value: f64,
    context: &str,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    super::numeric::computed_finite_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn check_nonzero(
    value: f64,
    context: &str,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    super::numeric::computed_nonzero_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// Dispatch an ordering operator (`<`, `>`, `<=`, `>=`) to the
/// `PartialOrd` comparison on any two homogeneous operands.
pub(super) fn apply_ordering<T: PartialOrd + ?Sized>(op: OrderingOp, lhs: &T, rhs: &T) -> bool {
    match op {
        OrderingOp::Lt => lhs < rhs,
        OrderingOp::Gt => lhs > rhs,
        OrderingOp::Le => lhs <= rhs,
        OrderingOp::Ge => lhs >= rhs,
    }
}

/// Evaluate an integer arithmetic operator with checked arithmetic.
pub(super) fn int_arith(
    op: IntArithOp,
    l: i64,
    r: i64,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<i64, GraphcalError> {
    match op {
        IntArithOp::Add => l.checked_add(r),
        IntArithOp::Sub => l.checked_sub(r),
        IntArithOp::Mul => l.checked_mul(r),
        IntArithOp::Div => {
            if r == 0 {
                return Err(ctx.eval_error("integer division by zero", span));
            }
            l.checked_div(r)
        }
        IntArithOp::Mod => {
            if r == 0 {
                return Err(ctx.eval_error("integer modulo by zero", span));
            }
            // The mathematical remainder is zero for every dividend when the
            // divisor is -1. `checked_rem` nevertheless returns `None` for
            // `i64::MIN % -1` because the corresponding machine instruction
            // traps, so handle this total case explicitly.
            if r == -1 { Some(0) } else { l.checked_rem(r) }
        }
    }
    .ok_or_else(|| ctx.eval_error("integer arithmetic overflow", span))
}

/// Raise an integer to an integer power with checked arithmetic.
pub(super) fn int_power(
    base: i64,
    exponent: i64,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<i64, GraphcalError> {
    if exponent < 0 {
        return Err(ctx.eval_error("integer exponent must be non-negative", span));
    }
    let exponent =
        u32::try_from(exponent).map_err(|_| ctx.eval_error("integer exponent too large", span))?;
    base.checked_pow(exponent)
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
    ctx: &EvalSession<'_>,
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

/// Evaluate an arithmetic operator on two finite quantities with the
/// checked finite arithmetic of [`FiniteQuantity`].
pub(super) fn quantity_arith(
    op: ArithOp,
    l: FiniteQuantity,
    r: FiniteQuantity,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    let context = "arithmetic operation";
    match op {
        ArithOp::Add => l.checked_add(r),
        ArithOp::Sub => l.checked_sub(r),
        ArithOp::Mul => l.checked_mul(r),
        ArithOp::Div => l.checked_div(r),
    }
    .map_err(|error| {
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

/// Raise a dimensionless quantity to a runtime dimensionless exponent; a
/// power of nonzero operands must not underflow to zero.
pub(super) fn quantity_power(
    base: FiniteQuantity,
    exponent: FiniteQuantity,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, GraphcalError> {
    let context = "arithmetic operation";
    let result = base.get().powf(exponent.get());
    if base.get() != 0.0 && exponent.get() != 0.0 {
        check_nonzero(result, context, ctx, span)
    } else {
        check_finite(result, context, ctx, span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_value::RuntimeValue;
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
