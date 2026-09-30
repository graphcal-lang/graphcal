//! Dependency-first project sequences with a distinguished root.
//!
//! Project phases (loaded sources, HIR, checked modules) all process modules
//! in the same order: every dependency before its dependents, ending with the
//! entry module. Storing the root as its own field makes "the root exists and
//! comes last" structural instead of a lookup that each phase re-validates.

use graphcal_compiler::dependency_graph::TopoOrder;

/// Modules in dependency order followed by the entry (root) module.
///
/// Invariants:
///
/// - The root is always present and is always yielded last.
/// - Every element of `deps` appears after all of its own dependencies, and
///   the root appears after every dependency (topological order).
///
/// The first invariant is structural. The second is established by the
/// constructors: `DependencyOrdered::from_topo_order` takes a
/// [`TopoOrder`] (which only a [`DependencyGraph`] produces) and checks that
/// the root comes last, `DependencyOrdered::root_only` has no dependencies,
/// and every later phase derives its sequence with [`DependencyOrdered::map`]
/// or [`DependencyOrdered::try_map`], which preserve positions.
///
/// [`DependencyGraph`]: graphcal_compiler::dependency_graph::DependencyGraph
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyOrdered<T> {
    deps: Vec<T>,
    root: T,
}

impl<T> DependencyOrdered<T> {
    /// A lone `root` without dependencies.
    pub(crate) const fn root_only(root: T) -> Self {
        Self {
            deps: Vec::new(),
            root,
        }
    }

    /// The value of every key of `order`, dependencies first, ending with the
    /// value of `root`.
    ///
    /// Returns `None` when `root` is not the last key of `order` (some key
    /// does not lead to it) or `value` has no value for some key.
    pub(crate) fn from_topo_order<K: PartialEq>(
        order: TopoOrder<K>,
        root: &K,
        mut value: impl FnMut(K) -> Option<T>,
    ) -> Option<Self> {
        let mut keys = order.into_vec();
        let last = keys.pop().filter(|last| last == root)?;
        let deps = keys
            .into_iter()
            .map(&mut value)
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            deps,
            root: value(last)?,
        })
    }

    /// The entry module.
    #[must_use]
    pub const fn root(&self) -> &T {
        &self.root
    }

    /// Every non-root module, dependencies first.
    #[must_use]
    pub fn deps(&self) -> &[T] {
        &self.deps
    }

    /// Number of modules, including the root. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.deps.len().saturating_add(1)
    }

    /// Always `false`: the root is always present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Iterate dependencies first, ending with the root.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + Clone {
        self.deps.iter().chain(std::iter::once(&self.root))
    }

    /// Element at `position` in [`Self::iter`] order.
    #[must_use]
    pub fn get(&self, position: usize) -> Option<&T> {
        match position.cmp(&self.deps.len()) {
            std::cmp::Ordering::Less => self.deps.get(position),
            std::cmp::Ordering::Equal => Some(&self.root),
            std::cmp::Ordering::Greater => None,
        }
    }

    /// Borrow every element, preserving positions.
    #[must_use]
    pub fn as_ref(&self) -> DependencyOrdered<&T> {
        DependencyOrdered {
            deps: self.deps.iter().collect(),
            root: &self.root,
        }
    }

    /// Transform every element in dependency order, preserving positions.
    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> DependencyOrdered<U> {
        let deps = self.deps.into_iter().map(&mut f).collect();
        DependencyOrdered {
            deps,
            root: f(self.root),
        }
    }

    /// Fallibly transform every element in dependency order, stopping at the
    /// first error. Dependencies are visited before the root, so `f` may rely
    /// on state accumulated from every dependency of the element it receives.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `f`.
    pub fn try_map<U, E>(
        self,
        mut f: impl FnMut(T) -> Result<U, E>,
    ) -> Result<DependencyOrdered<U>, E> {
        let deps = self
            .deps
            .into_iter()
            .map(&mut f)
            .collect::<Result<Vec<_>, E>>()?;
        Ok(DependencyOrdered {
            deps,
            root: f(self.root)?,
        })
    }

    /// Split into dependency-first non-root modules and the root.
    #[must_use]
    pub fn into_parts(self) -> (Vec<T>, T) {
        (self.deps, self.root)
    }
}

impl<'a, T> IntoIterator for &'a DependencyOrdered<T> {
    type Item = &'a T;
    type IntoIter = std::iter::Chain<std::slice::Iter<'a, T>, std::iter::Once<&'a T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.deps.iter().chain(std::iter::once(&self.root))
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dependency_graph::DependencyGraph;

    use super::DependencyOrdered;

    /// `root` depending on each of `deps`, which are independent.
    fn ordered<T: Clone + Eq + std::hash::Hash + std::fmt::Debug>(
        deps: &[T],
        root: &T,
    ) -> DependencyOrdered<T> {
        let mut graph = DependencyGraph::new();
        graph.add_node(root.clone());
        for dep in deps {
            graph.add_dependency(root.clone(), dep.clone());
        }
        let order = graph.into_depth_first_order().unwrap();
        DependencyOrdered::from_topo_order(order, root, Some).unwrap()
    }

    #[test]
    fn root_is_last_in_iteration_and_positions() {
        let ordered = ordered(&["a", "b"], &"root");
        assert_eq!(
            ordered.iter().copied().collect::<Vec<_>>(),
            ["a", "b", "root"]
        );
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered.get(0), Some(&"a"));
        assert_eq!(ordered.get(2), Some(&"root"));
        assert_eq!(ordered.get(3), None);
        assert_eq!(ordered.root(), &"root");
        assert_eq!(ordered.deps(), ["a", "b"]);
    }

    #[test]
    fn root_only_sequence_has_one_element() {
        let ordered = DependencyOrdered::root_only(7);
        assert_eq!(ordered.len(), 1);
        assert!(!ordered.is_empty());
        assert_eq!(ordered.iter().copied().collect::<Vec<_>>(), [7]);
    }

    #[test]
    fn from_topo_order_maps_keys_to_values() {
        let mut graph = DependencyGraph::new();
        graph.add_dependency("main", "lib");
        graph.add_dependency("lib", "core");
        let order = graph.into_depth_first_order().unwrap();
        let ordered =
            DependencyOrdered::from_topo_order(order, &"main", |key| Some(key.len())).unwrap();
        assert_eq!(ordered.into_parts(), (vec![4, 3], 4));
    }

    #[test]
    fn from_topo_order_requires_the_root_last_and_every_value() {
        let mut graph = DependencyGraph::new();
        graph.add_dependency("main", "lib");
        graph.add_node("unrelated");
        let order = graph.into_depth_first_order().unwrap();
        assert_eq!(
            DependencyOrdered::from_topo_order(order.clone(), &"main", Some),
            None
        );
        assert_eq!(
            DependencyOrdered::from_topo_order(order, &"unrelated", |key| {
                (key != "lib").then_some(key)
            }),
            None
        );
        let empty = DependencyGraph::<&str>::new()
            .into_depth_first_order()
            .unwrap();
        assert_eq!(
            DependencyOrdered::from_topo_order(empty, &"main", Some),
            None
        );
    }

    #[test]
    fn try_map_visits_dependencies_before_root_and_preserves_positions() {
        let ordered = ordered(&[1, 2], &3);
        let mut visited = Vec::new();
        let mapped = ordered
            .try_map(|value| {
                visited.push(value);
                Ok::<_, ()>(value * 10)
            })
            .unwrap();
        assert_eq!(visited, [1, 2, 3]);
        assert_eq!(mapped.into_parts(), (vec![10, 20], 30));
    }

    #[test]
    fn try_map_stops_at_first_error() {
        let ordered = ordered(&[1, 2], &3);
        let mut visited = Vec::new();
        let result = ordered.try_map(|value| {
            visited.push(value);
            if value == 2 { Err(value) } else { Ok(value) }
        });
        assert_eq!(result, Err(2));
        assert_eq!(visited, [1, 2]);
    }

    #[test]
    fn map_and_as_ref_preserve_positions() {
        let ordered = ordered(&[String::from("a")], &String::from("r"));
        let lengths = ordered.as_ref().map(String::len);
        assert_eq!(lengths.into_parts(), (vec![1], 1));
    }
}
