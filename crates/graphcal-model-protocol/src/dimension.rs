//! Physical dimensions as base-dimension exponent maps.
//!
//! A base dimension is either a Graphcal prelude dimension (`Length`) or one
//! a model declares, identified by its [`DeclaredName`].

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::declared_name::DeclaredName;
use crate::label::BaseDimensionName;

/// One base dimension.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaseDimension {
    /// A prelude base dimension, such as `Length`.
    Prelude {
        /// Its name.
        name: BaseDimensionName,
    },
    /// A base dimension a model declares.
    Declared(DeclaredName<BaseDimensionName>),
}

impl BaseDimension {
    /// The prelude base dimension `name`.
    #[must_use]
    pub const fn prelude(name: BaseDimensionName) -> Self {
        Self::Prelude { name }
    }
}

impl fmt::Display for BaseDimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prelude { name } => fmt::Display::fmt(name, formatter),
            Self::Declared(name) => fmt::Display::fmt(name, formatter),
        }
    }
}

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
///
/// Serialized as a list of `{"base": …, "exponent": …}` terms in base
/// dimension order; a base dimension may appear once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<DimensionTerm>", into = "Vec<DimensionTerm>")]
pub struct Dimension {
    exponents: BTreeMap<BaseDimension, Exponent>,
}

/// A base dimension listed twice in one dimension.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("base dimension `{base}` is listed more than once")]
pub struct DuplicateBaseDimension {
    /// The repeated base dimension.
    pub base: BaseDimension,
}

impl Dimension {
    /// The dimensionless dimension.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self {
            exponents: BTreeMap::new(),
        }
    }

    /// The dimension with `exponents`.
    ///
    /// # Errors
    ///
    /// Returns [`DuplicateBaseDimension`] when a base dimension repeats.
    pub fn new(
        exponents: impl IntoIterator<Item = (BaseDimension, Exponent)>,
    ) -> Result<Self, DuplicateBaseDimension> {
        let mut map = BTreeMap::new();
        for (base, exponent) in exponents {
            if map.contains_key(&base) {
                return Err(DuplicateBaseDimension { base });
            }
            map.insert(base, exponent);
        }
        Ok(Self { exponents: map })
    }

    /// Whether this is the dimensionless dimension.
    #[must_use]
    pub fn is_dimensionless(&self) -> bool {
        self.exponents.is_empty()
    }

    /// The non-zero exponents in base-dimension order.
    pub fn exponents(&self) -> impl ExactSizeIterator<Item = (&BaseDimension, Exponent)> {
        self.exponents
            .iter()
            .map(|(base, exponent)| (base, *exponent))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DimensionTerm {
    base: BaseDimension,
    exponent: Exponent,
}

impl TryFrom<Vec<DimensionTerm>> for Dimension {
    type Error = DuplicateBaseDimension;

    fn try_from(terms: Vec<DimensionTerm>) -> Result<Self, Self::Error> {
        Self::new(terms.into_iter().map(|term| (term.base, term.exponent)))
    }
}

impl From<Dimension> for Vec<DimensionTerm> {
    fn from(dimension: Dimension) -> Self {
        dimension
            .exponents
            .into_iter()
            .map(|(base, exponent)| DimensionTerm { base, exponent })
            .collect()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::label::{ModuleName, PackageName};

    pub fn prelude(name: &str) -> BaseDimension {
        BaseDimension::prelude(BaseDimensionName::new(name).unwrap())
    }

    /// A prelude base dimension to the first power.
    pub fn base_dimension(name: &str) -> Dimension {
        Dimension::new([(prelude(name), Exponent::new(1, 1).unwrap())]).unwrap()
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
    fn dimension_json_lists_typed_base_dimensions() {
        let money = BaseDimension::Declared(DeclaredName::new(
            PackageName::new("finance").unwrap(),
            vec![ModuleName::new("units").unwrap()],
            BaseDimensionName::new("Money").unwrap(),
        ));
        let dimension = Dimension::new([
            (money, Exponent::new(1, 1).unwrap()),
            (prelude("Time"), Exponent::new(-1, 2).unwrap()),
        ])
        .unwrap();
        let json = serde_json::to_string(&dimension).unwrap();
        assert_eq!(
            json,
            concat!(
                r#"[{"base":{"kind":"prelude","name":"Time"},"exponent":{"numerator":-1,"denominator":2}},"#,
                r#"{"base":{"kind":"declared","package":"finance","module":["units"],"name":"Money"},"exponent":1}]"#
            )
        );
        assert_eq!(serde_json::from_str::<Dimension>(&json).unwrap(), dimension);
    }

    #[test]
    fn a_dimension_lists_each_base_once_with_one_exponent_spelling() {
        let length = r#"{"kind":"prelude","name":"Length"}"#;
        assert!(
            serde_json::from_str::<Dimension>(&format!(
                r#"[{{"base":{length},"exponent":1}},{{"base":{length},"exponent":2}}]"#
            ))
            .is_err()
        );
        assert!(
            serde_json::from_str::<Dimension>(&format!(
                r#"[{{"base":{length},"exponent":{{"numerator":2,"denominator":1}}}}]"#
            ))
            .is_err()
        );
        assert!(
            serde_json::from_str::<Dimension>(&format!(r#"[{{"base":{length},"exponent":0}}]"#))
                .is_err()
        );
        assert!(
            serde_json::from_str::<Dimension>(
                r#"[{"base":{"kind":"prelude","name":"Length","module":[]},"exponent":1}]"#
            )
            .is_err()
        );
    }
}
