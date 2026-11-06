use graphcal_compiler::builtin::IntDivisionFn;
use graphcal_compiler::exact_rational::ExactRational;
use graphcal_compiler::finite_value::{ExactPowerFailure, FiniteArithmeticError, FiniteQuantity};
use graphcal_compiler::semantic::int_arith;
use graphcal_compiler::semantic::int_division::{self, IntDivisionError};
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::operators::{ArithOp, IntArithOp, OrderingOp};

use super::EvalSession;
use super::numeric::QuantityValidationError;
use super::runtime_failure::{IntegerFailure, RuntimeFailure};

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
pub(super) fn check_finite(
    value: f64,
    context: &str,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, SemanticError> {
    super::numeric::computed_finite_quantity(value, context)
        .map_err(|err| ctx.runtime_error(err, span))
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
) -> Result<i64, SemanticError> {
    int_arith::arith(op, l, r).map_err(|error| ctx.runtime_error(IntegerFailure::from(error), span))
}

/// Negate an integer with checked arithmetic.
pub(super) fn int_negate(
    value: i64,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<i64, SemanticError> {
    int_arith::negate(value).map_err(|error| ctx.runtime_error(IntegerFailure::from(error), span))
}

/// Evaluate a `div_*` built-in: the quotient rounded in the direction it
/// names.
pub(super) fn int_division(
    function: IntDivisionFn,
    dividend: i64,
    divisor: i64,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<i64, SemanticError> {
    int_division::divide(function, dividend, divisor).map_err(|error| {
        let failure = match error {
            IntDivisionError::DivisionByZero => IntegerFailure::DivisionByZero,
            IntDivisionError::Overflow => IntegerFailure::Overflow,
        };
        ctx.runtime_error(failure, span)
    })
}

/// Raise an integer to an integer power with checked arithmetic.
pub(super) fn int_power(
    base: i64,
    exponent: i64,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<i64, SemanticError> {
    int_arith::power(base, exponent)
        .map_err(|error| ctx.runtime_error(IntegerFailure::from(error), span))
}

/// Evaluate an exact rational power without discarding the rational metadata.
///
/// Negative bases have a real result exactly when the reduced denominator is
/// odd; preserving the exact denominator makes that rule deterministic
/// instead of relying on `powf`'s treatment of a rounded exponent.
pub(super) fn eval_exact_quantity_power(
    base: FiniteQuantity,
    exponent: ExactRational,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, SemanticError> {
    base.checked_exact_power(exponent)
        .map_err(|error| match error {
            ExactPowerFailure::NoRealValue(error) => ctx.runtime_error(error, span),
            ExactPowerFailure::Arithmetic(error) => ctx.runtime_error(
                QuantityValidationError::from_arithmetic(error, "power operation"),
                span,
            ),
        })
}

/// Evaluate an arithmetic operator on two finite quantities with the
/// checked finite arithmetic of [`FiniteQuantity`].
pub(super) fn quantity_arith(
    op: ArithOp,
    l: FiniteQuantity,
    r: FiniteQuantity,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, SemanticError> {
    let context = "arithmetic operation";
    match op {
        ArithOp::Add => l.checked_add(r),
        ArithOp::Sub => l.checked_sub(r),
        ArithOp::Mul => l.checked_mul(r),
        ArithOp::Div => l.checked_div(r),
    }
    .map_err(|error| {
        let failure = match error {
            FiniteArithmeticError::DivisionByZero => RuntimeFailure::QuantityDivisionByZero,
            FiniteArithmeticError::Infinite => {
                super::numeric::QuantityValidationError::InfiniteResult {
                    context: context.to_string(),
                }
                .into()
            }
            FiniteArithmeticError::Underflow => {
                super::numeric::QuantityValidationError::UnderflowToZero {
                    context: context.to_string(),
                }
                .into()
            }
            FiniteArithmeticError::NotANumber => QuantityValidationError::NanResult {
                context: context.to_string(),
            }
            .into(),
        };
        ctx.runtime_error(failure, span)
    })
}

/// Raise a dimensionless quantity to a runtime dimensionless exponent; a
/// power of nonzero operands must not underflow to zero.
pub(super) fn quantity_power(
    base: FiniteQuantity,
    exponent: FiniteQuantity,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<FiniteQuantity, SemanticError> {
    base.checked_powf(exponent).map_err(|error| {
        ctx.runtime_error(
            QuantityValidationError::from_arithmetic(error, "arithmetic operation"),
            span,
        )
    })
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
                Vec::new(),
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
