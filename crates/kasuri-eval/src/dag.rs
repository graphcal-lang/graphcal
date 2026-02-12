use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::Expr;
use kasuri_syntax::span::Span;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::error::KasuriError;
use crate::resolve::ResolvedFile;

pub type NodeIndex = petgraph::graph::NodeIndex;

/// The runtime DAG with topological order for evaluation.
#[derive(Debug)]
pub struct RuntimeGraph {
    pub graph: DiGraph<String, ()>,
    pub topo_order: Vec<NodeIndex>,
    /// Expressions for each param/node.
    pub expressions: HashMap<String, Expr>,
    /// Which names are params (with default value).
    pub param_names: Vec<String>,
    /// Which names are nodes.
    pub node_names: Vec<String>,
}

/// Build a petgraph DAG from param and node declarations.
/// Params and nodes are all in the same graph; edges come from `@` references.
pub fn build_dag(
    resolved: &ResolvedFile,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeGraph, KasuriError> {
    let mut graph = DiGraph::<String, ()>::new();
    let mut index_map: HashMap<String, NodeIndex> = HashMap::new();
    let mut expressions: HashMap<String, Expr> = HashMap::new();
    let mut param_names = Vec::new();
    let mut node_names = Vec::new();

    // Add all params and nodes as graph nodes
    for (name, expr, _) in &resolved.params {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
        expressions.insert(name.clone(), expr.clone());
        param_names.push(name.clone());
    }
    for (name, expr, _) in &resolved.nodes {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
        expressions.insert(name.clone(), expr.clone());
        node_names.push(name.clone());
    }

    // Add edges from dependencies
    for (name, deps) in &resolved.runtime_deps {
        if let Some(&to_idx) = index_map.get(name) {
            for dep in deps {
                if let Some(&from_idx) = index_map.get(dep) {
                    graph.add_edge(from_idx, to_idx, ()); // dep -> name
                }
            }
        }
    }

    // Topological sort — detect cycles
    let topo_order = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = resolved
            .nodes
            .iter()
            .chain(resolved.params.iter())
            .find(|(n, _, _)| n == cycle_node)
            .map(|(_, _, s)| *s)
            .unwrap_or(Span::new(0, 0));
        KasuriError::CyclicDependency {
            name: cycle_node.clone(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    Ok(RuntimeGraph {
        graph,
        topo_order,
        expressions,
        param_names,
        node_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve;
    use kasuri_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_resolve_build(source: &str) -> Result<RuntimeGraph, KasuriError> {
        let file = Parser::new(source).parse_file().unwrap();
        let src = make_src(source);
        let resolved = resolve(&file, &src)?;
        build_dag(&resolved, &src)
    }

    #[test]
    fn dag_rocket_ksr() {
        let source = include_str!("../../../tests/fixtures/rocket.ksr");
        let file = Parser::new(source).parse_file().unwrap();
        let src = make_src(source);
        let resolved = resolve(&file, &src).unwrap();
        let dag = build_dag(&resolved, &src).unwrap();

        assert_eq!(dag.topo_order.len(), 6);
        assert_eq!(dag.param_names.len(), 3);
        assert_eq!(dag.node_names.len(), 3);

        let topo_names: Vec<&str> = dag
            .topo_order
            .iter()
            .map(|idx| dag.graph[*idx].as_str())
            .collect();
        let isp_pos = topo_names.iter().position(|n| *n == "isp").unwrap();
        let v_exhaust_pos = topo_names.iter().position(|n| *n == "v_exhaust").unwrap();
        assert!(isp_pos < v_exhaust_pos);
    }

    #[test]
    fn dag_cycle_detected() {
        let err = parse_resolve_build("node a = @b + 1.0;\nnode b = @a + 1.0;").unwrap_err();
        assert!(matches!(err, KasuriError::CyclicDependency { .. }));
    }

    #[test]
    fn dag_simple_chain() {
        let dag =
            parse_resolve_build("param x = 1.0;\nnode y = @x + 1.0;\nnode z = @y * 2.0;").unwrap();
        let topo_names: Vec<&str> = dag
            .topo_order
            .iter()
            .map(|idx| dag.graph[*idx].as_str())
            .collect();
        let x_pos = topo_names.iter().position(|n| *n == "x").unwrap();
        let y_pos = topo_names.iter().position(|n| *n == "y").unwrap();
        let z_pos = topo_names.iter().position(|n| *n == "z").unwrap();
        assert!(x_pos < y_pos);
        assert!(y_pos < z_pos);
    }
}
