//! Graphviz DOT rendering of a [`GraphIr`].
//!
//! A pure serialization boundary: [`render`] maps the typed graph model to
//! DOT text and nothing else. Output is deterministic (the IR is already
//! ordered) so it diffs cleanly and snapshots stably. Pipe the result to
//! `dot -Tsvg` (or any Graphviz layout engine) to produce an image.

use std::fmt::Write as _;

use super::{GraphCluster, GraphIr, GraphNode, GraphNodeKind};

/// Render the graph as Graphviz DOT text.
#[must_use]
pub fn render(ir: &GraphIr) -> String {
    let mut out = String::new();
    out.push_str("digraph graphcal {\n");
    out.push_str("    rankdir=LR;\n");
    out.push_str("    node [fontname=\"Helvetica,Arial,sans-serif\"];\n");

    // The file's top-level body renders unclustered; inline `dag` blocks
    // become labeled clusters.
    render_nodes(&mut out, &ir.root.nodes, 1);
    for cluster in &ir.children {
        render_cluster(&mut out, cluster);
    }
    render_nodes(&mut out, &ir.external, 1);

    for edge in &ir.edges {
        let _ = writeln!(
            out,
            "    \"{}\" -> \"{}\";",
            escape(&edge.from.to_string()),
            escape(&edge.to.to_string())
        );
    }

    out.push_str("}\n");
    out
}

fn render_cluster(out: &mut String, cluster: &GraphCluster) {
    let _ = writeln!(
        out,
        "    subgraph \"cluster_{}\" {{",
        escape(&cluster.dag_id.to_string())
    );
    let _ = writeln!(
        out,
        "        label=\"dag {}\";",
        escape(cluster.dag_id.name())
    );
    render_nodes(out, &cluster.nodes, 2);
    out.push_str("    }\n");
}

fn render_nodes(out: &mut String, nodes: &[GraphNode], depth: usize) {
    let indent = "    ".repeat(depth);
    for node in nodes {
        let _ = writeln!(
            out,
            "{indent}\"{}\" [label=\"{}\"{}];",
            escape(&node.id.to_string()),
            node_label(node),
            kind_attrs(node.kind)
        );
    }
}

/// The visible label: leaf name, plus the resolved type on a second line.
/// External nodes keep their full canonical name — the leaf alone is
/// ambiguous once two files export the same name.
fn node_label(node: &GraphNode) -> String {
    let name = if node.kind == GraphNodeKind::External {
        escape(&node.id.to_string())
    } else {
        escape(node.id.as_str())
    };
    node.type_label
        .as_ref()
        .map_or_else(|| name.clone(), |ty| format!("{name}\\n{}", escape(ty)))
}

/// Styling per declaration kind: params are the graph's inputs (ellipses),
/// consts are rounded boxes, computed nodes plain boxes, and external
/// dependencies dashed boxes labeled with their full canonical name.
const fn kind_attrs(kind: GraphNodeKind) -> &'static str {
    match kind {
        GraphNodeKind::Const => ", shape=box, style=rounded",
        GraphNodeKind::Param => ", shape=ellipse",
        GraphNodeKind::Node => ", shape=box",
        GraphNodeKind::External => ", shape=box, style=dashed",
    }
}

/// Escape a string for use inside a double-quoted DOT id or label.
fn escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                escaped.push('\\');
                escaped.push(c);
            }
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_ir::{GraphEdge, GraphNodeId};
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::decl_name::DeclName;

    fn id(owner: &DagId, name: &str) -> GraphNodeId {
        GraphNodeId::from_def(owner.clone(), DeclName::expect_valid(name))
    }

    #[test]
    fn renders_clusters_external_nodes_and_edges() {
        let root_dag = DagId::root_in_package("test", "main");
        let child_dag = root_dag.child("scale");
        let dep_dag = DagId::root_in_package("test", "lib");

        let ir = GraphIr {
            root: GraphCluster {
                dag_id: root_dag.clone(),
                nodes: vec![
                    GraphNode {
                        id: id(&root_dag, "speed"),
                        kind: GraphNodeKind::Param,
                        type_label: Some("Velocity".to_string()),
                    },
                    GraphNode {
                        id: id(&root_dag, "doubled"),
                        kind: GraphNodeKind::Node,
                        type_label: Some("Velocity".to_string()),
                    },
                ],
            },
            children: vec![GraphCluster {
                dag_id: child_dag.clone(),
                nodes: vec![GraphNode {
                    id: id(&child_dag, "result"),
                    kind: GraphNodeKind::Node,
                    type_label: None,
                }],
            }],
            external: vec![GraphNode {
                id: id(&dep_dag, "g0"),
                kind: GraphNodeKind::External,
                type_label: None,
            }],
            edges: vec![
                GraphEdge {
                    from: id(&dep_dag, "g0"),
                    to: id(&root_dag, "doubled"),
                },
                GraphEdge {
                    from: id(&root_dag, "speed"),
                    to: id(&root_dag, "doubled"),
                },
            ],
        };

        insta::assert_snapshot!(render(&ir), @r#"
        digraph graphcal {
            rankdir=LR;
            node [fontname="Helvetica,Arial,sans-serif"];
            "main.speed" [label="speed\nVelocity", shape=ellipse];
            "main.doubled" [label="doubled\nVelocity", shape=box];
            subgraph "cluster_main.scale" {
                label="dag scale";
                "main.scale.result" [label="result", shape=box];
            }
            "lib.g0" [label="lib.g0", shape=box, style=dashed];
            "lib.g0" -> "main.doubled";
            "main.speed" -> "main.doubled";
        }
        "#);
    }

    #[test]
    #[ignore = "fixed by #1258 in Phase 4"]
    fn dot_ids_preserve_package_and_hierarchy_edge_identity() {
        let package_a = DagId::root_in_package("package-a", "lib");
        let package_b = DagId::root_in_package("package-b", "lib");
        let hierarchy_parent = DagId::root_in_package("package-a", "model");
        let source_child = hierarchy_parent.child("defaults");
        let instance_child = hierarchy_parent.instance_child("defaults");
        let root = DagId::root_in_package("root", "main");
        let external = [package_a, package_b, source_child, instance_child]
            .into_iter()
            .map(|owner| GraphNode {
                id: id(&owner, "value"),
                kind: GraphNodeKind::External,
                type_label: None,
            })
            .collect();
        let ir = GraphIr {
            root: GraphCluster {
                dag_id: root,
                nodes: Vec::new(),
            },
            children: Vec::new(),
            external,
            edges: Vec::new(),
        };

        let dot = render(&ir);
        let statement_ids = dot
            .lines()
            .filter(|line| line.contains("shape=box, style=dashed"))
            .filter_map(|line| line.trim().split_once(" [").map(|(id, _)| id))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            statement_ids.len(),
            4,
            "distinct typed IDs collapsed:\n{dot}"
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        assert_eq!(escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape("a\nb"), r"a\nb");
    }
}
