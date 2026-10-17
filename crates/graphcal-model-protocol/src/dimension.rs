//! Physical dimensions as base-dimension exponent maps.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::label::BaseDimensionName;

/// A non-zero, reduced rational exponent of one base dimension.
///
/// Serialized as a JSON integer when the denominator is one, and as
/// `{"numerator": n, "denominator": d}` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ExponentRepr", into = "ExponentRepr")]
pub struct Exponent {
    numerator: i32,
    denominator: u32,
}

/// Why a pair of integers is not a valid [`Exponent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExponentError {
    #[error("a dimension exponent must not be zero")]
    Zero,
    #[error("a dimension exponent must have a positive denominator")]
    ZeroDenominator,
    #[error("dimension exponent {numerator}/{denominator} is not in lowest terms")]
    NotReduced { numerator: i32, denominator: u32 },
}

impl Exponent {
    /// Validate a reduced exponent `numerator / denominator`.
    ///
    /// # Errors
    ///
    /// Returns [`ExponentError`] for a zero exponent, a zero denominator, or
    /// a fraction that is not in lowest terms.
    pub const fn new(numerator: i32, denominator: u32) -> Result<Self, ExponentError> {
        if denominator == 0 {
            return Err(ExponentError::ZeroDenominator);
        }
        if numerator == 0 {
            return Err(ExponentError::Zero);
        }
        if gcd(numerator.unsigned_abs(), denominator) != 1 {
            return Err(ExponentError::NotReduced {
                numerator,
                denominator,
            });
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    /// The signed numerator.
    #[must_use]
    pub const fn numerator(self) -> i32 {
        self.numerator
    }

    /// The positive denominator.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }
}

const fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

impl fmt::Display for Exponent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denominator == 1 {
            write!(formatter, "{}", self.numerator)
        } else {
            write!(formatter, "{}/{}", self.numerator, self.denominator)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum ExponentRepr {
    Integer(i32),
    Ratio(RatioRepr),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RatioRepr {
    numerator: i32,
    denominator: u32,
}

impl TryFrom<ExponentRepr> for Exponent {
    type Error = ExponentError;

    fn try_from(repr: ExponentRepr) -> Result<Self, Self::Error> {
        match repr {
            ExponentRepr::Integer(numerator) => Self::new(numerator, 1),
            ExponentRepr::Ratio(RatioRepr {
                numerator,
                denominator,
            }) => {
                if denominator == 1 {
                    // An integer exponent has exactly one spelling.
                    return Err(ExponentError::NotReduced {
                        numerator,
                        denominator,
                    });
                }
                Self::new(numerator, denominator)
            }
        }
    }
}

impl From<Exponent> for ExponentRepr {
    fn from(exponent: Exponent) -> Self {
        if exponent.denominator == 1 {
            Self::Integer(exponent.numerator)
        } else {
            Self::Ratio(RatioRepr {
                numerator: exponent.numerator,
                denominator: exponent.denominator,
            })
        }
    }
}

/// A physical dimension: a map from base dimension to its non-zero exponent.
///
/// The empty map is the dimensionless dimension. Two dimensions are equal
/// exactly when their exponent maps are equal, so dimensions compare without
/// parsing any unit spelling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Dimension {
    exponents: BTreeMap<BaseDimensionName, Exponent>,
}

impl Dimension {
    /// The dimensionless dimension.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self {
            exponents: BTreeMap::new(),
        }
    }

    /// Whether this is the dimensionless dimension.
    #[must_use]
    pub fn is_dimensionless(&self) -> bool {
        self.exponents.is_empty()
    }

    /// The non-zero exponents in base-dimension name order.
    pub fn exponents(&self) -> impl ExactSizeIterator<Item = (&BaseDimensionName, Exponent)> {
        self.exponents
            .iter()
            .map(|(name, exponent)| (name, *exponent))
    }
}

impl FromIterator<(BaseDimensionName, Exponent)> for Dimension {
    /// Collect exponents; a repeated base dimension keeps its last exponent.
    fn from_iter<T: IntoIterator<Item = (BaseDimensionName, Exponent)>>(iter: T) -> Self {
        Self {
            exponents: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> BaseDimensionName {
        BaseDimensionName::new(name).unwrap()
    }

    #[test]
    fn exponents_must_be_nonzero_and_reduced() {
        assert_eq!(Exponent::new(0, 1), Err(ExponentError::Zero));
        assert_eq!(Exponent::new(1, 0), Err(ExponentError::ZeroDenominator));
        assert!(matches!(
            Exponent::new(2, 4),
            Err(ExponentError::NotReduced { .. })
        ));
        assert_eq!(Exponent::new(-1, 2).unwrap().to_string(), "-1/2");
    }

    #[test]
    fn dimension_json_uses_integers_and_ratio_objects() {
        let dimension = Dimension::from_iter([
            (base("Length"), Exponent::new(1, 1).unwrap()),
            (base("Time"), Exponent::new(-1, 2).unwrap()),
        ]);
        let json = serde_json::to_string(&dimension).unwrap();
        assert_eq!(
            json,
            r#"{"Length":1,"Time":{"numerator":-1,"denominator":2}}"#
        );
        assert_eq!(serde_json::from_str::<Dimension>(&json).unwrap(), dimension);
    }

    #[test]
    fn integer_exponents_have_one_spelling() {
        assert!(
            serde_json::from_str::<Dimension>(r#"{"Length":{"numerator":2,"denominator":1}}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Dimension>(r#"{"Length":0}"#).is_err());
    }
}
