//! Generic dependency graphs with deterministic topological ordering.
//!
//! A [`DependencyGraph<K>`] records "`dependent` depends on `dependency`"
//! edges between keys. [`DependencyGraph::into_topo_order`] either proves the
//! graph acyclic by returning a [`TopoOrder<K>`] (every key after all of its
//! dependencies) or returns a [`Cycle<K>`] naming one dependency cycle.
//!
//! Both results are pure functions of the insertion order of nodes and edges:
//! no hash iteration order leaks into them. Callers that need an order that is
//! stable across runs must therefore insert nodes and edges in a stable order.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

/// A directed graph of "depends on" edges between keys.
///
/// Nodes and edges remember their insertion order, which fully determines the
/// resulting [`TopoOrder`] or [`Cycle`]. Adding a node or an edge twice is a
/// no-op, and a key may depend on itself (a self-loop is a cycle of length 1).
#[derive(Debug, Clone)]
pub struct DependencyGraph<K> {
    keys: Vec<K>,
    positions: HashMap<K, usize>,
    /// For each node, its dependencies in edge insertion order.
    dependencies: Vec<Vec<usize>>,
    /// For each node, its dependents in edge insertion order.
    dependents: Vec<Vec<usize>>,
    edges: HashSet<(usize, usize)>,
}

impl<K> Default for DependencyGraph<K> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            positions: HashMap::new(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            edges: HashSet::new(),
        }
    }
}

impl<K: Clone + Eq + Hash> DependencyGraph<K> {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `key` as a node. Adding an existing key keeps its original
    /// insertion position.
    pub fn add_node(&mut self, key: K) {
        self.position_of(key);
    }

    /// Record that `dependent` depends on `dependency`, adding either key as
    /// a node first if it is not yet present (`dependent` before
    /// `dependency`). Recording the same edge twice is a no-op.
    pub fn add_dependency(&mut self, dependent: K, dependency: K) {
        let dependent = self.position_of(dependent);
        let dependency = self.position_of(dependency);
        if self.edges.insert((dependent, dependency)) {
            self.dependencies[dependent].push(dependency);
            self.dependents[dependency].push(dependent);
        }
    }

    /// Whether `key` is a node of this graph.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.positions.contains_key(key)
    }

    /// Number of nodes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the graph has no nodes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Order every node after all of its dependencies.
    ///
    /// The order is Kahn's algorithm with a FIFO queue: nodes without
    /// dependencies are emitted in node insertion order, and each emitted node
    /// releases its dependents in edge insertion order.
    ///
    /// # Errors
    ///
    /// Returns the [`Cycle`] found by a depth-first search that visits roots in
    /// node insertion order and dependencies in edge insertion order.
    pub fn into_topo_order(self) -> Result<TopoOrder<K>, Cycle<K>> {
        if let Some(cycle) = self.find_cycle() {
            return Err(self.cycle_from_indices(cycle));
        }
        let rank = self.kahn_ranks();
        let mut ranked = rank.into_iter().zip(self.keys).collect::<Vec<_>>();
        ranked.sort_unstable_by_key(|(rank, _)| *rank);
        Ok(TopoOrder {
            order: ranked.into_iter().map(|(_, key)| key).collect(),
        })
    }

    fn position_of(&mut self, key: K) -> usize {
        if let Some(&position) = self.positions.get(&key) {
            return position;
        }
        let position = self.keys.len();
        self.positions.insert(key.clone(), position);
        self.keys.push(key);
        self.dependencies.push(Vec::new());
        self.dependents.push(Vec::new());
        position
    }

    /// Node indices of one cycle, starting at the node the search re-entered,
    /// or `None` when the graph is acyclic. Iterative, so deep graphs cannot
    /// overflow the call stack.
    fn find_cycle(&self) -> Option<Vec<usize>> {
        let mut state = vec![Visit::Unvisited; self.keys.len()];
        // Each frame is a node on the current path and its next edge to try.
        let mut path: Vec<(usize, usize)> = Vec::new();
        for root in 0..self.keys.len() {
            if state[root] != Visit::Unvisited {
                continue;
            }
            state[root] = Visit::OnPath(0);
            path.push((root, 0));
            while let Some(frame) = path.last_mut() {
                let (node, cursor) = *frame;
                let Some(&dependency) = self.dependencies[node].get(cursor) else {
                    state[node] = Visit::Done;
                    path.pop();
                    continue;
                };
                frame.1 = cursor.saturating_add(1);
                match state[dependency] {
                    Visit::Unvisited => {
                        state[dependency] = Visit::OnPath(path.len());
                        path.push((dependency, 0));
                    }
                    Visit::OnPath(position) => {
                        return Some(path[position..].iter().map(|(node, _)| *node).collect());
                    }
                    Visit::Done => {}
                }
            }
        }
        None
    }

    /// Emission rank of every node under FIFO Kahn. Only called on acyclic
    /// graphs, where every node is emitted exactly once.
    fn kahn_ranks(&self) -> Vec<usize> {
        let mut pending = self.dependencies.iter().map(Vec::len).collect::<Vec<_>>();
        let mut ready = (0..self.keys.len())
            .filter(|&node| pending[node] == 0)
            .collect::<VecDeque<_>>();
        let mut rank = vec![0; self.keys.len()];
        let mut emitted = 0_usize;
        while let Some(node) = ready.pop_front() {
            rank[node] = emitted;
            emitted = emitted.saturating_add(1);
            for &dependent in &self.dependents[node] {
                pending[dependent] = pending[dependent].saturating_sub(1);
                if pending[dependent] == 0 {
                    ready.push_back(dependent);
                }
            }
        }
        debug_assert_eq!(emitted, self.keys.len(), "Kahn on an acyclic graph");
        rank
    }

    /// Rotate the cycle to start at its earliest-inserted node so the report
    /// does not depend on where the search entered the cycle.
    fn cycle_from_indices(&self, mut cycle: Vec<usize>) -> Cycle<K> {
        let start = cycle
            .iter()
            .enumerate()
            .min_by_key(|(_, node)| **node)
            .map_or(0, |(position, _)| position);
        cycle.rotate_left(start);
        Cycle {
            entry: self.keys[cycle[0]].clone(),
            rest: cycle[1..]
                .iter()
                .map(|&node| self.keys[node].clone())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Unvisited,
    /// On the current search path, at this depth.
    OnPath(usize),
    Done,
}

/// Every node of an acyclic [`DependencyGraph`], each after all of its
/// dependencies.
///
/// Only [`DependencyGraph::into_topo_order`] constructs this, so holding one
/// proves the order is topological and covers every node exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopoOrder<K> {
    order: Vec<K>,
}

impl<K> TopoOrder<K> {
    /// Iterate dependencies first.
    pub fn iter(&self) -> std::slice::Iter<'_, K> {
        self.order.iter()
    }

    /// The order as a slice, dependencies first.
    #[must_use]
    pub fn as_slice(&self) -> &[K] {
        &self.order
    }

    /// Number of ordered nodes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the graph had no nodes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The order as a vector, dependencies first.
    #[must_use]
    pub fn into_vec(self) -> Vec<K> {
        self.order
    }
}

impl<K> IntoIterator for TopoOrder<K> {
    type Item = K;
    type IntoIter = std::vec::IntoIter<K>;

    fn into_iter(self) -> Self::IntoIter {
        self.order.into_iter()
    }
}

impl<'a, K> IntoIterator for &'a TopoOrder<K> {
    type Item = &'a K;
    type IntoIter = std::slice::Iter<'a, K>;

    fn into_iter(self) -> Self::IntoIter {
        self.order.iter()
    }
}

/// One dependency cycle of a [`DependencyGraph`].
///
/// The path starts at the cycle's earliest-inserted node ([`Cycle::entry`]).
/// Each node depends on the next one, and the last node depends on the entry.
/// A self-loop is a cycle whose path is just the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle<K> {
    entry: K,
    rest: Vec<K>,
}

impl<K> Cycle<K> {
    /// The cycle's earliest-inserted node, where its path starts.
    #[must_use]
    pub const fn entry(&self) -> &K {
        &self.entry
    }

    /// Iterate the cycle path starting at [`Self::entry`]. Each node depends
    /// on the next, and the last depends on the entry.
    pub fn path(&self) -> impl Iterator<Item = &K> + Clone {
        std::iter::once(&self.entry).chain(&self.rest)
    }

    /// Number of distinct nodes on the cycle. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rest.len().saturating_add(1)
    }

    /// Always `false`: a cycle has at least one node.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// The cycle path as a vector, starting at [`Self::entry`].
    #[must_use]
    pub fn into_path(self) -> Vec<K> {
        std::iter::once(self.entry).chain(self.rest).collect()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn graph(
        nodes: &[&'static str],
        edges: &[(&'static str, &'static str)],
    ) -> DependencyGraph<&'static str> {
        let mut graph = DependencyGraph::new();
        for node in nodes {
            graph.add_node(*node);
        }
        for (dependent, dependency) in edges {
            graph.add_dependency(*dependent, *dependency);
        }
        graph
    }

    fn order(graph: DependencyGraph<&'static str>) -> Vec<&'static str> {
        graph.into_topo_order().expect("acyclic").into_vec()
    }

    fn cycle(graph: DependencyGraph<&'static str>) -> Vec<&'static str> {
        graph.into_topo_order().expect_err("cyclic").into_path()
    }

    #[test]
    fn empty_graph_has_an_empty_order() {
        let graph = DependencyGraph::<u32>::new();
        assert!(graph.is_empty());
        assert_eq!(graph.len(), 0);
        let order = graph.into_topo_order().expect("acyclic");
        assert!(order.is_empty());
        assert_eq!(order.len(), 0);
        assert!(order.as_slice().is_empty());
    }

    #[test]
    fn independent_nodes_keep_insertion_order() {
        let graph = graph(&["c", "a", "b"], &[]);
        assert!(!graph.is_empty());
        assert_eq!(graph.len(), 3);
        let order = graph.into_topo_order().expect("acyclic");
        assert!(!order.is_empty());
        assert_eq!(order.into_vec(), ["c", "a", "b"]);
    }

    #[test]
    fn dependencies_come_first() {
        let graph = graph(&["app", "lib", "core"], &[("app", "lib"), ("lib", "core")]);
        assert_eq!(order(graph), ["core", "lib", "app"]);
    }

    #[test]
    fn ready_nodes_are_released_fifo_in_edge_insertion_order() {
        // `root` releases `y` before `x` because that edge was recorded first,
        // and `z` (ready from the start) precedes both.
        let graph = graph(&["x", "y", "root", "z"], &[("y", "root"), ("x", "root")]);
        assert_eq!(order(graph), ["root", "z", "y", "x"]);
    }

    #[test]
    fn diamond_emits_the_join_after_both_branches() {
        let graph = graph(
            &["top", "left", "right", "bottom"],
            &[
                ("left", "top"),
                ("right", "top"),
                ("bottom", "left"),
                ("bottom", "right"),
            ],
        );
        assert_eq!(order(graph), ["top", "left", "right", "bottom"]);
    }

    #[test]
    fn duplicate_nodes_and_edges_are_ignored() {
        let mut graph = graph(&["a", "b"], &[("b", "a"), ("b", "a")]);
        graph.add_node("a");
        assert_eq!(graph.len(), 2);
        assert!(graph.contains(&"a"));
        assert!(!graph.contains(&"c"));
        assert_eq!(order(graph), ["a", "b"]);
    }

    #[test]
    fn edges_add_missing_nodes_dependent_first() {
        let mut graph = DependencyGraph::new();
        graph.add_dependency("x", "y");
        graph.add_node("z");
        assert_eq!(graph.len(), 3);
        // Insertion order is x, y, z; only y and z are initially ready.
        assert_eq!(order(graph), ["y", "z", "x"]);
    }

    #[test]
    fn self_loop_is_a_cycle_of_one() {
        let graph = graph(&["a", "b"], &[("b", "b")]);
        let cycle = graph.into_topo_order().expect_err("cyclic");
        assert_eq!(cycle.entry(), &"b");
        assert_eq!(cycle.len(), 1);
        assert!(!cycle.is_empty());
        assert_eq!(cycle.path().copied().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn cycle_path_follows_dependencies_from_the_earliest_node() {
        let graph = graph(&["a", "b", "c"], &[("c", "a"), ("a", "b"), ("b", "c")]);
        assert_eq!(cycle(graph), ["a", "b", "c"]);

        let graph = graph_with_entry_late();
        assert_eq!(cycle(graph), ["b", "c", "d"]);
    }

    fn graph_with_entry_late() -> DependencyGraph<&'static str> {
        // `a` depends on the cycle but is not on it; the search starts at `a`
        // and enters the cycle at `c`.
        graph(
            &["a", "b", "c", "d"],
            &[("a", "c"), ("c", "d"), ("d", "b"), ("b", "c")],
        )
    }

    #[test]
    fn cycle_reports_only_nodes_on_the_cycle() {
        let graph = graph(
            &["a", "b", "c", "d"],
            &[("d", "c"), ("c", "b"), ("b", "c"), ("a", "d")],
        );
        let cycle = graph.into_topo_order().expect_err("cyclic");
        assert_eq!(cycle.entry(), &"b");
        assert_eq!(cycle.len(), 2);
        assert_eq!(cycle.into_path(), ["b", "c"]);
    }

    #[test]
    fn first_cycle_in_search_order_is_reported() {
        let graph = graph(
            &["p", "q", "x", "y"],
            &[("x", "y"), ("y", "x"), ("p", "q"), ("q", "p")],
        );
        assert_eq!(cycle(graph), ["p", "q"]);
    }

    #[test]
    fn borrowed_iteration_matches_owned_order() {
        let order = graph(&["b", "a"], &[("b", "a")])
            .into_topo_order()
            .expect("acyclic");
        let borrowed = (&order).into_iter().copied().collect::<Vec<_>>();
        assert_eq!(borrowed, order.iter().copied().collect::<Vec<_>>());
        assert_eq!(borrowed, order.into_iter().collect::<Vec<_>>());
        assert_eq!(borrowed, ["a", "b"]);
    }

    #[test]
    fn long_chains_do_not_overflow_the_stack() {
        let mut graph = DependencyGraph::new();
        let depth = 200_000_u32;
        for node in 0..depth {
            graph.add_dependency(node, node + 1);
        }
        let order = graph.clone().into_topo_order().expect("acyclic");
        assert_eq!(order.len(), 200_001);
        assert_eq!(order.as_slice().first(), Some(&depth));

        graph.add_dependency(depth, 0);
        let cycle = graph.into_topo_order().expect_err("cyclic");
        assert_eq!(cycle.entry(), &0);
        assert_eq!(cycle.len(), 200_001);
    }

    proptest! {
        #[test]
        fn result_is_a_valid_order_or_a_real_cycle(
            node_count in 0_usize..12,
            raw_edges in proptest::collection::vec((0_usize..12, 0_usize..12), 0..30),
        ) {
            let edges = raw_edges
                .into_iter()
                .filter(|(from, to)| *from < node_count && *to < node_count)
                .collect::<Vec<_>>();
            let mut graph = DependencyGraph::new();
            for node in 0..node_count {
                graph.add_node(node);
            }
            for &(dependent, dependency) in &edges {
                graph.add_dependency(dependent, dependency);
            }
            let edge_set = edges.iter().copied().collect::<HashSet<_>>();
            match graph.clone().into_topo_order() {
                Ok(order) => {
                    prop_assert_eq!(order.len(), node_count);
                    let rank = order
                        .iter()
                        .enumerate()
                        .map(|(rank, node)| (*node, rank))
                        .collect::<HashMap<_, _>>();
                    prop_assert_eq!(rank.len(), node_count);
                    for (dependent, dependency) in &edges {
                        prop_assert!(rank[dependency] < rank[dependent]);
                    }
                }
                Err(cycle) => {
                    let path = cycle.clone().into_path();
                    prop_assert_eq!(path.len(), cycle.len());
                    prop_assert_eq!(path.iter().collect::<HashSet<_>>().len(), path.len());
                    prop_assert_eq!(Some(&path[0]), path.iter().min());
                    for (position, node) in path.iter().enumerate() {
                        let next = path[(position + 1) % path.len()];
                        prop_assert!(edge_set.contains(&(*node, next)));
                    }
                }
            }
            // Determinism: the same insertions give the same result.
            prop_assert_eq!(graph.clone().into_topo_order(), graph.into_topo_order());
        }
    }
}
