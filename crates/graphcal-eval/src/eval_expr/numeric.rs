use graphcal_compiler::finite_value::{FiniteArithmeticError, FiniteQuantity};
use num_rational::BigRational;
use num_traits::ToPrimitive;
use thiserror::Error;

pub(crate) use graphcal_compiler::finite_value::{ExactI64ToF64Error, exact_i64_to_f64};

/// Error returned by pure quantity validation helpers.
#[derive(Debug, Clone, PartialEq, Error)]
pub(crate) enum QuantityValidationError {
    /// A value that must be finite was NaN or infinite.
    #[error("{context} must be finite, got {value}")]
    NonFinite { context: String, value: f64 },
    /// A computed quantity result was NaN.
    #[error("invalid argument for {context} (result is NaN)")]
    NanResult { context: String },
    /// A computed quantity result was infinite.
    #[error("{context} produced infinite result")]
    InfiniteResult { context: String },
    /// A mathematically non-zero computation lost its entire value to zero.
    #[error("{context} underflowed to zero")]
    UnderflowToZero { context: String },
    /// A quotient had a zero divisor.
    #[error("division by zero")]
    DivisionByZero,
}

impl QuantityValidationError {
    /// Describe a failed checked finite operation of `context`.
    pub(super) fn from_arithmetic(error: FiniteArithmeticError, context: &str) -> Self {
        match error {
            FiniteArithmeticError::Infinite => Self::InfiniteResult {
                context: context.to_string(),
            },
            FiniteArithmeticError::Underflow => Self::UnderflowToZero {
                context: context.to_string(),
            },
            FiniteArithmeticError::DivisionByZero => Self::DivisionByZero,
            FiniteArithmeticError::NotANumber => Self::NanResult {
                context: context.to_string(),
            },
        }
    }
}

/// Validate that a quantity value is finite.
pub(super) fn finite_quantity(
    value: f64,
    context: impl Into<String>,
) -> Result<FiniteQuantity, QuantityValidationError> {
    FiniteQuantity::try_new(value).map_err(|_| QuantityValidationError::NonFinite {
        context: context.into(),
        value,
    })
}

/// A finite sum represented as a scale and a compensated sum of normalized
/// values, so callers can divide by the mathematical total without first
/// materializing an overflowing total.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScaledSum {
    scale: f64,
    normalized_sum: f64,
}

impl ScaledSum {
    pub(crate) fn from_values(
        values: &[f64],
        context: impl Into<String>,
    ) -> Result<Self, QuantityValidationError> {
        let context = context.into();
        let scale = values.iter().try_fold(0.0_f64, |scale, value| {
            finite_quantity(*value, context.clone()).map(|value| scale.max(value.get().abs()))
        })?;
        if scale == 0.0 {
            return Ok(Self {
                scale,
                normalized_sum: 0.0,
            });
        }
        let normalized_sum = compensated_sum(values.iter().map(|value| value / scale));
        computed_finite_quantity(normalized_sum, context).map(|normalized_sum| Self {
            scale,
            normalized_sum: normalized_sum.get(),
        })
    }

    #[must_use]
    pub(crate) fn is_zero(self) -> bool {
        self.scale == 0.0 || self.normalized_sum == 0.0
    }

    pub(crate) fn normalized_ratio(
        self,
        value: f64,
        context: impl Into<String>,
    ) -> Result<FiniteQuantity, QuantityValidationError> {
        if self.is_zero() {
            return computed_finite_quantity(f64::NAN, context);
        }
        computed_finite_quantity((value / self.scale) / self.normalized_sum, context)
    }
}

fn compensated_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let (sum, correction) =
        values
            .into_iter()
            .fold((0.0_f64, 0.0_f64), |(sum, correction), value| {
                let next = sum + value;
                let recovered = if sum.abs() >= value.abs() {
                    (sum - next) + value
                } else {
                    (value - next) + sum
                };
                (next, correction + recovered)
            });
    sum + correction
}

/// Round an exact rational result to binary64 once.
pub(crate) fn rounded_rational(
    value: &BigRational,
    context: impl Into<String>,
) -> Result<FiniteQuantity, QuantityValidationError> {
    let context = context.into();
    let result = value
        .to_f64()
        .ok_or_else(|| QuantityValidationError::NanResult {
            context: context.clone(),
        })?;
    computed_finite_quantity(result, context)
}

/// Incremental scaled root-sum-square accumulator.
#[derive(Debug, Clone, Copy)]
pub(super) struct RootSumSquare {
    scale: f64,
    sum_squares: f64,
}

impl RootSumSquare {
    pub(super) const fn new() -> Self {
        Self {
            scale: 0.0,
            sum_squares: 1.0,
        }
    }

    pub(super) fn add(
        &mut self,
        value: f64,
        context: impl Into<String>,
    ) -> Result<(), QuantityValidationError> {
        let context = context.into();
        finite_quantity(value, context)?;
        if value == 0.0 {
            return Ok(());
        }
        let magnitude = value.abs();
        if self.scale < magnitude {
            let ratio = self.scale / magnitude;
            self.sum_squares = (self.sum_squares * ratio).mul_add(ratio, 1.0);
            self.scale = magnitude;
        } else {
            let ratio = magnitude / self.scale;
            self.sum_squares = ratio.mul_add(ratio, self.sum_squares);
        }
        Ok(())
    }

    pub(super) fn finish(
        self,
        context: impl Into<String>,
    ) -> Result<FiniteQuantity, QuantityValidationError> {
        let result = if self.scale == 0.0 {
            0.0
        } else {
            self.scale * self.sum_squares.sqrt()
        };
        computed_finite_quantity(result, context)
    }
}

/// Validate the result of a computation whose non-finite output indicates an error.
pub(crate) fn computed_finite_quantity(
    value: f64,
    context: impl Into<String>,
) -> Result<FiniteQuantity, QuantityValidationError> {
    if value.is_nan() {
        Err(QuantityValidationError::NanResult {
            context: context.into(),
        })
    } else {
        FiniteQuantity::try_new(value).map_err(|_| QuantityValidationError::InfiniteResult {
            context: context.into(),
        })
    }
}

/// Validate a computation that is mathematically guaranteed to remain non-zero.
///
/// Callers must establish that the operation and its inputs have that property.
/// This keeps legitimate zero results, such as cancellation, separate from
/// complete floating-point underflow.
pub(crate) fn computed_nonzero_quantity(
    value: f64,
    context: impl Into<String>,
) -> Result<FiniteQuantity, QuantityValidationError> {
    let context = context.into();
    let value = computed_finite_quantity(value, context.clone())?;
    if value.get() == 0.0 {
        Err(QuantityValidationError::UnderflowToZero { context })
    } else {
        Ok(value)
    }
}
