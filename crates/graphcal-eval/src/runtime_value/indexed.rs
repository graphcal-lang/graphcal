//! Indexed values: one entry per key of a concrete axis.

use graphcal_compiler::registry::checked_type::IndexTypeRef;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;

use super::index_axis::IndexAxis;
use super::key_value::KeyValue;

/// An indexed value: exactly one entry of type `V` for every key of its axis,
/// in axis order.
///
/// Entries are positional; keys come from the axis. The only constructors
/// build entries by walking the axis (or by mapping an existing value), so an
/// indexed value is never empty and can never miss, duplicate, or reorder a
/// key. Two indexed values are equal when their axes have the same identity
/// and their entries are equal position by position.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedValue<V> {
    axis: IndexAxis,
    entries: NonEmpty<V>,
}

impl<V> IndexedValue<V> {
    /// Build one entry per axis key, in axis order.
    ///
    /// `entry` receives each key of the axis.
    pub fn try_from_axis<E>(
        axis: IndexAxis,
        entry: impl FnMut(&KeyValue) -> Result<V, E>,
    ) -> Result<Self, E> {
        let entries = KeyValue::all(&axis).try_map_ref(entry)?;
        Ok(Self { axis, entries })
    }

    /// An indexed value with its entries given positionally, for tests.
    #[cfg(test)]
    #[must_use]
    pub fn for_test(axis: IndexAxis, entries: Vec<V>) -> Self {
        assert_eq!(axis.len(), entries.len(), "one entry per axis key");
        let entries = NonEmpty::try_from_vec(entries).unwrap();
        Self { axis, entries }
    }

    /// An indexed value over the structural axis `Fin(entries.len())`, for
    /// tests.
    #[cfg(test)]
    #[must_use]
    pub fn finite_for_test(entries: Vec<V>) -> Self {
        let cardinality = u64::try_from(entries.len()).unwrap();
        let index =
            graphcal_compiler::registry::index::FiniteIndex::try_from_u64(cardinality).unwrap();
        let axis = IndexAxis::finite(index).unwrap();
        let entries = NonEmpty::try_from_vec(entries).unwrap();
        Self { axis, entries }
    }

    /// Derive a value over the same axis from each entry and its key.
    pub fn try_map_ref<U, E>(
        &self,
        mut entry: impl FnMut(&IndexEntryKey, &V) -> Result<U, E>,
    ) -> Result<IndexedValue<U>, E> {
        IndexedValue::try_from_axis(self.axis.clone(), |key| {
            entry(key.entry_key(), &self.entries.as_slice()[key.position()])
        })
    }

    /// Derive a value over the same axis from each owned entry.
    #[must_use]
    pub fn map<U>(self, entry: impl FnMut(V) -> U) -> IndexedValue<U> {
        IndexedValue {
            axis: self.axis,
            entries: self.entries.map(entry),
        }
    }

    /// Derive a value over the same axis from each owned entry and its key.
    pub fn try_map<U, E>(
        self,
        mut entry: impl FnMut(&IndexEntryKey, V) -> Result<U, E>,
    ) -> Result<IndexedValue<U>, E> {
        let Self { axis, entries } = self;
        let keys = axis.keys().as_slice();
        let mut position = 0;
        let entries = entries.try_map(|value| {
            let mapped = entry(&keys[position], value);
            position = position.saturating_add(1);
            mapped
        })?;
        Ok(IndexedValue { axis, entries })
    }

    /// The owned entry for `key`, when `key` belongs to the axis.
    #[must_use]
    pub fn into_entry(self, key: &IndexEntryKey) -> Option<V> {
        let position = self.axis.position(key)?;
        self.entries.into_iter().nth(position)
    }

    /// The axis this value is indexed by.
    #[must_use]
    pub const fn axis(&self) -> &IndexAxis {
        &self.axis
    }

    /// The index this value is indexed by.
    #[must_use]
    pub fn index(&self) -> &IndexTypeRef {
        self.axis.index()
    }

    /// The entry for `key`, when `key` belongs to the axis.
    #[must_use]
    pub fn get(&self, key: &IndexEntryKey) -> Option<&V> {
        self.axis
            .position(key)
            .and_then(|position| self.entries.as_slice().get(position))
    }

    /// The entry `key` selects: a key of this axis, or a narrower `Fin` key
    /// widened onto it (see [`IndexAxis::admits`]).
    #[must_use]
    pub fn get_key(&self, key: &KeyValue) -> Option<&V> {
        if self.axis.admits(key.axis()) {
            self.entries.as_slice().get(key.position())
        } else {
            None
        }
    }

    /// Entries with their keys, in axis order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&IndexEntryKey, &V)> {
        self.axis.keys().iter().zip(self.entries.iter())
    }

    /// Entries in axis order (at least one).
    #[must_use]
    pub const fn values(&self) -> &NonEmpty<V> {
        &self.entries
    }
}

impl<A, B> IndexedValue<(A, B)> {
    /// Split paired entries into two values over the same axis.
    #[must_use]
    pub fn unzip(self) -> (IndexedValue<A>, IndexedValue<B>) {
        let (left, right) = self.entries.unzip();
        (
            IndexedValue {
                axis: self.axis.clone(),
                entries: left,
            },
            IndexedValue {
                axis: self.axis,
                entries: right,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};

    use super::{IndexAxis, IndexedValue};

    fn key(name: &str) -> IndexEntryKey {
        IndexEntryKey::named(IndexVariantName::expect_valid(name))
    }

    fn axis() -> IndexAxis {
        IndexAxis::named_for_test(
            DagId::root_in_package("indexed-tests", "main"),
            "Phase",
            &["A", "B", "C"],
        )
    }

    #[test]
    fn entries_follow_the_axis_positions() {
        let mut seen = Vec::new();
        let indexed = IndexedValue::try_from_axis(axis(), |key| {
            seen.push((key.position(), key.entry_key().clone()));
            Ok::<_, ()>(key.position() * 10)
        })
        .unwrap();
        assert_eq!(seen, vec![(0, key("A")), (1, key("B")), (2, key("C"))]);
        assert_eq!(indexed.values().as_slice(), &[0, 10, 20]);
        assert_eq!(indexed.get(&key("B")), Some(&10));
        assert!(indexed.get(&key("D")).is_none());
        let keys = indexed
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec![key("A"), key("B"), key("C")]);
        assert!(indexed.axis().matches(&axis()));
        assert_eq!(indexed.index(), axis().index());
    }

    #[test]
    fn construction_stops_at_the_first_entry_error() {
        let mut calls = 0;
        let error = IndexedValue::try_from_axis(axis(), |key| {
            calls += 1;
            if key.position() == 1 {
                Err("second")
            } else {
                Ok(true)
            }
        })
        .unwrap_err();
        assert_eq!(error, "second");
        assert_eq!(calls, 2);
    }

    #[test]
    fn map_keeps_the_axis_and_pairs_keys_with_entries() {
        let indexed = IndexedValue::for_test(axis(), vec![1_i64, 2, 3]);
        let mapped = indexed
            .try_map_ref(|entry_key, value| {
                Ok::<_, ()>(if *entry_key == key("C") {
                    -value
                } else {
                    value + 1
                })
            })
            .unwrap();
        assert_eq!(mapped.values().as_slice(), &[2, 3, -3]);
        assert!(mapped.axis().matches(indexed.axis()));
    }

    #[test]
    fn owned_maps_selection_and_unzip_keep_the_axis() {
        let indexed = IndexedValue::for_test(axis(), vec![1_i64, 2, 3]);
        let mut seen = Vec::new();
        let mapped = indexed
            .clone()
            .try_map(|entry_key, value| {
                seen.push(entry_key.clone());
                Ok::<_, ()>((value, value * 10))
            })
            .unwrap();
        assert_eq!(seen, vec![key("A"), key("B"), key("C")]);
        let (left, right) = mapped.unzip();
        assert_eq!(left.values().as_slice(), &[1, 2, 3]);
        assert_eq!(right.values().as_slice(), &[10, 20, 30]);
        assert!(left.axis().matches(&axis()) && right.axis().matches(&axis()));
        assert_eq!(
            indexed.clone().map(|value| -value).values().as_slice(),
            &[-1, -2, -3]
        );
        assert_eq!(indexed.clone().into_entry(&key("C")), Some(3));
        assert_eq!(indexed.into_entry(&key("D")), None);
        let failed =
            IndexedValue::for_test(axis(), vec![1_i64, 2, 3]).try_map(|entry_key, value| {
                if *entry_key == key("B") {
                    Err(value)
                } else {
                    Ok(value)
                }
            });
        assert_eq!(failed.unwrap_err(), 2);
    }

    #[test]
    fn finite_test_values_use_a_structural_axis() {
        let indexed = IndexedValue::finite_for_test(vec!['a', 'b']);
        assert_eq!(indexed.get(&IndexEntryKey::position(1)), Some(&'b'));
        assert_eq!(indexed.axis().len(), 2);
    }

    #[test]
    fn keys_select_entries_of_their_axis_or_a_wider_fin_axis() {
        use crate::runtime_value::KeyValue;
        use graphcal_compiler::registry::index::FiniteIndex;

        let named = IndexedValue::for_test(axis(), vec![1, 2, 3]);
        let b = KeyValue::for_entry(axis(), &key("B")).unwrap();
        assert_eq!(named.get_key(&b), Some(&2));
        let fin = |n| IndexAxis::finite(FiniteIndex::try_from_u64(n).unwrap()).unwrap();
        assert_eq!(named.get_key(&KeyValue::at(fin(3), 1).unwrap()), None);

        let wide = IndexedValue::finite_for_test(vec!['a', 'b', 'c', 'd']);
        assert_eq!(wide.get_key(&KeyValue::at(fin(2), 1).unwrap()), Some(&'b'));
        assert_eq!(wide.get_key(&KeyValue::at(fin(4), 3).unwrap()), Some(&'d'));
        let narrow = IndexedValue::finite_for_test(vec!['a', 'b']);
        assert_eq!(narrow.get_key(&KeyValue::at(fin(4), 1).unwrap()), None);
    }
}
