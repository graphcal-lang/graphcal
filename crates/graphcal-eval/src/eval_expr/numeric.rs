use thiserror::Error;

/// Error returned by pure quantity validation helpers.
#[derive(Debug, Clone, PartialEq, Error)]
pub(super) enum QuantityValidationError {
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

/// Validate the result of a computation whose non-finite output indicates an error.
pub(super) fn computed_finite_quantity(
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
