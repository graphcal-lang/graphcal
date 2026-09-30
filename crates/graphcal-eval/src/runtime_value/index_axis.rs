//! A concrete index axis as runtime values see it.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::registry::checked_type::IndexTypeRef;
use graphcal_compiler::registry::index::{ConcreteIndexKind, CoordinateIndexData};
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::typed::checked::CheckedTir;

/// A concrete index together with its ordered entry keys.
///
/// The keys are derived from the index's own concrete definition, so an axis
/// is never empty and its keys are exactly the index's entries in declaration
/// order. Cloning shares the key set.
#[derive(Debug, Clone)]
pub struct IndexAxis(Arc<AxisData>);

#[derive(Debug)]
struct AxisData {
    index: IndexTypeRef,
    kind: ConcreteIndexKind,
    keys: NonEmpty<IndexEntryKey>,
    positions: HashMap<IndexEntryKey, usize>,
}

impl IndexAxis {
    /// Resolve `index` to its concrete definition in `tir`.
    ///
    /// Returns `None` when the index is unknown or still required (not bound
    /// to a concrete definition).
    #[must_use]
    pub fn resolve(tir: &CheckedTir, index: &IndexTypeRef) -> Option<Self> {
        let definition = tir.index_def(index)?;
        let kind = definition.concrete()?.clone();
        Self::from_concrete(index.clone(), kind)
    }

    /// The axis of a structural `Fin(N)` index, which needs no registry.
    #[cfg(test)]
    #[must_use]
    pub fn finite(index: graphcal_compiler::registry::index::FiniteIndex) -> Option<Self> {
        Self::from_concrete(
            IndexTypeRef::from_finite_index(index),
            ConcreteIndexKind::Finite { index },
        )
    }

    /// The axis of a named index whose identity is given directly, for tests.
    #[cfg(test)]
    #[must_use]
    pub fn named_for_test(
        owner: graphcal_compiler::dag_id::DagId,
        index: &str,
        variants: &[&str],
    ) -> Self {
        use graphcal_compiler::syntax::index_name::{IndexName, IndexVariantName};
        use graphcal_compiler::syntax::non_empty::NonEmptyUnique;
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

    fn from_concrete(index: IndexTypeRef, kind: ConcreteIndexKind) -> Option<Self> {
        let keys = NonEmpty::try_from_vec(kind.entry_keys()).ok()?;
        let positions = keys
            .iter()
            .enumerate()
            .map(|(position, key)| (key.clone(), position))
            .collect::<HashMap<_, _>>();
        Some(Self(Arc::new(AxisData {
            index,
            kind,
            keys,
            positions,
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
    pub fn len(&self) -> usize {
        self.0.keys.len()
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
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::registry::index::FiniteIndex;
    use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};

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
    fn axes_match_by_index_identity() {
        let phase = IndexAxis::named_for_test(owner(), "Phase", &["A", "B"]);
        let same = IndexAxis::named_for_test(owner(), "Phase", &["A", "B"]);
        let other = IndexAxis::named_for_test(owner(), "Mode", &["A", "B"]);
        let finite = IndexAxis::finite(FiniteIndex::try_from_u64(2).unwrap()).unwrap();
        assert!(phase.matches(&same));
        assert!(!phase.matches(&other));
        assert!(!phase.matches(&finite));
    }
}
