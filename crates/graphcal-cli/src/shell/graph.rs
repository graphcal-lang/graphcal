//! Dependency graph visualization for the interactive shell `:graph` command.
//!
//! Builds a `petgraph::DiGraph` from the TIR and delegates rendering to
//! the `graphcal_dag` crate.

use std::collections::{HashMap, HashSet};

use petgraph::graph::DiGraph;

use graphcal_eval::tir::TIR;

/// Render the dependency graph from a TIR as a Unicode string.
///
/// Shows params/consts at the top, flowing down to leaf nodes at the bottom.
pub fn render_graph(tir: &TIR) -> String {
    let graph = build_graph(tir);

    if graph.node_count() == 0 {
        return "  (empty graph)".to_string();
    }

    graphcal_dag::render(&graph)
}

/// Build a petgraph `DiGraph` from the TIR's dependency information.
///
/// Each node is labeled with its category and name (e.g., `"param x"`).
/// Edges go from dependency → dependent (top-down flow).
fn build_graph(tir: &TIR) -> DiGraph<String, ()> {
    let mut graph = DiGraph::<String, ()>::new();
    let mut name_to_idx = HashMap::new();

    // Add all declarations as nodes, using the category from source_order directly.
    for (name, category) in &tir.source_order {
        let label = format!("{category} {name}");
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

    graph
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use graphcal_eval::eval::compile_to_tir;

    use super::*;

    #[test]
    fn snapshot_graph_rocket() {
        let source = include_str!("../../../../tests/fixtures/rocket.gcl");
        let tir = compile_to_tir(source, "rocket.gcl").unwrap();
        let rendered = render_graph(&tir);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_graph_simple_chain() {
        let source = "\
param x: Dimensionless = 1.0;
node y: Dimensionless = @x * 2.0;
node z: Dimensionless = @y + 1.0;
";
        let tir = compile_to_tir(source, "<test>").unwrap();
        let rendered = render_graph(&tir);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_graph_diamond() {
        let source = "\
param a: Dimensionless = 1.0;
node b: Dimensionless = @a * 2.0;
node c: Dimensionless = @a + 3.0;
node d: Dimensionless = @b + @c;
";
        let tir = compile_to_tir(source, "<test>").unwrap();
        let rendered = render_graph(&tir);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn snapshot_graph_no_deps() {
        let source = "\
param x: Dimensionless = 1.0;
param y: Dimensionless = 2.0;
";
        let tir = compile_to_tir(source, "<test>").unwrap();
        let rendered = render_graph(&tir);
        insta::assert_snapshot!(rendered);
    }

    #[test]
    fn transitive_dependents_finds_chain() {
        let source = "\
param x: Dimensionless = 1.0;
node y: Dimensionless = @x * 2.0;
node z: Dimensionless = @y + 1.0;
";
        let tir = compile_to_tir(source, "<test>").unwrap();
        let deps = transitive_dependents(&tir, "x");
        assert!(deps.contains("y"));
        assert!(deps.contains("z"));
        assert!(!deps.contains("x"));
    }

    #[test]
    fn direct_dependents_does_not_include_transitive() {
        let source = "\
param x: Dimensionless = 1.0;
node y: Dimensionless = @x * 2.0;
node z: Dimensionless = @y + 1.0;
";
        let tir = compile_to_tir(source, "<test>").unwrap();
        let deps = direct_dependents(&tir, "x");
        assert!(deps.contains("y"));
        assert!(!deps.contains("z"));
    }
}
