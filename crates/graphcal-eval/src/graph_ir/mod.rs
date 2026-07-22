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

use std::collections::{BTreeMap, BTreeSet};

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::registry::resolve_types::DeclCategory;
use graphcal_compiler::tir::typed::{DagTIR, TIR};

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

/// One DAG body projected as a group of nodes (the file's top-level body or
/// an inline `dag` block). Nodes keep source order.
#[derive(Debug, Clone)]
pub struct GraphCluster {
    dag_id: DagId,
    nodes: Vec<GraphNode>,
}

/// The projected dependency graph of one compiled file.
#[derive(Debug, Clone)]
pub struct GraphIr {
    /// The file's own top-level body.
    root: GraphCluster,
    /// Inline `dag` blocks nested (at any depth) inside the file, sorted by
    /// canonical id for deterministic output.
    children: Vec<GraphCluster>,
    /// Placeholder nodes for dependencies declared outside the projected
    /// DAGs, sorted by id.
    external: Vec<GraphNode>,
    /// Dataflow edges, deduplicated and sorted.
    edges: Vec<GraphEdge>,
}

/// Project a compiled [`TIR`] into its dependency [`GraphIr`].
///
/// Vertices are the const/param/node declarations of the file's root DAG and
/// every inline `dag` block nested inside it; asserts, plots, figures, and
/// layers are not dataflow vertices. Edges come from the TIR's canonical
/// dependency maps (`const_deps` + `runtime_deps`), oriented in dataflow
/// direction (dependency → dependent). Dependencies on declarations outside
/// the projected DAGs surface as [`GraphNodeKind::External`] placeholders.
#[must_use]
pub fn project_tir(tir: &TIR) -> GraphIr {
    let root = project_dag(tir, tir.root());

    let mut child_dags: Vec<&DagTIR> = tir
        .dags
        .values()
        .filter(|dag| dag.dag_id.is_descendant_of(&tir.root_dag_id))
        .collect();
    child_dags.sort_by(|a, b| a.dag_id.cmp(&b.dag_id));
    let children: Vec<GraphCluster> = child_dags.iter().map(|dag| project_dag(tir, dag)).collect();

    let mut edges: BTreeSet<GraphEdge> = BTreeSet::new();
    for dag in std::iter::once(tir.root()).chain(child_dags) {
        let deps = &dag.semantic.dependencies;
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
    // becomes an external placeholder node.
    let declared: BTreeSet<&GraphNodeId> = std::iter::once(&root)
        .chain(children.iter())
        .flat_map(|cluster| cluster.nodes.iter().map(|n| &n.id))
        .collect();
    let external: BTreeMap<&GraphNodeId, GraphNode> = edges
        .iter()
        .flat_map(|edge| [&edge.from, &edge.to])
        .filter(|id| !declared.contains(id))
        .map(|id| {
            (
                id,
                GraphNode {
                    id: id.clone(),
                    kind: GraphNodeKind::External,
                    type_label: None,
                },
            )
        })
        .collect();

    GraphIr {
        root,
        children,
        external: external.into_values().collect(),
        edges: edges.into_iter().collect(),
    }
}

/// Project one DAG body's const/param/node declarations, in source order.
fn project_dag(tir: &TIR, dag: &DagTIR) -> GraphCluster {
    let nodes = dag
        .source_order
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
            let id = dag.resolved_decl_key_for_local(name)?;
            let type_label = dag
                .resolved_decl_types
                .get(name)
                .map(|ty| ty.format(&tir.registry));
            Some(GraphNode {
                id,
                kind,
                type_label,
            })
        })
        .collect();
    GraphCluster {
        dag_id: dag.dag_id.clone(),
        nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::syntax::module_resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::{ModuleTypeRegistry, type_resolve_with_modules};
    use miette::NamedSource;
    use std::sync::Arc;

    fn tir_from_source(source: &str) -> TIR {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let file = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let ir = lower(&file, &src).unwrap();
        let dag_id = graphcal_compiler::dag_id::DagId::from_virtual_relative_path(
            std::path::Path::new("test.gcl"),
        )
        .unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(dag_id.clone(), &file.declarations)
            .unwrap();
        let mut module_types = ModuleTypeRegistry::default();
        module_types.insert_graphcal_prelude().unwrap();
        module_types.insert_registry(&dag_id, &ir.registry);
        type_resolve_with_modules(ir, dag_id, &src, &resolver, &module_types).unwrap()
    }

    /// Compile through the full project pipeline (loader + inline-DAG body
    /// compilation), which is what the CLI does. Needed for inline `dag`
    /// blocks: bare `type_resolve_with_modules` does not compile their bodies.
    fn tir_from_project_source(source: &str) -> TIR {
        let mut fs = graphcal_io::InMemoryFileSystem::new();
        fs.add_file(
            std::path::PathBuf::from("/proj/test.gcl"),
            source.to_string(),
        );
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
        let ir = project_tir(&tir);

        let kinds: Vec<(&str, GraphNodeKind)> = ir
            .root
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.kind))
            .collect();
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
        assert!(ir.children.is_empty());
        assert!(ir.external.is_empty());
    }

    #[test]
    fn projects_dataflow_edges_including_const_reads() {
        let tir = tir_from_source(ROCKET_SOURCE);
        let ir = project_tir(&tir);

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
        let ir = project_tir(&tir);

        let labels: BTreeMap<&str, Option<&str>> = ir
            .root
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.type_label.as_deref()))
            .collect();
        // Scalar types format with named dimension aliases preferred over
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
node doubled: Dimensionless = @scale(factor: 2.0, v: @speed).result;
",
        );
        let ir = project_tir(&tir);

        let root_names: Vec<&str> = ir.root.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(root_names, vec!["speed", "doubled"]);

        assert_eq!(ir.children.len(), 1);
        let child = &ir.children[0];
        assert_eq!(child.dag_id.to_string(), "test.scale");
        let child_names: Vec<&str> = child.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(child_names, vec!["factor", "v", "result"]);

        // The child cluster's internal dataflow is part of the graph.
        assert!(ir.edges.iter().any(|e| {
            e.from.owner().to_string() == "test.scale"
                && e.from.as_str() == "v"
                && e.to.as_str() == "result"
        }));
    }
}
