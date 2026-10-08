//! The DAGs the inline calls of one body target, each named by a slot
//! relative to that body.
//!
//! A call node carries only its [`CallSlot`]; the table that gives the slot
//! its target travels with the checked trees of the body that holds the node,
//! so a body shared by several programs or instances means the same callees
//! in each of them.

use indexmap::IndexSet;

use crate::dag_id::DagId;

/// The position of one call target in the [`CallTargets`] of the body whose
/// call node carries it.
///
/// Created only by [`CallTargets`], so a slot always names a target of the
/// table that created it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallSlot(usize);

impl CallSlot {
    /// The slot's position in its table, which [`CallTargets::iter`] visits
    /// in order.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

/// The distinct DAGs the inline calls of one body target, in the order the
/// calls were first checked.
///
/// A table only grows: interning a new target never renumbers an existing
/// slot, so a copy of a table may be extended while trees numbered by the
/// original keep their meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallTargets {
    targets: IndexSet<DagId>,
}

impl CallTargets {
    /// The slot of `target`, added after every existing target when the
    /// table does not have it yet.
    pub(crate) fn intern(&mut self, target: DagId) -> CallSlot {
        CallSlot(self.targets.insert_full(target).0)
    }

    /// The DAG a slot of this table targets.
    ///
    /// # Panics
    ///
    /// Panics when `slot` was created by another table that has more
    /// targets; a slot is only ever read through the table of the body that
    /// carries it.
    #[must_use]
    pub fn target(&self, slot: CallSlot) -> &DagId {
        &self.targets[slot.0]
    }

    /// Every slot with its target, in slot order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (CallSlot, &DagId)> {
        self.targets
            .iter()
            .enumerate()
            .map(|(position, target)| (CallSlot(position), target))
    }

    /// The number of distinct targets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    /// Whether the body calls no DAG.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dag(path: &str) -> DagId {
        DagId::from_virtual_relative_path(std::path::Path::new(path)).unwrap()
    }

    #[test]
    fn interning_reuses_slots_and_appends_new_targets() {
        let mut table = CallTargets::default();
        assert!(table.is_empty());
        let first = table.intern(dag("a.gcl"));
        let second = table.intern(dag("b.gcl"));
        assert_eq!(table.intern(dag("a.gcl")), first);
        assert_eq!((first.index(), second.index()), (0, 1));
        assert_eq!(table.len(), 2);
        assert_eq!(table.target(second), &dag("b.gcl"));
        assert_eq!(
            table
                .iter()
                .map(|(slot, target)| (slot, target.clone()))
                .collect::<Vec<_>>(),
            [(first, dag("a.gcl")), (second, dag("b.gcl"))]
        );
    }

    #[test]
    fn an_extended_copy_keeps_every_original_slot() {
        let mut original = CallTargets::default();
        let slot = original.intern(dag("a.gcl"));
        let mut extended = original.clone();
        let added = extended.intern(dag("c.gcl"));
        assert_eq!(extended.intern(dag("a.gcl")), slot);
        assert_eq!(extended.target(slot), original.target(slot));
        assert_eq!(added.index(), 1);
    }
}
