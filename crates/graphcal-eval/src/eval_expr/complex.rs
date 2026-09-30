//! Pure complex arithmetic and built-in kernels.

use crate::invariant::{Failure, Invariant};
use crate::runtime_value::RuntimeValue;
use graphcal_compiler::builtin::ComplexFn;
use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::desugar::desugared_ast::BinOp;
use graphcal_compiler::finite_value::FiniteQuantity;
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub(super) enum ComplexEvalError {
    #[error("complex division by zero")]
    DivisionByZero,
    #[error("polar() magnitude must be non-negative, got {value}")]
    NegativeMagnitude { value: f64 },
    #[error("{operation} produced a non-finite complex component")]
    NonFinite { operation: &'static str },
    #[error("{operation} produced a non-finite (NaN or infinite) quantity")]
    NonFiniteQuantity { operation: &'static str },
}

impl From<ComplexEvalError> for Failure<ComplexEvalError> {
    fn from(error: ComplexEvalError) -> Self {
        Self::Error(error)
    }
}

fn unsupported_operands(operator: BinOp) -> Invariant {
    Invariant::violated(format_args!(
        "internal complex arithmetic received unsupported operands for {operator:?}"
    ))
}

/// Evaluate one complex built-in call.
///
/// The caller has already checked the call against the function's static
/// entry, so `arguments` holds exactly `function.arity()` values.
pub(super) fn evaluate_builtin(
    function: ComplexFn,
    arguments: &[RuntimeValue],
) -> Result<RuntimeValue, Failure<ComplexEvalError>> {
    Ok(match function {
        ComplexFn::Rectangular => {
            let re = quantity(&arguments[0])?;
            let im = quantity(&arguments[1])?;
            RuntimeValue::Complex(ComplexValue::from_parts(re, im))
        }
        ComplexFn::Polar => {
            let magnitude = quantity(&arguments[0])?.get();
            let phase = quantity(&arguments[1])?.get();
            if magnitude < 0.0 {
                return Err(ComplexEvalError::NegativeMagnitude { value: magnitude }.into());
            }
            RuntimeValue::Complex(finite_complex(
                magnitude * phase.cos(),
                magnitude * phase.sin(),
                "polar()",
            )?)
        }
        ComplexFn::ToComplex => RuntimeValue::Complex(ComplexValue::from_parts(
            quantity(&arguments[0])?,
            FiniteQuantity::ZERO,
        )),
        ComplexFn::Real => RuntimeValue::Quantity(complex(&arguments[0])?.real_part()),
        ComplexFn::Imaginary => RuntimeValue::Quantity(complex(&arguments[0])?.imaginary_part()),
        ComplexFn::Phase => {
            let value = complex(&arguments[0])?;
            finite_quantity(value.im().atan2(value.re()), "phase()")?
        }
        ComplexFn::Conjugate => RuntimeValue::Complex(complex(&arguments[0])?.conjugate()),
        ComplexFn::Absolute => match &arguments[0] {
            RuntimeValue::Complex(value) => finite_quantity(value.re().hypot(value.im()), "abs()")?,
            value => finite_quantity(quantity(value)?.get().abs(), "abs()")?,
        },
        ComplexFn::Exponential => match &arguments[0] {
            RuntimeValue::Complex(value) => {
                let scale = value.re().exp();
                RuntimeValue::Complex(finite_complex(
                    scale * value.im().cos(),
                    scale * value.im().sin(),
                    "exp()",
                )?)
            }
            value => finite_quantity(quantity(value)?.get().exp(), "exp()")?,
        },
    })
}

pub(super) fn evaluate_binary(
    operator: BinOp,
    lhs: &RuntimeValue,
    rhs: &RuntimeValue,
) -> Result<RuntimeValue, Failure<ComplexEvalError>> {
    let value = match (lhs, rhs) {
        (RuntimeValue::Complex(lhs), RuntimeValue::Complex(rhs)) => {
            complex_binary(operator, *lhs, *rhs)?
        }
        (RuntimeValue::Complex(lhs), RuntimeValue::Quantity(rhs)) => match operator {
            BinOp::Mul => finite_complex(
                lhs.re() * rhs.get(),
                lhs.im() * rhs.get(),
                "complex scalar multiplication",
            )?,
            BinOp::Div => {
                if rhs.get() == 0.0 {
                    return Err(ComplexEvalError::DivisionByZero.into());
                }
                finite_complex(
                    lhs.re() / rhs.get(),
                    lhs.im() / rhs.get(),
                    "complex scalar division",
                )?
            }
            _ => return Err(unsupported_operands(operator).into()),
        },
        (RuntimeValue::Quantity(lhs), RuntimeValue::Complex(rhs)) => match operator {
            BinOp::Mul => finite_complex(
                lhs.get() * rhs.re(),
                lhs.get() * rhs.im(),
                "scalar complex multiplication",
            )?,
            BinOp::Div => divide(ComplexValue::from_parts(*lhs, FiniteQuantity::ZERO), *rhs)?,
            _ => return Err(unsupported_operands(operator).into()),
        },
        _ => return Err(unsupported_operands(operator).into()),
    };
    Ok(RuntimeValue::Complex(value))
}

fn complex_binary(
    operator: BinOp,
    lhs: ComplexValue,
    rhs: ComplexValue,
) -> Result<ComplexValue, Failure<ComplexEvalError>> {
    Ok(match operator {
        BinOp::Add => finite_complex(lhs.re() + rhs.re(), lhs.im() + rhs.im(), "complex addition")?,
        BinOp::Sub => finite_complex(
            lhs.re() - rhs.re(),
            lhs.im() - rhs.im(),
            "complex subtraction",
        )?,
        BinOp::Mul => finite_complex(
            lhs.im().mul_add(-rhs.im(), lhs.re() * rhs.re()),
            lhs.im().mul_add(rhs.re(), lhs.re() * rhs.im()),
            "complex multiplication",
        )?,
        BinOp::Div => divide(lhs, rhs)?,
        _ => return Err(unsupported_operands(operator).into()),
    })
}

/// Divide the exact binary input components and round each final component once.
/// A common floating scale is insufficient: a product or partial quotient can
/// underflow before a small divisor restores the final value's exponent.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "arbitrary-precision rationals cannot overflow; rejecting a zero divisor makes its exact squared norm positive"
)]
fn divide(lhs: ComplexValue, rhs: ComplexValue) -> Result<ComplexValue, ComplexEvalError> {
    if rhs.re() == 0.0 && rhs.im() == 0.0 {
        return Err(ComplexEvalError::DivisionByZero);
    }
    let rational = |value| {
        BigRational::from_float(value).ok_or(ComplexEvalError::NonFinite {
            operation: "complex division",
        })
    };
    let (a, b, c, d) = (
        rational(lhs.re())?,
        rational(lhs.im())?,
        rational(rhs.re())?,
        rational(rhs.im())?,
    );
    let denominator = &c * &c + &d * &d;
    let component = |numerator: BigRational, zero_sign: f64| {
        let exact_zero = numerator.is_zero();
        let value = (numerator / &denominator)
            .to_f64()
            .ok_or(ComplexEvalError::NonFinite {
                operation: "complex division",
            })?;
        // Preserve signed-zero arithmetic where the ordinary numerator is zero;
        // a nonzero exact numerator keeps the sign of its rounded subnormal.
        Ok::<_, ComplexEvalError>(if exact_zero && zero_sign == 0.0 {
            value.copysign(zero_sign)
        } else {
            value
        })
    };
    let re = component(
        &a * &c + &b * &d,
        lhs.re().mul_add(rhs.re(), lhs.im() * rhs.im()),
    )?;
    let im = component(
        &b * &c - &a * &d,
        lhs.im().mul_add(rhs.re(), -(lhs.re() * rhs.im())),
    )?;
    finite_complex(re, im, "complex division")
}

fn quantity(value: &RuntimeValue) -> Result<FiniteQuantity, Invariant> {
    match value {
        RuntimeValue::Quantity(value) => Ok(*value),
        other => Err(type_mismatch("a quantity", other)),
    }
}

fn complex(value: &RuntimeValue) -> Result<ComplexValue, Invariant> {
    match value {
        RuntimeValue::Complex(value) => Ok(*value),
        other => Err(type_mismatch("a complex quantity", other)),
    }
}

fn type_mismatch(expected: &str, actual: &RuntimeValue) -> Invariant {
    Invariant::violated(format_args!(
        "internal complex operation expected {expected}, got {}",
        actual.describe()
    ))
}

/// The complex value of computed components, when both are finite.
fn finite_complex(
    re: f64,
    im: f64,
    operation: &'static str,
) -> Result<ComplexValue, ComplexEvalError> {
    ComplexValue::try_new(re, im).map_err(|_| ComplexEvalError::NonFinite { operation })
}

/// The quantity value of a computed real result, when it is finite.
fn finite_quantity(value: f64, operation: &'static str) -> Result<RuntimeValue, ComplexEvalError> {
    FiniteQuantity::try_new(value)
        .map(RuntimeValue::Quantity)
        .map_err(|_| ComplexEvalError::NonFiniteQuantity { operation })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> ComplexValue {
        ComplexValue::try_new(re, im).unwrap()
    }

    #[test]
    fn division_preserves_signed_subnormal_components() {
        let tiny = f64::from_bits(1);
        for sign in [1.0, -1.0] {
            let quotient = divide(c(sign * tiny, 0.0), c(0.5, 0.5)).unwrap();
            assert_eq!(quotient.re().to_bits(), (sign * tiny).to_bits());
            assert_eq!(quotient.im().to_bits(), (-sign * tiny).to_bits());
        }
        let quotient = divide(c(tiny, tiny), c(0.5, 0.5)).unwrap();
        assert_eq!(quotient.re().to_bits(), (2.0 * tiny).to_bits());
        assert_eq!(quotient.im().to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn division_preserves_terms_before_rescaling_and_exact_cancellation() {
        let quotient = divide(c(2.0e-308, 0.0), c(1.0e-250, 1.0e-308)).unwrap();
        assert!((quotient.im() / -2.0e-116 - 1.0).abs() < 4.0 * f64::EPSILON);
        for value in [f64::from_bits(1), f64::MIN_POSITIVE, 1.0, f64::MAX] {
            assert_eq!(
                divide(c(value, value), c(value, value)).unwrap(),
                c(1.0, 0.0)
            );
        }
    }

    #[test]
    fn multiplication_and_division_round_trip() {
        let lhs = RuntimeValue::complex(3.0, 4.0).unwrap();
        let rhs = RuntimeValue::complex(5.0, 6.0).unwrap();
        let product = evaluate_binary(BinOp::Mul, &lhs, &rhs).unwrap();
        let RuntimeValue::Complex(product_value) = product else {
            panic!("expected complex product");
        };
        assert_eq!(product_value, ComplexValue::try_new(-9.0, 38.0).unwrap());
        let product = RuntimeValue::Complex(product_value);
        let RuntimeValue::Complex(quotient) = evaluate_binary(BinOp::Div, &product, &rhs).unwrap()
        else {
            panic!("expected complex quotient");
        };
        assert!((quotient.re() - 3.0).abs() < 1e-12);
        assert!((quotient.im() - 4.0).abs() < 1e-12);
    }

    #[test]
    fn division_avoids_overflow_for_large_equal_operands() {
        let large = RuntimeValue::complex(f64::MAX, f64::MAX).unwrap();
        let RuntimeValue::Complex(quotient) = evaluate_binary(BinOp::Div, &large, &large).unwrap()
        else {
            panic!("expected complex quotient");
        };
        assert_eq!(quotient, ComplexValue::try_new(1.0, 0.0).unwrap());
    }

    #[test]
    fn division_preserves_large_finite_quotient() {
        let lhs = RuntimeValue::complex(1.0e208, 0.0).unwrap();
        let rhs = RuntimeValue::complex(1.0e-100, 1.0e-100).unwrap();
        let RuntimeValue::Complex(quotient) = evaluate_binary(BinOp::Div, &lhs, &rhs).unwrap()
        else {
            panic!("expected complex quotient");
        };
        assert!((quotient.re() / 5.0e307 - 1.0).abs() < 1.0e-15);
        assert!((quotient.im() / -5.0e307 - 1.0).abs() < 1.0e-15);
    }

    #[test]
    fn absolute_value_uses_stable_hypot() {
        let result = evaluate_builtin(
            ComplexFn::Absolute,
            &[RuntimeValue::complex(3.0, 4.0).unwrap()],
        );
        let RuntimeValue::Quantity(magnitude) = result.unwrap() else {
            panic!("expected quantity magnitude");
        };
        assert!((magnitude.get() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn non_finite_result_is_rejected() {
        let value = RuntimeValue::complex(f64::MAX, 0.0).unwrap();
        assert!(matches!(
            evaluate_binary(BinOp::Mul, &value, &RuntimeValue::quantity(2.0).unwrap()),
            Err(Failure::Error(ComplexEvalError::NonFinite { .. }))
        ));
        assert!(matches!(
            evaluate_builtin(
                ComplexFn::Exponential,
                &[RuntimeValue::complex(1_000.0, 0.0).unwrap()],
            ),
            Err(Failure::Error(ComplexEvalError::NonFinite { .. }))
        ));
    }

    #[test]
    fn zero_divisor_is_rejected() {
        assert!(matches!(
            evaluate_binary(
                BinOp::Div,
                &RuntimeValue::complex(1.0, 2.0).unwrap(),
                &RuntimeValue::complex(0.0, 0.0).unwrap(),
            ),
            Err(Failure::Error(ComplexEvalError::DivisionByZero))
        ));
    }

    #[test]
    fn checker_guaranteed_operand_kinds_are_invariants() {
        let invariant = |result: Result<RuntimeValue, Failure<ComplexEvalError>>| match result {
            Err(Failure::Invariant(invariant)) => invariant.to_string(),
            other => panic!("expected an invariant, got {other:?}"),
        };
        let complex = RuntimeValue::complex(1.0, 2.0).unwrap();
        assert_eq!(
            invariant(evaluate_binary(
                BinOp::Add,
                &complex,
                &RuntimeValue::Bool(true)
            )),
            "internal complex arithmetic received unsupported operands for Add"
        );
        assert_eq!(
            invariant(evaluate_binary(
                BinOp::Sub,
                &complex,
                &RuntimeValue::quantity(1.0).unwrap()
            )),
            "internal complex arithmetic received unsupported operands for Sub"
        );
        assert_eq!(
            invariant(evaluate_builtin(
                ComplexFn::Real,
                &[RuntimeValue::Bool(true)]
            )),
            "internal complex operation expected a complex quantity, got Bool"
        );
        assert_eq!(
            invariant(evaluate_builtin(ComplexFn::ToComplex, &[complex])),
            "internal complex operation expected a quantity, got Complex"
        );
    }
}
