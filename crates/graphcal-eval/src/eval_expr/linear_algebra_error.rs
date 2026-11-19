//! Why a built-in linear-algebra operation failed.

use thiserror::Error;

use super::numeric::QuantityValidationError;

/// Why a built-in linear-algebra operation failed.
#[derive(Debug, Error)]
pub(super) enum LinearAlgebraError {
    #[error(transparent)]
    Numeric(#[from] QuantityValidationError),
    #[error(transparent)]
    Algorithm(#[from] super::linear_algebra_lu::LuError),
}
