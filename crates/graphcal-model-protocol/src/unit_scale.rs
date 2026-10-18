//! Scale factors between a client-chosen unit and the canonical SI unit.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::finite::{FiniteF64, NonFiniteError};

/// The size of one chosen unit in the canonical SI unit at scale one: a
/// value `v` in the chosen unit is `v * scale` in SI.
///
/// Graphcal units are pure scales (there are no affine units), so this one
/// factor converts in both directions.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct SiScale(f64);

/// A value that is not a valid [`SiScale`].
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("a unit scale must be positive and finite, got {value}")]
pub struct SiScaleError {
    /// The rejected value.
    pub value: f64,
}

impl SiScale {
    /// Validate a scale factor.
    ///
    /// # Errors
    ///
    /// Returns [`SiScaleError`] unless `value` is positive and finite.
    pub const fn new(value: f64) -> Result<Self, SiScaleError> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(SiScaleError { value })
        }
    }

    /// The scale factor.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }

    /// `value`, given in the chosen unit, in SI.
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] when the converted value overflows.
    pub const fn to_si(self, value: FiniteF64) -> Result<FiniteF64, NonFiniteError> {
        FiniteF64::new(value.get() * self.0)
    }

    /// `value`, given in SI, in the chosen unit.
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] when the converted value overflows.
    pub const fn from_si(self, value: FiniteF64) -> Result<FiniteF64, NonFiniteError> {
        FiniteF64::new(value.get() / self.0)
    }
}

impl TryFrom<f64> for SiScale {
    type Error = SiScaleError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<SiScale> for f64 {
    fn from(scale: SiScale) -> Self {
        scale.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    #[test]
    fn scales_are_positive_and_finite() {
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(SiScale::new(value).is_err(), "{value}");
        }
        assert!(serde_json::from_str::<SiScale>("0.0").is_err());
        assert_eq!(
            serde_json::from_str::<SiScale>("1000.0").unwrap().get(),
            1000.0
        );
    }

    #[test]
    fn converts_both_ways_and_reports_overflow() {
        let kilo = SiScale::new(1000.0).unwrap();
        assert_eq!(kilo.to_si(finite(2.5)), Ok(finite(2500.0)));
        assert_eq!(kilo.from_si(finite(2500.0)), Ok(finite(2.5)));
        assert!(kilo.to_si(finite(f64::MAX)).is_err());
        assert!(
            SiScale::new(1e-300)
                .unwrap()
                .from_si(finite(1e300))
                .is_err()
        );
    }
}
