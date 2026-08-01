use thiserror::Error;

/// Reason a binary64 value cannot be converted to `Int` without changing it.
#[derive(Debug, Clone, PartialEq, Error)]
pub(super) enum ExactIntConversionError {
    /// `Int` values are always finite.
    #[error("is not finite (got {value})")]
    NonFinite { value: f64 },
    /// Converting the value would require an implicit rounding policy.
    #[error("is not integer-valued (got {value})")]
    NonInteger { value: f64 },
    /// The value is integral but cannot be represented as an `i64`.
    #[error(
        "is outside the representable integer range ({}..={}) (got {value})",
        i64::MIN,
        i64::MAX
    )]
    OutOfRange { value: f64 },
}

/// Convert a binary64 value to `Int` only when the conversion preserves it exactly.
///
/// Rust's direct `as i64` cast truncates fractional values and saturates values
/// outside the `i64` range. Explicit range and round-trip checks reject both
/// behaviors. Numeric equality deliberately accepts `-0.0` as the integer zero.
pub(super) fn exact_f64_to_i64(value: f64) -> Result<i64, ExactIntConversionError> {
    if !value.is_finite() {
        return Err(ExactIntConversionError::NonFinite { value });
    }

    let lower_inclusive = -2.0_f64.powi(63);
    let upper_exclusive = 2.0_f64.powi(63);
    if value < lower_inclusive || value >= upper_exclusive {
        return Err(ExactIntConversionError::OutOfRange { value });
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the following binary64 round trip rejects values that would be truncated"
    )]
    let converted = value as i64;
    #[expect(
        clippy::cast_precision_loss,
        reason = "the binary64 round trip is the exactness check"
    )]
    let round_trip = converted as f64;
    #[expect(
        clippy::float_cmp,
        reason = "to_int requires exact equality; no tolerance or rounding policy is implicit"
    )]
    if round_trip != value {
        return Err(ExactIntConversionError::NonInteger { value });
    }

    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_conversion_accepts_integer_valued_inputs() {
        for (value, expected) in [
            (0.0, 0),
            (-0.0, 0),
            (42.0, 42),
            (-42.0, -42),
            (1.0e18, 1_000_000_000_000_000_000),
            (-2.0_f64.powi(63), i64::MIN),
        ] {
            assert_eq!(exact_f64_to_i64(value), Ok(expected));
        }
    }

    #[test]
    fn exact_conversion_rejects_fractional_inputs() {
        for value in [3.7, -3.7, f64::MIN_POSITIVE] {
            assert_eq!(
                exact_f64_to_i64(value),
                Err(ExactIntConversionError::NonInteger { value })
            );
        }
    }

    #[test]
    fn exact_conversion_rejects_non_finite_inputs() {
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(matches!(
                exact_f64_to_i64(value),
                Err(ExactIntConversionError::NonFinite { .. })
            ));
        }
    }

    #[test]
    fn exact_conversion_rejects_values_outside_int_range() {
        for value in [2.0_f64.powi(63), 1.0e20, -1.0e20] {
            assert_eq!(
                exact_f64_to_i64(value),
                Err(ExactIntConversionError::OutOfRange { value })
            );
        }
    }
}
