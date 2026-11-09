//! Fixed index axes of indexed wire values and keys, and the keys that
//! select one of their entries.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::declared_name::DeclaredName;
use crate::dimension::Dimension;
use crate::finite::FiniteF64;
use crate::label::{IndexName, VariantName};

/// The entries of one fixed index axis, in declaration order, with the
/// identity of a declared index.
#[derive(Debug, Clone, PartialEq)]
pub enum AxisKind {
    /// A declared index of labels.
    Named {
        /// The declared index.
        index: DeclaredName<IndexName>,
        /// Labels in declaration order.
        variants: Vec<VariantName>,
    },
    /// The structural axis `Fin(size)`: positions `0..size`. It is
    /// identified by its size alone.
    Finite {
        /// The number of positions.
        size: usize,
    },
    /// A declared index of physical coordinates of one dimension.
    Coordinate {
        /// The declared index.
        index: DeclaredName<IndexName>,
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
    kind: AxisKind,
}

/// Why an axis description is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AxisError {
    #[error("an index axis has no entries")]
    Empty,
    #[error("index `{index}` repeats variant `{variant}`")]
    DuplicateVariant {
        index: DeclaredName<IndexName>,
        variant: VariantName,
    },
    #[error("an index axis has {size} entries, more than an Arrow list can hold")]
    TooLarge { size: usize },
}

impl Axis {
    /// An axis with entries `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`AxisError`] for an empty axis, a repeated variant, or an
    /// axis longer than an Arrow `FixedSizeList` (`i32::MAX`).
    pub fn new(kind: AxisKind) -> Result<Self, AxisError> {
        let size = match &kind {
            AxisKind::Named { index, variants } => {
                let mut seen = HashSet::with_capacity(variants.len());
                if let Some(variant) = variants.iter().find(|variant| !seen.insert(*variant)) {
                    return Err(AxisError::DuplicateVariant {
                        index: index.clone(),
                        variant: variant.clone(),
                    });
                }
                variants.len()
            }
            AxisKind::Finite { size } => *size,
            AxisKind::Coordinate { coordinates, .. } => coordinates.len(),
        };
        if size == 0 {
            return Err(AxisError::Empty);
        }
        if i32::try_from(size).is_err() {
            return Err(AxisError::TooLarge { size });
        }
        Ok(Self { kind })
    }

    /// The declared index, or `None` for a `Fin` axis.
    #[must_use]
    pub const fn index(&self) -> Option<&DeclaredName<IndexName>> {
        match &self.kind {
            AxisKind::Named { index, .. } | AxisKind::Coordinate { index, .. } => Some(index),
            AxisKind::Finite { .. } => None,
        }
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
            AxisKind::Named { variants, .. } => variants.len(),
            AxisKind::Finite { size } => *size,
            AxisKind::Coordinate { coordinates, .. } => coordinates.len(),
        }
    }

    /// Always `false`: an axis has at least one entry.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// The key of the entry at `position`, when it is in range.
    #[must_use]
    pub fn key(&self, position: usize) -> Option<KeyRef<'_>> {
        match &self.kind {
            AxisKind::Named { variants, .. } => variants.get(position).map(KeyRef::Variant),
            AxisKind::Finite { size } => (position < *size).then_some(KeyRef::Position(position)),
            AxisKind::Coordinate { coordinates, .. } => {
                coordinates.get(position).copied().map(KeyRef::Coordinate)
            }
        }
    }

    /// The position of the entry `key` selects: a variant by name, a `Fin`
    /// position in range, or a coordinate equal to one of the axis's. A key
    /// of another axis kind selects no entry.
    #[must_use]
    pub fn position(&self, key: KeyRef<'_>) -> Option<usize> {
        match (&self.kind, key) {
            (AxisKind::Named { variants, .. }, KeyRef::Variant(name)) => {
                variants.iter().position(|variant| variant == name)
            }
            (AxisKind::Finite { size }, KeyRef::Position(position)) => {
                (position < *size).then_some(position)
            }
            (AxisKind::Coordinate { coordinates, .. }, KeyRef::Coordinate(coordinate)) => {
                coordinates
                    .iter()
                    .position(|candidate| *candidate == coordinate)
            }
            _ => None,
        }
    }
}

/// A key: one entry of an axis, as the protocol identifies it.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyValue {
    /// A variant of a named axis.
    Variant(VariantName),
    /// A zero-based position of a `Fin` axis.
    Position(usize),
    /// A coordinate of a coordinate axis, in the canonical SI unit.
    Coordinate(FiniteF64),
}

impl KeyValue {
    /// A borrowed view of this key.
    #[must_use]
    pub const fn as_key_ref(&self) -> KeyRef<'_> {
        match self {
            Self::Variant(variant) => KeyRef::Variant(variant),
            Self::Position(position) => KeyRef::Position(*position),
            Self::Coordinate(coordinate) => KeyRef::Coordinate(*coordinate),
        }
    }
}

impl fmt::Display for KeyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_key_ref().fmt(f)
    }
}

/// A borrowed [`KeyValue`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyRef<'a> {
    /// A variant of a named axis.
    Variant(&'a VariantName),
    /// A zero-based position of a `Fin` axis.
    Position(usize),
    /// A coordinate of a coordinate axis, in the canonical SI unit.
    Coordinate(FiniteF64),
}

impl KeyRef<'_> {
    /// An owned copy of this key.
    #[must_use]
    pub fn to_key_value(self) -> KeyValue {
        match self {
            Self::Variant(variant) => KeyValue::Variant(variant.clone()),
            Self::Position(position) => KeyValue::Position(position),
            Self::Coordinate(coordinate) => KeyValue::Coordinate(coordinate),
        }
    }
}

impl fmt::Display for KeyRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Variant(variant) => write!(f, "variant `{variant}`"),
            Self::Position(position) => write!(f, "position {position}"),
            Self::Coordinate(coordinate) => write!(f, "coordinate {}", coordinate.get()),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AxisRepr {
    Named {
        index: DeclaredName<IndexName>,
        variants: Vec<VariantName>,
    },
    Finite {
        size: usize,
    },
    Coordinate {
        index: DeclaredName<IndexName>,
        dimension: Dimension,
        coordinates: Vec<FiniteF64>,
    },
}

impl TryFrom<AxisRepr> for Axis {
    type Error = AxisError;

    fn try_from(repr: AxisRepr) -> Result<Self, Self::Error> {
        Self::new(match repr {
            AxisRepr::Named { index, variants } => AxisKind::Named { index, variants },
            AxisRepr::Finite { size } => AxisKind::Finite { size },
            AxisRepr::Coordinate {
                index,
                dimension,
                coordinates,
            } => AxisKind::Coordinate {
                index,
                dimension,
                coordinates,
            },
        })
    }
}

impl From<Axis> for AxisRepr {
    fn from(axis: Axis) -> Self {
        match axis.kind {
            AxisKind::Named { index, variants } => Self::Named { index, variants },
            AxisKind::Finite { size } => Self::Finite { size },
            AxisKind::Coordinate {
                index,
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
pub mod tests {
    use super::*;
    use crate::label::PackageName;

    fn variant(name: &str) -> VariantName {
        VariantName::new(name).unwrap()
    }

    /// The index `name` declared in the root module of package `model`.
    pub fn declared_index(name: &str) -> DeclaredName<IndexName> {
        DeclaredName::new(
            PackageName::new("model").unwrap(),
            Vec::new(),
            IndexName::new(name).unwrap(),
        )
    }

    #[test]
    fn axes_are_non_empty_and_unique() {
        assert!(matches!(
            Axis::new(AxisKind::Finite { size: 0 }),
            Err(AxisError::Empty)
        ));
        assert!(matches!(
            Axis::new(AxisKind::Named {
                index: declared_index("Phase"),
                variants: vec![variant("A"), variant("A")]
            }),
            Err(AxisError::DuplicateVariant { .. })
        ));
        let axis = Axis::new(AxisKind::Named {
            index: declared_index("Phase"),
            variants: vec![variant("Climb"), variant("Cruise")],
        })
        .unwrap();
        assert_eq!(axis.len(), 2);
        let json = serde_json::to_string(&axis).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"named","index":{"package":"model","module":[],"name":"Phase"},"variants":["Climb","Cruise"]}"#
        );
        assert_eq!(serde_json::from_str::<Axis>(&json).unwrap(), axis);
        assert!(serde_json::from_str::<Axis>(r#"{"kind":"finite","size":0}"#).is_err());
        assert!(
            serde_json::from_str::<Axis>(r#"{"kind":"finite","index":"Fin(2)","size":2}"#).is_err(),
            "a Fin axis has no declared index"
        );
    }

    #[test]
    fn keys_select_entries_of_their_own_axis_kind() {
        let named = Axis::new(AxisKind::Named {
            index: declared_index("Phase"),
            variants: vec![variant("Climb"), variant("Cruise")],
        })
        .unwrap();
        let finite = Axis::new(AxisKind::Finite { size: 3 }).unwrap();
        let coordinate = |value: f64| FiniteF64::new(value).unwrap();
        let grid = Axis::new(AxisKind::Coordinate {
            index: declared_index("Step"),
            dimension: Dimension::default(),
            coordinates: vec![coordinate(0.0), coordinate(0.5)],
        })
        .unwrap();

        let cruise = variant("Cruise");
        assert_eq!(named.position(KeyRef::Variant(&cruise)), Some(1));
        assert_eq!(named.key(1), Some(KeyRef::Variant(&cruise)));
        assert_eq!(named.position(KeyRef::Position(1)), None);

        assert_eq!(finite.position(KeyRef::Position(2)), Some(2));
        assert_eq!(finite.position(KeyRef::Position(3)), None);
        assert_eq!(finite.key(3), None);
        assert_eq!(finite.index(), None);

        assert_eq!(grid.position(KeyRef::Coordinate(coordinate(0.5))), Some(1));
        assert_eq!(grid.position(KeyRef::Coordinate(coordinate(0.25))), None);
        assert_eq!(grid.key(0), Some(KeyRef::Coordinate(coordinate(0.0))));
        assert_eq!(grid.index(), Some(&declared_index("Step")));
        assert_eq!(
            KeyValue::Coordinate(coordinate(0.5)).to_string(),
            "coordinate 0.5"
        );
    }

    #[test]
    fn indexes_of_the_same_name_in_different_modules_differ() {
        let axis = |module: &[&str]| {
            Axis::new(AxisKind::Named {
                index: DeclaredName::new(
                    PackageName::new("model").unwrap(),
                    module
                        .iter()
                        .map(|segment| crate::label::ModuleName::new(*segment).unwrap())
                        .collect(),
                    IndexName::new("Mode").unwrap(),
                ),
                variants: vec![variant("A")],
            })
            .unwrap()
        };
        assert_ne!(axis(&["left"]), axis(&["right"]));
        assert_ne!(
            serde_json::to_string(&axis(&["left"])).unwrap(),
            serde_json::to_string(&axis(&["right"])).unwrap()
        );
    }
}
