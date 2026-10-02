//! Generic dependency graphs with deterministic topological ordering.
//!
//! A [`DependencyGraph<K>`] records "`dependent` depends on `dependency`"
//! edges between keys. [`DependencyGraph::into_topo_order`] and
//! [`DependencyGraph::into_depth_first_order`] either prove the graph acyclic
//! by returning a [`TopoOrder<K>`] (every key after all of its dependencies)
//! or return a [`Cycle<K>`] naming one dependency cycle. They differ only in
//! how they order independent keys.
//!
//! Both results are pure functions of the insertion order of nodes and edges:
//! no hash iteration order leaks into them. Callers that need an order that is
//! stable across runs must therefore insert nodes and edges in a stable order.
//!
//! An edge may carry a label `E` (such as the source span of the reference it
//! models); a [`Cycle`] then reports the label of the edge that closes it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

/// A directed graph of "depends on" edges between keys, each edge labelled
/// by an `E`.
///
/// Nodes and edges remember their insertion order, which fully determines the
/// resulting [`TopoOrder`] or [`Cycle`]. Adding a node or an edge twice is a
/// no-op (an edge keeps its first label), and a key may depend on itself (a
/// self-loop is a cycle of length 1).
#[derive(Debug, Clone)]
pub struct DependencyGraph<K, E = ()> {
    keys: Vec<K>,
    positions: HashMap<K, usize>,
    /// For each node, its dependencies and their edge labels in edge
    /// insertion order.
    dependencies: Vec<Vec<(usize, E)>>,
    /// For each node, its dependents in edge insertion order.
    dependents: Vec<Vec<usize>>,
    edges: HashSet<(usize, usize)>,
}

impl<K, E> Default for DependencyGraph<K, E> {
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
    /// Record that `dependent` depends on `dependency`, adding either key as
    /// a node first if it is not yet present (`dependent` before
    /// `dependency`). Recording the same edge twice is a no-op.
    pub fn add_dependency(&mut self, dependent: K, dependency: K) {
        self.add_labelled_dependency(dependent, dependency, ());
    }
}

impl<K: Clone + Eq + Hash, E: Clone> DependencyGraph<K, E> {
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

    /// Record that `dependent` depends on `dependency` through an edge
    /// labelled `label`, adding either key as a node first if it is not yet
    /// present (`dependent` before `dependency`). Recording the same edge
    /// twice is a no-op that keeps the first label.
    pub fn add_labelled_dependency(&mut self, dependent: K, dependency: K, label: E) {
        let dependent = self.position_of(dependent);
        let dependency = self.position_of(dependency);
        if self.edges.insert((dependent, dependency)) {
            self.dependencies[dependent].push((dependency, label));
            self.dependents[dependency].push(dependent);
        }
    }

    /// Record that the node `dependent` depends on the node `dependency`
    /// through an edge labelled `label`, when both are nodes of this graph,
    /// naming each by any form `K` borrows as; returns whether both are.
    /// Recording the same edge twice is a no-op that keeps the first label.
    pub fn add_labelled_dependency_between<Q>(
        &mut self,
        dependent: &Q,
        dependency: &Q,
        label: E,
    ) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let (Some(&dependent), Some(&dependency)) = (
            self.positions.get(dependent),
            self.positions.get(dependency),
        ) else {
            return false;
        };
        if self.edges.insert((dependent, dependency)) {
            self.dependencies[dependent].push((dependency, label));
            self.dependents[dependency].push(dependent);
        }
        true
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
    /// Returns the [`Cycle`] found by the depth-first search of
    /// [`Self::into_depth_first_order`].
    pub fn into_topo_order(self) -> Result<TopoOrder<K>, Cycle<K, E>> {
        if let Err(cycle) = self.depth_first() {
            return Err(self.cycle_from_indices(cycle));
        }
        let rank = self.kahn_ranks();
        let mut ranked = rank.into_iter().zip(self.keys).collect::<Vec<_>>();
        ranked.sort_unstable_by_key(|(rank, _)| *rank);
        Ok(TopoOrder {
            order: ranked.into_iter().map(|(_, key)| key).collect(),
        })
    }

    /// Order every node after all of its dependencies, in depth-first
    /// post-order.
    ///
    /// The search starts from each not yet visited node in node insertion
    /// order and follows dependencies in edge insertion order; a node is
    /// emitted once all of its dependencies are. When the first node inserted
    /// reaches every other node, it is emitted last.
    ///
    /// # Errors
    ///
    /// Returns the first [`Cycle`] this search closes: the path from the node
    /// it re-entered to the node whose dependency re-entered it.
    pub fn into_depth_first_order(self) -> Result<TopoOrder<K>, Cycle<K, E>> {
        match self.depth_first() {
            Ok(post_order) => {
                let mut keys = self.keys.into_iter().map(Some).collect::<Vec<_>>();
                Ok(TopoOrder {
                    order: post_order
                        .into_iter()
                        .filter_map(|node| keys[node].take())
                        .collect(),
                })
            }
            Err(cycle) => Err(self.cycle_from_indices(cycle)),
        }
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

    /// Node indices in depth-first post-order, or the first cycle found as
    /// the node the search re-entered, the rest of the cycle path, and the
    /// label of the edge that re-entered it. Iterative, so deep graphs cannot
    /// overflow the call stack.
    fn depth_first(&self) -> Result<Vec<usize>, IndexCycle<E>> {
        let mut state = vec![Visit::Unvisited; self.keys.len()];
        let mut post_order = Vec::with_capacity(self.keys.len());
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
                let Some((dependency, label)) = self.dependencies[node].get(cursor) else {
                    state[node] = Visit::Done;
                    post_order.push(node);
                    path.pop();
                    continue;
                };
                frame.1 = cursor.saturating_add(1);
                let dependency = *dependency;
                match state[dependency] {
                    Visit::Unvisited => {
                        state[dependency] = Visit::OnPath(path.len());
                        path.push((dependency, 0));
                    }
                    Visit::OnPath(position) => {
                        let rest = path[position..].iter().skip(1).map(|(node, _)| *node);
                        return Err(IndexCycle {
                            entry: dependency,
                            rest: rest.collect(),
                            closing: label.clone(),
                        });
                    }
                    Visit::Done => {}
                }
            }
        }
        Ok(post_order)
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

    fn cycle_from_indices(&self, cycle: IndexCycle<E>) -> Cycle<K, E> {
        Cycle {
            entry: self.keys[cycle.entry].clone(),
            rest: cycle
                .rest
                .iter()
                .map(|&node| self.keys[node].clone())
                .collect(),
            closing: cycle.closing,
        }
    }
}

/// A cycle the depth-first search found, by node index.
struct IndexCycle<E> {
    entry: usize,
    rest: Vec<usize>,
    closing: E,
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

    /// Transform every node, keeping the order.
    #[must_use]
    pub fn map<U>(self, f: impl FnMut(K) -> U) -> TopoOrder<U> {
        TopoOrder {
            order: self.order.into_iter().map(f).collect(),
        }
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
/// The path starts at [`Cycle::entry`], the node at which the depth-first
/// search (roots in node insertion order, dependencies in edge insertion
/// order) re-entered its current path. Each node depends on the next one, and
/// the last node depends on the entry, through the edge whose label is
/// [`Cycle::closing_label`]. A self-loop is a cycle whose path is just the
/// entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle<K, E = ()> {
    entry: K,
    rest: Vec<K>,
    closing: E,
}

impl<K, E> Cycle<K, E> {
    /// The node the search re-entered, where the cycle path starts.
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

    /// The label of the edge by which the last node on the path depends on
    /// the entry.
    #[must_use]
    pub const fn closing_label(&self) -> &E {
        &self.closing
    }

    /// Transform every node, keeping the path order and the closing label.
    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(K) -> U) -> Cycle<U, E> {
        Cycle {
            entry: f(self.entry),
            rest: self.rest.into_iter().map(f).collect(),
            closing: self.closing,
        }
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
    fn cycle_reports_the_label_of_its_closing_edge() {
        let mut graph = DependencyGraph::new();
        graph.add_labelled_dependency("a", "b", 1);
        graph.add_labelled_dependency("b", "c", 2);
        graph.add_labelled_dependency("c", "a", 3);
        graph.add_labelled_dependency("c", "a", 4);
        let cycle = graph.into_topo_order().unwrap_err();
        assert_eq!(cycle.entry(), &"a");
        assert_eq!(cycle.closing_label(), &3);
        assert_eq!(cycle.map(str::len).closing_label(), &3);
    }

    #[test]
    fn edges_between_existing_nodes_are_named_by_a_borrowed_form() {
        let mut graph = DependencyGraph::<String>::new();
        graph.add_node("a".to_owned());
        graph.add_node("b".to_owned());
        assert!(graph.add_labelled_dependency_between("a", "b", ()));
        // A dependency on a key that is not a node is not recorded.
        assert!(!graph.add_labelled_dependency_between("a", "missing", ()));
        assert!(!graph.add_labelled_dependency_between("missing", "a", ()));
        assert_eq!(graph.len(), 2);
        let order = graph.into_topo_order().unwrap().map(|key| key.len());
        assert_eq!(order.as_slice(), [1, 1]);
        let mut cyclic = DependencyGraph::<String, u8>::new();
        cyclic.add_node("a".to_owned());
        cyclic.add_node("b".to_owned());
        assert!(cyclic.add_labelled_dependency_between("a", "b", 1));
        assert!(cyclic.add_labelled_dependency_between("b", "a", 2));
        // The first label of an edge is kept.
        assert!(cyclic.add_labelled_dependency_between("b", "a", 3));
        let cycle = cyclic.into_topo_order().unwrap_err();
        assert_eq!(cycle.path().cloned().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(cycle.closing_label(), &2);
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
    fn cycle_path_starts_where_the_search_re_entered_it() {
        let graph = graph(&["a", "b", "c"], &[("c", "a"), ("a", "b"), ("b", "c")]);
        assert_eq!(cycle(graph), ["a", "b", "c"]);

        // `b` was inserted before `c`, but the search entered the cycle at `c`.
        let graph = graph_with_entry_late();
        assert_eq!(cycle(graph), ["c", "d", "b"]);
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
        assert_eq!(cycle.entry(), &"c");
        assert_eq!(cycle.len(), 2);
        assert_eq!(cycle.into_path(), ["c", "b"]);
    }

    #[test]
    fn depth_first_order_emits_each_node_after_its_dependencies_in_post_order() {
        // FIFO Kahn would emit the independent `leaf` before `mid`.
        let graph = graph(
            &["root", "mid", "base", "leaf"],
            &[("root", "mid"), ("mid", "base"), ("root", "leaf")],
        );
        assert_eq!(order(graph.clone()), ["base", "leaf", "mid", "root"]);
        let order = graph.into_depth_first_order().expect("acyclic");
        assert_eq!(order.into_vec(), ["base", "mid", "leaf", "root"]);
    }

    #[test]
    fn depth_first_order_visits_later_roots_in_insertion_order() {
        let graph = graph(&["a", "b", "c"], &[("c", "a")]);
        let order = graph.into_depth_first_order().expect("acyclic");
        assert_eq!(order.into_vec(), ["a", "b", "c"]);
    }

    #[test]
    fn depth_first_order_reports_the_same_cycle() {
        let graph = graph_with_entry_late();
        let cycle = graph.into_depth_first_order().expect_err("cyclic");
        assert_eq!(cycle.into_path(), ["c", "d", "b"]);
    }

    #[test]
    fn cycle_map_keeps_the_path_order() {
        let cycle = graph_with_entry_late()
            .into_topo_order()
            .expect_err("cyclic")
            .map(str::len);
        assert_eq!(cycle.entry(), &1);
        assert_eq!(cycle.len(), 3);
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
                    for (position, node) in path.iter().enumerate() {
                        let next = path[(position + 1) % path.len()];
                        prop_assert!(edge_set.contains(&(*node, next)));
                    }
                }
            }
            // Both orders agree on acyclicity and on the reported cycle.
            match (graph.clone().into_topo_order(), graph.clone().into_depth_first_order()) {
                (Ok(kahn), Ok(depth_first)) => {
                    prop_assert_eq!(depth_first.len(), node_count);
                    let rank = depth_first
                        .iter()
                        .enumerate()
                        .map(|(rank, node)| (*node, rank))
                        .collect::<HashMap<_, _>>();
                    prop_assert_eq!(rank.len(), node_count);
                    for (dependent, dependency) in &edges {
                        prop_assert!(rank[dependency] < rank[dependent]);
                    }
                    prop_assert_eq!(kahn.len(), depth_first.len());
                }
                (Err(kahn), Err(depth_first)) => prop_assert_eq!(kahn, depth_first),
                (kahn, depth_first) => {
                    prop_assert!(false, "orders disagree: {kahn:?} vs {depth_first:?}");
                }
            }
            // Determinism: the same insertions give the same result.
            prop_assert_eq!(graph.clone().into_topo_order(), graph.into_topo_order());
        }
    }
}
