//! Graph IR — a one-way projection of a compiled [`CheckedTir`] into a node-link
//! dependency-graph model for visualization exports (#512).
//!
//! The IR is a *projection*: TIR → IR → renderer. It is never parsed back
//! into source text; source text stays canonical for editing. The model is
//! the boundary between the compiler's typed world and serialized export
//! formats, so node identities stay typed ([`GraphNodeId`]) while
//! display-only fields (the resolved-type label) are pre-rendered here —
//! renderers like [`dot`] are pure functions from this model to a string
//! and never reach back into the TIR.
//!
//! **Experimental:** this module backs the experimental `graphcal graph`
//! subcommand. The model and renderer output may change in any release
//! while the visualizer design (#512) evolves.

pub mod dot;

use std::collections::{BTreeMap, BTreeSet, HashSet};

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::tir::typed::{CheckedDag, CheckedTir, DiagnosticDeclProbe};
use thiserror::Error;

/// One inline DAG call (`@target(…)::output`) made by a projected
/// declaration: the `ordinal`-th call of the caller's definition, counting
/// from zero in pre-order.
///
/// A call has no instance identity in the compiled program (it is evaluated
/// in place), so the graph names its instance by where it is made.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GraphCallId {
    caller: ResolvedDeclName,
    ordinal: usize,
}

impl GraphCallId {
    /// The declaration whose definition makes the call.
    #[must_use]
    pub const fn caller(&self) -> &ResolvedDeclName {
        &self.caller
    }

    /// The call's position among the caller's calls, in pre-order.
    #[must_use]
    pub const fn ordinal(&self) -> usize {
        self.ordinal
    }
}

impl std::fmt::Display for GraphCallId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "call #{} of `{}`", self.ordinal, self.caller)
    }
}

/// Stable identity of a graph node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphNodeId {
    /// A declaration, by its canonical resolved name.
    Declaration(ResolvedDeclName),
    /// The declaration `output` of the DAG an inline call instantiates, as
    /// that call's instance computes it.
    CallOutput {
        call: GraphCallId,
        output: ResolvedDeclName,
    },
}

impl GraphNodeId {
    /// The cluster that owns this node: a declaration's owning DAG, or the
    /// instance of the call computing an output.
    #[must_use]
    pub fn owner(&self) -> GraphClusterId {
        match self {
            Self::Declaration(declaration) => GraphClusterId::Dag(declaration.owner().clone()),
            Self::CallOutput { call, .. } => GraphClusterId::Call(call.clone()),
        }
    }

    /// The declaration's local name; for a call output, the name of the
    /// output in the called DAG.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Declaration(declaration)
            | Self::CallOutput {
                output: declaration,
                ..
            } => declaration.as_str(),
        }
    }

    /// The declaration this node projects, unless it is a call output.
    #[must_use]
    pub const fn declaration(&self) -> Option<&ResolvedDeclName> {
        match self {
            Self::Declaration(declaration) => Some(declaration),
            Self::CallOutput { .. } => None,
        }
    }
}

impl std::fmt::Display for GraphNodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declaration(declaration) => declaration.fmt(formatter),
            Self::CallOutput { call, output } => write!(formatter, "`{output}` of {call}"),
        }
    }
}

/// Stable identity of a graph cluster.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphClusterId {
    /// A source module, include instance, or external module.
    Dag(DagId),
    /// The instance of an inline DAG call.
    Call(GraphCallId),
}

impl GraphClusterId {
    /// The DAG this cluster projects, unless it is a call instance.
    #[must_use]
    pub const fn dag(&self) -> Option<&DagId> {
        match self {
            Self::Dag(dag) => Some(dag),
            Self::Call(_) => None,
        }
    }
}

impl std::fmt::Display for GraphClusterId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dag(dag) => dag.fmt(formatter),
            Self::Call(call) => call.fmt(formatter),
        }
    }
}

/// The declaration kind behind a graph node. Drives renderer styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphNodeKind {
    /// A `const node` declaration.
    Const,
    /// A `param` declaration.
    Param,
    /// A `node` declaration.
    Node,
    /// A dependency declared outside the projected DAGs (e.g. a value
    /// imported from another file). Synthesized from edge endpoints so the
    /// graph stays closed; carries no type label.
    External,
}

/// A vertex in the dependency graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    id: GraphNodeId,
    kind: GraphNodeKind,
    /// Whether this calculated node is a public output of its source template.
    /// Parameters remain visually classified as inputs even though callers can
    /// also project their effective values.
    is_public_output: bool,
    /// The source definition is an explicit TODO rather than a formula.
    is_unfinished: bool,
    /// Human-readable resolved type (e.g. `"Length / Time^2"`), pre-rendered
    /// because renderers have no access to the registry. `None` when the
    /// declaration's resolved type is unknown (external nodes).
    type_label: Option<String>,
}

/// A directed dataflow edge: `to` reads `from`, via `@` or through an
/// inline call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GraphEdge {
    from: GraphNodeId,
    to: GraphNodeId,
}

/// Semantic provenance of a visualization cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphClusterKind {
    /// A file or lexically nested inline `dag` definition.
    SourceModule,
    /// A concrete include instance of a reusable template.
    Instance { template: DagId },
    /// The instance of an inline call of `template`. It holds only the output
    /// the call reads: a call's body is not expanded.
    Call { template: DagId },
    /// A source module referenced through `import` but not expanded into the
    /// entry DAG.
    ExternalModule,
}

/// One source module, include instance, or call instance projected as a
/// group.
#[derive(Debug, Clone)]
pub struct GraphCluster {
    id: GraphClusterId,
    parent: Option<DagId>,
    kind: GraphClusterKind,
    /// Canonical identities of nodes directly owned by this cluster. Global
    /// node order stays in [`GraphIr::nodes`] so flat rendering is unchanged by
    /// grouping.
    node_ids: Vec<GraphNodeId>,
}

/// The projected dependency graph of one compiled file.
#[derive(Debug, Clone)]
pub struct GraphIr {
    /// Projected declarations in deterministic source/module order, then the
    /// outputs inline calls read, ordered by call.
    nodes: Vec<GraphNode>,
    /// The root source module, nested source modules, include and call
    /// instances, and referenced external modules. The root is first; the
    /// remainder are sorted by canonical identity.
    clusters: Vec<GraphCluster>,
    /// Placeholder nodes for dependencies declared outside the expanded DAGs,
    /// sorted by id.
    external: Vec<GraphNode>,
    /// Dataflow edges, deduplicated and sorted.
    edges: Vec<GraphEdge>,
}

impl GraphNode {
    /// The declaration or call output this node projects.
    #[must_use]
    pub const fn id(&self) -> &GraphNodeId {
        &self.id
    }

    /// The declaration kind behind this node.
    #[must_use]
    pub const fn kind(&self) -> GraphNodeKind {
        self.kind
    }

    /// Whether this calculated node is a public output of its source template.
    #[must_use]
    pub const fn is_public_output(&self) -> bool {
        self.is_public_output
    }

    /// Whether the source definition is an explicit TODO rather than a formula.
    #[must_use]
    pub const fn is_unfinished(&self) -> bool {
        self.is_unfinished
    }

    /// Human-readable resolved type, when known.
    #[must_use]
    pub fn type_label(&self) -> Option<&str> {
        self.type_label.as_deref()
    }
}

impl GraphEdge {
    /// The node read.
    #[must_use]
    pub const fn from(&self) -> &GraphNodeId {
        &self.from
    }

    /// The declaration doing the reading.
    #[must_use]
    pub const fn to(&self) -> &GraphNodeId {
        &self.to
    }
}

impl GraphCluster {
    /// The DAG or call instance this cluster projects.
    #[must_use]
    pub const fn id(&self) -> &GraphClusterId {
        &self.id
    }

    /// The enclosing cluster, if any.
    #[must_use]
    pub const fn parent(&self) -> Option<&DagId> {
        self.parent.as_ref()
    }

    /// Semantic provenance of this cluster.
    #[must_use]
    pub const fn kind(&self) -> &GraphClusterKind {
        &self.kind
    }

    /// Canonical identities of the nodes this cluster directly owns.
    #[must_use]
    pub fn node_ids(&self) -> &[GraphNodeId] {
        &self.node_ids
    }
}

impl GraphIr {
    /// Projected declarations in deterministic source/module order, then the
    /// outputs inline calls read.
    #[must_use]
    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    /// The root source module first, then the remaining clusters.
    #[must_use]
    pub fn clusters(&self) -> &[GraphCluster] {
        &self.clusters
    }

    /// Placeholder nodes for dependencies declared outside the expanded DAGs.
    #[must_use]
    pub fn external(&self) -> &[GraphNode] {
        &self.external
    }

    /// Dataflow edges, deduplicated and sorted.
    #[must_use]
    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }
}

struct ProjectedClusterProvenance {
    clusters: BTreeMap<GraphClusterId, GraphCluster>,
    output_names: BTreeMap<DagId, HashSet<DeclName>>,
}

/// The instance of one inline call: its cluster and the output it reads.
struct ProjectedCall {
    cluster: GraphCluster,
    output: GraphNode,
}

/// A checked-program invariant prevented graph provenance from being projected.
#[derive(Debug, Error)]
pub enum GraphProjectionError {
    /// A source-facing declaration lacked its authoritative canonical identity.
    #[error(transparent)]
    UnboundDeclaration(#[from] DiagnosticDeclProbe),
    /// An instance record referred to a template absent from the checked DAG registry.
    #[error("instance `{instance}` refers to missing template `{template}`")]
    MissingTemplate { instance: DagId, template: DagId },
    /// An inline call read a declaration that is not a checked value
    /// declaration of the program.
    #[error("{call} reads `{output}`, which is not a checked value declaration")]
    MissingCallOutput {
        call: Box<GraphCallId>,
        output: ResolvedDeclName,
    },
    /// Two provenance records claimed the same concrete owner.
    #[error("multiple graph clusters claim canonical owner `{owner}`")]
    DuplicateClusterOwner { owner: DagId },
    /// A projected node had no matching source-module, instance, or call record.
    #[error("projected node `{node}` has untracked owner `{owner}`")]
    UntrackedNodeOwner {
        node: Box<GraphNodeId>,
        owner: Box<GraphClusterId>,
    },
    /// A nested cluster's explicit parent was absent or not an ancestor.
    #[error("graph cluster `{cluster}` has invalid parent `{parent}`")]
    InvalidClusterParent {
        cluster: Box<GraphClusterId>,
        parent: DagId,
    },
    /// The root source cluster disappeared during projection.
    #[error("graph projection is missing root source cluster `{root}`")]
    MissingRootCluster { root: DagId },
}

/// Project a compiled [`CheckedTir`] into its dependency [`GraphIr`].
///
/// Vertices are the const/param/node declarations of the file's root DAG and
/// every inline `dag` block nested inside it; asserts, plots, figures, and
/// layers are not dataflow vertices. Edges come from the TIR's canonical
/// dependency maps (`const_deps` + `runtime_deps`), oriented in dataflow
/// direction (dependency → dependent). Dependencies on declarations outside
/// the projected DAGs surface as [`GraphNodeKind::External`] placeholders.
///
/// Each inline DAG call a vertex makes is a call-instance cluster inside the
/// caller's cluster, holding the output the call reads, with an edge from
/// that output to the caller. The reads of the call's arguments stay edges
/// into the caller.
///
/// Source-module, concrete-instance, and external-module ownership is retained
/// independently of rendering. A renderer can therefore choose a flat,
/// clustered, or module-level view without reconstructing provenance from
/// display strings.
///
/// # Errors
///
/// Returns [`GraphProjectionError`] if the checked TIR is missing an
/// authoritative declaration identity or consistent module provenance.
pub fn project_tir(tir: &CheckedTir) -> Result<GraphIr, GraphProjectionError> {
    let mut child_dags: Vec<&CheckedDag> = tir
        .local_dags()
        .filter_map(|(dag_id, dag)| (dag_id != tir.root_dag_id()).then_some(dag))
        .collect();
    child_dags.sort_by(|a, b| a.dag_id().cmp(b.dag_id()));
    let local_dags: Vec<&CheckedDag> = std::iter::once(tir.root())
        .chain(child_dags.iter().copied())
        .collect();

    let ProjectedClusterProvenance {
        mut clusters,
        output_names,
    } = project_cluster_provenance(tir, &local_dags)?;

    let mut nodes = local_dags
        .iter()
        .flat_map(|dag| project_dag_nodes(tir, dag, &output_names))
        .collect::<Vec<_>>();

    for node in &nodes {
        let owner = node.id.owner();
        let Some(cluster) = clusters.get_mut(&owner) else {
            return Err(GraphProjectionError::UntrackedNodeOwner {
                node: Box::new(node.id.clone()),
                owner: Box::new(owner),
            });
        };
        cluster.node_ids.push(node.id.clone());
    }

    let mut edges = BTreeSet::<GraphEdge>::new();
    for dag in &local_dags {
        let deps = dag.dependencies();
        for (dependent, dep_set) in deps.const_deps.iter().chain(deps.runtime_deps.iter()) {
            for dep in dep_set {
                edges.insert(GraphEdge {
                    from: GraphNodeId::Declaration(dep.clone()),
                    to: GraphNodeId::Declaration(dependent.clone()),
                });
            }
        }
    }

    let declared: BTreeSet<&ResolvedDeclName> = nodes
        .iter()
        .filter_map(|node| node.id.declaration())
        .collect();
    let calls = project_calls(tir, &local_dags, &declared)?;
    for (call, ProjectedCall { cluster, output }) in calls {
        edges.insert(GraphEdge {
            from: output.id.clone(),
            to: GraphNodeId::Declaration(call.caller.clone()),
        });
        clusters.insert(GraphClusterId::Call(call), cluster);
        nodes.push(output);
    }

    // Close the graph: any edge endpoint that is not a projected node becomes
    // an external placeholder node. External declarations are grouped by their
    // typed owner, but their source bodies remain intentionally unexpanded.
    let projected: BTreeSet<&GraphNodeId> = nodes.iter().map(|node| &node.id).collect();
    let external: Vec<GraphNode> = edges
        .iter()
        .flat_map(|edge| [&edge.from, &edge.to])
        .filter(|id| !projected.contains(id))
        .map(|id| {
            (
                id,
                GraphNode {
                    id: id.clone(),
                    kind: GraphNodeKind::External,
                    is_public_output: false,
                    is_unfinished: false,
                    type_label: None,
                },
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();

    for node in &external {
        let owner = node.id.owner();
        clusters
            .entry(owner.clone())
            .or_insert_with(|| GraphCluster {
                id: owner,
                parent: None,
                kind: GraphClusterKind::ExternalModule,
                node_ids: Vec::new(),
            })
            .node_ids
            .push(node.id.clone());
    }

    check_cluster_parents(&clusters)?;

    let root_id = tir.root_dag_id();
    let root = clusters
        .remove(&GraphClusterId::Dag(root_id.clone()))
        .ok_or_else(|| GraphProjectionError::MissingRootCluster {
            root: root_id.clone(),
        })?;
    let clusters = std::iter::once(root)
        .chain(clusters.into_values())
        .collect();

    Ok(GraphIr {
        nodes,
        clusters,
        external,
        edges: edges.into_iter().collect(),
    })
}

/// Every nested cluster's parent is a projected DAG cluster enclosing it: a
/// DAG's ancestor, or the owner of a call's caller.
fn check_cluster_parents(
    clusters: &BTreeMap<GraphClusterId, GraphCluster>,
) -> Result<(), GraphProjectionError> {
    for cluster in clusters.values() {
        let Some(parent) = &cluster.parent else {
            continue;
        };
        let nested = match &cluster.id {
            GraphClusterId::Dag(dag) => dag.is_descendant_of(parent),
            GraphClusterId::Call(call) => call.caller.owner() == parent,
        };
        if !nested || !clusters.contains_key(&GraphClusterId::Dag(parent.clone())) {
            return Err(GraphProjectionError::InvalidClusterParent {
                cluster: Box::new(cluster.id.clone()),
                parent: parent.clone(),
            });
        }
    }
    Ok(())
}

fn project_cluster_provenance(
    tir: &CheckedTir,
    local_dags: &[&CheckedDag],
) -> Result<ProjectedClusterProvenance, GraphProjectionError> {
    let mut clusters = BTreeMap::<GraphClusterId, GraphCluster>::new();
    let mut output_names = BTreeMap::<DagId, HashSet<DeclName>>::new();

    for dag in local_dags {
        if dag.is_semantic_instance() {
            continue;
        }
        let dag_id = dag.dag_id().clone();
        let parent = (dag.dag_id() != tir.root_dag_id())
            .then(|| dag.dag_id().parent())
            .flatten();
        let cluster = GraphCluster {
            id: GraphClusterId::Dag(dag_id.clone()),
            parent,
            kind: GraphClusterKind::SourceModule,
            node_ids: Vec::new(),
        };
        if clusters
            .insert(GraphClusterId::Dag(dag_id.clone()), cluster)
            .is_some()
        {
            return Err(GraphProjectionError::DuplicateClusterOwner { owner: dag_id });
        }
        output_names.insert(dag_id, dag.projectable_outputs().clone());
    }

    for record in local_dags
        .iter()
        .flat_map(|dag| dag.semantic_instances())
        .map(|record| &record.instance)
    {
        let owner = record.id().owner().clone();
        let template = record.id().template().clone();
        let template_dag = tir.dag_registry().get(&template).ok_or_else(|| {
            GraphProjectionError::MissingTemplate {
                instance: owner.clone(),
                template: template.clone(),
            }
        })?;
        let cluster = GraphCluster {
            id: GraphClusterId::Dag(owner.clone()),
            parent: Some(record.id().parent().clone()),
            kind: GraphClusterKind::Instance { template },
            node_ids: Vec::new(),
        };
        if clusters
            .insert(GraphClusterId::Dag(owner.clone()), cluster)
            .is_some()
        {
            return Err(GraphProjectionError::DuplicateClusterOwner { owner });
        }
        output_names.insert(owner, template_dag.projectable_outputs().clone());
    }

    Ok(ProjectedClusterProvenance {
        clusters,
        output_names,
    })
}

/// Project one physical DAG body's declarations in source order. Canonical
/// declaration owners may identify nested concrete instances merged into this
/// body; grouping happens after this projection.
fn project_dag_nodes(
    tir: &CheckedTir,
    dag: &CheckedDag,
    output_names: &BTreeMap<DagId, HashSet<DeclName>>,
) -> Vec<GraphNode> {
    dag.declarations()
        .filter_map(|entry| Some((entry, node_kind(entry.value()?.category))))
        .map(|(entry, kind)| {
            let id = entry.identity().clone();
            let is_public_output = kind == GraphNodeKind::Node
                && output_names
                    .get(id.owner())
                    .is_some_and(|names| names.contains(&id.to_unowned_def_name()));
            let type_label = dag
                .value_decl_type(&id)
                .map(|ty| ty.resolved().format(tir.registry()));
            GraphNode {
                is_unfinished: dag.todo(&id).is_some(),
                id: GraphNodeId::Declaration(id),
                kind,
                is_public_output,
                type_label,
            }
        })
        .collect()
}

/// Project the instance of every inline call a projected declaration makes,
/// keyed by call. A declaration that is not a vertex (an assertion) has no
/// call instances.
fn project_calls(
    tir: &CheckedTir,
    local_dags: &[&CheckedDag],
    declared: &BTreeSet<&ResolvedDeclName>,
) -> Result<BTreeMap<GraphCallId, ProjectedCall>, GraphProjectionError> {
    let mut calls = BTreeMap::new();
    for (caller, reads) in local_dags
        .iter()
        .flat_map(|dag| &dag.dependencies().call_reads)
        .filter(|(caller, _)| declared.contains(caller))
    {
        for (ordinal, read) in reads.iter().enumerate() {
            let call = GraphCallId {
                caller: caller.clone(),
                ordinal,
            };
            let output_dag = tir.dag_registry().get(read.output.owner());
            let Some((dag, value)) = output_dag.and_then(|dag| {
                let value = dag.declaration(&read.output)?.value()?;
                Some((dag, value))
            }) else {
                return Err(GraphProjectionError::MissingCallOutput {
                    call: Box::new(call),
                    output: read.output.clone(),
                });
            };
            let kind = node_kind(value.category);
            let id = GraphNodeId::CallOutput {
                call: call.clone(),
                output: read.output.clone(),
            };
            let output = GraphNode {
                id: id.clone(),
                kind,
                is_public_output: kind == GraphNodeKind::Node
                    && dag
                        .projectable_outputs()
                        .contains(&read.output.to_unowned_def_name()),
                is_unfinished: dag.todo(&read.output).is_some(),
                type_label: dag
                    .value_decl_type(&read.output)
                    .map(|ty| ty.resolved().format(tir.registry())),
            };
            let cluster = GraphCluster {
                id: GraphClusterId::Call(call.clone()),
                parent: Some(caller.owner().clone()),
                kind: GraphClusterKind::Call {
                    template: read.target.clone(),
                },
                node_ids: vec![id],
            };
            calls.insert(call, ProjectedCall { cluster, output });
        }
    }
    Ok(calls)
}

const fn node_kind(category: ValueDeclCategory) -> GraphNodeKind {
    match category {
        ValueDeclCategory::Const => GraphNodeKind::Const,
        ValueDeclCategory::Param => GraphNodeKind::Param,
        ValueDeclCategory::Node => GraphNodeKind::Node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::ProjectTypeStore;

    use std::sync::Arc;

    fn tir_from_source(source: &str) -> CheckedTir {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let file = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
        let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
        let src = sources.register("test.gcl", Arc::new(source.to_string()));
        let ir = lower(&file, "test.gcl", src).unwrap();
        let resolver =
            ModuleResolver::without_edges([(ir.dag_id().clone(), file.declarations.as_slice())])
                .unwrap();
        let mut project_types = ProjectTypeStore::default();
        project_types.insert_graphcal_prelude().unwrap();
        project_types.insert_module(ir.definitions()).unwrap();
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let signed =
            graphcal_compiler::tir::typed::resolve_hir_signature_with_modules_and_cancellation(
                ir,
                src,
                &resolver,
                &project_types,
                &cancellation,
            )
            .unwrap();
        graphcal_compiler::tir::typed::TirDraft::resolve_root(
            signed,
            &|_| None,
            src,
            &resolver,
            Arc::new(project_types),
            &cancellation,
        )
        .unwrap()
        .instantiate(
            &graphcal_compiler::tir::typed::CheckedOverrideDependencies::default(),
            src,
        )
        .unwrap()
        .check(src, &cancellation)
        .unwrap()
    }

    /// Compile through the full project pipeline (loader + inline-DAG body
    /// compilation), which is what the CLI does. Needed for inline `dag`
    /// blocks: a bare root check does not compile their bodies.
    fn tir_from_project_source(source: &str) -> CheckedTir {
        let mut fs = graphcal_io::InMemoryFileSystem::new();
        fs.add_file(
            graphcal_io::VirtualAbsolutePath::new("/proj/test.gcl").unwrap(),
            source.to_string(),
        )
        .unwrap();
        let (tir, _project) = crate::tests::compile_to_tir_project(
            std::path::Path::new("/proj/test.gcl"),
            Some(std::path::Path::new("/proj")),
            &fs,
        )
        .unwrap();
        tir
    }

    fn node_id(name: &str) -> GraphNodeId {
        GraphNodeId::Declaration(test_decl(name))
    }

    fn test_decl(name: &str) -> ResolvedDeclName {
        ResolvedDeclName::for_test(
            graphcal_compiler::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(
                "test.gcl",
            ))
            .unwrap(),
            graphcal_compiler::syntax::decl_name::DeclName::expect_valid(name),
        )
    }

    const ROCKET_SOURCE: &str = "\
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
const node g0: Acceleration = 9.80665 m/s^2;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = 320.0 s * @g0 * ln(@mass_ratio);
assert positive_dv = @delta_v > 0.0 m/s;
";

    #[test]
    fn projects_decls_in_source_order_with_kinds() {
        let tir = tir_from_source(ROCKET_SOURCE);
        let ir = project_tir(&tir).unwrap();

        let kinds: Vec<(&str, GraphNodeKind)> =
            ir.nodes.iter().map(|n| (n.id.as_str(), n.kind)).collect();
        // Asserts are not dataflow vertices.
        assert_eq!(
            kinds,
            vec![
                ("dry_mass", GraphNodeKind::Param),
                ("fuel_mass", GraphNodeKind::Param),
                ("g0", GraphNodeKind::Const),
                ("mass_ratio", GraphNodeKind::Node),
                ("delta_v", GraphNodeKind::Node),
            ]
        );
        assert_eq!(ir.clusters.len(), 1);
        assert_eq!(ir.clusters[0].kind, GraphClusterKind::SourceModule);
        assert!(ir.external.is_empty());
    }

    #[test]
    fn projects_dataflow_edges_including_const_reads() {
        let tir = tir_from_source(ROCKET_SOURCE);
        let ir = project_tir(&tir).unwrap();

        let expected = [
            ("dry_mass", "mass_ratio"),
            ("fuel_mass", "mass_ratio"),
            ("g0", "delta_v"),
            ("mass_ratio", "delta_v"),
        ];
        let edges: Vec<GraphEdge> = expected
            .iter()
            .map(|(from, to)| GraphEdge {
                from: node_id(from),
                to: node_id(to),
            })
            .collect();
        assert_eq!(ir.edges, edges);
    }

    #[test]
    fn projects_type_labels() {
        let tir = tir_from_source(ROCKET_SOURCE);
        let ir = project_tir(&tir).unwrap();

        let labels: BTreeMap<&str, Option<&str>> = ir
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.type_label.as_deref()))
            .collect();
        // Quantity types format with named dimension aliases preferred over
        // canonical dimension expressions (`Velocity`, not `Length / Time`).
        assert_eq!(labels["dry_mass"], Some("Mass"));
        assert_eq!(labels["mass_ratio"], Some("Dimensionless"));
        assert_eq!(labels["delta_v"], Some("Velocity"));
    }

    #[test]
    fn projects_inline_dag_blocks_as_child_clusters() {
        let tir = tir_from_project_source(
            "\
dag scale {
    param factor: Dimensionless;
    param v: Dimensionless;
    pub node result: Dimensionless = @v * @factor;
}

param speed: Dimensionless = 10.0;
node doubled: Dimensionless = @scale(factor: 2.0, v: @speed)::result;
",
        );
        let ir = project_tir(&tir).unwrap();

        let root = &ir.clusters[0];
        let root_names: Vec<&str> = root.node_ids.iter().map(GraphNodeId::as_str).collect();
        assert_eq!(root_names, vec!["speed", "doubled"]);

        // The root, the inline `dag`, and the instance of the call `doubled` makes.
        assert_eq!(ir.clusters.len(), 3);
        assert!(matches!(ir.clusters[2].kind, GraphClusterKind::Call { .. }));
        let child = &ir.clusters[1];
        assert_eq!(child.id.to_string(), "test.scale");
        assert_eq!(child.parent.as_ref(), root.id.dag());
        assert_eq!(child.kind, GraphClusterKind::SourceModule);
        let child_names: Vec<&str> = child.node_ids.iter().map(GraphNodeId::as_str).collect();
        assert_eq!(child_names, vec!["factor", "v", "result"]);

        // The child cluster's internal dataflow is part of the graph.
        assert!(ir.edges.iter().any(|e| {
            e.from.owner().to_string() == "test.scale"
                && e.from.as_str() == "v"
                && e.to.as_str() == "result"
        }));
    }

    #[test]
    fn preserves_repeated_include_instance_provenance() {
        let tir = tir_from_project_source(
            "\
dag scale {
    param factor: Dimensionless;
    param v: Velocity;
    pub node result: Velocity = @v * @factor;
}

param speed: Velocity = 10.0 m/s;
include scale(factor: 2.0, v: @speed) as doubled;
include scale(factor: 3.0, v: @speed) as tripled;
node doubled_result: Velocity = @doubled::result;
node tripled_result: Velocity = @tripled::result;
",
        );
        let ir = project_tir(&tir).unwrap();
        let instance_clusters: Vec<&GraphCluster> = ir
            .clusters
            .iter()
            .filter(|cluster| matches!(&cluster.kind, GraphClusterKind::Instance { .. }))
            .collect();

        assert_eq!(instance_clusters.len(), 2);
        assert_eq!(
            instance_clusters[0].id.dag().unwrap().leaf().to_string(),
            "doubled"
        );
        assert_eq!(
            instance_clusters[1].id.dag().unwrap().leaf().to_string(),
            "tripled"
        );
        assert!(instance_clusters.iter().all(|cluster| {
            matches!(
                &cluster.kind,
                GraphClusterKind::Instance { template }
                    if template.to_string() == "test.scale"
            )
        }));
        assert!(
            instance_clusters
                .iter()
                .all(|cluster| cluster.parent.as_ref() == Some(tir.root_dag_id()))
        );
        assert!(
            instance_clusters
                .iter()
                .all(|cluster| cluster.node_ids.iter().any(|id| id.as_str() == "result"))
        );
    }

    const CALL_SOURCE: &str = "\
dag checked {
    param v: Dimensionless;
    pub node out: Dimensionless = @v * 2.0;
}
param start: Dimensionless = 1.0;
node out: Dimensionless = @checked(v: @start)::out + @checked(v: 3.0)::v;
include checked(v: @checked(v: 5.0)::out) as also_fine;
node z: Dimensionless = @out + @also_fine::out;
assert positive = @checked(v: 4.0)::out > 0.0;
";

    fn call_clusters(ir: &GraphIr) -> Vec<(&GraphCluster, &GraphCallId)> {
        ir.clusters
            .iter()
            .filter_map(|cluster| match &cluster.id {
                GraphClusterId::Call(call) => Some((cluster, call)),
                GraphClusterId::Dag(_) => None,
            })
            .collect()
    }

    #[test]
    fn projects_each_inline_call_as_an_instance_read_by_its_caller() {
        let tir = tir_from_project_source(CALL_SOURCE);
        let ir = project_tir(&tir).unwrap();
        let calls = call_clusters(&ir);

        // Two calls by `out`, one by the include's bound `v`; the assertion's
        // call has no vertex to be read by.
        assert_eq!(calls.len(), 3);
        let out = test_decl("out");
        let out_calls: Vec<_> = calls
            .iter()
            .filter(|(_, call)| call.caller() == &out)
            .collect();
        assert_eq!(
            out_calls
                .iter()
                .map(|(_, call)| call.ordinal())
                .collect::<Vec<_>>(),
            [0, 1]
        );
        for (cluster, _) in out_calls {
            assert_eq!(cluster.parent.as_ref(), Some(tir.root_dag_id()));
            assert!(matches!(
                &cluster.kind,
                GraphClusterKind::Call { template } if template.to_string() == "test.checked"
            ));
            let [output] = cluster.node_ids.as_slice() else {
                panic!("a call instance holds the one output it reads");
            };
            assert!(ir.edges.contains(&GraphEdge {
                from: output.clone(),
                to: node_id("out"),
            }));
        }
        let outputs: Vec<(&str, GraphNodeKind, bool)> = ir
            .nodes
            .iter()
            .filter(|node| node.id.declaration().is_none())
            .map(|node| (node.id.as_str(), node.kind, node.is_public_output))
            .collect();
        assert_eq!(
            outputs,
            [
                ("out", GraphNodeKind::Node, true),
                ("v", GraphNodeKind::Param, false),
                ("out", GraphNodeKind::Node, true),
            ]
        );
        // The reads of a call's arguments still go to the caller.
        assert!(ir.edges.contains(&GraphEdge {
            from: node_id("start"),
            to: node_id("out"),
        }));
        assert!(ir.external.is_empty());
    }

    #[test]
    fn an_include_binding_call_is_read_by_the_instance_port() {
        let tir = tir_from_project_source(CALL_SOURCE);
        let ir = project_tir(&tir).unwrap();
        let (cluster, call) = call_clusters(&ir)
            .into_iter()
            .find(|(_, call)| call.caller().owner() != tir.root_dag_id())
            .unwrap();

        assert_eq!(call.caller().as_str(), "v");
        assert_eq!(cluster.parent.as_ref(), Some(call.caller().owner()));
        assert!(ir.clusters.iter().any(|instance| {
            instance.id.dag() == Some(call.caller().owner())
                && matches!(instance.kind, GraphClusterKind::Instance { .. })
        }));
    }
}
