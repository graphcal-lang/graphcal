use thiserror::Error;

/// Error produced by checked `to_int` conversion.
#[derive(Debug, Clone, PartialEq, Error)]
pub(super) enum ToIntError {
    /// `to_int` only accepts finite scalar values.
    #[error("to_int() requires a finite value, got {value}")]
    NonFinite { value: f64 },
    /// The truncated value cannot be represented as an i64.
    #[error(
        "to_int() argument {value} is outside the representable integer range ({}..={})",
        i64::MIN,
        i64::MAX
    )]
    OutOfRange { value: f64 },
}

/// Convert a finite scalar to `Int` using graphcal's truncation semantics.
///
/// Rust's direct `as i64` cast saturates for out-of-range floats. This helper
/// performs the range check explicitly so boundary values such as `2^63` are
/// rejected instead of becoming `i64::MAX`.
pub(super) fn checked_f64_to_i64(value: f64) -> Result<i64, ToIntError> {
    if !value.is_finite() {
        return Err(ToIntError::NonFinite { value });
    }

    let truncated = value.trunc();
    let lower_inclusive = -2.0_f64.powi(63);
    let upper_exclusive = 2.0_f64.powi(63);
    if truncated < lower_inclusive || truncated >= upper_exclusive {
        return Err(ToIntError::OutOfRange { value });
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite f64 value is explicitly range-checked before truncating to Int"
    )]
    {
        Ok(truncated as i64)
    }
}
