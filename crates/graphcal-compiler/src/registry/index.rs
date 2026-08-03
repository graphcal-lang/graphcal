use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;

use thiserror::Error;

use crate::dimension::Dimension;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};

/// Largest concrete index that Graphcal will materialize eagerly.
///
/// Index variants and indexed values are currently allocated eagerly. Keeping
/// this bound in the semantic cardinality type prevents an otherwise valid
/// source literal from turning into an unbounded allocation request.
pub const MAX_INDEX_CARDINALITY: usize = 1_000_000;

/// Validated, non-empty cardinality for an eagerly materialized index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IndexCardinality(NonZeroUsize);

impl IndexCardinality {
    /// Validate a source/runtime cardinality before any allocation.
    ///
    /// # Errors
    ///
    /// Returns an error for zero, values that do not fit the target, or values
    /// above Graphcal's practical eager-allocation limit.
    pub fn try_from_u64(value: u64) -> Result<Self, IndexCardinalityError> {
        if value == 0 {
            return Err(IndexCardinalityError::Empty);
        }
        let value =
            usize::try_from(value).map_err(|_| IndexCardinalityError::DoesNotFitUsize { value })?;
        if value > MAX_INDEX_CARDINALITY {
            return Err(IndexCardinalityError::TooLarge {
                value,
                maximum: MAX_INDEX_CARDINALITY,
            });
        }
        let value = NonZeroUsize::new(value).ok_or(IndexCardinalityError::Empty)?;
        Ok(Self(value))
    }

    /// Return the validated cardinality.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }

    /// Return the validated non-zero cardinality.
    #[must_use]
    pub const fn non_zero(self) -> NonZeroUsize {
        self.0
    }
}

/// Failure to validate a concrete index cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IndexCardinalityError {
    #[error("indexes must contain at least one element")]
    Empty,
    #[error("index cardinality {value} does not fit in usize on this target")]
    DoesNotFitUsize { value: u64 },
    #[error("index cardinality {value} exceeds the practical limit of {maximum} elements")]
    TooLarge { value: usize, maximum: usize },
}

/// Direct coordinate-generation rule for a concrete coordinate index.
#[derive(Debug, Clone, Copy)]
pub enum CoordinateSpacing {
    /// Exact increment supplied by `range(..., step: ...)`.
    Step { step: f64 },
    /// Exact count supplied by `linspace(..., points: ...)`.
    Linspace,
}

#[derive(Debug, Clone)]
pub struct CoordinateIndexData {
    pub start: f64,
    pub end: f64,
    pub spacing: CoordinateSpacing,
    /// Validated number of coordinate points.
    pub(crate) cardinality: IndexCardinality,
    pub dimension: Dimension,
    /// Display unit label (e.g., `"s"`) for formatting coordinate values.
    pub display_label: Option<String>,
    /// Scale factor from SI to display unit: `display_value = si_value / scale`.
    pub display_scale: f64,
}

impl CoordinateIndexData {
    /// Returns the SI coordinate at position `i`, computed directly rather than
    /// by cumulative addition.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "validated index cardinalities are far below f64's exact integer range"
    )]
    pub fn coordinate_value(&self, i: usize) -> f64 {
        match self.spacing {
            CoordinateSpacing::Step { .. } if i == self.cardinality.get() - 1 => self.end,
            CoordinateSpacing::Step { step } => (i as f64).mul_add(step, self.start),
            CoordinateSpacing::Linspace => match (i, self.cardinality.get()) {
                (0, _) => self.start,
                (position, count) if position + 1 == count => self.end,
                (position, count) => {
                    let t = position as f64 / (count - 1) as f64;
                    (1.0 - t).mul_add(self.start, t * self.end)
                }
            },
        }
    }

    /// Returns the number of coordinates in this index.
    #[must_use]
    pub const fn cardinality(&self) -> usize {
        self.cardinality.get()
    }
}

const fn index_position_key(position: usize) -> IndexEntryKey {
    IndexEntryKey::position(position as u64)
}

/// Coarse semantic category used when checking whether one index may bind another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexCategory {
    Named,
    Coordinate,
    Finite,
}

impl fmt::Display for IndexCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Named => "named",
            Self::Coordinate => "coordinate",
            Self::Finite => "finite",
        })
    }
}

/// Semantic category accepted by an index binding contract.
///
/// `Discrete` is the capability exposed by an unconstrained required index:
/// callers may supply either a label-bearing named axis or a structural
/// `Fin(N)` axis. Concrete named declarations remain nominal and therefore use
/// the narrower `Named` requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexBindingCategory {
    /// Only a concrete or forwarded named axis is accepted.
    Named,
    /// A named axis or structural finite axis is accepted.
    Discrete,
    /// Only a dimension-compatible coordinate axis is accepted.
    Coordinate,
}

impl fmt::Display for IndexBindingCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Named => "named",
            Self::Discrete => "named or finite",
            Self::Coordinate => "coordinate",
        })
    }
}

/// A required or overridable declared index's typed binding contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexBindingContract {
    /// A concrete named declaration can only retain another named identity.
    Named,
    /// An unconstrained required index accepts named and structural finite axes.
    Discrete,
    /// A coordinate declaration requires an axis with the same dimension.
    Coordinate { dimension: Dimension },
}

impl IndexBindingContract {
    /// Return the semantic index category required by this contract.
    #[must_use]
    pub const fn category(&self) -> IndexBindingCategory {
        match self {
            Self::Named => IndexBindingCategory::Named,
            Self::Discrete => IndexBindingCategory::Discrete,
            Self::Coordinate { .. } => IndexBindingCategory::Coordinate,
        }
    }

    /// Check a candidate index without erasing its category or coordinate dimension.
    ///
    /// Both concrete and required declared indexes are valid candidates. Allowing a
    /// compatible required candidate lets an outer generic DAG forward its own input
    /// to an inner DAG; the outer include chain must eventually provide a concrete axis.
    pub fn validate(&self, candidate: &IndexDef) -> Result<(), IndexBindingContractError> {
        let found = candidate.category();
        let kind_matches = matches!(
            (self.category(), found),
            (IndexBindingCategory::Named, IndexCategory::Named)
                | (
                    IndexBindingCategory::Discrete,
                    IndexCategory::Named | IndexCategory::Finite
                )
                | (IndexBindingCategory::Coordinate, IndexCategory::Coordinate)
        );
        if !kind_matches {
            return Err(IndexBindingContractError::KindMismatch {
                expected: self.category(),
                found,
            });
        }

        match (self, &candidate.kind) {
            (
                Self::Coordinate {
                    dimension: expected,
                },
                IndexKind::Coordinate(CoordinateIndexData {
                    dimension: found, ..
                })
                | IndexKind::RequiredCoordinate { dimension: found },
            ) if expected != found => Err(IndexBindingContractError::DimensionMismatch {
                expected: expected.clone(),
                found: found.clone(),
            }),
            _ => Ok(()),
        }
    }
}

/// Failure to satisfy a typed index binding contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IndexBindingContractError {
    #[error("index categories do not match")]
    KindMismatch {
        expected: IndexBindingCategory,
        found: IndexCategory,
    },
    #[error("coordinate-index dimensions do not match")]
    DimensionMismatch {
        expected: Dimension,
        found: Dimension,
    },
}

/// Closed semantic categories of indexes.
#[derive(Debug, Clone)]
pub enum IndexKind {
    /// A named label set, e.g. `index Maneuver = { Departure, Correction, Insertion };`
    Named { variants: Vec<IndexVariantName> },
    /// A coordinate index created by `range(..., step: ...)` or
    /// `linspace(..., points: ...)`.
    Coordinate(CoordinateIndexData),
    /// Required named index (no variants): must be bound via parameterized include.
    RequiredNamed,
    /// Required coordinate index with a dimension constraint.
    RequiredCoordinate { dimension: Dimension },
    /// A structural finite index `Fin(N)` with labels `0` through `N - 1`.
    Finite { cardinality: IndexCardinality },
}

/// A concrete or required index with its ordered elements.
#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: IndexName,
    pub kind: IndexKind,
}

impl IndexDef {
    /// Return this definition's coarse semantic category.
    #[must_use]
    pub const fn category(&self) -> IndexCategory {
        match &self.kind {
            IndexKind::Named { .. } | IndexKind::RequiredNamed => IndexCategory::Named,
            IndexKind::Coordinate(_) | IndexKind::RequiredCoordinate { .. } => {
                IndexCategory::Coordinate
            }
            IndexKind::Finite { .. } => IndexCategory::Finite,
        }
    }

    /// Returns typed entry keys in index order.
    ///
    /// Named indexes carry declared labels; coordinate and finite indexes carry
    /// numeric positions. Required indexes have no entries until bound.
    #[must_use]
    pub fn entry_keys(&self) -> Vec<IndexEntryKey> {
        match &self.kind {
            IndexKind::Named { variants } => {
                variants.iter().cloned().map(IndexEntryKey::named).collect()
            }
            IndexKind::Coordinate(data) => {
                (0..data.cardinality()).map(index_position_key).collect()
            }
            IndexKind::Finite { cardinality } => {
                (0..cardinality.get()).map(index_position_key).collect()
            }
            IndexKind::RequiredNamed | IndexKind::RequiredCoordinate { .. } => vec![],
        }
    }

    /// Returns the validated cardinality when this index is concrete.
    ///
    /// Required indexes have no cardinality until a caller binds them.
    #[must_use]
    pub fn concrete_cardinality(&self) -> Option<IndexCardinality> {
        match &self.kind {
            IndexKind::Named { variants } => {
                NonZeroUsize::new(variants.len()).map(IndexCardinality)
            }
            IndexKind::Coordinate(data) => Some(data.cardinality),
            IndexKind::Finite { cardinality } => Some(*cardinality),
            IndexKind::RequiredNamed | IndexKind::RequiredCoordinate { .. } => None,
        }
    }

    /// Returns the number of positions in this index.
    ///
    /// Returns 0 for required indexes (no variants until bound). Prefer
    /// [`Self::concrete_cardinality`] when required and concrete indexes must be
    /// distinguished semantically.
    #[must_use]
    pub const fn cardinality(&self) -> usize {
        match &self.kind {
            IndexKind::Named { variants } => variants.len(),
            IndexKind::Coordinate(data) => data.cardinality(),
            IndexKind::Finite { cardinality } => cardinality.get(),
            IndexKind::RequiredNamed | IndexKind::RequiredCoordinate { .. } => 0,
        }
    }

    /// Returns coordinate data if this is a concrete coordinate index.
    #[must_use]
    pub const fn coordinate_data(&self) -> Option<&CoordinateIndexData> {
        match &self.kind {
            IndexKind::Coordinate(data) => Some(data),
            _ => None,
        }
    }

    /// Returns true if this is a coordinate index (concrete or required).
    #[must_use]
    pub const fn is_coordinate(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::Coordinate(_) | IndexKind::RequiredCoordinate { .. }
        )
    }

    /// Returns true if this is a named index (concrete or required).
    #[must_use]
    pub const fn is_named(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::Named { .. } | IndexKind::RequiredNamed
        )
    }

    /// Returns true if this is a structural `Fin(N)` index.
    #[must_use]
    pub(crate) const fn is_finite_index(&self) -> bool {
        matches!(self.kind, IndexKind::Finite { .. })
    }

    /// Returns the cardinality of a concrete structural `Fin(N)` index.
    #[must_use]
    pub const fn finite_index_size(&self) -> Option<u64> {
        match &self.kind {
            IndexKind::Finite { cardinality } => Some(cardinality.get() as u64),
            _ => None,
        }
    }

    /// Returns true if this is a required index (must be bound via parameterized include).
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::RequiredNamed | IndexKind::RequiredCoordinate { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Finite structural indexes
// ---------------------------------------------------------------------------

/// Error returned when `Fin(N)` cannot become a concrete structural index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FiniteIndexError {
    #[error("Fin(0) is not allowed; indexes must contain at least one element")]
    Empty,
    #[error("Fin cardinality {size} does not fit in usize on this target")]
    DoesNotFitUsize { size: u64 },
    #[error("Fin cardinality {size} exceeds the practical limit of {maximum} elements")]
    TooLarge { size: usize, maximum: usize },
}

/// Typed identity for a concrete compiler-generated structural `Fin(N)` index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FiniteIndex {
    cardinality: IndexCardinality,
}

impl FiniteIndex {
    /// Create an identity from an already validated cardinality.
    #[must_use]
    pub(crate) const fn new(cardinality: IndexCardinality) -> Self {
        Self { cardinality }
    }

    /// Try to create an identity from an AST/runtime Nat cardinality.
    ///
    /// # Errors
    ///
    /// Returns an error for zero, an unrepresentable value, or a value above
    /// the practical eager-allocation limit.
    pub fn try_from_u64(size: u64) -> Result<Self, FiniteIndexError> {
        IndexCardinality::try_from_u64(size)
            .map(Self::new)
            .map_err(|error| match error {
                IndexCardinalityError::Empty => FiniteIndexError::Empty,
                IndexCardinalityError::DoesNotFitUsize { value } => {
                    FiniteIndexError::DoesNotFitUsize { size: value }
                }
                IndexCardinalityError::TooLarge { value, maximum } => FiniteIndexError::TooLarge {
                    size: value,
                    maximum,
                },
            })
    }

    /// Return the validated cardinality.
    #[must_use]
    pub const fn cardinality(self) -> IndexCardinality {
        self.cardinality
    }

    /// Return the cardinality as a `u64` for Nat-expression comparisons and display.
    #[must_use]
    pub(crate) const fn size_u64(self) -> u64 {
        self.cardinality.get() as u64
    }

    /// Render this identity at a source/diagnostic boundary.
    #[must_use]
    pub fn display_name(self) -> IndexName {
        IndexName::expect_valid(format!("Fin({})", self.size_u64()))
    }
}

/// Resolved importer-side argument supplied to an include index port.
///
/// Declared axes retain their typed declaration names. Structural axes retain
/// their validated finite identity instead of fabricating a recoverable name
/// such as `"Fin(3)"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexBindingTarget {
    /// A declared axis in the including DAG's registry.
    Declared(IndexName),
    /// A validated structural finite identity.
    Finite(FiniteIndex),
}

impl IndexBindingTarget {
    /// Return the declared target name, when the argument names a declaration.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&IndexName> {
        match self {
            Self::Declared(name) => Some(name),
            Self::Finite(_) => None,
        }
    }
}

impl fmt::Display for IndexBindingTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declared(name) => name.fmt(formatter),
            Self::Finite(index) => index.display_name().fmt(formatter),
        }
    }
}

/// Index registry: maps declared names and typed structural identities to definitions.
#[derive(Debug, Clone)]
pub struct IndexRegistry {
    pub(crate) indexes: HashMap<IndexName, IndexDef>,
    pub(crate) finite_indexes: HashMap<FiniteIndex, IndexDef>,
}

impl IndexRegistry {
    /// Look up a declared index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    /// Look up a compiler-generated structural index by typed identity.
    #[must_use]
    pub fn get_finite_index(&self, index: FiniteIndex) -> Option<&IndexDef> {
        self.finite_indexes.get(&index)
    }

    /// Iterate over declared index definitions.
    pub fn declared_indexes(&self) -> impl Iterator<Item = &IndexDef> {
        self.indexes.values()
    }

    /// Iterate over compiler-generated structural index identities.
    pub fn finite_indexes(&self) -> impl Iterator<Item = FiniteIndex> + '_ {
        self.finite_indexes.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_indexes_have_no_concrete_cardinality() {
        let definition = IndexDef {
            name: IndexName::expect_valid("Axis"),
            kind: IndexKind::RequiredNamed,
        };
        assert_eq!(definition.concrete_cardinality(), None);
        assert_eq!(definition.cardinality(), 0);
    }

    #[test]
    fn binding_contract_accepts_compatible_required_forwarding() {
        let time = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Time,
        ));
        let contract = IndexBindingContract::Coordinate {
            dimension: time.clone(),
        };
        let candidate = IndexDef {
            name: IndexName::expect_valid("OuterStep"),
            kind: IndexKind::RequiredCoordinate { dimension: time },
        };

        assert_eq!(contract.validate(&candidate), Ok(()));
    }

    #[test]
    fn discrete_binding_contract_accepts_named_and_finite_axes() {
        let contract = IndexBindingContract::Discrete;
        let named = IndexDef {
            name: IndexName::expect_valid("Phase"),
            kind: IndexKind::Named {
                variants: vec![IndexVariantName::expect_valid("Only")],
            },
        };
        let finite = IndexDef {
            name: IndexName::expect_valid("Fin(3)"),
            kind: IndexKind::Finite {
                cardinality: IndexCardinality::try_from_u64(3).unwrap(),
            },
        };

        assert_eq!(contract.validate(&named), Ok(()));
        assert_eq!(contract.validate(&finite), Ok(()));
        assert_eq!(
            IndexBindingContract::Named.validate(&finite),
            Err(IndexBindingContractError::KindMismatch {
                expected: IndexBindingCategory::Named,
                found: IndexCategory::Finite,
            })
        );
    }

    #[test]
    fn binding_contract_preserves_kind_and_dimension_errors() {
        let time = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Time,
        ));
        let length = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ));
        let contract = IndexBindingContract::Coordinate {
            dimension: time.clone(),
        };
        let named = IndexDef {
            name: IndexName::expect_valid("Phase"),
            kind: IndexKind::Named {
                variants: vec![IndexVariantName::expect_valid("Only")],
            },
        };
        assert_eq!(
            contract.validate(&named),
            Err(IndexBindingContractError::KindMismatch {
                expected: IndexBindingCategory::Coordinate,
                found: IndexCategory::Named,
            })
        );

        let coordinate = IndexDef {
            name: IndexName::expect_valid("DistanceStep"),
            kind: IndexKind::RequiredCoordinate {
                dimension: length.clone(),
            },
        };
        assert_eq!(
            contract.validate(&coordinate),
            Err(IndexBindingContractError::DimensionMismatch {
                expected: time,
                found: length,
            })
        );
    }

    #[test]
    fn finite_and_coordinate_entry_keys_are_typed_positions() {
        let cardinality = IndexCardinality::try_from_u64(3).unwrap();
        for kind in [
            IndexKind::Finite { cardinality },
            IndexKind::Coordinate(CoordinateIndexData {
                start: 0.0,
                end: 2.0,
                spacing: CoordinateSpacing::Step { step: 1.0 },
                cardinality,
                dimension: Dimension::dimensionless(),
                display_label: None,
                display_scale: 1.0,
            }),
        ] {
            let definition = IndexDef {
                name: IndexName::expect_valid("Axis"),
                kind,
            };
            assert_eq!(
                definition.entry_keys(),
                vec![
                    IndexEntryKey::Position(0),
                    IndexEntryKey::Position(1),
                    IndexEntryKey::Position(2),
                ]
            );
        }
    }
}
