//! Graph IR — a one-way projection of a compiled [`TIR`] into a node-link
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
use graphcal_compiler::registry::resolve_types::DeclCategory;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::tir::typed::{DagTIR, DiagnosticDeclProbe, TIR};
use thiserror::Error;

/// Stable identity of a graph node: the declaration's canonical resolved name.
pub type GraphNodeId = graphcal_compiler::syntax::decl_name::ResolvedDeclName;

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
    /// Human-readable resolved type (e.g. `"Length / Time^2"`), pre-rendered
    /// because renderers have no access to the registry. `None` when the
    /// declaration's resolved type is unknown (external nodes).
    type_label: Option<String>,
}

/// A directed dataflow edge: `to` reads `from` via `@`.
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
    /// A concrete include or DAG-call instance of a reusable template.
    Instance { template: DagId },
    /// A source module referenced through `import` but not expanded into the
    /// entry DAG.
    ExternalModule,
}

/// One source module or concrete include instance projected as a group.
#[derive(Debug, Clone)]
pub struct GraphCluster {
    dag_id: DagId,
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
    /// Projected declarations in deterministic source/module order.
    nodes: Vec<GraphNode>,
    /// The root source module, nested source modules, concrete instances, and
    /// referenced external modules. The root is first; the remainder are
    /// sorted by canonical identity.
    clusters: Vec<GraphCluster>,
    /// Placeholder nodes for dependencies declared outside the expanded DAGs,
    /// sorted by id.
    external: Vec<GraphNode>,
    /// Dataflow edges, deduplicated and sorted.
    edges: Vec<GraphEdge>,
}

struct ProjectedClusterProvenance {
    clusters: BTreeMap<DagId, GraphCluster>,
    output_names: BTreeMap<DagId, HashSet<DeclName>>,
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
    /// Two provenance records claimed the same concrete owner.
    #[error("multiple graph clusters claim canonical owner `{owner}`")]
    DuplicateClusterOwner { owner: DagId },
    /// A projected declaration had no matching source-module or instance record.
    #[error("projected declaration `{declaration}` has untracked owner `{owner}`")]
    UntrackedDeclarationOwner {
        declaration: GraphNodeId,
        owner: DagId,
    },
    /// A nested cluster's explicit parent was absent or not an ancestor.
    #[error("graph cluster `{cluster}` has invalid parent `{parent}`")]
    InvalidClusterParent { cluster: DagId, parent: DagId },
    /// The root source cluster disappeared during projection.
    #[error("graph projection is missing root source cluster `{root}`")]
    MissingRootCluster { root: DagId },
}

/// Project a compiled [`TIR`] into its dependency [`GraphIr`].
///
/// Vertices are the const/param/node declarations of the file's root DAG and
/// every inline `dag` block nested inside it; asserts, plots, figures, and
/// layers are not dataflow vertices. Edges come from the TIR's canonical
/// dependency maps (`const_deps` + `runtime_deps`), oriented in dataflow
/// direction (dependency → dependent). Dependencies on declarations outside
/// the projected DAGs surface as [`GraphNodeKind::External`] placeholders.
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
pub fn project_tir(tir: &TIR) -> Result<GraphIr, GraphProjectionError> {
    let mut child_dags: Vec<&DagTIR> = tir
        .local_dags()
        .filter_map(|(dag_id, dag)| (dag_id != tir.root_dag_id()).then_some(dag))
        .collect();
    child_dags.sort_by(|a, b| a.dag_id().cmp(b.dag_id()));
    let local_dags: Vec<&DagTIR> = std::iter::once(tir.root())
        .chain(child_dags.iter().copied())
        .collect();

    let ProjectedClusterProvenance {
        mut clusters,
        output_names,
    } = project_cluster_provenance(tir, &local_dags)?;

    let nodes = local_dags
        .iter()
        .map(|dag| project_dag_nodes(tir, dag, &output_names))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    for node in &nodes {
        let owner = node.id.owner();
        let Some(cluster) = clusters.get_mut(owner) else {
            return Err(GraphProjectionError::UntrackedDeclarationOwner {
                declaration: node.id.clone(),
                owner: owner.clone(),
            });
        };
        cluster.node_ids.push(node.id.clone());
    }

    let mut edges = BTreeSet::<GraphEdge>::new();
    for dag in &local_dags {
        let deps = &dag.semantic().dependencies;
        for (dependent, dep_set) in deps.const_deps.iter().chain(deps.runtime_deps.iter()) {
            for dep in dep_set {
                edges.insert(GraphEdge {
                    from: dep.clone(),
                    to: dependent.clone(),
                });
            }
        }
    }

    // Close the graph: any edge endpoint that is not a projected declaration
    // becomes an external placeholder node. External declarations are grouped
    // by their typed owner, but their source bodies remain intentionally
    // unexpanded.
    let declared: BTreeSet<&GraphNodeId> = nodes.iter().map(|node| &node.id).collect();
    let external: Vec<GraphNode> = edges
        .iter()
        .flat_map(|edge| [&edge.from, &edge.to])
        .filter(|id| !declared.contains(id))
        .map(|id| {
            (
                id,
                GraphNode {
                    id: id.clone(),
                    kind: GraphNodeKind::External,
                    is_public_output: false,
                    type_label: None,
                },
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();

    for node in &external {
        let owner = node.id.owner().clone();
        clusters
            .entry(owner.clone())
            .or_insert_with(|| GraphCluster {
                dag_id: owner,
                parent: None,
                kind: GraphClusterKind::ExternalModule,
                node_ids: Vec::new(),
            })
            .node_ids
            .push(node.id.clone());
    }

    for cluster in clusters.values() {
        if let Some(parent) = &cluster.parent
            && (!cluster.dag_id.is_descendant_of(parent) || !clusters.contains_key(parent))
        {
            return Err(GraphProjectionError::InvalidClusterParent {
                cluster: cluster.dag_id.clone(),
                parent: parent.clone(),
            });
        }
    }

    let root_id = tir.root_dag_id();
    let root =
        clusters
            .remove(root_id)
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

fn project_cluster_provenance(
    tir: &TIR,
    local_dags: &[&DagTIR],
) -> Result<ProjectedClusterProvenance, GraphProjectionError> {
    let mut clusters = BTreeMap::<DagId, GraphCluster>::new();
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
            dag_id: dag_id.clone(),
            parent,
            kind: GraphClusterKind::SourceModule,
            node_ids: Vec::new(),
        };
        if clusters.insert(dag_id.clone(), cluster).is_some() {
            return Err(GraphProjectionError::DuplicateClusterOwner { owner: dag_id });
        }
        output_names.insert(dag_id, dag.projectable_outputs().clone());
    }

    for record in local_dags.iter().flat_map(|dag| dag.instances()) {
        let owner = record.id.owner().clone();
        let template = record.id.template().clone();
        let template_dag = tir.dag_registry().get(&template).ok_or_else(|| {
            GraphProjectionError::MissingTemplate {
                instance: owner.clone(),
                template: template.clone(),
            }
        })?;
        let cluster = GraphCluster {
            dag_id: owner.clone(),
            parent: Some(record.parent_owner.clone()),
            kind: GraphClusterKind::Instance { template },
            node_ids: Vec::new(),
        };
        if clusters.insert(owner.clone(), cluster).is_some() {
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
    tir: &TIR,
    dag: &DagTIR,
    output_names: &BTreeMap<DagId, HashSet<DeclName>>,
) -> Result<Vec<GraphNode>, DiagnosticDeclProbe> {
    dag.source_order()
        .iter()
        .filter_map(|(name, category)| {
            let kind = match category {
                DeclCategory::Const => GraphNodeKind::Const,
                DeclCategory::Param => GraphNodeKind::Param,
                DeclCategory::Node => GraphNodeKind::Node,
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer => return None,
            };
            Some((name, kind))
        })
        .map(|(name, kind)| {
            let id = dag.lookup_decl_identity(name).into_bound()?;
            let is_public_output = kind == GraphNodeKind::Node
                && output_names
                    .get(id.owner())
                    .is_some_and(|names| names.contains(&id.to_unowned_def_name()));
            let type_label = dag
                .resolved_decl_types()
                .get(name)
                .map(|ty| ty.format(tir.registry()));
            Ok(GraphNode {
                id,
                kind,
                is_public_output,
                type_label,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::syntax::module_resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::{ProjectTypeStore, type_resolve_with_modules};
    use miette::NamedSource;
    use std::sync::Arc;

    fn tir_from_source(source: &str) -> TIR {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let file = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let ir = lower(&file, &src).unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(ir.dag_id().clone(), &file.declarations)
            .unwrap();
        let mut project_types = ProjectTypeStore::default();
        project_types.insert_graphcal_prelude().unwrap();
        project_types.insert_local_hir(&ir).unwrap();
        type_resolve_with_modules(ir, &src, &resolver, &project_types).unwrap()
    }

    /// Compile through the full project pipeline (loader + inline-DAG body
    /// compilation), which is what the CLI does. Needed for inline `dag`
    /// blocks: bare `type_resolve_with_modules` does not compile their bodies.
    fn tir_from_project_source(source: &str) -> TIR {
        let mut fs = graphcal_io::InMemoryFileSystem::new();
        fs.add_file(
            graphcal_io::VirtualAbsolutePath::new("/proj/test.gcl").unwrap(),
            source.to_string(),
        )
        .unwrap();
        let (tir, _project) = crate::eval::compile_to_tir_project(
            std::path::Path::new("/proj/test.gcl"),
            Some(std::path::Path::new("/proj")),
            &fs,
        )
        .unwrap();
        tir
    }

    fn node_id(name: &str) -> GraphNodeId {
        graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
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

        assert_eq!(ir.clusters.len(), 2);
        let child = &ir.clusters[1];
        assert_eq!(child.dag_id.to_string(), "test.scale");
        assert_eq!(child.parent.as_ref(), Some(&root.dag_id));
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
        assert_eq!(instance_clusters[0].dag_id.name(), "doubled");
        assert_eq!(instance_clusters[1].dag_id.name(), "tripled");
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
}
