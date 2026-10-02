//! The modules of one resolver, addressable by canonical identity or by the
//! [`ModuleHandle`] the resolver issued for each of them.

use std::collections::HashMap;

use crate::dag_id::DagId;

/// The position of one module in the resolver that issued it.
///
/// Created only by that resolver, so every lookup by handle is total: a
/// handle always names a module of the resolver that handed it out, the way
/// a `DagPosition` names a DAG of its registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleHandle(usize);

/// Modules keyed by canonical identity, in insertion order.
///
/// Equality ignores the insertion order: two tables are equal when they hold
/// equal entries for the same identities.
#[derive(Debug, Clone)]
pub(super) struct ModuleTable<E> {
    entries: Vec<(DagId, E)>,
    positions: HashMap<DagId, ModuleHandle>,
}

impl<E> Default for ModuleTable<E> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            positions: HashMap::new(),
        }
    }
}

impl<E: PartialEq> PartialEq for ModuleTable<E> {
    fn eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .all(|(owner, entry)| other.get(owner) == Some(entry))
    }
}

impl<E: Eq> Eq for ModuleTable<E> {}

impl<E> ModuleTable<E> {
    /// The entry of `owner`, if the table holds it.
    pub(super) fn get(&self, owner: &DagId) -> Option<&E> {
        self.handle(owner).map(|handle| self.entry(handle))
    }

    /// The entry of `owner` for update, if the table holds it.
    pub(super) fn get_mut(&mut self, owner: &DagId) -> Option<&mut E> {
        let ModuleHandle(index) = self.handle(owner)?;
        self.entries.get_mut(index).map(|(_, entry)| entry)
    }

    /// Whether the table holds `owner`.
    pub(super) fn contains_key(&self, owner: &DagId) -> bool {
        self.positions.contains_key(owner)
    }

    /// Hold `entry` for `owner`, replacing a previous entry of `owner` in
    /// place, and return the handle of `owner`.
    pub(super) fn insert(&mut self, owner: DagId, entry: E) -> ModuleHandle {
        if let Some(handle) = self.handle(&owner) {
            self.entries[handle.0].1 = entry;
            return handle;
        }
        let handle = ModuleHandle(self.entries.len());
        self.positions.insert(owner.clone(), handle);
        self.entries.push((owner, entry));
        handle
    }

    /// The handle of `owner`, if the table holds it.
    pub(super) fn handle(&self, owner: &DagId) -> Option<ModuleHandle> {
        self.positions.get(owner).copied()
    }

    /// The identity of the module `handle` names.
    pub(super) fn owner(&self, handle: ModuleHandle) -> &DagId {
        &self.entries[handle.0].0
    }

    /// The entry of the module `handle` names.
    pub(super) fn entry(&self, handle: ModuleHandle) -> &E {
        &self.entries[handle.0].1
    }

    /// Every identity, in handle order.
    pub(super) fn owners(&self) -> impl Iterator<Item = &DagId> {
        self.entries.iter().map(|(owner, _)| owner)
    }

    /// Every identity with its entry, in handle order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (&DagId, &E)> {
        self.entries.iter().map(|(owner, entry)| (owner, entry))
    }

    /// The table of `f` applied to every entry, where every module keeps its
    /// handle.
    pub(super) fn map<F>(self, mut f: impl FnMut(E) -> F) -> ModuleTable<F> {
        ModuleTable {
            entries: self
                .entries
                .into_iter()
                .map(|(owner, entry)| (owner, f(entry)))
                .collect(),
            positions: self.positions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dag(name: &str) -> DagId {
        DagId::root_in_package("test", name)
    }

    #[test]
    fn handles_address_the_entries_they_were_issued_for() {
        let mut table = ModuleTable::default();
        let main = table.insert(dag("main"), 1);
        let lib = table.insert(dag("lib"), 2);
        assert_ne!(main, lib);
        assert_eq!(table.entry(main), &1);
        assert_eq!(table.owner(lib), &dag("lib"));
        assert_eq!(table.handle(&dag("lib")), Some(lib));
        assert_eq!(table.handle(&dag("other")), None);
        assert!(table.contains_key(&dag("main")));
        assert_eq!(table.get(&dag("other")), None);
    }

    #[test]
    fn reinserting_replaces_the_entry_and_keeps_the_handle() {
        let mut table = ModuleTable::default();
        let main = table.insert(dag("main"), 1);
        assert_eq!(table.insert(dag("main"), 3), main);
        assert_eq!(table.get(&dag("main")), Some(&3));
        if let Some(entry) = table.get_mut(&dag("main")) {
            *entry = 4;
        }
        assert_eq!(table.entry(main), &4);
    }

    #[test]
    fn mapping_keeps_every_handle_and_order() {
        let mut table = ModuleTable::default();
        let main = table.insert(dag("main"), 1);
        let lib = table.insert(dag("lib"), 2);
        assert_eq!(
            table.owners().cloned().collect::<Vec<_>>(),
            [dag("main"), dag("lib")]
        );
        assert_eq!(
            table.iter().map(|(_, entry)| *entry).collect::<Vec<_>>(),
            [1, 2]
        );
        let mapped = table.map(|entry| entry * 10);
        assert_eq!(mapped.entry(main), &10);
        assert_eq!(mapped.entry(lib), &20);
        assert_eq!(mapped.handle(&dag("lib")), Some(lib));
    }

    #[test]
    fn equality_ignores_insertion_order() {
        let mut first = ModuleTable::default();
        first.insert(dag("main"), 1);
        first.insert(dag("lib"), 2);
        let mut second = ModuleTable::default();
        second.insert(dag("lib"), 2);
        second.insert(dag("main"), 1);
        assert_eq!(first, second);
        second.insert(dag("lib"), 5);
        assert_ne!(first, second);
        let mut shorter = ModuleTable::default();
        shorter.insert(dag("main"), 1);
        assert_ne!(first, shorter);
    }
}
