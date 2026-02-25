//! ASCII DAG renderer for the interactive shell `:graph` command.
//!
//! Uses `petgraph` for topological layering and renders the dependency graph
//! with Unicode box-drawing characters.

use std::collections::{HashMap, HashSet};

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};

use graphcal_eval::tir::TIR;

/// Render the dependency graph from a TIR as a Unicode string.
///
/// Shows params/consts at the top, flowing down to leaf nodes at the bottom.
pub fn render_graph(tir: &TIR) -> String {
    // Build a petgraph DiGraph from runtime_deps.
    let mut graph = DiGraph::<String, ()>::new();
    let mut name_to_idx: HashMap<String, NodeIndex> = HashMap::new();

    // Build sets for category lookup.
    let const_names: HashSet<&str> = tir.consts.iter().map(|(n, _, _, _)| n.as_str()).collect();
    let param_names: HashSet<&str> = tir.params.iter().map(|(n, _, _, _)| n.as_str()).collect();
    let assert_names: &HashSet<String> = &tir.assert_names;

    // Add all declarations as nodes.
    for (name, _) in &tir.source_order {
        let prefix = if const_names.contains(name.as_str()) {
            "const"
        } else if param_names.contains(name.as_str()) {
            "param"
        } else if assert_names.contains(name) {
            "assert"
        } else {
            "node"
        };
        let label = format!("{prefix} {name}");
        let idx = graph.add_node(label);
        name_to_idx.insert(name.clone(), idx);
    }

    // Add edges: dep → dependent (so flow goes top-down).
    for (name, deps) in &tir.runtime_deps {
        if let Some(&to_idx) = name_to_idx.get(name) {
            for dep in deps {
                if let Some(&from_idx) = name_to_idx.get(dep) {
                    graph.add_edge(from_idx, to_idx, ());
                }
            }
        }
    }

    // Also add const_deps edges.
    for (name, deps) in &tir.const_deps {
        if let Some(&to_idx) = name_to_idx.get(name) {
            for dep in deps {
                if let Some(&from_idx) = name_to_idx.get(dep) {
                    graph.add_edge(from_idx, to_idx, ());
                }
            }
        }
    }

    // Topological sort to assign layers.
    let Ok(topo) = toposort(&graph, None) else {
        return "  (cycle detected in graph)".to_string();
    };

    if topo.is_empty() {
        return "  (empty graph)".to_string();
    }

    // Assign layers: each node gets layer = max(predecessor layers) + 1.
    let mut layers: HashMap<NodeIndex, usize> = HashMap::new();
    for &idx in &topo {
        let layer = graph
            .neighbors_directed(idx, petgraph::Direction::Incoming)
            .filter_map(|pred| layers.get(&pred))
            .max()
            .map_or(0, |max| max + 1);
        layers.insert(idx, layer);
    }

    // Group nodes by layer.
    let max_layer = layers.values().copied().max().unwrap_or(0);
    let mut layer_nodes: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];
    for (&idx, &layer) in &layers {
        layer_nodes[layer].push(idx);
    }

    // Sort nodes within each layer by name for deterministic output.
    for nodes in &mut layer_nodes {
        nodes.sort_by(|a, b| graph[*a].cmp(&graph[*b]));
    }

    // Render: simple top-to-bottom text representation.
    render_layers(&graph, &layer_nodes)
}

/// Render layers as a simple text representation.
///
/// Each layer is a row of node labels. Edges are shown as vertical lines
/// between layers with connectors.
fn render_layers(graph: &DiGraph<String, ()>, layer_nodes: &[Vec<NodeIndex>]) -> String {
    let mut lines = Vec::new();

    for (layer_idx, nodes) in layer_nodes.iter().enumerate() {
        // Render the node labels for this layer.
        let labels: Vec<&str> = nodes.iter().map(|&idx| graph[idx].as_str()).collect();
        let label_line = labels
            .iter()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("    ");
        lines.push(label_line);

        // If not the last layer, render edge connectors.
        if layer_idx < layer_nodes.len() - 1 {
            let edge_lines = render_edges(graph, nodes);
            lines.extend(edge_lines);
        }
    }

    lines.join("\n")
}

/// Render edge connectors between the current layer and subsequent layers.
fn render_edges(graph: &DiGraph<String, ()>, current_nodes: &[NodeIndex]) -> Vec<String> {
    let mut edge_strs = Vec::new();

    for &node in current_nodes {
        if graph
            .neighbors_directed(node, petgraph::Direction::Outgoing)
            .next()
            .is_none()
        {
            continue;
        }

        let from_label = &graph[node];
        let to_labels: Vec<&str> = graph
            .neighbors_directed(node, petgraph::Direction::Outgoing)
            .map(|s| graph[s].as_str())
            .collect();
        let to_str = to_labels.join(", ");
        edge_strs.push(format!("    {from_label} -> {to_str}"));
    }

    if edge_strs.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    result.push("    │".to_string());
    result.extend(edge_strs);
    result.push(String::new());
    result
}

/// Get the set of transitive dependents of a given name.
///
/// Returns all nodes that directly or transitively depend on `name`.
pub fn transitive_dependents(tir: &TIR, name: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let mut stack = vec![name.to_string()];
    while let Some(current) = stack.pop() {
        for (dep_name, deps) in &tir.runtime_deps {
            if deps.contains(&current) && result.insert(dep_name.clone()) {
                stack.push(dep_name.clone());
            }
        }
    }
    result
}

/// Get the set of direct dependents of a given name.
pub fn direct_dependents(tir: &TIR, name: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    for (dep_name, deps) in &tir.runtime_deps {
        if deps.contains(name) {
            result.insert(dep_name.clone());
        }
    }
    result
}
