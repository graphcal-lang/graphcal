//! A keyed table whose source-visible aliases can never form a cycle.
//!
//! Every registry namespace (dimensions, units, nominal types, indexes) binds
//! definitions under a key and may additionally bind source-visible aliases
//! (`import m::{dim Rate as R}` projections) that resolve to another key.
//! [`AliasedTable`] is the single implementation of that lookup: aliases are
//! checked for cycles when they are inserted, so resolution is a plain walk
//! along an acyclic chain instead of a fuel-bounded loop duplicated per
//! namespace.

use std::collections::HashMap;
use std::hash::Hash;

use thiserror::Error;

/// Inserting an alias would make its own target chain lead back to it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("alias `{alias}` would resolve to itself")]
pub struct AliasCycle<K> {
    /// The alias whose insertion was rejected.
    pub alias: K,
}

/// Definitions keyed by `K`, plus acyclic alias edges between keys.
///
/// A lookup returns the definition of the first key along the alias chain
/// that has one, so a key that is both defined and aliased resolves to its
/// own definition. Alias targets may be defined after the alias (importer
/// projections are installed before the importer's own declarations), but
/// the alias graph is acyclic by construction.
#[derive(Debug, Clone)]
pub struct AliasedTable<K, V> {
    entries: HashMap<K, V>,
    aliases: HashMap<K, K>,
}

impl<K, V> Default for AliasedTable<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            aliases: HashMap::new(),
        }
    }
}

impl<K: Clone + Eq + Hash + std::fmt::Display, V> AliasedTable<K, V> {
    /// Define `key`, replacing a previous definition under the same key.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.entries.insert(key, value)
    }

    /// Define `key` only if it has no definition yet.
    pub fn insert_if_missing(&mut self, key: K, value: impl FnOnce() -> V) {
        self.entries.entry(key).or_insert_with(value);
    }

    /// Bind `alias` to resolve through `target`, replacing a previous alias
    /// edge from `alias`.
    ///
    /// Binding a key to itself adds nothing (the key resolves to its own
    /// definition either way) and is accepted as a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCycle`] when `target`'s alias chain already leads back
    /// to `alias`; the table is left unchanged.
    pub fn insert_alias(&mut self, alias: K, target: K) -> Result<(), AliasCycle<K>> {
        if alias == target {
            return Ok(());
        }
        if self.chain(&target).any(|key| *key == alias) {
            return Err(AliasCycle { alias });
        }
        self.aliases.insert(alias, target);
        Ok(())
    }

    /// Copy every definition and alias of `parent` whose key this table does
    /// not bind yet.
    ///
    /// Existing bindings take precedence, so a parent alias edge that would
    /// close a cycle through this table's own aliases is not copied.
    pub fn merge_missing_from(&mut self, parent: &Self)
    where
        V: Clone,
    {
        for (key, value) in &parent.entries {
            self.insert_if_missing(key.clone(), || value.clone());
        }
        for (alias, target) in &parent.aliases {
            if !self.binds(alias) {
                // A cycle can only close through this table's own edges,
                // which take precedence over the parent's.
                let _ = self.insert_alias(alias.clone(), target.clone());
            }
        }
    }

    /// Resolve `key` through its alias chain to the first definition.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<&V> {
        self.chain(key).find_map(|key| self.entries.get(key))
    }

    /// The definition stored directly under `key`, ignoring aliases.
    #[must_use]
    pub fn get_defined(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
    }

    /// Whether `key` is defined or aliased.
    #[must_use]
    pub fn binds(&self, key: &K) -> bool {
        self.entries.contains_key(key) || self.aliases.contains_key(key)
    }

    /// Iterate over directly defined keys and their definitions.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
    }

    /// Iterate over directly defined values.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values()
    }

    /// Iterate over directly defined keys.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }

    /// `key` followed by every alias target reachable from it. Finite because
    /// the alias graph is acyclic and each key has at most one outgoing edge.
    fn chain<'a>(&'a self, key: &'a K) -> impl Iterator<Item = &'a K> {
        std::iter::successors(Some(key), |current| self.aliases.get(*current))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> AliasedTable<String, u32> {
        AliasedTable::default()
    }

    fn s(value: &str) -> String {
        value.to_string()
    }

    #[test]
    fn get_follows_alias_chains_to_the_first_definition() {
        let mut t = table();
        t.insert(s("c"), 3);
        t.insert_alias(s("a"), s("b")).unwrap();
        t.insert_alias(s("b"), s("c")).unwrap();
        assert_eq!(t.get(&s("a")), Some(&3));
        assert_eq!(t.get(&s("b")), Some(&3));
        assert_eq!(t.get(&s("c")), Some(&3));
        assert_eq!(t.get_defined(&s("a")), None);
        assert_eq!(t.get(&s("missing")), None);
    }

    #[test]
    fn definitions_shadow_aliases_along_the_chain() {
        let mut t = table();
        t.insert_alias(s("a"), s("b")).unwrap();
        t.insert_alias(s("b"), s("c")).unwrap();
        t.insert(s("c"), 3);
        t.insert(s("b"), 2);
        assert_eq!(t.get(&s("a")), Some(&2));
        t.insert(s("a"), 1);
        assert_eq!(t.get(&s("a")), Some(&1));
    }

    #[test]
    fn aliases_may_precede_their_targets() {
        let mut t = table();
        t.insert_alias(s("a"), s("b")).unwrap();
        assert_eq!(t.get(&s("a")), None);
        assert!(t.binds(&s("a")));
        assert!(!t.binds(&s("b")));
        t.insert(s("b"), 2);
        assert_eq!(t.get(&s("a")), Some(&2));
    }

    #[test]
    fn insert_alias_rejects_cycles_and_accepts_identity() {
        let mut t = table();
        assert_eq!(t.insert_alias(s("a"), s("a")), Ok(()));
        assert!(!t.binds(&s("a")));

        t.insert_alias(s("a"), s("b")).unwrap();
        t.insert_alias(s("b"), s("c")).unwrap();
        assert_eq!(
            t.insert_alias(s("c"), s("a")),
            Err(AliasCycle { alias: s("c") })
        );
        assert_eq!(
            t.insert_alias(s("b"), s("a")),
            Err(AliasCycle { alias: s("b") })
        );
        // The rejected insertions left the table unchanged.
        assert!(!t.binds(&s("c")));
        t.insert(s("c"), 3);
        assert_eq!(t.get(&s("a")), Some(&3));
        assert_eq!(
            AliasCycle { alias: s("c") }.to_string(),
            "alias `c` would resolve to itself"
        );
    }

    #[test]
    fn insert_alias_replaces_an_existing_edge() {
        let mut t = table();
        t.insert(s("x"), 1);
        t.insert(s("y"), 2);
        t.insert_alias(s("a"), s("x")).unwrap();
        t.insert_alias(s("a"), s("y")).unwrap();
        assert_eq!(t.get(&s("a")), Some(&2));
    }

    #[test]
    fn insert_if_missing_keeps_existing_definitions() {
        let mut t = table();
        t.insert_if_missing(s("a"), || 1);
        t.insert_if_missing(s("a"), || 2);
        assert_eq!(t.get(&s("a")), Some(&1));
        assert_eq!(t.insert(s("a"), 3), Some(1));
    }

    #[test]
    fn merge_missing_from_keeps_own_bindings_and_skips_cycles() {
        let mut parent = table();
        parent.insert(s("a"), 10);
        parent.insert(s("p"), 20);
        parent.insert_alias(s("x"), s("p")).unwrap();
        parent.insert_alias(s("y"), s("z")).unwrap();
        parent.insert_alias(s("q"), s("r")).unwrap();

        let mut child = table();
        child.insert(s("a"), 1);
        child.insert_alias(s("x"), s("a")).unwrap();
        // `r -> q` would close a cycle with the parent's `q -> r`.
        child.insert_alias(s("r"), s("q")).unwrap();
        child.merge_missing_from(&parent);

        assert_eq!(child.get(&s("a")), Some(&1));
        assert_eq!(child.get(&s("x")), Some(&1));
        assert_eq!(child.get(&s("p")), Some(&20));
        assert!(child.binds(&s("y")));
        assert!(child.binds(&s("r")));
        assert!(!child.binds(&s("q")));
        child.insert(s("z"), 5);
        assert_eq!(child.get(&s("y")), Some(&5));
        let mut keys: Vec<_> = child.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, [s("a"), s("p"), s("z")]);
        let mut values: Vec<_> = child.values().copied().collect();
        values.sort_unstable();
        assert_eq!(values, [1, 5, 20]);
        assert_eq!(child.iter().count(), 3);
    }
}
