//! Fixed index axes of indexed wire values.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dimension::Dimension;
use crate::finite::FiniteF64;
use crate::label::{IndexName, VariantName};

/// The entries of one fixed index axis, in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub enum AxisKind {
    /// Declared labels.
    Named {
        /// Labels in declaration order.
        variants: Vec<VariantName>,
    },
    /// The structural axis `Fin(size)`: positions `0..size`.
    Finite {
        /// The number of positions.
        size: usize,
    },
    /// Physical coordinates of one dimension.
    Coordinate {
        /// The coordinates' dimension.
        dimension: Dimension,
        /// Coordinates in the canonical SI unit, in declaration order.
        coordinates: Vec<FiniteF64>,
    },
}

/// One fixed index axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AxisRepr", into = "AxisRepr")]
pub struct Axis {
    index: IndexName,
    kind: AxisKind,
}

/// Why an axis description is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AxisError {
    #[error("index `{index}` has no entries")]
    Empty { index: IndexName },
    #[error("index `{index}` repeats variant `{variant}`")]
    DuplicateVariant {
        index: IndexName,
        variant: VariantName,
    },
    #[error("index `{index}` has {size} entries, more than an Arrow list can hold")]
    TooLarge { index: IndexName, size: usize },
}

impl Axis {
    /// An axis of `index` with entries `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`AxisError`] for an empty axis, a repeated variant, or an
    /// axis longer than an Arrow `FixedSizeList` (`i32::MAX`).
    pub fn new(index: IndexName, kind: AxisKind) -> Result<Self, AxisError> {
        let size = match &kind {
            AxisKind::Named { variants } => {
                let mut seen = HashSet::with_capacity(variants.len());
                if let Some(variant) = variants.iter().find(|variant| !seen.insert(*variant)) {
                    return Err(AxisError::DuplicateVariant {
                        variant: variant.clone(),
                        index,
                    });
                }
                variants.len()
            }
            AxisKind::Finite { size } => *size,
            AxisKind::Coordinate { coordinates, .. } => coordinates.len(),
        };
        if size == 0 {
            return Err(AxisError::Empty { index });
        }
        if i32::try_from(size).is_err() {
            return Err(AxisError::TooLarge { index, size });
        }
        Ok(Self { index, kind })
    }

    /// The index identity.
    #[must_use]
    pub const fn index(&self) -> &IndexName {
        &self.index
    }

    /// The entries.
    #[must_use]
    pub const fn kind(&self) -> &AxisKind {
        &self.kind
    }

    /// The number of entries (at least one).
    #[must_use]
    pub const fn len(&self) -> usize {
        match &self.kind {
            AxisKind::Named { variants } => variants.len(),
            AxisKind::Finite { size } => *size,
            AxisKind::Coordinate { coordinates, .. } => coordinates.len(),
        }
    }

    /// Always `false`: an axis has at least one entry.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AxisRepr {
    Named {
        index: IndexName,
        variants: Vec<VariantName>,
    },
    Finite {
        index: IndexName,
        size: usize,
    },
    Coordinate {
        index: IndexName,
        dimension: Dimension,
        coordinates: Vec<FiniteF64>,
    },
}

impl TryFrom<AxisRepr> for Axis {
    type Error = AxisError;

    fn try_from(repr: AxisRepr) -> Result<Self, Self::Error> {
        let (index, kind) = match repr {
            AxisRepr::Named { index, variants } => (index, AxisKind::Named { variants }),
            AxisRepr::Finite { index, size } => (index, AxisKind::Finite { size }),
            AxisRepr::Coordinate {
                index,
                dimension,
                coordinates,
            } => (
                index,
                AxisKind::Coordinate {
                    dimension,
                    coordinates,
                },
            ),
        };
        Self::new(index, kind)
    }
}

impl From<Axis> for AxisRepr {
    fn from(axis: Axis) -> Self {
        let index = axis.index;
        match axis.kind {
            AxisKind::Named { variants } => Self::Named { index, variants },
            AxisKind::Finite { size } => Self::Finite { index, size },
            AxisKind::Coordinate {
                dimension,
                coordinates,
            } => Self::Coordinate {
                index,
                dimension,
                coordinates,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(name: &str) -> VariantName {
        VariantName::new(name).unwrap()
    }

    #[test]
    fn axes_are_non_empty_and_unique() {
        let index = IndexName::new("Phase").unwrap();
        assert!(matches!(
            Axis::new(index.clone(), AxisKind::Finite { size: 0 }),
            Err(AxisError::Empty { .. })
        ));
        assert!(matches!(
            Axis::new(
                index.clone(),
                AxisKind::Named {
                    variants: vec![variant("A"), variant("A")]
                }
            ),
            Err(AxisError::DuplicateVariant { .. })
        ));
        let axis = Axis::new(
            index,
            AxisKind::Named {
                variants: vec![variant("Climb"), variant("Cruise")],
            },
        )
        .unwrap();
        assert_eq!(axis.len(), 2);
        let json = serde_json::to_string(&axis).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"named","index":"Phase","variants":["Climb","Cruise"]}"#
        );
        assert_eq!(serde_json::from_str::<Axis>(&json).unwrap(), axis);
        assert!(
            serde_json::from_str::<Axis>(r#"{"index":"Fin(0)","kind":"finite","size":0}"#).is_err()
        );
    }
}
