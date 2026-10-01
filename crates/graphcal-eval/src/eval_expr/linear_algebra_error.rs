//! Why a built-in linear-algebra operation failed.

use graphcal_compiler::builtin::LinearAlgebraFn;
use thiserror::Error;

use super::numeric::QuantityValidationError;
use super::work_budget::{WorkAmountError, WorkBudgetError};

/// Why a built-in linear-algebra operation failed.
#[derive(Debug, Error)]
pub(super) enum LinearAlgebraError {
    #[error(transparent)]
    Numeric(#[from] QuantityValidationError),
    #[error(transparent)]
    Algorithm(#[from] super::linear_algebra_lu::LuError),
    #[error("linear-algebra work estimate failed: {0}")]
    WorkAmount(#[from] WorkAmountError),
    #[error("`{function}()` {source}")]
    WorkBudget {
        function: LinearAlgebraFn,
        #[source]
        source: WorkBudgetError,
    },
}
