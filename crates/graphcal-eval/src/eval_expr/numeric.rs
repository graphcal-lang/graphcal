use num_rational::BigRational;
use num_traits::ToPrimitive;
use thiserror::Error;

/// Failure to project an `i64` into binary64 without changing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("integer {value} cannot be represented exactly as binary64")]
pub struct ExactI64ToF64Error {
    value: i64,
}

/// Convert an integer to binary64 only when the conversion is exact.
pub const fn exact_i64_to_f64(value: i64) -> Result<f64, ExactI64ToF64Error> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the following wider-integer round trip proves whether this specific value is exact"
    )]
    let converted = value as f64;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "i128 contains every rounded binary64 value reachable from an i64"
    )]
    let round_trip = converted as i128;
    let original = value as i128;
    if round_trip == original {
        Ok(converted)
    } else {
        Err(ExactI64ToF64Error { value })
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
    /// A mathematically non-zero computation lost its entire value to zero.
    #[error("{context} underflowed to zero")]
    UnderflowToZero { context: String },
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

/// Average the exact binary64 inputs, rounding only the final quotient.
///
/// A common floating scale can erase a small term before cancellation reveals
/// it. Exact binary rationals retain that term and also avoid overflowing a
/// representable mean's intermediate sum. This is not decimal reinterpretation.
pub fn exact_mean(
    values: &[f64],
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    let context = context.into();
    if values.is_empty() {
        return Err(QuantityValidationError::NanResult { context });
    }
    let total = values
        .iter()
        .try_fold(BigRational::default(), |total, value| {
            let rational = BigRational::from_float(*value).ok_or_else(|| {
                QuantityValidationError::NonFinite {
                    context: context.clone(),
                    value: *value,
                }
            })?;
            Ok::<_, QuantityValidationError>(total + rational)
        })?;
    let average = total / BigRational::from_integer(values.len().into());
    let result = average
        .to_f64()
        .ok_or_else(|| QuantityValidationError::NanResult {
            context: context.clone(),
        })?;
    computed_finite_quantity(result, context)
}

#[cfg(test)]
mod mean_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn mean_matches_independent_bounded_integer_reference(
            integers in proptest::collection::vec(-1_000_000_i32..=1_000_000, 1..32),
        ) {
            // Both integer accumulation and conversion are exact in this range;
            // the independent reference performs one binary64 division.
            let total: i32 = integers.iter().sum();
            let count = i32::try_from(integers.len()).unwrap();
            let values = integers.into_iter().map(f64::from).collect::<Vec<_>>();
            prop_assert_eq!(exact_mean(&values, "mean()").unwrap(), f64::from(total) / f64::from(count));
        }
    }

    #[test]
    fn cancellation_preserves_a_small_representable_mean_in_every_order() {
        for values in [
            [1.0e308, 1.0e-100, -1.0e308],
            [1.0e-100, -1.0e308, 1.0e308],
            [-1.0e308, 1.0e308, 1.0e-100],
            [1.0e308, -1.0e308, 1.0e-100],
            [-1.0e308, 1.0e-100, 1.0e308],
            [1.0e-100, 1.0e308, -1.0e308],
        ] {
            assert_eq!(exact_mean(&values, "mean()").unwrap(), 1.0e-100 / 3.0);
        }
        assert_eq!(
            exact_mean(&[f64::MAX, f64::MAX], "mean()").unwrap(),
            f64::MAX
        );
        assert_eq!(
            exact_mean(
                &[f64::MAX, f64::MAX, 1.0e-100, -f64::MAX, -f64::MAX],
                "mean()"
            )
            .unwrap(),
            1.0e-100 / 5.0
        );
    }

    #[test]
    fn mean_rounds_subnormals_once_and_rejects_non_finite_inputs() {
        let tiny = f64::from_bits(1);
        for value in [tiny, -tiny, f64::MIN_POSITIVE, f64::MAX] {
            assert_eq!(exact_mean(&[value, value], "mean()").unwrap(), value);
        }
        assert_eq!(
            exact_mean(&[tiny, f64::from_bits(2)], "mean()")
                .unwrap()
                .to_bits(),
            2
        );
        assert_eq!(exact_mean(&[tiny, -tiny], "mean()").unwrap(), 0.0);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(exact_mean(&[value], "mean()").is_err());
        }
        assert!(exact_mean(&[], "mean()").is_err());
    }
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

/// Validate a computation that is mathematically guaranteed to remain non-zero.
///
/// Callers must establish that the operation and its inputs have that property.
/// This keeps legitimate zero results, such as cancellation, separate from
/// complete floating-point underflow.
pub fn computed_nonzero_quantity(
    value: f64,
    context: impl Into<String>,
) -> Result<f64, QuantityValidationError> {
    let context = context.into();
    let value = computed_finite_quantity(value, context.clone())?;
    if value == 0.0 {
        Err(QuantityValidationError::UnderflowToZero { context })
    } else {
        Ok(value)
    }
}
