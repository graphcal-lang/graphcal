//! Edge routing and node placement on a [`Canvas`].
//!
//! This module takes the layer assignments and node ordering from [`layout`]
//! and draws the actual DAG on a 2D character grid.

use std::collections::HashMap;

use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::canvas::Canvas;

/// Padding between node labels within a layer.
const NODE_GAP: usize = 4;

/// Vertical rows allocated per inter-layer gap (for edge routing).
const EDGE_ROWS: usize = 3;

/// Layout positions for all nodes and layers.
pub struct LayoutPositions {
    pub node_centers: HashMap<NodeIndex, usize>,
    pub node_widths: HashMap<NodeIndex, usize>,
    pub layer_y: Vec<usize>,
    pub canvas_width: usize,
    pub canvas_height: usize,
}

/// Compute the center x-coordinate and width for each node.
pub fn compute_positions(
    graph: &DiGraph<String, ()>,
    layer_nodes: &[Vec<NodeIndex>],
) -> LayoutPositions {
    // 1. Compute max width needed for each "column slot" across all layers.
    //    We allocate columns based on the widest layer.
    let max_nodes_in_layer = layer_nodes.iter().map(Vec::len).max().unwrap_or(0);
    if max_nodes_in_layer == 0 {
        return LayoutPositions {
            node_centers: HashMap::new(),
            node_widths: HashMap::new(),
            layer_y: Vec::new(),
            canvas_width: 0,
            canvas_height: 0,
        };
    }

    // For each column index, find the max label width across all layers.
    let mut col_widths: Vec<usize> = vec![0; max_nodes_in_layer];
    for layer in layer_nodes {
        for (col, &node) in layer.iter().enumerate() {
            let w = graph[node].chars().count();
            if col < col_widths.len() && w > col_widths[col] {
                col_widths[col] = w;
            }
        }
    }

    // 2. Compute total canvas width from column widths.
    let total: usize = col_widths.iter().sum();
    let canvas_width = if max_nodes_in_layer > 0 {
        total + NODE_GAP * (max_nodes_in_layer - 1) + 2
    } else {
        2
    };

    // 3. Assign each node a center-x based on its column in its layer.
    //    For layers with fewer nodes than the widest layer, center the layer.
    let mut node_centers: HashMap<NodeIndex, usize> = HashMap::new();
    let mut node_widths: HashMap<NodeIndex, usize> = HashMap::new();

    for layer in layer_nodes {
        let full_width = compute_layer_width_for_cols(&col_widths, layer.len(), NODE_GAP);
        let offset = if full_width < canvas_width {
            (canvas_width - full_width) / 2
        } else {
            0
        };

        let mut lx = offset;
        for (col, &node) in layer.iter().enumerate() {
            let w = graph[node].chars().count();
            let effective_col_width = if col < col_widths.len() {
                col_widths[col]
            } else {
                w
            };
            let center = lx + effective_col_width / 2;
            node_centers.insert(node, center);
            node_widths.insert(node, w);
            lx += effective_col_width + NODE_GAP;
        }
    }

    // 4. Compute y-positions for each layer.
    let mut layer_y = Vec::with_capacity(layer_nodes.len());
    let mut y = 0;
    for i in 0..layer_nodes.len() {
        layer_y.push(y);
        if i < layer_nodes.len() - 1 {
            y += 1 + EDGE_ROWS; // 1 for the node row + gap rows
        }
    }
    let canvas_height = y + 1; // +1 for the last layer row

    LayoutPositions {
        node_centers,
        node_widths,
        layer_y,
        canvas_width,
        canvas_height,
    }
}

/// Compute the total width for `n` columns from `col_widths`.
fn compute_layer_width_for_cols(col_widths: &[usize], n: usize, gap: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let sum: usize = col_widths.iter().take(n).sum();
    sum + gap * (n - 1)
}

/// Draw all nodes on the canvas.
pub fn draw_nodes(
    canvas: &mut Canvas,
    graph: &DiGraph<String, ()>,
    layer_nodes: &[Vec<NodeIndex>],
    node_centers: &HashMap<NodeIndex, usize>,
    node_widths: &HashMap<NodeIndex, usize>,
    layer_y: &[usize],
) {
    for (layer_idx, layer) in layer_nodes.iter().enumerate() {
        let y = layer_y[layer_idx];
        for &node in layer {
            let center = node_centers[&node];
            let w = node_widths[&node];
            let x_start = center.saturating_sub(w / 2);
            canvas.put_str(y, x_start, &graph[node]);
        }
    }
}

/// Draw edges between all adjacent layers.
pub fn draw_edges(
    canvas: &mut Canvas,
    graph: &DiGraph<String, ()>,
    layer_nodes: &[Vec<NodeIndex>],
    node_centers: &HashMap<NodeIndex, usize>,
    layer_y: &[usize],
) {
    for layer_idx in 0..layer_nodes.len().saturating_sub(1) {
        draw_layer_edges(
            canvas,
            graph,
            &layer_nodes[layer_idx],
            layer_y[layer_idx],
            layer_y[layer_idx + 1],
            node_centers,
            &layer_nodes[layer_idx + 1],
        );
    }
}

/// Draw edges from one layer to the next.
///
/// Collects all edges between the two layers, then draws them together.
/// This ensures that fan-in (N→1) and fan-out (1→N) produce correct junctions.
fn draw_layer_edges(
    canvas: &mut Canvas,
    graph: &DiGraph<String, ()>,
    src_layer: &[NodeIndex],
    src_y: usize,
    dst_y: usize,
    node_centers: &HashMap<NodeIndex, usize>,
    dst_layer: &[NodeIndex],
) {
    // Build a set of nodes in the destination layer for filtering.
    let dst_set: HashMap<NodeIndex, usize> =
        dst_layer.iter().map(|&n| (n, node_centers[&n])).collect();

    // Collect all edges as (src_x, dst_x) pairs.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for &src in src_layer {
        let src_x = node_centers[&src];
        for child in graph.neighbors_directed(src, Direction::Outgoing) {
            if let Some(&dst_x) = dst_set.get(&child) {
                edges.push((src_x, dst_x));
            }
        }
    }

    if edges.is_empty() {
        return;
    }

    // The routing row sits between source and destination layers.
    let route_y = src_y + 2;

    // Check for straight-down cases (single source → single aligned target).
    // A column is "straight" if it's the only source for that target AND
    // the only target for that source AND src_x == dst_x.
    let mut straight_columns = std::collections::HashSet::new();
    for &(sx, dx) in &edges {
        if sx == dx {
            let src_has_other_targets = edges.iter().any(|&(s, d)| s == sx && d != dx);
            let dst_has_other_sources = edges.iter().any(|&(s, d)| d == dx && s != sx);
            if !src_has_other_targets && !dst_has_other_sources {
                straight_columns.insert(sx);
            }
        }
    }

    // Draw straight-down connections.
    for &col in &straight_columns {
        canvas.vline(col, src_y + 1, dst_y - 1);
        canvas.put(dst_y - 1, col, '↓');
    }

    // For non-straight edges, draw horizontal routing + junctions.
    let routed_edges: Vec<(usize, usize)> = edges
        .iter()
        .filter(|&&(sx, dx)| !straight_columns.contains(&sx) || sx != dx)
        .copied()
        .collect();

    if routed_edges.is_empty() {
        return;
    }

    // Find the horizontal span of the routing line.
    let all_xs: Vec<usize> = routed_edges
        .iter()
        .flat_map(|&(sx, dx)| <[usize; 2]>::from((sx, dx)))
        .collect();
    let min_x = *all_xs.iter().min().unwrap_or(&0);
    let max_x = *all_xs.iter().max().unwrap_or(&0);

    // Collect which columns come from above / go below on the routing line.
    let mut route_from_above = std::collections::HashSet::new();
    let mut route_to_below = std::collections::HashSet::new();
    for &(sx, dx) in &routed_edges {
        route_from_above.insert(sx);
        route_to_below.insert(dx);
    }

    // Draw verticals from source nodes down to the routing row (not including it).
    for &col in &route_from_above {
        canvas.vline(col, src_y + 1, route_y - 1);
    }

    // Draw verticals from routing row down to destination nodes.
    for &col in &route_to_below {
        canvas.vline(col, route_y + 1, dst_y - 1);
        canvas.put(dst_y - 1, col, '↓');
    }

    // Build the routing row character-by-character.
    for col in min_x..=max_x {
        let from_above = route_from_above.contains(&col);
        let to_below = route_to_below.contains(&col);
        let is_left = col == min_x;
        let is_right = col == max_x;

        let ch = match (from_above, to_below, is_left, is_right) {
            // Corners (endpoints of horizontal line).
            (true, true, true, _) => '├',
            (true, true, _, true) => '┤',
            (true, false, true, _) => '└',
            (true, false, _, true) => '┘',
            (false, true, true, _) => '┌',
            (false, true, _, true) => '┐',
            // Interior junctions.
            (true, true, false, false) => '┼',
            (true, false, false, false) => '┴',
            (false, true, false, false) => '┬',
            // Plain horizontal.
            (false, false, _, _) => '─',
        };
        canvas.put_overwrite(route_y, col, ch);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn positions_single_node() {
        let mut g = DiGraph::new();
        g.add_node("hello".into());
        let layers = vec![vec![NodeIndex::new(0)]];
        let pos = compute_positions(&g, &layers);
        assert_eq!(pos.node_widths[&NodeIndex::new(0)], 5);
        assert!(pos.node_centers[&NodeIndex::new(0)] >= 2); // center of "hello"
        assert_eq!(pos.layer_y, vec![0]);
        assert!(pos.canvas_width > 0);
        assert_eq!(pos.canvas_height, 1);
    }

    #[test]
    fn positions_two_layers() {
        let mut g = DiGraph::new();
        let a = g.add_node("A".into());
        let b = g.add_node("B".into());
        g.add_edge(a, b, ());
        let layers = vec![vec![a], vec![b]];
        let pos = compute_positions(&g, &layers);
        assert_eq!(pos.layer_y.len(), 2);
        assert_eq!(pos.layer_y[0], 0);
        assert_eq!(pos.layer_y[1], 4); // 0 + 1 + EDGE_ROWS(3)
        assert_eq!(pos.canvas_height, 5); // row 4 + 1
    }
}
