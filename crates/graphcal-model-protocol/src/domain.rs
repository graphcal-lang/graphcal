//! Declared value domains of parameters.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::finite::FiniteF64;

/// Inclusive bounds with at least one side declared and `min <= max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    try_from = "BoundsRepr<T>",
    into = "BoundsRepr<T>",
    bound(
        serialize = "T: Serialize + Clone",
        deserialize = "T: Deserialize<'de> + PartialOrd"
    )
)]
pub struct Bounds<T> {
    min: Option<T>,
    max: Option<T>,
}

/// Why a pair of bounds is not valid [`Bounds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BoundsError {
    #[error("a domain must declare at least one bound")]
    Unbounded,
    #[error("the lower domain bound exceeds the upper bound")]
    Inverted,
}

impl<T: PartialOrd> Bounds<T> {
    /// Inclusive bounds from `min` to `max`.
    ///
    /// # Errors
    ///
    /// Returns [`BoundsError`] when neither bound is declared or when `min`
    /// exceeds `max`.
    pub fn new(min: Option<T>, max: Option<T>) -> Result<Self, BoundsError> {
        match (&min, &max) {
            (None, None) => Err(BoundsError::Unbounded),
            (Some(lower), Some(upper)) if lower > upper => Err(BoundsError::Inverted),
            _ => Ok(Self { min, max }),
        }
    }

    /// Whether `value` lies within the bounds.
    pub fn contains(&self, value: &T) -> bool {
        self.min.as_ref().is_none_or(|min| min <= value)
            && self.max.as_ref().is_none_or(|max| value <= max)
    }
}

impl<T> Bounds<T> {
    /// The inclusive lower bound, when declared.
    #[must_use]
    pub const fn min(&self) -> Option<&T> {
        self.min.as_ref()
    }

    /// The inclusive upper bound, when declared.
    #[must_use]
    pub const fn max(&self) -> Option<&T> {
        self.max.as_ref()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundsRepr<T> {
    min: Option<T>,
    max: Option<T>,
}

impl<T: PartialOrd> TryFrom<BoundsRepr<T>> for Bounds<T> {
    type Error = BoundsError;

    fn try_from(repr: BoundsRepr<T>) -> Result<Self, Self::Error> {
        Self::new(repr.min, repr.max)
    }
}

impl<T> From<Bounds<T>> for BoundsRepr<T> {
    fn from(bounds: Bounds<T>) -> Self {
        Self {
            min: bounds.min,
            max: bounds.max,
        }
    }
}

/// The declared domain of one parameter, in the parameter's wire unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Domain {
    /// Bounds of a real quantity.
    Quantity(Bounds<FiniteF64>),
    /// Bounds of an integer.
    Int(Bounds<i64>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_need_one_ordered_side() {
        assert_eq!(Bounds::<i64>::new(None, None), Err(BoundsError::Unbounded));
        assert_eq!(Bounds::new(Some(2), Some(1)), Err(BoundsError::Inverted));
        let bounds = Bounds::new(Some(0), None).unwrap();
        assert!(bounds.contains(&5));
        assert!(!bounds.contains(&-1));
    }

    #[test]
    fn domain_json_is_tagged_and_validated() {
        let domain = Domain::Int(Bounds::new(Some(0), Some(10)).unwrap());
        let json = serde_json::to_string(&domain).unwrap();
        assert_eq!(json, r#"{"kind":"int","min":0,"max":10}"#);
        assert_eq!(serde_json::from_str::<Domain>(&json).unwrap(), domain);
        assert!(serde_json::from_str::<Domain>(r#"{"kind":"int","min":3,"max":1}"#).is_err());
    }
}
