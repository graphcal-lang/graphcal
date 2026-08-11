//! Graphviz DOT rendering of a [`GraphIr`] as a deterministic boundary.
//!
//! Canonical compiler identities never double as DOT identifiers. The renderer
//! assigns opaque local node and cluster ids, keeping readable names in labels.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use graphcal_compiler::dag_id::{DagHierarchyEdge, DagId};

use super::{GraphCluster, GraphIr, GraphNode, GraphNodeId, GraphNodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DotNodeId(usize);

impl std::fmt::Display for DotNodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "n{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DotClusterId(usize);

impl std::fmt::Display for DotClusterId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "c{}", self.0)
    }
}

struct RendererIds {
    nodes: BTreeMap<GraphNodeId, DotNodeId>,
    clusters: BTreeMap<DagId, DotClusterId>,
}

impl RendererIds {
    fn for_graph(ir: &GraphIr) -> Self {
        let mut ids = Self {
            nodes: BTreeMap::new(),
            clusters: BTreeMap::new(),
        };
        for node in ir
            .root
            .nodes
            .iter()
            .chain(ir.children.iter().flat_map(|cluster| &cluster.nodes))
            .chain(&ir.external)
        {
            ids.register_node(&node.id);
        }
        for edge in &ir.edges {
            ids.register_node(&edge.from);
            ids.register_node(&edge.to);
        }
        for cluster in &ir.children {
            if !ids.clusters.contains_key(&cluster.dag_id) {
                ids.clusters
                    .insert(cluster.dag_id.clone(), DotClusterId(ids.clusters.len()));
            }
        }
        ids
    }

    fn register_node(&mut self, identity: &GraphNodeId) {
        if !self.nodes.contains_key(identity) {
            self.nodes
                .insert(identity.clone(), DotNodeId(self.nodes.len()));
        }
    }

    fn node(&self, identity: &GraphNodeId) -> DotNodeId {
        self.nodes[identity]
    }

    fn cluster(&self, identity: &DagId) -> DotClusterId {
        self.clusters[identity]
    }
}

struct RendererLabels {
    ambiguous_external_names: BTreeSet<String>,
}

impl RendererLabels {
    fn for_graph(ir: &GraphIr) -> Self {
        let mut counts = BTreeMap::<String, usize>::new();
        for node in &ir.external {
            counts
                .entry(node.id.to_string())
                .and_modify(|count| *count = 2)
                .or_insert(1);
        }
        Self {
            ambiguous_external_names: counts
                .into_iter()
                .filter_map(|(name, count)| (count > 1).then_some(name))
                .collect(),
        }
    }

    fn node_name(&self, node: &GraphNode) -> String {
        if node.kind != GraphNodeKind::External {
            return node.id.as_str().to_string();
        }
        let display = node.id.to_string();
        if self.ambiguous_external_names.contains(&display) {
            qualified_decl_label(&node.id)
        } else {
            display
        }
    }
}

/// Render the graph as Graphviz DOT text.
#[must_use]
pub fn render(ir: &GraphIr) -> String {
    let ids = RendererIds::for_graph(ir);
    let labels = RendererLabels::for_graph(ir);
    let mut out = String::new();
    out.push_str("digraph graphcal {\n");
    out.push_str("    rankdir=LR;\n");
    out.push_str("    node [fontname=\"Helvetica,Arial,sans-serif\"];\n");

    render_nodes(&mut out, &ir.root.nodes, 1, &ids, &labels);
    for cluster in &ir.children {
        render_cluster(&mut out, cluster, &ids, &labels);
    }
    render_nodes(&mut out, &ir.external, 1, &ids, &labels);

    for edge in &ir.edges {
        let _ = writeln!(
            out,
            "    \"{}\" -> \"{}\";",
            ids.node(&edge.from),
            ids.node(&edge.to)
        );
    }
    out.push_str("}\n");
    out
}

fn render_cluster(
    out: &mut String,
    cluster: &GraphCluster,
    ids: &RendererIds,
    labels: &RendererLabels,
) {
    let _ = writeln!(out, "    subgraph \"{}\" {{", ids.cluster(&cluster.dag_id));
    let _ = writeln!(
        out,
        "        label=\"dag {}\";",
        escape(cluster.dag_id.name())
    );
    render_nodes(out, &cluster.nodes, 2, ids, labels);
    out.push_str("    }\n");
}

fn render_nodes(
    out: &mut String,
    nodes: &[GraphNode],
    indent: usize,
    ids: &RendererIds,
    labels: &RendererLabels,
) {
    let pad = "    ".repeat(indent);
    for node in nodes {
        let attrs = node_attrs(node, labels);
        let _ = writeln!(out, "{pad}\"{}\" [{attrs}];", ids.node(&node.id));
    }
}

fn node_attrs(node: &GraphNode, labels: &RendererLabels) -> String {
    let name = escape(&labels.node_name(node));
    let label = node
        .type_label
        .as_ref()
        .map_or_else(|| name.clone(), |ty| format!("{name}\\n{}", escape(ty)));
    match node.kind {
        GraphNodeKind::Const => format!("label=\"{label}\", shape=box, style=rounded"),
        GraphNodeKind::Param => format!("label=\"{label}\", shape=ellipse"),
        GraphNodeKind::Node => format!("label=\"{label}\", shape=box"),
        GraphNodeKind::External => {
            format!("label=\"{label}\", shape=box, style=dashed")
        }
    }
}

fn qualified_decl_label(identity: &GraphNodeId) -> String {
    format!(
        "{}.{}",
        qualified_dag_label(identity.owner()),
        identity.as_str()
    )
}

fn qualified_dag_label(identity: &DagId) -> String {
    let mut label = format!("{}::{}", identity.package(), identity.segments().first());
    for (edge, segment) in identity
        .hierarchy_edges()
        .iter()
        .zip(identity.segments().iter().skip(1))
    {
        label.push(match edge {
            DagHierarchyEdge::SourceModule => '.',
            DagHierarchyEdge::ConcreteInstance => '@',
        });
        label.push_str(segment);
    }
    label
}

fn escape(value: &str) -> String {
    value.chars().fold(
        String::with_capacity(value.len()),
        |mut escaped, character| {
            match character {
                '"' | '\\' => {
                    escaped.push('\\');
                    escaped.push(character);
                }
                '\n' => escaped.push_str("\\n"),
                _ => escaped.push(character),
            }
            escaped
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};

    use super::*;
    use crate::graph_ir::{GraphCluster, GraphEdge};

    fn id(owner: &DagId, name: &str) -> ResolvedDeclName {
        ResolvedDeclName::from_def(owner.clone(), DeclName::expect_valid(name))
    }

    #[test]
    fn renders_stable_dot() {
        let root_id = DagId::root_in_package("test", "main");
        let child_id = root_id.child("child");
        let external_id = DagId::root_in_package("test", "external");
        let ir = GraphIr {
            root: GraphCluster {
                dag_id: root_id.clone(),
                nodes: vec![GraphNode {
                    id: id(&root_id, "input"),
                    kind: GraphNodeKind::Param,
                    type_label: Some("Real".into()),
                }],
            },
            children: vec![GraphCluster {
                dag_id: child_id.clone(),
                nodes: vec![GraphNode {
                    id: id(&child_id, "output"),
                    kind: GraphNodeKind::Node,
                    type_label: Some("Real".into()),
                }],
            }],
            external: vec![GraphNode {
                id: id(&external_id, "source"),
                kind: GraphNodeKind::External,
                type_label: None,
            }],
            edges: vec![GraphEdge {
                from: id(&external_id, "source"),
                to: id(&child_id, "output"),
            }],
        };

        let dot = render(&ir);
        assert_eq!(
            dot,
            concat!(
                "digraph graphcal {\n",
                "    rankdir=LR;\n",
                "    node [fontname=\"Helvetica,Arial,sans-serif\"];\n",
                "    \"n0\" [label=\"input\\nReal\", shape=ellipse];\n",
                "    subgraph \"c0\" {\n",
                "        label=\"dag child\";\n",
                "        \"n1\" [label=\"output\\nReal\", shape=box];\n",
                "    }\n",
                "    \"n2\" [label=\"external.source\", shape=box, style=dashed];\n",
                "    \"n2\" -> \"n1\";\n",
                "}\n",
            )
        );
    }

    #[test]
    fn opaque_ids_preserve_package_and_hierarchy_identity() {
        let package_a = DagId::root_in_package("package-a", "lib");
        let package_b = DagId::root_in_package("package-b", "lib");
        let source_child = DagId::root_in_package("package-a", "model").child("defaults");
        let instance_child = DagId::root_in_package("package-a", "model")
            .instance_child(Arc::<str>::from("defaults"));
        let external = [package_a, package_b, source_child, instance_child]
            .into_iter()
            .map(|owner| GraphNode {
                id: id(&owner, "value"),
                kind: GraphNodeKind::External,
                type_label: None,
            })
            .collect();
        let root_id = DagId::root_in_package("test", "main");
        let ir = GraphIr {
            root: GraphCluster {
                dag_id: root_id,
                nodes: Vec::new(),
            },
            children: Vec::new(),
            external,
            edges: Vec::new(),
        };

        let dot = render(&ir);
        let statement_ids: Vec<_> = dot
            .lines()
            .filter(|line| line.contains("style=dashed"))
            .map(|line| line.split('"').nth(1).expect("quoted DOT id"))
            .collect();
        assert_eq!(statement_ids.len(), 4);
        assert_eq!(
            statement_ids.iter().copied().collect::<BTreeSet<_>>().len(),
            4
        );
        assert!(dot.contains("package-a::lib.value"));
        assert!(dot.contains("package-b::lib.value"));
        assert!(dot.contains("package-a::model.defaults.value"));
        assert!(dot.contains("package-a::model@defaults.value"));
    }

    #[test]
    fn escapes_quotes_backslashes_and_newlines() {
        assert_eq!(escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape("a\nb"), r"a\nb");
    }
}
