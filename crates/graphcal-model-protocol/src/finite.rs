//! Finite real numbers.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A real number that is neither NaN nor infinite.
///
/// Graphcal quantities are always finite, so every quantity value on the wire
/// is a `FiniteF64`; NaN never encodes a missing or failed value.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct FiniteF64(f64);

/// A NaN or infinite value was offered as a [`FiniteF64`].
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("{value} is not a finite number")]
pub struct NonFiniteError {
    /// The rejected value.
    pub value: f64,
}

impl FiniteF64 {
    /// Zero.
    pub const ZERO: Self = Self(0.0);

    /// Validate a finite value.
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] for NaN or an infinity.
    pub const fn new(value: f64) -> Result<Self, NonFiniteError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(NonFiniteError { value })
        }
    }

    /// The finite value.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for FiniteF64 {
    type Error = NonFiniteError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<FiniteF64> for f64 {
    fn from(value: FiniteF64) -> Self {
        value.0
    }
}

impl fmt::Display for FiniteF64 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nan_and_infinities() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(FiniteF64::new(value).is_err());
        }
        assert_eq!(FiniteF64::new(-0.5).unwrap().get(), -0.5);
    }
}
