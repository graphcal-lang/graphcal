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

    /// The least value in the chosen unit whose [`Self::to_si`] is not below
    /// the SI lower bound `si_min`.
    ///
    /// A published bound must accept itself when a client sends it back:
    /// `from_si` followed by `to_si` can round past the SI bound, so the
    /// converted bound is stepped inward until it converts back inside.
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] when the bound has no finite value in the
    /// chosen unit.
    pub fn lower_bound_from_si(self, si_min: FiniteF64) -> Result<FiniteF64, NonFiniteError> {
        self.inward_bound(si_min, f64::next_up, |si| si >= si_min.get())
    }

    /// The greatest value in the chosen unit whose [`Self::to_si`] is not
    /// above the SI upper bound `si_max`; see [`Self::lower_bound_from_si`].
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] when the bound has no finite value in the
    /// chosen unit.
    pub fn upper_bound_from_si(self, si_max: FiniteF64) -> Result<FiniteF64, NonFiniteError> {
        self.inward_bound(si_max, f64::next_down, |si| si <= si_max.get())
    }

    fn inward_bound(
        self,
        si_bound: FiniteF64,
        step_inward: fn(f64) -> f64,
        inside: impl Fn(f64) -> bool,
    ) -> Result<FiniteF64, NonFiniteError> {
        let mut bound = self.from_si(si_bound)?;
        // Division and multiplication are each correctly rounded, so the
        // round trip is off by a few units in the last place at most.
        while !inside(bound.get() * self.0) {
            bound = FiniteF64::new(step_inward(bound.get()))?;
        }
        Ok(bound)
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

    #[test]
    fn converted_bounds_accept_themselves_when_sent_back() {
        // 0.35 m / 0.01 = 35.0 cm, but 35.0 * 0.01 = 0.35000000000000003.
        let centi = SiScale::new(0.01).unwrap();
        let max = centi.upper_bound_from_si(finite(0.35)).unwrap();
        assert!(max.get() * 0.01 <= 0.35);
        assert!(max.get() < 35.0);

        for scale in [0.01, 3600.0, std::f64::consts::PI / 180.0, 0.3048, 1e-9] {
            let scale = SiScale::new(scale).unwrap();
            for hundredths in -1000_i32..=1000 {
                let si = finite(f64::from(hundredths) / 100.0);
                let max = scale.upper_bound_from_si(si).unwrap();
                let min = scale.lower_bound_from_si(si).unwrap();
                assert!(
                    scale.to_si(max).unwrap().get() <= si.get(),
                    "{si:?} {scale:?}"
                );
                assert!(
                    scale.to_si(min).unwrap().get() >= si.get(),
                    "{si:?} {scale:?}"
                );
                // Only rounding is corrected: the bound stays within a few
                // units in the last place of the plain conversion.
                let plain = scale.from_si(si).unwrap().get();
                assert!((max.get() - plain).abs() <= plain.abs() * 1e-15, "{si:?}");
            }
        }
        assert!(
            SiScale::new(1e-300)
                .unwrap()
                .upper_bound_from_si(finite(1e300))
                .is_err()
        );
    }
}
