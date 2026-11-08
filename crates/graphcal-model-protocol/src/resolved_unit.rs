//! A unit expression of a model, resolved to its dimension and scale.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dimension::Dimension;
use crate::finite::{FiniteF64, NonFiniteError};
use crate::label::UnitSpelling;
use crate::unit_scale::SiScale;
use crate::wire_type::QuantityType;

/// A unit expression resolved by a model: what it measures and its size in
/// the canonical SI unit.
///
/// A client converts a quantity it read in a wire unit into this unit when
/// the quantity has the same dimension (see [`ResolvedUnit::from_si`]), for
/// example to show a nested output field in hours rather than seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedUnit {
    unit: UnitSpelling,
    dimension: Dimension,
    si_scale: SiScale,
}

impl ResolvedUnit {
    /// A resolved unit.
    #[must_use]
    pub const fn new(unit: UnitSpelling, dimension: Dimension, si_scale: SiScale) -> Self {
        Self {
            unit,
            dimension,
            si_scale,
        }
    }

    /// The unit expression as written.
    #[must_use]
    pub const fn unit(&self) -> &UnitSpelling {
        &self.unit
    }

    /// The dimension the unit measures.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    /// The size of the unit in the canonical SI unit.
    #[must_use]
    pub const fn si_scale(&self) -> SiScale {
        self.si_scale
    }

    /// Whether the unit measures quantities of `quantity`.
    #[must_use]
    pub fn measures(&self, quantity: &QuantityType) -> bool {
        quantity.dimension() == &self.dimension
    }

    /// `value`, a quantity of `dimension` in its canonical SI unit, in this
    /// unit.
    ///
    /// The value must be in SI: a quantity read in a session's chosen unit
    /// is first converted with that port's [`SiScale`].
    ///
    /// # Errors
    ///
    /// Returns [`UnitConversionError`] when the unit measures another
    /// dimension or the converted value overflows.
    pub fn from_si(
        &self,
        dimension: &Dimension,
        value: FiniteF64,
    ) -> Result<FiniteF64, UnitConversionError> {
        if dimension != &self.dimension {
            return Err(UnitConversionError::DimensionMismatch {
                unit: self.unit.clone(),
            });
        }
        self.si_scale
            .from_si(value)
            .map_err(UnitConversionError::Overflow)
    }
}

/// Why a quantity cannot be expressed in a [`ResolvedUnit`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum UnitConversionError {
    #[error("unit `{unit}` measures another dimension")]
    DimensionMismatch { unit: UnitSpelling },
    #[error(transparent)]
    Overflow(NonFiniteError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::Exponent;
    use crate::label::BaseDimensionName;

    fn time() -> Dimension {
        Dimension::from_iter([(
            BaseDimensionName::new("Time").unwrap(),
            Exponent::new(1, 1).unwrap(),
        )])
    }

    #[test]
    fn converts_quantities_of_its_dimension_only() {
        let hour = ResolvedUnit::new(
            UnitSpelling::new("h").unwrap(),
            time(),
            SiScale::new(3600.0).unwrap(),
        );
        let seconds = QuantityType::new(time(), Some(UnitSpelling::new("s").unwrap())).unwrap();
        let value = FiniteF64::new(7200.0).unwrap();
        assert!(hour.measures(&seconds));
        assert_eq!(
            hour.from_si(&time(), value),
            Ok(FiniteF64::new(2.0).unwrap())
        );
        assert!(matches!(
            hour.from_si(&Dimension::dimensionless(), value),
            Err(UnitConversionError::DimensionMismatch { .. })
        ));
        let tiny = ResolvedUnit::new(
            UnitSpelling::new("tiny").unwrap(),
            time(),
            SiScale::new(1e-300).unwrap(),
        );
        assert!(matches!(
            tiny.from_si(&time(), FiniteF64::new(1e300).unwrap()),
            Err(UnitConversionError::Overflow(_))
        ));
    }

    #[test]
    fn round_trips_through_json() {
        let hour = ResolvedUnit::new(
            UnitSpelling::new("h").unwrap(),
            time(),
            SiScale::new(3600.0).unwrap(),
        );
        let json = serde_json::to_string(&hour).unwrap();
        assert_eq!(
            json,
            r#"{"unit":"h","dimension":{"Time":1},"si_scale":3600.0}"#
        );
        assert_eq!(serde_json::from_str::<ResolvedUnit>(&json).unwrap(), hour);
    }
}
