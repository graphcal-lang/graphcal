use thiserror::Error;

/// Failure to project an `i64` into binary64 without changing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("integer {value} is outside the exactly representable binary64 range")]
pub struct ExactI64ToF64Error {
    value: i64,
}

/// Convert an integer to binary64 only when the conversion is exact.
pub const fn exact_i64_to_f64(value: i64) -> Result<f64, ExactI64ToF64Error> {
    const MAX_EXACT_F64_INT: u64 = 1_u64 << f64::MANTISSA_DIGITS;
    if value.unsigned_abs() > MAX_EXACT_F64_INT {
        return Err(ExactI64ToF64Error { value });
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer magnitude is checked to be exactly representable before casting"
    )]
    {
        Ok(value as f64)
    }
}

/// Error returned by pure quantity validation helpers.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum QuantityValidationError {
    /// A value that must be finite was NaN or infinite.
    #[error("{context} must be finite, got {value}")]
    NonFinite { context: String, value: f64 },
    /// A scale-like value was finite but not strictly positive.
    #[error("{context} must be greater than zero, got {value}")]
    NonPositive { context: String, value: f64 },
    /// A computed quantity result was NaN.
    #[error("invalid argument for {context} (result is NaN)")]
    NanResult { context: String },
    /// A computed quantity result was infinite.
    #[error("{context} produced infinite result")]
    InfiniteResult { context: String },
}

/// Validate that a quantity value is finite.
pub(super) fn finite_quantity(
    value: f64,
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(QuantityValidationError::NonFinite {
            context: context.into(),
            value,
        })
    }
}

/// Validate that a scale factor is finite and strictly positive.
pub(super) fn positive_finite_scale(
    value: f64,
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    if !value.is_finite() {
        Err(QuantityValidationError::NonFinite {
            context: context.into(),
            value,
        })
    } else if value <= 0.0 {
        Err(QuantityValidationError::NonPositive {
            context: context.into(),
            value,
        })
    } else {
        Ok(value)
    }
}

/// A finite sum represented as a scale and a compensated sum of normalized
/// values, so callers can divide by the mathematical total without first
/// materializing an overflowing total.
#[derive(Debug, Clone, Copy)]
pub struct ScaledSum {
    scale: f64,
    normalized_sum: f64,
}

impl ScaledSum {
    pub fn from_values(
        values: &[f64],
        context: impl Into<String>,
    ) -> Result<Self, QuantityValidationError> {
        let context = context.into();
        let scale = values.iter().try_fold(0.0_f64, |scale, value| {
            finite_quantity(*value, context.clone()).map(|value| scale.max(value.abs()))
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
            normalized_sum,
        })
    }

    pub fn is_zero(self) -> bool {
        self.scale == 0.0 || self.normalized_sum == 0.0
    }

    pub fn normalized_ratio(
        self,
        value: f64,
        context: impl Into<String>,
    ) -> Result<f64, QuantityValidationError> {
        if self.is_zero() {
            return computed_finite_quantity(f64::NAN, context);
        }
        computed_finite_quantity((value / self.scale) / self.normalized_sum, context)
    }

    fn mean(
        self,
        count: usize,
        context: impl Into<String>,
    ) -> Result<f64, QuantityValidationError> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "materialized collection lengths are exactly representable in binary64"
        )]
        let count = count as f64;
        computed_finite_quantity((self.normalized_sum / count) * self.scale, context)
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

/// Compute a mean with scaled, compensated accumulation.
pub fn scaled_mean(
    values: &[f64],
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    let context = context.into();
    ScaledSum::from_values(values, context.clone())?.mean(values.len(), context)
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

    pub(super) fn finish(self, context: impl Into<String>) -> Result<f64, QuantityValidationError> {
        let result = if self.scale == 0.0 {
            0.0
        } else {
            self.scale * self.sum_squares.sqrt()
        };
        computed_finite_quantity(result, context)
    }
}

/// Compute a root-sum-square with scaled accumulation to avoid intermediate
/// overflow and underflow.
pub(super) fn root_sum_square(
    values: impl IntoIterator<Item = f64>,
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    let context = context.into();
    let accumulator =
        values
            .into_iter()
            .try_fold(RootSumSquare::new(), |mut accumulator, value| {
                accumulator.add(value, context.clone())?;
                Ok::<_, QuantityValidationError>(accumulator)
            })?;
    accumulator.finish(context)
}

/// Validate the result of a computation whose non-finite output indicates an error.
pub fn computed_finite_quantity(
    value: f64,
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    if value.is_nan() {
        Err(QuantityValidationError::NanResult {
            context: context.into(),
        })
    } else if value.is_infinite() {
        Err(QuantityValidationError::InfiniteResult {
            context: context.into(),
        })
    } else {
        Ok(value)
    }
}
