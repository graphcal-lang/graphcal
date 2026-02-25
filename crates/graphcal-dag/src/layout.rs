//! Layer assignment and crossing reduction for DAG layout.
//!
//! Implements a simplified Sugiyama algorithm:
//! 1. Topological sort + longest-path layer assignment
//! 2. Median heuristic for crossing reduction

use std::collections::HashMap;

use petgraph::Direction;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};

/// Assign each node to a layer. Layer 0 = roots (no incoming edges),
/// layer N = max(predecessor layers) + 1.
///
/// Returns a mapping from `NodeIndex` to layer number.
pub fn assign_layers(graph: &DiGraph<String, ()>) -> HashMap<NodeIndex, usize> {
    let Ok(topo) = toposort(graph, None) else {
        return HashMap::new();
    };

    let mut layers: HashMap<NodeIndex, usize> = HashMap::new();
    for &idx in &topo {
        let layer = graph
            .neighbors_directed(idx, Direction::Incoming)
            .filter_map(|pred| layers.get(&pred))
            .max()
            .map_or(0, |max| max + 1);
        layers.insert(idx, layer);
    }
    layers
}

/// Group nodes by layer, returning a vec of layers (each layer is a vec of node indices).
/// Layers are sorted by layer number; nodes within each layer are sorted by label
/// for a stable initial order.
pub fn group_by_layer(
    graph: &DiGraph<String, ()>,
    layers: &HashMap<NodeIndex, usize>,
) -> Vec<Vec<NodeIndex>> {
    let max_layer = layers.values().copied().max().unwrap_or(0);
    let mut layer_nodes: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];

    for (&idx, &layer) in layers {
        layer_nodes[layer].push(idx);
    }

    // Sort each layer by label for deterministic initial order.
    for nodes in &mut layer_nodes {
        nodes.sort_by(|a, b| graph[*a].cmp(&graph[*b]));
    }

    layer_nodes
}

/// Reduce edge crossings between layers using the median heuristic.
///
/// Performs `passes` rounds of alternating top-down and bottom-up sweeps.
/// Each sweep reorders nodes in a layer based on the median position of
/// their neighbors in the adjacent (already-fixed) layer.
pub fn reduce_crossings(
    layer_nodes: &mut [Vec<NodeIndex>],
    graph: &DiGraph<String, ()>,
    passes: usize,
) {
    if layer_nodes.len() < 2 {
        return;
    }

    for _ in 0..passes {
        // Top-down pass: order each layer by median of parents in the layer above.
        for i in 1..layer_nodes.len() {
            let (prev, rest) = layer_nodes.split_at_mut(i);
            let parent_layer = &prev[i - 1];
            sort_by_median(&mut rest[0], parent_layer, graph, Direction::Incoming);
        }

        // Bottom-up pass: order each layer by median of children in the layer below.
        for i in (0..layer_nodes.len() - 1).rev() {
            let (left, right) = layer_nodes.split_at_mut(i + 1);
            let child_layer = &right[0];
            sort_by_median(&mut left[i], child_layer, graph, Direction::Outgoing);
        }
    }
}

/// Sort `layer` nodes by the median position of their neighbors in `adjacent_layer`.
///
/// `direction` controls which neighbors to look at:
/// - `Incoming` → look at parents (in the layer above)
/// - `Outgoing` → look at children (in the layer below)
fn sort_by_median(
    layer: &mut [NodeIndex],
    adjacent_layer: &[NodeIndex],
    graph: &DiGraph<String, ()>,
    direction: Direction,
) {
    // Build a position map for the adjacent layer.
    let pos_map: HashMap<NodeIndex, usize> = adjacent_layer
        .iter()
        .enumerate()
        .map(|(i, &idx)| (idx, i))
        .collect();

    // Compute median position for each node.
    #[expect(clippy::cast_precision_loss, reason = "layer positions are small")]
    let mut medians: Vec<(NodeIndex, f64)> = layer
        .iter()
        .enumerate()
        .map(|(original_pos, &node)| {
            let mut neighbor_positions: Vec<usize> = graph
                .neighbors_directed(node, direction)
                .filter_map(|n| pos_map.get(&n).copied())
                .collect();
            neighbor_positions.sort_unstable();

            let median = if neighbor_positions.is_empty() {
                // Keep original relative position for nodes with no connections.
                original_pos as f64
            } else if neighbor_positions.len() % 2 == 1 {
                neighbor_positions[neighbor_positions.len() / 2] as f64
            } else {
                let mid = neighbor_positions.len() / 2;
                (neighbor_positions[mid - 1] + neighbor_positions[mid]) as f64 / 2.0
            };

            (node, median)
        })
        .collect();

    medians.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    for (i, (node, _)) in medians.into_iter().enumerate() {
        layer[i] = node;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::many_single_char_names, reason = "graph node names in tests")]

    use super::*;

    fn make_chain() -> DiGraph<String, ()> {
        let mut g = DiGraph::new();
        let a = g.add_node("a".into());
        let b = g.add_node("b".into());
        let c = g.add_node("c".into());
        g.add_edge(a, b, ());
        g.add_edge(b, c, ());
        g
    }

    #[test]
    fn layers_linear_chain() {
        let g = make_chain();
        let layers = assign_layers(&g);
        let a = NodeIndex::new(0);
        let b = NodeIndex::new(1);
        let c = NodeIndex::new(2);
        assert_eq!(layers[&a], 0);
        assert_eq!(layers[&b], 1);
        assert_eq!(layers[&c], 2);
    }

    #[test]
    fn layers_diamond() {
        let mut g = DiGraph::new();
        let a = g.add_node("a".into());
        let b = g.add_node("b".into());
        let c = g.add_node("c".into());
        let d = g.add_node("d".into());
        g.add_edge(a, b, ());
        g.add_edge(a, c, ());
        g.add_edge(b, d, ());
        g.add_edge(c, d, ());

        let layers = assign_layers(&g);
        assert_eq!(layers[&a], 0);
        assert_eq!(layers[&b], 1);
        assert_eq!(layers[&c], 1);
        assert_eq!(layers[&d], 2);
    }

    #[test]
    fn group_by_layer_sorts_by_label() {
        let mut g = DiGraph::new();
        let z = g.add_node("z".into());
        let a = g.add_node("a".into());
        g.add_edge(z, a, ());

        let layers = assign_layers(&g);
        let grouped = group_by_layer(&g, &layers);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0], vec![z]); // z at layer 0
        assert_eq!(grouped[1], vec![a]); // a at layer 1
    }
}
