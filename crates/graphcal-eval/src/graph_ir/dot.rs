//! Graphviz DOT rendering of a [`GraphIr`] as a deterministic boundary.
//!
//! Canonical compiler identities never double as DOT identifiers. The renderer
//! assigns opaque local node and cluster ids, keeping readable names in labels.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::num::NonZeroUsize;

use graphcal_compiler::dag_id::{DagHierarchyEdge, DagId};

use super::{GraphCluster, GraphClusterKind, GraphIr, GraphNode, GraphNodeId, GraphNodeKind};

/// Level of composition detail emitted into DOT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphView {
    /// Every declaration and dependency edge without composition boundaries.
    Flat,
    /// Every declaration and dependency edge, clustered by source module or
    /// concrete include instance. When `max_depth` is set, clusters at that
    /// composition depth are collapsed to summary nodes.
    Grouped {
        /// Positive number of composition levels to display. Level 1 expands
        /// the root and collapses its direct child DAGs.
        max_depth: Option<NonZeroUsize>,
    },
    /// One node per source module/include instance with declaration-level edges
    /// collapsed to module-level dataflow.
    Module,
}

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
        // Graphviz recognizes a subgraph as a cluster only when its identifier
        // starts with `cluster`.
        write!(formatter, "cluster_c{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DotSummaryNodeId(usize);

impl std::fmt::Display for DotSummaryNodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "s{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DotModuleId(usize);

impl std::fmt::Display for DotModuleId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "m{}", self.0)
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
        for node in ir.nodes.iter().chain(&ir.external) {
            ids.register_node(&node.id);
        }
        for edge in &ir.edges {
            ids.register_node(&edge.from);
            ids.register_node(&edge.to);
        }
        for cluster in &ir.clusters {
            ids.clusters
                .insert(cluster.dag_id.clone(), DotClusterId(ids.clusters.len()));
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

    fn summary(&self, identity: &DagId) -> DotSummaryNodeId {
        DotSummaryNodeId(self.cluster(identity).0)
    }
}

struct ModuleRendererIds {
    modules: BTreeMap<DagId, DotModuleId>,
}

impl ModuleRendererIds {
    fn for_graph(ir: &GraphIr) -> Self {
        let modules = ir
            .clusters
            .iter()
            .enumerate()
            .map(|(index, cluster)| (cluster.dag_id.clone(), DotModuleId(index)))
            .collect();
        Self { modules }
    }

    fn module(&self, identity: &DagId) -> DotModuleId {
        self.modules[identity]
    }
}

struct RendererLabels {
    ambiguous_internal_names: BTreeSet<String>,
    ambiguous_external_names: BTreeSet<String>,
}

impl RendererLabels {
    fn for_graph(ir: &GraphIr) -> Self {
        let mut internal_counts = BTreeMap::<String, usize>::new();
        for node in &ir.nodes {
            internal_counts
                .entry(node.id.as_str().to_string())
                .and_modify(|count| *count = 2)
                .or_insert(1);
        }

        let mut external_counts = BTreeMap::<String, usize>::new();
        for node in &ir.external {
            external_counts
                .entry(node.id.to_string())
                .and_modify(|count| *count = 2)
                .or_insert(1);
        }

        Self {
            ambiguous_internal_names: internal_counts
                .into_iter()
                .filter_map(|(name, count)| (count > 1).then_some(name))
                .collect(),
            ambiguous_external_names: external_counts
                .into_iter()
                .filter_map(|(name, count)| (count > 1).then_some(name))
                .collect(),
        }
    }

    fn node_name(&self, node: &GraphNode, qualify_internal: bool) -> String {
        match node.kind {
            GraphNodeKind::External => {
                let display = node.id.to_string();
                if self.ambiguous_external_names.contains(&display) {
                    qualified_decl_label(&node.id)
                } else {
                    display
                }
            }
            GraphNodeKind::Const | GraphNodeKind::Param | GraphNodeKind::Node
                if qualify_internal && self.ambiguous_internal_names.contains(node.id.as_str()) =>
            {
                qualified_decl_label(&node.id)
            }
            GraphNodeKind::Const | GraphNodeKind::Param | GraphNodeKind::Node => {
                node.id.as_str().to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum GroupedEndpoint {
    Node(GraphNodeId),
    Collapsed(DagId),
}

impl GroupedEndpoint {
    const fn owner(&self) -> &DagId {
        match self {
            Self::Node(node) => node.owner(),
            Self::Collapsed(owner) => owner,
        }
    }

    fn dot_id(&self, ids: &RendererIds) -> DotGroupedEndpointId {
        match self {
            Self::Node(node) => DotGroupedEndpointId::Node(ids.node(node)),
            Self::Collapsed(owner) => DotGroupedEndpointId::Summary(ids.summary(owner)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DotGroupedEndpointId {
    Node(DotNodeId),
    Summary(DotSummaryNodeId),
}

impl std::fmt::Display for DotGroupedEndpointId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Node(node) => node.fmt(formatter),
            Self::Summary(summary) => summary.fmt(formatter),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupedEdge {
    from: GroupedEndpoint,
    to: GroupedEndpoint,
}

struct GroupedRenderPlan {
    collapsed: BTreeSet<DagId>,
    edges: BTreeSet<GroupedEdge>,
}

impl GroupedRenderPlan {
    fn for_graph(ir: &GraphIr, max_depth: Option<NonZeroUsize>) -> Self {
        let collapsed = max_depth.map_or_else(BTreeSet::new, |max_depth| {
            ir.clusters
                .iter()
                .filter(|cluster| {
                    cluster.parent.is_some() && composition_depth(ir, cluster) == max_depth.get()
                })
                .map(|cluster| cluster.dag_id.clone())
                .collect()
        });
        let edges = ir
            .edges
            .iter()
            .filter_map(|edge| {
                let from = grouped_endpoint(ir, &collapsed, &edge.from);
                let to = grouped_endpoint(ir, &collapsed, &edge.to);
                (from != to).then_some(GroupedEdge { from, to })
            })
            .collect();
        Self { collapsed, edges }
    }
}

struct GroupedRenderer<'a> {
    ir: &'a GraphIr,
    plan: &'a GroupedRenderPlan,
    node_lookup: &'a BTreeMap<GraphNodeId, &'a GraphNode>,
    ids: &'a RendererIds,
    labels: &'a RendererLabels,
}

/// Render the graph as Graphviz DOT text in the requested view.
#[must_use]
pub fn render(ir: &GraphIr, view: GraphView) -> String {
    match view {
        GraphView::Flat => render_flat(ir),
        GraphView::Grouped { max_depth } => render_grouped(ir, max_depth),
        GraphView::Module => render_modules(ir),
    }
}

fn render_flat(ir: &GraphIr) -> String {
    let ids = RendererIds::for_graph(ir);
    let labels = RendererLabels::for_graph(ir);
    let mut out = graph_header();

    render_nodes(
        &mut out,
        ir.nodes.iter().chain(&ir.external),
        1,
        &ids,
        &labels,
        true,
        false,
    );
    render_plain_edges(&mut out, ir, &ids);
    out.push_str("}\n");
    out
}

fn render_grouped(ir: &GraphIr, max_depth: Option<NonZeroUsize>) -> String {
    let ids = RendererIds::for_graph(ir);
    let labels = RendererLabels::for_graph(ir);
    let plan = GroupedRenderPlan::for_graph(ir, max_depth);
    let node_lookup: BTreeMap<GraphNodeId, &GraphNode> = ir
        .nodes
        .iter()
        .chain(&ir.external)
        .map(|node| (node.id.clone(), node))
        .collect();
    let renderer = GroupedRenderer {
        ir,
        plan: &plan,
        node_lookup: &node_lookup,
        ids: &ids,
        labels: &labels,
    };
    let mut out = graph_header();
    out.push_str("    compound=true;\n");
    out.push_str("    newrank=true;\n");
    out.push_str("    edge [fontname=\"Helvetica,Arial,sans-serif\"];\n");

    for cluster in ir
        .clusters
        .iter()
        .filter(|cluster| cluster.parent.is_none())
    {
        renderer.render_cluster(&mut out, cluster, 1);
    }
    render_grouped_legend(&mut out);

    for edge in &plan.edges {
        let attrs = grouped_edge_attrs(ir, &ids, edge.from.owner(), edge.to.owner());
        let from = edge.from.dot_id(&ids);
        let to = edge.to.dot_id(&ids);
        if attrs.is_empty() {
            let _ = writeln!(out, "    \"{from}\" -> \"{to}\";");
        } else {
            let _ = writeln!(out, "    \"{from}\" -> \"{to}\" [{}];", attrs.join(", "));
        }
    }
    out.push_str("}\n");
    out
}

fn render_modules(ir: &GraphIr) -> String {
    let ids = ModuleRendererIds::for_graph(ir);
    let mut out = graph_header();
    out.push_str("    edge [fontname=\"Helvetica,Arial,sans-serif\"];\n");

    for cluster in &ir.clusters {
        let attrs = module_attrs(cluster);
        let _ = writeln!(out, "    \"{}\" [{}];", ids.module(&cluster.dag_id), attrs);
    }

    // Composition edges make containment and repeated instantiation explicit,
    // independently of whether data happens to flow across the boundary.
    for cluster in ir
        .clusters
        .iter()
        .filter(|cluster| cluster.parent.is_some())
    {
        if let Some(parent) = &cluster.parent {
            let relation = match &cluster.kind {
                GraphClusterKind::Instance { .. } => "instantiates",
                GraphClusterKind::SourceModule | GraphClusterKind::ExternalModule => "contains",
            };
            let _ = writeln!(
                out,
                "    \"{}\" -> \"{}\" [label=\"{relation}\", style=dashed, color=\"#90A4AE\", fontcolor=\"#607D8B\", arrowhead=none, constraint=false];",
                ids.module(parent),
                ids.module(&cluster.dag_id)
            );
        }
    }

    let module_edges: BTreeSet<(DagId, DagId)> = ir
        .edges
        .iter()
        .filter_map(|edge| {
            let from = edge.from.owner();
            let to = edge.to.owner();
            (from != to).then(|| (from.clone(), to.clone()))
        })
        .collect();
    for (from, to) in module_edges {
        let _ = writeln!(
            out,
            "    \"{}\" -> \"{}\" [color=\"#37474F\", penwidth=1.5];",
            ids.module(&from),
            ids.module(&to)
        );
    }
    render_module_legend(&mut out);
    out.push_str("}\n");
    out
}

fn graph_header() -> String {
    concat!(
        "digraph graphcal {\n",
        "    rankdir=LR;\n",
        "    node [fontname=\"Helvetica,Arial,sans-serif\"];\n",
    )
    .to_string()
}

fn render_plain_edges(out: &mut String, ir: &GraphIr, ids: &RendererIds) {
    for edge in &ir.edges {
        let _ = writeln!(
            out,
            "    \"{}\" -> \"{}\";",
            ids.node(&edge.from),
            ids.node(&edge.to)
        );
    }
}

impl GroupedRenderer<'_> {
    fn render_cluster(&self, out: &mut String, cluster: &GraphCluster, indent: usize) {
        let pad = "    ".repeat(indent);
        let child_indent = indent.saturating_add(1);
        let inner_pad = "    ".repeat(child_indent);
        let _ = writeln!(
            out,
            "{pad}subgraph \"{}\" {{",
            self.ids.cluster(&cluster.dag_id)
        );
        let _ = writeln!(out, "{inner_pad}label=\"{}\";", cluster_label(cluster));
        let _ = writeln!(out, "{inner_pad}{}", cluster_style(cluster));

        if self.plan.collapsed.contains(&cluster.dag_id) {
            self.render_collapsed_summary(out, cluster, child_indent);
        } else {
            render_nodes(
                out,
                cluster
                    .node_ids
                    .iter()
                    .filter_map(|identity| self.node_lookup.get(identity).copied()),
                child_indent,
                self.ids,
                self.labels,
                false,
                true,
            );
            for child in self
                .ir
                .clusters
                .iter()
                .filter(|candidate| candidate.parent.as_ref() == Some(&cluster.dag_id))
            {
                self.render_cluster(out, child, child_indent);
            }
        }
        let _ = writeln!(out, "{pad}}}");
    }

    fn render_collapsed_summary(&self, out: &mut String, cluster: &GraphCluster, indent: usize) {
        let hidden = hidden_node_count(self.ir, &cluster.dag_id);
        let noun = if hidden == 1 { "value" } else { "values" };
        let pad = "    ".repeat(indent);
        let _ = writeln!(
            out,
            "{pad}\"{}\" [label=\"{hidden} {noun} hidden\", shape=box, style=\"rounded,dashed\", color=\"#607D8B\"];",
            self.ids.summary(&cluster.dag_id)
        );
    }
}

fn hidden_node_count(ir: &GraphIr, owner: &DagId) -> usize {
    ir.clusters
        .iter()
        .filter(|cluster| {
            &cluster.dag_id == owner || is_cluster_descendant(ir, &cluster.dag_id, owner)
        })
        .flat_map(|cluster| &cluster.node_ids)
        .count()
}

fn render_nodes<'a>(
    out: &mut String,
    nodes: impl IntoIterator<Item = &'a GraphNode>,
    indent: usize,
    ids: &RendererIds,
    labels: &RendererLabels,
    qualify_internal: bool,
    emphasize_output: bool,
) {
    let pad = "    ".repeat(indent);
    for node in nodes {
        let attrs = node_attrs(node, labels, qualify_internal, emphasize_output);
        let _ = writeln!(out, "{pad}\"{}\" [{attrs}];", ids.node(&node.id));
    }
}

fn node_attrs(
    node: &GraphNode,
    labels: &RendererLabels,
    qualify_internal: bool,
    emphasize_output: bool,
) -> String {
    let name = escape(&labels.node_name(node, qualify_internal));
    let label = node
        .type_label
        .as_ref()
        .map_or_else(|| name.clone(), |ty| format!("{name}\\n{}", escape(ty)));
    let mut attrs = match node.kind {
        GraphNodeKind::Const => format!("label=\"{label}\", shape=box, style=rounded"),
        GraphNodeKind::Param => format!("label=\"{label}\", shape=ellipse"),
        GraphNodeKind::Node => format!("label=\"{label}\", shape=box"),
        GraphNodeKind::External => {
            format!("label=\"{label}\", shape=box, style=dashed")
        }
    };
    if emphasize_output && node.is_public_output {
        attrs.push_str(", peripheries=2, color=\"#2E7D32\", penwidth=2");
    }
    attrs
}

fn cluster_label(cluster: &GraphCluster) -> String {
    escape(&cluster_label_text(cluster))
}

const fn cluster_style(cluster: &GraphCluster) -> &'static str {
    match &cluster.kind {
        GraphClusterKind::SourceModule => "color=\"#607D8B\"; style=\"rounded\"; penwidth=1.5;",
        GraphClusterKind::Instance { .. } => {
            "color=\"#1976D2\"; fillcolor=\"#E3F2FD\"; style=\"rounded,filled\"; penwidth=1.5;"
        }
        GraphClusterKind::ExternalModule => "color=\"#9E9E9E\"; style=\"rounded,dashed\";",
    }
}

fn composition_depth(ir: &GraphIr, cluster: &GraphCluster) -> usize {
    std::iter::successors(cluster.parent.as_ref(), |owner| {
        ir.clusters
            .iter()
            .find(|candidate| &candidate.dag_id == *owner)
            .and_then(|parent| parent.parent.as_ref())
    })
    .count()
}

fn grouped_endpoint(
    ir: &GraphIr,
    collapsed: &BTreeSet<DagId>,
    node: &GraphNodeId,
) -> GroupedEndpoint {
    let owner_cluster = ir
        .clusters
        .iter()
        .find(|cluster| &cluster.dag_id == node.owner());
    let collapsed_owner = std::iter::successors(owner_cluster, |cluster| {
        cluster.parent.as_ref().and_then(|parent| {
            ir.clusters
                .iter()
                .find(|candidate| &candidate.dag_id == parent)
        })
    })
    .find(|cluster| collapsed.contains(&cluster.dag_id));
    collapsed_owner.map_or_else(
        || GroupedEndpoint::Node(node.clone()),
        |cluster| GroupedEndpoint::Collapsed(cluster.dag_id.clone()),
    )
}

fn grouped_edge_attrs(ir: &GraphIr, ids: &RendererIds, from: &DagId, to: &DagId) -> Vec<String> {
    if from == to {
        return Vec::new();
    }
    let from_cluster = ir.clusters.iter().any(|cluster| &cluster.dag_id == from);
    let to_cluster = ir.clusters.iter().any(|cluster| &cluster.dag_id == to);
    if !from_cluster || !to_cluster {
        return Vec::new();
    }

    if is_cluster_descendant(ir, from, to) {
        vec![format!("ltail=\"{}\"", ids.cluster(from))]
    } else if is_cluster_descendant(ir, to, from) {
        vec![format!("lhead=\"{}\"", ids.cluster(to))]
    } else {
        vec![
            format!("ltail=\"{}\"", ids.cluster(from)),
            format!("lhead=\"{}\"", ids.cluster(to)),
        ]
    }
}

fn is_cluster_descendant(ir: &GraphIr, child: &DagId, ancestor: &DagId) -> bool {
    let mut current = ir
        .clusters
        .iter()
        .find(|cluster| &cluster.dag_id == child)
        .and_then(|cluster| cluster.parent.as_ref());
    while let Some(owner) = current {
        if owner == ancestor {
            return true;
        }
        current = ir
            .clusters
            .iter()
            .find(|cluster| &cluster.dag_id == owner)
            .and_then(|cluster| cluster.parent.as_ref());
    }
    false
}

fn module_attrs(cluster: &GraphCluster) -> String {
    let label = escape(&cluster_label_text(cluster));
    match &cluster.kind {
        GraphClusterKind::SourceModule if cluster.parent.is_none() => format!(
            "label=\"{label}\", shape=component, style=\"rounded,filled\", fillcolor=\"#ECEFF1\", color=\"#455A64\", penwidth=2"
        ),
        GraphClusterKind::SourceModule => format!(
            "label=\"{label}\", shape=component, style=\"rounded,filled\", fillcolor=\"#ECEFF1\", color=\"#607D8B\""
        ),
        GraphClusterKind::Instance { .. } => format!(
            "label=\"{label}\", shape=folder, style=filled, fillcolor=\"#E3F2FD\", color=\"#1976D2\", penwidth=1.5"
        ),
        GraphClusterKind::ExternalModule => {
            format!("label=\"{label}\", shape=component, style=dashed, color=\"#9E9E9E\"")
        }
    }
}

fn cluster_label_text(cluster: &GraphCluster) -> String {
    match &cluster.kind {
        GraphClusterKind::SourceModule if cluster.parent.is_none() => {
            format!("module {}", qualified_dag_label(&cluster.dag_id))
        }
        GraphClusterKind::SourceModule => {
            format!("dag {}", qualified_dag_label(&cluster.dag_id))
        }
        GraphClusterKind::Instance { template } => format!(
            "include {}\ntemplate {}",
            cluster.dag_id.name(),
            qualified_dag_label(template)
        ),
        GraphClusterKind::ExternalModule => {
            format!("external {}", qualified_dag_label(&cluster.dag_id))
        }
    }
}

fn render_grouped_legend(out: &mut String) {
    out.push_str(
        concat!(
            "    subgraph \"cluster_legend\" {\n",
            "        label=\"Legend\";\n",
            "        color=\"#CFD8DC\"; style=rounded;\n",
            "        \"legend_param\" [label=\"parameter\", shape=ellipse];\n",
            "        \"legend_const\" [label=\"const node\", shape=box, style=rounded];\n",
            "        \"legend_node\" [label=\"calculated node\", shape=box];\n",
            "        \"legend_output\" [label=\"public output\", shape=box, peripheries=2, color=\"#2E7D32\", penwidth=2];\n",
            "        \"legend_external\" [label=\"external value\", shape=box, style=dashed];\n",
            "        \"legend_collapsed\" [label=\"collapsed DAG\\nvalues hidden\", shape=box, style=\"rounded,dashed\"];\n",
            "    }\n",
        )
    );
}

fn render_module_legend(out: &mut String) {
    out.push_str(
        concat!(
            "    subgraph \"cluster_legend\" {\n",
            "        label=\"Legend\";\n",
            "        color=\"#CFD8DC\"; style=rounded;\n",
            "        \"legend_source\" [label=\"source module / dag\", shape=component, style=filled, fillcolor=\"#ECEFF1\"];\n",
            "        \"legend_instance\" [label=\"include instance\", shape=folder, style=filled, fillcolor=\"#E3F2FD\"];\n",
            "        \"legend_external_module\" [label=\"external module\", shape=component, style=dashed];\n",
            "        \"legend_flow_from\" [label=\"\", shape=point];\n",
            "        \"legend_flow_to\" [label=\"dataflow\", shape=plaintext];\n",
            "        \"legend_flow_from\" -> \"legend_flow_to\" [color=\"#37474F\", penwidth=1.5];\n",
            "    }\n",
        )
    );
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

    fn sample_ir() -> GraphIr {
        let root_id = DagId::root_in_package("test", "main");
        let child_id = root_id.child("child");
        let external_id = DagId::root_in_package("test", "external");
        GraphIr {
            nodes: vec![
                GraphNode {
                    id: id(&root_id, "input"),
                    kind: GraphNodeKind::Param,
                    is_public_output: false,
                    type_label: Some("Real".into()),
                },
                GraphNode {
                    id: id(&child_id, "output"),
                    kind: GraphNodeKind::Node,
                    is_public_output: true,
                    type_label: Some("Real".into()),
                },
            ],
            clusters: vec![
                GraphCluster {
                    dag_id: root_id.clone(),
                    parent: None,
                    kind: GraphClusterKind::SourceModule,
                    node_ids: vec![id(&root_id, "input")],
                },
                GraphCluster {
                    dag_id: child_id.clone(),
                    parent: Some(root_id),
                    kind: GraphClusterKind::SourceModule,
                    node_ids: vec![id(&child_id, "output")],
                },
                GraphCluster {
                    dag_id: external_id.clone(),
                    parent: None,
                    kind: GraphClusterKind::ExternalModule,
                    node_ids: vec![id(&external_id, "source")],
                },
            ],
            external: vec![GraphNode {
                id: id(&external_id, "source"),
                kind: GraphNodeKind::External,
                is_public_output: false,
                type_label: None,
            }],
            edges: vec![GraphEdge {
                from: id(&external_id, "source"),
                to: id(&child_id, "output"),
            }],
        }
    }

    #[test]
    fn renders_stable_flat_dot() {
        let dot = render(&sample_ir(), GraphView::Flat);
        assert_eq!(
            dot,
            concat!(
                "digraph graphcal {\n",
                "    rankdir=LR;\n",
                "    node [fontname=\"Helvetica,Arial,sans-serif\"];\n",
                "    \"n0\" [label=\"input\\nReal\", shape=ellipse];\n",
                "    \"n1\" [label=\"output\\nReal\", shape=box];\n",
                "    \"n2\" [label=\"external.source\", shape=box, style=dashed];\n",
                "    \"n2\" -> \"n1\";\n",
                "}\n",
            )
        );
    }

    #[test]
    fn grouped_view_emits_real_nested_clusters_and_routed_edges() {
        let dot = render(&sample_ir(), GraphView::Grouped { max_depth: None });
        assert!(dot.contains("compound=true;"));
        assert!(dot.contains("subgraph \"cluster_c0\" {"));
        assert!(dot.contains("subgraph \"cluster_c1\" {"));
        assert!(dot.contains("label=\"dag test::main.child\";"));
        assert!(dot.contains("peripheries=2"));
        assert!(dot.contains("ltail=\"cluster_c2\", lhead=\"cluster_c1\""));
    }

    #[test]
    fn grouped_depth_collapses_frontier_and_redirects_edges() {
        let dot = render(
            &sample_ir(),
            GraphView::Grouped {
                max_depth: NonZeroUsize::new(1),
            },
        );

        assert!(dot.contains("\"s1\" [label=\"1 value hidden\""));
        assert!(!dot.contains("output\\nReal"));
        assert!(dot.contains("\"n2\" -> \"s1\" [ltail=\"cluster_c2\", lhead=\"cluster_c1\"]"));
    }

    #[test]
    fn module_view_collapses_declarations_to_dag_edges() {
        let dot = render(&sample_ir(), GraphView::Module);
        assert!(dot.contains("\"m0\" [label=\"module test::main\""));
        assert!(dot.contains("\"m1\" [label=\"dag test::main.child\""));
        assert!(dot.contains("\"m2\" [label=\"external test::external\""));
        assert!(dot.contains("\"m2\" -> \"m1\" [color=\"#37474F\""));
        assert!(!dot.contains("input\\nReal"));
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
                is_public_output: false,
                type_label: None,
            })
            .collect::<Vec<_>>();
        let root_id = DagId::root_in_package("test", "main");
        let ir = GraphIr {
            nodes: Vec::new(),
            clusters: std::iter::once(GraphCluster {
                dag_id: root_id,
                parent: None,
                kind: GraphClusterKind::SourceModule,
                node_ids: Vec::new(),
            })
            .chain(external.iter().map(|node| GraphCluster {
                dag_id: node.id.owner().clone(),
                parent: None,
                kind: GraphClusterKind::ExternalModule,
                node_ids: vec![node.id.clone()],
            }))
            .collect(),
            external,
            edges: Vec::new(),
        };

        let dot = render(&ir, GraphView::Flat);
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
