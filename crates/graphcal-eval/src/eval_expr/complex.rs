//! Pure complex arithmetic and built-in kernels.

use graphcal_compiler::builtin::ComplexFn;
use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::desugar::desugared_ast::BinOp;
use graphcal_compiler::registry::runtime_value::{RuntimeValue, RuntimeValueKind};
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
            finite_complex(ComplexValue::new(re, im), "complex() construction")
                .map(RuntimeValue::Complex)
        }
        ComplexFn::Polar => {
            let magnitude = quantity(&arguments[0])?;
            let phase = quantity(&arguments[1])?;
            if magnitude < 0.0 {
                return Err(ComplexEvalError::NegativeMagnitude { value: magnitude });
            }
            finite_complex(
                ComplexValue::new(magnitude * phase.cos(), magnitude * phase.sin()),
                "polar()",
            )
            .map(RuntimeValue::Complex)
        }
        ComplexFn::ToComplex => quantity(&arguments[0])
            .map(|value| RuntimeValue::Complex(ComplexValue::new(value, 0.0))),
        ComplexFn::Real => complex(&arguments[0]).map(|value| RuntimeValue::Quantity(value.re())),
        ComplexFn::Imaginary => {
            complex(&arguments[0]).map(|value| RuntimeValue::Quantity(value.im()))
        }
        ComplexFn::Phase => complex(&arguments[0])
            .and_then(|value| finite_quantity(value.im().atan2(value.re()), "phase()"))
            .map(RuntimeValue::Quantity),
        ComplexFn::Conjugate => complex(&arguments[0])
            .and_then(|value| finite_complex(ComplexValue::new(value.re(), -value.im()), "conj()"))
            .map(RuntimeValue::Complex),
        ComplexFn::Absolute => match &arguments[0] {
            RuntimeValue::Complex(value) => {
                finite_quantity(value.re().hypot(value.im()), "abs()").map(RuntimeValue::Quantity)
            }
            value => quantity(value)
                .and_then(|value| finite_quantity(value.abs(), "abs()"))
                .map(RuntimeValue::Quantity),
        },
        ComplexFn::Exponential => match &arguments[0] {
            RuntimeValue::Complex(value) => {
                let scale = value.re().exp();
                finite_complex(
                    ComplexValue::new(scale * value.im().cos(), scale * value.im().sin()),
                    "exp()",
                )
                .map(RuntimeValue::Complex)
            }
            value => quantity(value)
                .and_then(|value| finite_quantity(value.exp(), "exp()"))
                .map(RuntimeValue::Quantity),
        },
    }
}

pub(super) fn evaluate_binary(
    operator: BinOp,
    lhs: &RuntimeValue,
    rhs: &RuntimeValue,
) -> Result<RuntimeValue, ComplexEvalError> {
    match (lhs, rhs) {
        (RuntimeValue::Complex(lhs), RuntimeValue::Complex(rhs)) => {
            complex_binary(operator, *lhs, *rhs).map(RuntimeValue::Complex)
        }
        (RuntimeValue::Complex(lhs), RuntimeValue::Quantity(rhs)) => match operator {
            BinOp::Mul => finite_complex(
                ComplexValue::new(lhs.re() * rhs, lhs.im() * rhs),
                "complex scalar multiplication",
            )
            .map(RuntimeValue::Complex),
            BinOp::Div => {
                if *rhs == 0.0 {
                    return Err(ComplexEvalError::DivisionByZero);
                }
                finite_complex(
                    ComplexValue::new(lhs.re() / rhs, lhs.im() / rhs),
                    "complex scalar division",
                )
                .map(RuntimeValue::Complex)
            }
            _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
        },
        (RuntimeValue::Quantity(lhs), RuntimeValue::Complex(rhs)) => match operator {
            BinOp::Mul => finite_complex(
                ComplexValue::new(lhs * rhs.re(), lhs * rhs.im()),
                "scalar complex multiplication",
            )
            .map(RuntimeValue::Complex),
            BinOp::Div => divide(ComplexValue::new(*lhs, 0.0), *rhs).map(RuntimeValue::Complex),
            _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
        },
        _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
    }
}

pub(super) fn negate(value: ComplexValue) -> Result<ComplexValue, ComplexEvalError> {
    finite_complex(
        ComplexValue::new(-value.re(), -value.im()),
        "complex negation",
    )
}

fn complex_binary(
    operator: BinOp,
    lhs: ComplexValue,
    rhs: ComplexValue,
) -> Result<ComplexValue, ComplexEvalError> {
    match operator {
        BinOp::Add => finite_complex(
            ComplexValue::new(lhs.re() + rhs.re(), lhs.im() + rhs.im()),
            "complex addition",
        ),
        BinOp::Sub => finite_complex(
            ComplexValue::new(lhs.re() - rhs.re(), lhs.im() - rhs.im()),
            "complex subtraction",
        ),
        BinOp::Mul => finite_complex(
            ComplexValue::new(
                lhs.im().mul_add(-rhs.im(), lhs.re() * rhs.re()),
                lhs.im().mul_add(rhs.re(), lhs.re() * rhs.im()),
            ),
            "complex multiplication",
        ),
        BinOp::Div => divide(lhs, rhs),
        _ => Err(ComplexEvalError::UnsupportedOperands { operator }),
    }
}

/// Normalize the divisor before Cartesian division, avoiding avoidable
/// overflow/underflow in `c²+d²` while preserving large finite quotients.
fn divide(lhs: ComplexValue, rhs: ComplexValue) -> Result<ComplexValue, ComplexEvalError> {
    let (c, d) = (rhs.re(), rhs.im());
    if c == 0.0 && d == 0.0 {
        return Err(ComplexEvalError::DivisionByZero);
    }
    let scale = c.abs().max(d.abs());
    let c = c / scale;
    let d = d / scale;
    let denominator = c.mul_add(c, d * d);
    let (a, b) = (lhs.re(), lhs.im());
    let result = if scale >= 1.0 {
        let a = a / scale;
        let b = b / scale;
        ComplexValue::new(
            a.mul_add(c, b * d) / denominator,
            b.mul_add(c, -(a * d)) / denominator,
        )
    } else {
        ComplexValue::new(
            ((a * c) / denominator + (b * d) / denominator) / scale,
            ((b * c) / denominator - (a * d) / denominator) / scale,
        )
    };
    finite_complex(result, "complex division")
}

fn quantity(value: &RuntimeValue) -> Result<f64, ComplexEvalError> {
    match value {
        RuntimeValue::Quantity(value) | RuntimeValue::CoordinateLabel { value, .. } => Ok(*value),
        other => Err(ComplexEvalError::TypeMismatch {
            expected: "a quantity",
            actual: other.kind(),
        }),
    }
}

fn complex(value: &RuntimeValue) -> Result<ComplexValue, ComplexEvalError> {
    match value {
        RuntimeValue::Complex(value) => Ok(*value),
        other => Err(ComplexEvalError::TypeMismatch {
            expected: "a complex quantity",
            actual: other.kind(),
        }),
    }
}

const fn finite_complex(
    value: ComplexValue,
    operation: &'static str,
) -> Result<ComplexValue, ComplexEvalError> {
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
    fn multiplication_and_division_round_trip() {
        let lhs = RuntimeValue::Complex(ComplexValue::new(3.0, 4.0));
        let rhs = RuntimeValue::Complex(ComplexValue::new(5.0, 6.0));
        let product = evaluate_binary(BinOp::Mul, &lhs, &rhs).unwrap();
        let RuntimeValue::Complex(product_value) = product else {
            panic!("expected complex product");
        };
        assert_eq!(product_value, ComplexValue::new(-9.0, 38.0));
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
        let large = RuntimeValue::Complex(ComplexValue::new(f64::MAX, f64::MAX));
        let RuntimeValue::Complex(quotient) = evaluate_binary(BinOp::Div, &large, &large).unwrap()
        else {
            panic!("expected complex quotient");
        };
        assert_eq!(quotient, ComplexValue::new(1.0, 0.0));
    }

    #[test]
    fn division_preserves_large_finite_quotient() {
        let lhs = RuntimeValue::Complex(ComplexValue::new(1.0e208, 0.0));
        let rhs = RuntimeValue::Complex(ComplexValue::new(1.0e-100, 1.0e-100));
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
            &[RuntimeValue::Complex(ComplexValue::new(3.0, 4.0))],
        );
        let RuntimeValue::Quantity(magnitude) = result.unwrap() else {
            panic!("expected quantity magnitude");
        };
        assert!((magnitude - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn non_finite_result_is_rejected() {
        let value = RuntimeValue::Complex(ComplexValue::new(f64::MAX, 0.0));
        assert!(matches!(
            evaluate_binary(BinOp::Mul, &value, &RuntimeValue::Quantity(2.0)),
            Err(ComplexEvalError::NonFinite { .. })
        ));
        assert!(matches!(
            evaluate_builtin(
                ComplexFn::Exponential,
                &[RuntimeValue::Complex(ComplexValue::new(1_000.0, 0.0))],
            ),
            Err(ComplexEvalError::NonFinite { .. })
        ));
    }

    #[test]
    fn zero_divisor_is_rejected() {
        assert!(matches!(
            evaluate_binary(
                BinOp::Div,
                &RuntimeValue::Complex(ComplexValue::new(1.0, 2.0)),
                &RuntimeValue::Complex(ComplexValue::new(0.0, 0.0)),
            ),
            Err(ComplexEvalError::DivisionByZero)
        ));
    }
}
