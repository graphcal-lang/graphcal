//! ASCII DAG renderer using Unicode box-drawing characters.
//!
//! Renders a `petgraph::DiGraph<String, ()>` as a top-to-bottom Unicode string
//! suitable for terminal display.
//!
//! # Algorithm
//!
//! Uses a simplified Sugiyama layered layout:
//! 1. **Layer assignment** — topological sort, `layer = max(predecessor layers) + 1`
//! 2. **Crossing reduction** — median heuristic (top-down + bottom-up sweeps)
//! 3. **Coordinate assignment** — center nodes in column slots
//! 4. **Edge routing** — box-drawing characters with fan-out/fan-in support

mod canvas;
mod layout;
mod render;

use petgraph::graph::DiGraph;

use canvas::Canvas;
use layout::{assign_layers, group_by_layer, reduce_crossings};
use render::{compute_positions, draw_edges, draw_nodes};

/// Number of crossing-reduction passes (top-down + bottom-up sweeps).
const CROSSING_REDUCTION_PASSES: usize = 2;

/// Render a directed acyclic graph as a Unicode string.
///
/// Nodes flow top-to-bottom (roots at top, leaves at bottom).
/// Node labels are taken from the graph's node weights.
///
/// Returns an empty string for an empty graph, and `"(cycle detected)"` if
/// the graph contains a cycle.
#[must_use]
pub fn render(graph: &DiGraph<String, ()>) -> String {
    if graph.node_count() == 0 {
        return String::new();
    }

    // 1. Assign layers.
    let layers = assign_layers(graph);
    if layers.is_empty() {
        return "(cycle detected)".to_string();
    }

    // 2. Group by layer and apply crossing reduction.
    let mut layer_nodes = group_by_layer(graph, &layers);
    reduce_crossings(&mut layer_nodes, graph, CROSSING_REDUCTION_PASSES);

    // 3. Compute positions.
    let pos = compute_positions(graph, &layer_nodes);

    if pos.canvas_width == 0 || pos.canvas_height == 0 {
        return String::new();
    }

    // 4. Create canvas and draw.
    let mut canvas = Canvas::new(pos.canvas_width, pos.canvas_height);
    draw_nodes(
        &mut canvas,
        graph,
        &layer_nodes,
        &pos.node_centers,
        &pos.node_widths,
        &pos.layer_y,
    );
    draw_edges(
        &mut canvas,
        graph,
        &layer_nodes,
        &pos.node_centers,
        &pos.layer_y,
    );

    canvas.to_string_trimmed()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::many_single_char_names, reason = "graph node names in tests")]

    use petgraph::graph::DiGraph;

    use super::*;

    #[test]
    fn empty_graph() {
        let g: DiGraph<String, ()> = DiGraph::new();
        assert_eq!(render(&g), "");
    }

    #[test]
    fn single_node() {
        let mut g = DiGraph::new();
        g.add_node("param x".into());
        let rendered = render(&g);
        assert!(rendered.contains("param x"));
    }

    #[test]
    fn snapshot_linear_chain() {
        let mut g = DiGraph::new();
        let a = g.add_node("param x".into());
        let b = g.add_node("node y".into());
        let c = g.add_node("node z".into());
        g.add_edge(a, b, ());
        g.add_edge(b, c, ());
        insta::assert_snapshot!(render(&g));
    }

    #[test]
    fn snapshot_diamond() {
        let mut g = DiGraph::new();
        let a = g.add_node("param a".into());
        let b = g.add_node("node b".into());
        let c = g.add_node("node c".into());
        let d = g.add_node("node d".into());
        g.add_edge(a, b, ());
        g.add_edge(a, c, ());
        g.add_edge(b, d, ());
        g.add_edge(c, d, ());
        insta::assert_snapshot!(render(&g));
    }

    #[test]
    fn snapshot_fan_in() {
        let mut g = DiGraph::new();
        let x = g.add_node("param x".into());
        let y = g.add_node("param y".into());
        let z = g.add_node("node z".into());
        g.add_edge(x, z, ());
        g.add_edge(y, z, ());
        insta::assert_snapshot!(render(&g));
    }

    #[test]
    fn snapshot_independent_params() {
        let mut g = DiGraph::new();
        g.add_node("param x".into());
        g.add_node("param y".into());
        insta::assert_snapshot!(render(&g));
    }

    #[test]
    fn snapshot_fan_out() {
        let mut g = DiGraph::new();
        let a = g.add_node("param a".into());
        let b = g.add_node("node b".into());
        let c = g.add_node("node c".into());
        let d = g.add_node("node d".into());
        g.add_edge(a, b, ());
        g.add_edge(a, c, ());
        g.add_edge(a, d, ());
        insta::assert_snapshot!(render(&g));
    }

    #[test]
    fn snapshot_rocket() {
        let mut g = DiGraph::new();
        let g0 = g.add_node("const G0".into());
        let dm = g.add_node("param dry_mass".into());
        let fm = g.add_node("param fuel_mass".into());
        let isp = g.add_node("param isp".into());
        let mr = g.add_node("node mass_ratio".into());
        let ve = g.add_node("node v_exhaust".into());
        let dv = g.add_node("node delta_v".into());
        g.add_edge(dm, mr, ());
        g.add_edge(fm, mr, ());
        g.add_edge(isp, ve, ());
        g.add_edge(g0, ve, ());
        g.add_edge(mr, dv, ());
        g.add_edge(ve, dv, ());
        insta::assert_snapshot!(render(&g));
    }
}
