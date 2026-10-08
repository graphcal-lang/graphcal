//! A concrete index axis as runtime values see it.

use std::collections::HashMap;
use std::sync::Arc;

use crate::finite_value::FiniteQuantity;
use crate::semantic::checked_type::IndexTypeRef;
use crate::semantic::index_def::{ConcreteIndexKind, CoordinateIndexData};
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
    index: IndexTypeRef,
    kind: ConcreteIndexKind,
    keys: NonEmpty<IndexEntryKey>,
    positions: HashMap<IndexEntryKey, usize>,
    /// One finite coordinate per key; empty unless `kind` is a coordinate index.
    coordinates: Vec<FiniteQuantity>,
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
    pub fn finite(index: crate::semantic::index_def::FiniteIndex) -> Option<Self> {
        Self::from_concrete(
            IndexTypeRef::from_finite_index(index),
            ConcreteIndexKind::Finite { index },
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

    pub(crate) fn from_concrete(index: IndexTypeRef, kind: ConcreteIndexKind) -> Option<Self> {
        let keys = NonEmpty::try_from_vec(kind.entry_keys()).ok()?;
        let positions = keys
            .iter()
            .enumerate()
            .map(|(position, key)| (key.clone(), position))
            .collect::<HashMap<_, _>>();
        let coordinates = match &kind {
            ConcreteIndexKind::Coordinate(data) => (0..keys.len())
                .map(|position| FiniteQuantity::try_new(data.coordinate_value(position)).ok())
                .collect::<Option<Vec<_>>>()?,
            ConcreteIndexKind::Named { .. } | ConcreteIndexKind::Finite { .. } => Vec::new(),
        };
        Some(Self(Arc::new(AxisData {
            index,
            kind,
            keys,
            positions,
            coordinates,
        })))
    }

    /// The index this axis enumerates.
    #[must_use]
    pub fn index(&self) -> &IndexTypeRef {
        &self.0.index
    }

    /// The concrete definition the keys were derived from.
    #[must_use]
    pub fn kind(&self) -> &ConcreteIndexKind {
        &self.0.kind
    }

    /// Coordinate data, when this is a coordinate axis.
    #[must_use]
    pub fn coordinate_data(&self) -> Option<&CoordinateIndexData> {
        match &self.0.kind {
            ConcreteIndexKind::Coordinate(data) => Some(data),
            ConcreteIndexKind::Named { .. } | ConcreteIndexKind::Finite { .. } => None,
        }
    }

    /// The entry keys in axis order.
    #[must_use]
    pub fn keys(&self) -> &NonEmpty<IndexEntryKey> {
        &self.0.keys
    }

    /// Number of entries (at least one).
    #[must_use]
    #[expect(
        clippy::len_without_is_empty,
        reason = "an index axis has at least one entry, so it is never empty"
    )]
    pub fn len(&self) -> usize {
        self.0.keys.len()
    }

    /// The finite coordinates of a coordinate axis, one per key in axis
    /// order; empty for named and `Fin` axes.
    #[must_use]
    pub fn coordinates(&self) -> &[FiniteQuantity] {
        &self.0.coordinates
    }

    /// Position of `key` on this axis.
    #[must_use]
    pub fn position(&self, key: &IndexEntryKey) -> Option<usize> {
        self.0.positions.get(key).copied()
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
        match (&self.0.kind, &key_axis.0.kind) {
            (ConcreteIndexKind::Finite { .. }, ConcreteIndexKind::Finite { .. }) => {
                key_axis.len() <= self.len()
            }
            _ => self.matches(key_axis),
        }
    }
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
            axis.keys().first(),
            &IndexEntryKey::named(IndexVariantName::expect_valid("Launch"))
        );
        assert_eq!(axis.position(&cruise), Some(1));
        assert_eq!(axis.position(&IndexEntryKey::position(1)), None);
        assert!(axis.coordinate_data().is_none());
    }

    #[test]
    fn finite_axis_uses_positions() {
        let axis = IndexAxis::finite(FiniteIndex::try_from_u64(3).unwrap()).unwrap();
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
        assert_eq!(
            axis.keys().as_slice().get(2),
            Some(&IndexEntryKey::position(2))
        );
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
        let finite = IndexAxis::finite(FiniteIndex::try_from_u64(2).unwrap()).unwrap();
        assert!(phase.matches(&same));
        assert_eq!(phase, same);
        assert_ne!(phase, other);
        assert!(!phase.matches(&other));
        assert!(!phase.matches(&finite));
    }
}
