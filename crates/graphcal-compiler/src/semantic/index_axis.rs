//! A concrete index axis as runtime values see it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::finite_value::FiniteQuantity;
use crate::semantic::checked_type::IndexTypeRef;
use crate::semantic::index_def::{ConcreteIndexKind, CoordinateIndexData, IndexCardinality};
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::non_empty::NonEmpty;

/// A concrete index together with its ordered entry keys.
///
/// The keys are derived from the index's own concrete definition, so an axis
/// is never empty and its keys are exactly the index's entries in declaration
/// order. A coordinate axis also holds its coordinates, each proven finite
/// when the axis is built. Cloning shares the key set.
///
/// Two axes are equal when they enumerate the same index identity: the keys
/// and coordinates are functions of that identity within one program.
#[derive(Clone)]
pub struct IndexAxis(Arc<AxisData>);

#[derive(Debug)]
struct AxisData {
    /// The index as this axis's users spell it.
    index: IndexTypeRef,
    /// What the index's definition determines, shared by every spelling.
    content: Arc<AxisContent>,
}

#[derive(Debug)]
struct AxisContent {
    kind: ConcreteIndexKind,
    keys: AxisKeys,
    /// One finite coordinate per key; empty unless `kind` is a coordinate index.
    coordinates: Vec<FiniteQuantity>,
}

/// The entry keys of an axis.
#[derive(Debug)]
enum AxisKeys {
    /// Coordinate and `Fin` axes are keyed by position `0..len`, so building
    /// one costs nothing per entry, however long it is.
    Positional(IndexCardinality),
    /// A named axis keys each entry by its variant.
    Named {
        keys: NonEmpty<IndexEntryKey>,
        positions: HashMap<IndexEntryKey, usize>,
    },
}

/// Why a concrete index has no axis.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IndexAxisError {
    /// A coordinate of a coordinate index is not finite.
    #[error("coordinate #{position} of the index is not finite")]
    NonFiniteCoordinate { position: usize },
}

/// Why an index of a checked program has no axis.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolveAxisError {
    /// The program defines no such index.
    #[error("the index is not defined")]
    Unknown,
    /// The index is a required port not bound to a concrete definition.
    #[error("the index is required and not bound")]
    Required,
    /// The concrete definition has no axis.
    #[error(transparent)]
    Axis(IndexAxisError),
}

/// Debug output names the axis by its identity, like its equality.
impl std::fmt::Debug for IndexAxis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("IndexAxis").field(&self.0.index).finish()
    }
}

impl PartialEq for IndexAxis {
    fn eq(&self, other: &Self) -> bool {
        self.matches(other)
    }
}

impl IndexAxis {
    /// The axis of a structural `Fin(N)` index, which needs no registry.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn finite(index: crate::semantic::index_def::FiniteIndex) -> Self {
        Self::positional(
            IndexTypeRef::from_finite_index(index),
            ConcreteIndexKind::Finite { index },
            Vec::new(),
        )
    }

    /// The axis of a named index whose identity is given directly, for tests.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    #[expect(
        clippy::unwrap_used,
        reason = "a test fixture panics on an invalid axis"
    )]
    pub fn named_for_test(owner: crate::dag_id::DagId, index: &str, variants: &[&str]) -> Self {
        use crate::syntax::index_name::{IndexName, IndexVariantName};
        use crate::syntax::non_empty::NonEmptyUnique;
        let variants = variants
            .iter()
            .map(|variant| IndexVariantName::expect_valid(*variant))
            .collect::<Vec<_>>();
        let variants =
            NonEmptyUnique::try_from_non_empty(NonEmpty::try_from_vec(variants).unwrap()).unwrap();
        Self::from_concrete(
            IndexTypeRef::with_owner(owner, IndexName::expect_valid(index)),
            ConcreteIndexKind::Named { variants },
        )
        .unwrap()
    }

    /// The axis of a coordinate index whose identity is given directly, for
    /// tests.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    #[expect(
        clippy::unwrap_used,
        reason = "a test fixture panics on an invalid axis"
    )]
    pub fn coordinate_for_test(
        owner: crate::dag_id::DagId,
        index: &str,
        data: CoordinateIndexData,
    ) -> Self {
        use crate::syntax::index_name::IndexName;
        Self::from_concrete(
            IndexTypeRef::with_owner(owner, IndexName::expect_valid(index)),
            ConcreteIndexKind::Coordinate(data),
        )
        .unwrap()
    }

    /// The axis of `index`, whose concrete definition is `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`IndexAxisError::NonFiniteCoordinate`] when a coordinate of
    /// a coordinate index is not finite.
    pub(crate) fn from_concrete(
        index: IndexTypeRef,
        kind: ConcreteIndexKind,
    ) -> Result<Self, IndexAxisError> {
        match &kind {
            ConcreteIndexKind::Named { variants } => {
                let keys = variants.as_non_empty().clone().map(IndexEntryKey::named);
                let positions = keys
                    .iter()
                    .enumerate()
                    .map(|(position, key)| (key.clone(), position))
                    .collect();
                Ok(Self::with_content(
                    index,
                    AxisContent {
                        kind,
                        keys: AxisKeys::Named { keys, positions },
                        coordinates: Vec::new(),
                    },
                ))
            }
            ConcreteIndexKind::Coordinate(data) => {
                let coordinates = (0..kind.cardinality().get())
                    .map(|position| {
                        FiniteQuantity::try_new(data.coordinate_value(position))
                            .map_err(|_| IndexAxisError::NonFiniteCoordinate { position })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::positional(index, kind, coordinates))
            }
            ConcreteIndexKind::Finite { .. } => Ok(Self::positional(index, kind, Vec::new())),
        }
    }

    /// A coordinate or `Fin` axis, keyed by position.
    fn positional(
        index: IndexTypeRef,
        kind: ConcreteIndexKind,
        coordinates: Vec<FiniteQuantity>,
    ) -> Self {
        let len = kind.cardinality();
        Self::with_content(
            index,
            AxisContent {
                kind,
                keys: AxisKeys::Positional(len),
                coordinates,
            },
        )
    }

    fn with_content(index: IndexTypeRef, content: AxisContent) -> Self {
        Self(Arc::new(AxisData {
            index,
            content: Arc::new(content),
        }))
    }

    /// This axis spelled as `index`, an index with the same identity:
    /// the keys and coordinates are shared, not rebuilt.
    #[must_use]
    pub(crate) fn respelled(&self, index: IndexTypeRef) -> Self {
        debug_assert!(self.0.index.matches_ref(&index));
        Self(Arc::new(AxisData {
            index,
            content: Arc::clone(&self.0.content),
        }))
    }

    /// The index this axis enumerates.
    #[must_use]
    pub fn index(&self) -> &IndexTypeRef {
        &self.0.index
    }

    /// The concrete definition the keys were derived from.
    #[must_use]
    pub fn kind(&self) -> &ConcreteIndexKind {
        &self.0.content.kind
    }

    /// Coordinate data, when this is a coordinate axis.
    #[must_use]
    pub fn coordinate_data(&self) -> Option<&CoordinateIndexData> {
        match &self.0.content.kind {
            ConcreteIndexKind::Coordinate(data) => Some(data),
            ConcreteIndexKind::Named { .. } | ConcreteIndexKind::Finite { .. } => None,
        }
    }

    /// The entry keys in axis order. A positional axis computes each key
    /// on demand; a named axis lends its stored keys.
    pub fn keys(&self) -> impl ExactSizeIterator<Item = Cow<'_, IndexEntryKey>> + '_ {
        (0..self.len()).map(|position| self.key_at(position))
    }

    /// The key at `position`, which must lie on this axis.
    ///
    /// # Panics
    ///
    /// Panics when a named axis has no entry at `position`, like indexing a
    /// slice.
    #[must_use]
    pub fn key_at(&self, position: usize) -> Cow<'_, IndexEntryKey> {
        match &self.0.content.keys {
            AxisKeys::Positional(_) => Cow::Owned(position_key(position)),
            AxisKeys::Named { keys, .. } => Cow::Borrowed(&keys.as_slice()[position]),
        }
    }

    /// The key of the entry at `position`, when the axis has one.
    #[must_use]
    pub fn key(&self, position: usize) -> Option<Cow<'_, IndexEntryKey>> {
        match &self.0.content.keys {
            AxisKeys::Positional(len) => {
                (position < len.get()).then(|| Cow::Owned(position_key(position)))
            }
            AxisKeys::Named { keys, .. } => keys.as_slice().get(position).map(Cow::Borrowed),
        }
    }

    /// Whether both axes have the same keys in the same order.
    #[must_use]
    pub fn same_keys(&self, other: &Self) -> bool {
        match (&self.0.content.keys, &other.0.content.keys) {
            (AxisKeys::Positional(len), AxisKeys::Positional(other)) => len == other,
            (AxisKeys::Named { keys, .. }, AxisKeys::Named { keys: other, .. }) => keys == other,
            (AxisKeys::Positional(_), AxisKeys::Named { .. })
            | (AxisKeys::Named { .. }, AxisKeys::Positional(_)) => false,
        }
    }

    /// Number of entries (at least one).
    #[must_use]
    #[expect(
        clippy::len_without_is_empty,
        reason = "an index axis has at least one entry, so it is never empty"
    )]
    pub fn len(&self) -> usize {
        match &self.0.content.keys {
            AxisKeys::Positional(len) => len.get(),
            AxisKeys::Named { keys, .. } => keys.len(),
        }
    }

    /// The finite coordinates of a coordinate axis, one per key in axis
    /// order; empty for named and `Fin` axes.
    #[must_use]
    pub fn coordinates(&self) -> &[FiniteQuantity] {
        &self.0.content.coordinates
    }

    /// Position of `key` on this axis.
    #[must_use]
    pub fn position(&self, key: &IndexEntryKey) -> Option<usize> {
        match (&self.0.content.keys, key) {
            (AxisKeys::Positional(len), IndexEntryKey::Position(position)) => {
                usize::try_from(*position)
                    .ok()
                    .filter(|position| *position < len.get())
            }
            (AxisKeys::Named { positions, .. }, IndexEntryKey::Named(_)) => {
                positions.get(key).copied()
            }
            (AxisKeys::Positional(_), IndexEntryKey::Named(_))
            | (AxisKeys::Named { .. }, IndexEntryKey::Position(_)) => None,
        }
    }

    /// Whether both axes are one shared build.
    #[cfg(test)]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.content, &other.0.content)
    }

    /// Whether both axes enumerate the same index.
    ///
    /// Keys are derived from the index definition, so equal indexes have equal
    /// keys.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.0.index.matches_ref(&other.0.index)
    }

    /// Whether a key of `key_axis` selects an entry of this axis: the same
    /// index, or — `Fin`-key widening — a structural `Fin(N)` key on a
    /// `Fin(M)` axis with `N <= M`. Positions of an admitted key are
    /// positions of this axis.
    #[must_use]
    pub fn admits(&self, key_axis: &Self) -> bool {
        match (&self.0.content.kind, &key_axis.0.content.kind) {
            (ConcreteIndexKind::Finite { .. }, ConcreteIndexKind::Finite { .. }) => {
                key_axis.len() <= self.len()
            }
            _ => self.matches(key_axis),
        }
    }
}

/// The key of position `position` of a positional axis.
fn position_key(position: usize) -> IndexEntryKey {
    IndexEntryKey::position(u64::try_from(position).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use crate::dag_id::DagId;
    use crate::semantic::index_def::FiniteIndex;
    use crate::syntax::index_name::{IndexEntryKey, IndexVariantName};

    use super::IndexAxis;

    fn owner() -> DagId {
        DagId::root_in_package("axis-tests", "main")
    }

    #[test]
    fn named_axis_enumerates_declared_variants_in_order() {
        let axis = IndexAxis::named_for_test(owner(), "Phase", &["Launch", "Cruise", "Landing"]);
        let cruise = IndexEntryKey::named(IndexVariantName::expect_valid("Cruise"));
        assert_eq!(axis.len(), 3);
        assert_eq!(
            axis.keys().next().as_deref(),
            Some(&IndexEntryKey::named(IndexVariantName::expect_valid(
                "Launch"
            )))
        );
        assert_eq!(axis.position(&cruise), Some(1));
        assert_eq!(axis.position(&IndexEntryKey::position(1)), None);
        assert!(axis.coordinate_data().is_none());
    }

    #[test]
    fn positional_axes_store_no_key_per_entry() {
        // A `Fin(N)` axis used to allocate a key and a position entry per
        // element: about 95 MB for `Fin(1000000)`, once per DAG instance.
        let size = crate::semantic::index_def::MAX_INDEX_CARDINALITY;
        let axis = IndexAxis::finite(FiniteIndex::try_from_u64(size as u64).unwrap());
        assert!(matches!(
            axis.0.content.keys,
            super::AxisKeys::Positional(_)
        ));
        assert_eq!(axis.len(), size);
        let last = IndexEntryKey::position(size as u64 - 1);
        assert_eq!(axis.position(&last), Some(size - 1));
        assert_eq!(axis.key(size - 1).as_deref(), Some(&last));
        assert_eq!(axis.position(&IndexEntryKey::position(size as u64)), None);
        assert_eq!(axis.key(size), None);
        assert_eq!(axis.keys().len(), size);
    }

    #[test]
    fn finite_axis_uses_positions() {
        let axis = IndexAxis::finite(FiniteIndex::try_from_u64(3).unwrap());
        assert_eq!(axis.len(), 3);
        assert_eq!(axis.position(&IndexEntryKey::position(2)), Some(2));
        assert_eq!(axis.position(&IndexEntryKey::position(3)), None);
    }

    #[test]
    fn coordinate_axes_hold_one_finite_coordinate_per_key() {
        use crate::dimension::Dimension;
        use crate::semantic::index_def::{CoordinateDisplayUnit, CoordinateIndexData};
        let data = CoordinateIndexData::try_range(
            0.0,
            2.0,
            1.0,
            Dimension::dimensionless(),
            CoordinateDisplayUnit::SI,
        )
        .unwrap();
        let axis = IndexAxis::coordinate_for_test(owner(), "Step", data);
        let coordinates = axis
            .coordinates()
            .iter()
            .map(|coordinate| coordinate.get())
            .collect::<Vec<_>>();
        assert_eq!(coordinates, vec![0.0, 1.0, 2.0]);
        assert_eq!(axis.key(2).as_deref(), Some(&IndexEntryKey::position(2)));
        assert_eq!(axis.keys().len(), 3);
        assert_eq!(
            IndexAxis::named_for_test(owner(), "Phase", &["A"]).coordinates(),
            []
        );
        assert_eq!(
            format!("{axis:?}"),
            format!("IndexAxis({:?})", axis.index())
        );
    }

    #[test]
    fn axes_match_by_index_identity() {
        let phase = IndexAxis::named_for_test(owner(), "Phase", &["A", "B"]);
        let same = IndexAxis::named_for_test(owner(), "Phase", &["A", "B"]);
        let other = IndexAxis::named_for_test(owner(), "Mode", &["A", "B"]);
        let finite = IndexAxis::finite(FiniteIndex::try_from_u64(2).unwrap());
        assert!(phase.matches(&same));
        assert_eq!(phase, same);
        assert_ne!(phase, other);
        assert!(!phase.matches(&other));
        assert!(!phase.matches(&finite));
    }
}
