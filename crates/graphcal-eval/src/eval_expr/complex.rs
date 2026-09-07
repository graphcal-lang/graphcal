//! Pure complex arithmetic and built-in kernels.

use graphcal_compiler::builtin::ComplexFn;
use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::desugar::desugared_ast::BinOp;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::registry::runtime_value::{RuntimeValue, RuntimeValueKind};
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
struct RawComplex {
    re: f64,
    im: f64,
}

impl RawComplex {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    const fn re(self) -> f64 {
        self.re
    }

    const fn im(self) -> f64 {
        self.im
    }
}

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
    #[error("internal complex call expected {expected} argument(s), got {got}")]
    WrongArity { expected: usize, got: usize },
    #[error("internal complex operation expected {expected}, got {actual}")]
    TypeMismatch {
        expected: &'static str,
        actual: RuntimeValueKind,
    },
    #[error("internal complex arithmetic received unsupported operands for {operator:?}")]
    UnsupportedOperands { operator: BinOp },
}

impl ComplexEvalError {
    #[must_use]
    pub(super) const fn is_internal_invariant(&self) -> bool {
        matches!(
            self,
            Self::WrongArity { .. } | Self::TypeMismatch { .. } | Self::UnsupportedOperands { .. }
        )
    }
}

pub(super) fn evaluate_builtin(
    function: ComplexFn,
    arguments: &[RuntimeValue],
) -> Result<RuntimeValue, ComplexEvalError> {
    if arguments.len() != function.arity() {
        return Err(ComplexEvalError::WrongArity {
            expected: function.arity(),
            got: arguments.len(),
        });
    }
    match function {
        ComplexFn::Rectangular => {
            let re = quantity(&arguments[0])?;
            let im = quantity(&arguments[1])?;
            finite_complex(RawComplex::new(re, im), "complex() construction")
                .and_then(|value| runtime_complex(value, "complex() construction"))
        }
        ComplexFn::Polar => {
            let magnitude = quantity(&arguments[0])?;
            let phase = quantity(&arguments[1])?;
            if magnitude < 0.0 {
                return Err(ComplexEvalError::NegativeMagnitude { value: magnitude });
            }
            finite_complex(
                RawComplex::new(magnitude * phase.cos(), magnitude * phase.sin()),
                "polar()",
            )
            .and_then(|value| runtime_complex(value, "polar()"))
        }
        ComplexFn::ToComplex => quantity(&arguments[0])
            .and_then(|value| runtime_complex(RawComplex::new(value, 0.0), "to_complex()")),
        ComplexFn::Real => {
            complex(&arguments[0]).and_then(|value| runtime_quantity(value.re(), "real()"))
        }
        ComplexFn::Imaginary => {
            complex(&arguments[0]).and_then(|value| runtime_quantity(value.im(), "imaginary()"))
        }
        ComplexFn::Phase => complex(&arguments[0])
            .and_then(|value| finite_quantity(value.im().atan2(value.re()), "phase()"))
            .and_then(|value| runtime_quantity(value, "phase()")),
        ComplexFn::Conjugate => complex(&arguments[0])
            .and_then(|value| finite_complex(RawComplex::new(value.re(), -value.im()), "conj()"))
            .and_then(|value| runtime_complex(value, "conj()")),
        ComplexFn::Absolute => match &arguments[0] {
            RuntimeValue::Complex(value) => finite_quantity(value.re().hypot(value.im()), "abs()")
                .and_then(|value| runtime_quantity(value, "abs()")),
            value => quantity(value)
                .and_then(|value| finite_quantity(value.abs(), "abs()"))
                .and_then(|value| runtime_quantity(value, "abs()")),
        },
        ComplexFn::Exponential => match &arguments[0] {
            RuntimeValue::Complex(value) => {
                let scale = value.re().exp();
                finite_complex(
                    RawComplex::new(scale * value.im().cos(), scale * value.im().sin()),
                    "exp()",
                )
                .and_then(|value| runtime_complex(value, "exp()"))
            }
            value => quantity(value)
                .and_then(|value| finite_quantity(value.exp(), "exp()"))
                .and_then(|value| runtime_quantity(value, "exp()")),
        },
    }
}

pub(super) fn evaluate_binary(
    operator: BinOp,
    lhs: &RuntimeValue,
    rhs: &RuntimeValue,
) -> Result<RuntimeValue, ComplexEvalError> {
    match (lhs, rhs) {
        (RuntimeValue::Complex(lhs), RuntimeValue::Complex(rhs)) => complex_binary(
            operator,
            RawComplex::new(lhs.re(), lhs.im()),
            RawComplex::new(rhs.re(), rhs.im()),
        )
        .and_then(|value| runtime_complex(value, "complex binary operation")),
        (RuntimeValue::Complex(lhs), RuntimeValue::Quantity(rhs)) => match operator {
            BinOp::Mul => finite_complex(
                RawComplex::new(lhs.re() * rhs.get(), lhs.im() * rhs.get()),
                "complex scalar multiplication",
            )
            .and_then(|value| runtime_complex(value, "complex scalar multiplication")),
            BinOp::Div => {
                if rhs.get() == 0.0 {
                    return Err(ComplexEvalError::DivisionByZero);
                }
                finite_complex(
                    RawComplex::new(lhs.re() / rhs.get(), lhs.im() / rhs.get()),
                    "complex scalar division",
                )
                .and_then(|value| runtime_complex(value, "complex scalar division"))
            }
            _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
        },
        (RuntimeValue::Quantity(lhs), RuntimeValue::Complex(rhs)) => match operator {
            BinOp::Mul => finite_complex(
                RawComplex::new(lhs.get() * rhs.re(), lhs.get() * rhs.im()),
                "scalar complex multiplication",
            )
            .and_then(|value| runtime_complex(value, "scalar complex multiplication")),
            BinOp::Div => divide(
                RawComplex::new(lhs.get(), 0.0),
                RawComplex::new(rhs.re(), rhs.im()),
            )
            .and_then(|value| runtime_complex(value, "complex division")),
            _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
        },
        _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
    }
}

pub(super) fn negate(value: ComplexValue) -> Result<ComplexValue, ComplexEvalError> {
    finite_complex(
        RawComplex::new(-value.re(), -value.im()),
        "complex negation",
    )
    .and_then(|value| finite_complex_value(value, "complex negation"))
}

fn complex_binary(
    operator: BinOp,
    lhs: RawComplex,
    rhs: RawComplex,
) -> Result<RawComplex, ComplexEvalError> {
    match operator {
        BinOp::Add => finite_complex(
            RawComplex::new(lhs.re() + rhs.re(), lhs.im() + rhs.im()),
            "complex addition",
        ),
        BinOp::Sub => finite_complex(
            RawComplex::new(lhs.re() - rhs.re(), lhs.im() - rhs.im()),
            "complex subtraction",
        ),
        BinOp::Mul => finite_complex(
            RawComplex::new(
                lhs.im().mul_add(-rhs.im(), lhs.re() * rhs.re()),
                lhs.im().mul_add(rhs.re(), lhs.re() * rhs.im()),
            ),
            "complex multiplication",
        ),
        BinOp::Div => divide(lhs, rhs),
        _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
    }
}

/// Divide the exact binary input components and round each final component once.
/// A common floating scale is insufficient: a product or partial quotient can
/// underflow before a small divisor restores the final value's exponent.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "arbitrary-precision rationals cannot overflow; rejecting a zero divisor makes its exact squared norm positive"
)]
fn divide(lhs: RawComplex, rhs: RawComplex) -> Result<RawComplex, ComplexEvalError> {
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
    finite_complex(RawComplex::new(re, im), "complex division")
}

fn quantity(value: &RuntimeValue) -> Result<f64, ComplexEvalError> {
    match value {
        RuntimeValue::Quantity(value) | RuntimeValue::CoordinateLabel { value, .. } => {
            Ok(value.get())
        }
        other => Err(ComplexEvalError::TypeMismatch {
            expected: "a quantity",
            actual: other.kind(),
        }),
    }
}

fn complex(value: &RuntimeValue) -> Result<RawComplex, ComplexEvalError> {
    match value {
        RuntimeValue::Complex(value) => Ok(RawComplex::new(value.re(), value.im())),
        other => Err(ComplexEvalError::TypeMismatch {
            expected: "a complex quantity",
            actual: other.kind(),
        }),
    }
}

fn runtime_complex(
    value: RawComplex,
    operation: &'static str,
) -> Result<RuntimeValue, ComplexEvalError> {
    finite_complex_value(value, operation).map(RuntimeValue::Complex)
}

fn finite_complex_value(
    value: RawComplex,
    operation: &'static str,
) -> Result<ComplexValue, ComplexEvalError> {
    ComplexValue::try_new(value.re(), value.im())
        .map_err(|_| ComplexEvalError::NonFinite { operation })
}

fn runtime_quantity(value: f64, operation: &'static str) -> Result<RuntimeValue, ComplexEvalError> {
    FiniteQuantity::try_new(value)
        .map(RuntimeValue::Quantity)
        .map_err(|_| ComplexEvalError::NonFiniteQuantity { operation })
}

const fn finite_complex(
    value: RawComplex,
    operation: &'static str,
) -> Result<RawComplex, ComplexEvalError> {
    if value.re().is_finite() && value.im().is_finite() {
        Ok(value)
    } else {
        Err(ComplexEvalError::NonFinite { operation })
    }
}

const fn finite_quantity(value: f64, operation: &'static str) -> Result<f64, ComplexEvalError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ComplexEvalError::NonFiniteQuantity { operation })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_preserves_signed_subnormal_components() {
        let tiny = f64::from_bits(1);
        for sign in [1.0, -1.0] {
            let quotient =
                divide(RawComplex::new(sign * tiny, 0.0), RawComplex::new(0.5, 0.5)).unwrap();
            assert_eq!(quotient.re().to_bits(), (sign * tiny).to_bits());
            assert_eq!(quotient.im().to_bits(), (-sign * tiny).to_bits());
        }
        let quotient = divide(RawComplex::new(tiny, tiny), RawComplex::new(0.5, 0.5)).unwrap();
        assert_eq!(quotient.re().to_bits(), (2.0 * tiny).to_bits());
        assert_eq!(quotient.im().to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn division_preserves_terms_before_rescaling_and_exact_cancellation() {
        let quotient = divide(
            RawComplex::new(2.0e-308, 0.0),
            RawComplex::new(1.0e-250, 1.0e-308),
        )
        .unwrap();
        assert!((quotient.im() / -2.0e-116 - 1.0).abs() < 4.0 * f64::EPSILON);
        for value in [f64::from_bits(1), f64::MIN_POSITIVE, 1.0, f64::MAX] {
            assert_eq!(
                divide(RawComplex::new(value, value), RawComplex::new(value, value)).unwrap(),
                RawComplex::new(1.0, 0.0)
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
            Err(ComplexEvalError::NonFinite { .. })
        ));
        assert!(matches!(
            evaluate_builtin(
                ComplexFn::Exponential,
                &[RuntimeValue::complex(1_000.0, 0.0).unwrap()],
            ),
            Err(ComplexEvalError::NonFinite { .. })
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
            Err(ComplexEvalError::DivisionByZero)
        ));
    }
}
