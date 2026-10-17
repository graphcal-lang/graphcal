//! Per-row evaluation outcomes.

use thiserror::Error;

use crate::label::PortName;

/// Why one row produced no outputs.
///
/// Each failure is an ordinary result of evaluating that row; protocol
/// violations are reported separately and end the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowFailure {
    /// An assertion of the model did not hold. In optimization, such a row
    /// is an infeasible candidate.
    AssertionFailed {
        /// The assertion, as the model displays it.
        assertion: String,
        /// The assertion's diagnostic.
        message: String,
    },
    /// A finite input value lies outside the parameter's declared domain.
    InputOutOfDomain {
        /// The parameter whose value is out of domain.
        parameter: PortName,
        /// The domain diagnostic.
        message: String,
    },
    /// Evaluation failed for any other reason, such as a division by zero.
    EvaluationError {
        /// The evaluation diagnostic.
        message: String,
    },
}

impl RowFailure {
    /// The status code this failure is reported with.
    #[must_use]
    pub const fn status(&self) -> RowStatus {
        match self {
            Self::AssertionFailed { .. } => RowStatus::AssertionFailed,
            Self::InputOutOfDomain { .. } => RowStatus::InputOutOfDomain,
            Self::EvaluationError { .. } => RowStatus::EvaluationError,
        }
    }

    /// The human-readable diagnostic.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::AssertionFailed { message, .. }
            | Self::InputOutOfDomain { message, .. }
            | Self::EvaluationError { message } => message,
        }
    }
}

/// The outcome of evaluating one row: its outputs, or why there are none.
#[derive(Debug, Clone, PartialEq)]
pub enum RowOutcome<T> {
    /// Every selected output has a value.
    Success(T),
    /// The row produced no outputs.
    Failure(RowFailure),
}

impl<T> RowOutcome<T> {
    /// The status code of this outcome.
    #[must_use]
    pub const fn status(&self) -> RowStatus {
        match self {
            Self::Success(_) => RowStatus::Success,
            Self::Failure(failure) => failure.status(),
        }
    }
}

/// The wire code of a row outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RowStatus {
    /// Every selected output has a value.
    Success = 0,
    /// See [`RowFailure::AssertionFailed`].
    AssertionFailed = 1,
    /// See [`RowFailure::InputOutOfDomain`].
    InputOutOfDomain = 2,
    /// See [`RowFailure::EvaluationError`].
    EvaluationError = 3,
}

/// A status byte that names no [`RowStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unknown row status code {0}")]
pub struct UnknownRowStatus(pub u8);

impl RowStatus {
    /// The wire code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for RowStatus {
    type Error = UnknownRowStatus;

    fn try_from(code: u8) -> Result<Self, Self::Error> {
        match code {
            0 => Ok(Self::Success),
            1 => Ok(Self::AssertionFailed),
            2 => Ok(Self::InputOutOfDomain),
            3 => Ok(Self::EvaluationError),
            other => Err(UnknownRowStatus(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_round_trip() {
        for status in [
            RowStatus::Success,
            RowStatus::AssertionFailed,
            RowStatus::InputOutOfDomain,
            RowStatus::EvaluationError,
        ] {
            assert_eq!(RowStatus::try_from(status.code()), Ok(status));
        }
        assert_eq!(RowStatus::try_from(9), Err(UnknownRowStatus(9)));
    }
}
