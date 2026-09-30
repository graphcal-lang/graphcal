//! Index keys: one entry of a concrete axis, identified by its position.

use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::registry::checked_type::IndexTypeRef;
use graphcal_compiler::registry::index::{ConcreteIndexKind, CoordinateIndexData};
use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};
use graphcal_compiler::syntax::non_empty::NonEmpty;

use super::index_axis::IndexAxis;

/// A value of type `Key<I>`: one entry of the concrete axis `I`.
///
/// The constructors check the position against the axis, so a key always
/// names an existing entry. Its label, coordinate, or `Fin` position is
/// derived from the axis rather than stored beside it, so the parts can never
/// disagree. Equality is axis identity plus position.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyValue {
    axis: IndexAxis,
    position: usize,
}

/// What a key denotes, by the kind of its axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyElement<'a> {
    /// A named-index variant (`Maneuver#Departure`).
    Named(&'a IndexVariantName),
    /// A coordinate-index entry: its finite SI coordinate on the axis `data`.
    Coordinate {
        value: FiniteQuantity,
        data: &'a CoordinateIndexData,
    },
    /// A structural `Fin(N)` position.
    Finite(usize),
}

impl KeyValue {
    /// The key at `position` of `axis`, when the axis has that many entries.
    #[must_use]
    pub fn at(axis: IndexAxis, position: usize) -> Option<Self> {
        (position < axis.len()).then_some(Self { axis, position })
    }

    /// The key naming `entry` of `axis`, when `entry` belongs to the axis.
    #[must_use]
    pub fn for_entry(axis: IndexAxis, entry: &IndexEntryKey) -> Option<Self> {
        let position = axis.position(entry)?;
        Some(Self { axis, position })
    }

    /// Every key of `axis`, in axis order (an axis has at least one).
    #[must_use]
    pub fn all(axis: &IndexAxis) -> NonEmpty<Self> {
        let key = |position| Self {
            axis: axis.clone(),
            position,
        };
        NonEmpty::new(key(0), (1..axis.len()).map(key).collect())
    }

    /// The axis this key belongs to.
    #[must_use]
    pub const fn axis(&self) -> &IndexAxis {
        &self.axis
    }

    /// The index identity of the key's axis.
    #[must_use]
    pub fn index(&self) -> &IndexTypeRef {
        self.axis.index()
    }

    /// The key's position on its axis.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// The entry key this key selects on its axis.
    #[must_use]
    pub fn entry_key(&self) -> &IndexEntryKey {
        &self.axis.keys().as_slice()[self.position]
    }

    /// What this key denotes: a variant, a coordinate, or a `Fin` position.
    #[must_use]
    pub fn element(&self) -> KeyElement<'_> {
        match self.axis.kind() {
            ConcreteIndexKind::Named { variants } => {
                KeyElement::Named(&variants.as_slice()[self.position])
            }
            ConcreteIndexKind::Coordinate(data) => KeyElement::Coordinate {
                value: self.axis.coordinates()[self.position],
                data,
            },
            ConcreteIndexKind::Finite { .. } => KeyElement::Finite(self.position),
        }
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::registry::index::FiniteIndex;
    use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};

    use super::{IndexAxis, KeyElement, KeyValue};

    fn phase() -> IndexAxis {
        IndexAxis::named_for_test(
            DagId::root_in_package("key-tests", "main"),
            "Phase",
            &["Launch", "Cruise"],
        )
    }

    fn fin(n: u64) -> IndexAxis {
        IndexAxis::finite(FiniteIndex::try_from_u64(n).unwrap()).unwrap()
    }

    #[test]
    fn keys_exist_only_on_their_axis() {
        assert!(KeyValue::at(phase(), 1).is_some());
        assert!(KeyValue::at(phase(), 2).is_none());
        let cruise = IndexEntryKey::named(IndexVariantName::expect_valid("Cruise"));
        let key = KeyValue::for_entry(phase(), &cruise).unwrap();
        assert_eq!(key.position(), 1);
        assert_eq!(key.entry_key(), &cruise);
        assert!(KeyValue::for_entry(phase(), &IndexEntryKey::position(0)).is_none());
    }

    #[test]
    fn elements_derive_from_the_axis_kind() {
        let key = KeyValue::at(phase(), 0).unwrap();
        assert_eq!(
            key.element(),
            KeyElement::Named(&IndexVariantName::expect_valid("Launch"))
        );
        assert_eq!(
            KeyValue::at(fin(3), 2).unwrap().element(),
            KeyElement::Finite(2)
        );
    }

    #[test]
    fn equality_is_axis_identity_and_position() {
        let key = KeyValue::at(phase(), 1).unwrap();
        assert_eq!(key, KeyValue::at(phase(), 1).unwrap());
        assert_ne!(key, KeyValue::at(phase(), 0).unwrap());
        assert_ne!(
            KeyValue::at(fin(3), 1).unwrap(),
            KeyValue::at(fin(4), 1).unwrap()
        );
        let positions = KeyValue::all(&fin(3))
            .iter()
            .map(KeyValue::position)
            .collect::<Vec<_>>();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn finite_axes_admit_narrower_finite_keys() {
        assert!(fin(4).admits(&fin(3)));
        assert!(fin(3).admits(&fin(3)));
        assert!(!fin(3).admits(&fin(4)));
        assert!(!fin(2).admits(&phase()));
        assert!(phase().admits(&phase()));
    }
}
