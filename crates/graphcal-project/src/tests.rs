mod checked_expressions;
mod exec_plan;
mod presentation_evidence;
mod sealed_program;

use crate::binding_error::BindingError;

use std::collections::HashSet;

use std::collections::HashMap;

use crate::compile_error::CompileError;
use crate::prepare::*;
use crate::project_compiler::{ProjectCompiler, compile_to_tir, compile_to_tir_from_project};
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::SemanticErrorKind;
use graphcal_compiler::semantic_error::attribute::AttributeError;
use graphcal_compiler::semantic_error::dimension::DimensionError;
use graphcal_compiler::semantic_error::domain::DomainError;
use graphcal_compiler::semantic_error::graph::GraphError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::semantic_error::name::NameError;
use graphcal_compiler::semantic_error::rendered::RenderedSemanticError;
use graphcal_compiler::semantic_error::structure::StructError;
use graphcal_compiler::semantic_error::visibility::VisibilityError;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_eval::eval::{AssertResult, EvalResult, NodeUnavailable, UnitLabel, Value};
use graphcal_io::RealFileSystem;

/// Test-only convenience projection for a loaded multi-file project.
pub fn compile_to_tir_project<F: graphcal_io::FileSystemReader>(
    root_path: &std::path::Path,
    project_root: Option<&std::path::Path>,
    fs: &F,
) -> Result<
    (
        graphcal_compiler::tir::typed::CheckedTir,
        crate::loader::LoadedProject,
    ),
    CompileError,
> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}

fn fs() -> RealFileSystem {
    RealFileSystem::default()
}

fn scoped_name(name: &str) -> ScopedName {
    ScopedName::local(graphcal_compiler::syntax::decl_name::DeclName::expect_valid(name))
}

/// The checked type of the root declaration written as `name`.
fn root_decl_type<'a>(
    tir: &'a graphcal_compiler::tir::typed::CheckedTir,
    name: &str,
) -> &'a graphcal_compiler::tir::typed::CheckedDeclType {
    let identity = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name(name))
        .unwrap_or_else(|| panic!("`{name}` is not declared in the root"));
    tir.decl_type(identity)
        .unwrap_or_else(|| panic!("`{name}` has no checked type"))
}

fn member_name(owner: &[&str], leaf: &str) -> ScopedName {
    let owner = owner
        .iter()
        .map(|segment| {
            graphcal_compiler::syntax::module_name::ModuleAliasName::expect_valid(*segment).into()
        })
        .collect();
    ScopedName::qualified(
        graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(owner).unwrap(),
        graphcal_compiler::syntax::decl_name::DeclName::expect_valid(leaf),
    )
}

/// Find the SI value of a named quantity declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    // Check consts first
    if let Some((_, val)) = result.consts().find(|(n, _)| n.to_string() == name) {
        return val.as_ref().unwrap().si_value().unwrap().get();
    }
    // Check params and nodes (wrapped in Result)
    result
        .params()
        .chain(result.nodes())
        .find(|(n, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .si_value()
        .unwrap()
        .get()
}

fn assert_quantity_value(result: &EvalResult, name: &str, expected: f64) {
    let actual = find_value(result, name);
    assert!(
        (actual - expected).abs() < f64::EPSILON,
        "expected `{name}` to equal {expected}, got {actual}"
    );
}

/// Write a small package-shaped project for pipeline regression tests.
fn write_pipeline_project(
    files: &[(&str, &str)],
    root: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let source_root = directory.path().join("src/pipeline");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"pipeline\"\n",
    )
    .unwrap();
    for (name, source) in files {
        std::fs::write(source_root.join(name), source).unwrap();
    }
    (directory, source_root.join(root))
}

/// One node of a checked tree: its span, and its concrete type when the tree
/// is executable and the node is a value.
type CheckedNodeSummary = (
    graphcal_compiler::syntax::span::Span,
    Option<graphcal_compiler::semantic::checked_type::CheckedType>,
    bool,
);

/// Every node of every checked tree of `dag`, by occurrence.
fn checked_nodes(
    dag: &graphcal_compiler::tir::typed::CheckedDag,
) -> HashMap<graphcal_compiler::expression_id::ExprId, CheckedNodeSummary> {
    use graphcal_compiler::tir::texpr::{CheckedBody, TNodeRef, visit_tnodes};
    let mut nodes = HashMap::new();
    for (_, body) in dag.bodies_for_test().roots() {
        match body {
            CheckedBody::Executable(body) => visit_tnodes(body.as_node(), &mut |node| {
                let summary = match node {
                    TNodeRef::Value(expr) => (expr.span(), Some(expr.ty().clone()), false),
                    TNodeRef::Contextual(literal) => (literal.span(), None, true),
                };
                nodes.insert(node.id().clone(), summary);
            }),
            CheckedBody::Deferred(body) => visit_tnodes(body.as_node(), &mut |node| {
                let summary = match node {
                    TNodeRef::Value(expr) => (expr.span(), None, false),
                    TNodeRef::Contextual(literal) => (literal.span(), None, true),
                };
                nodes.insert(node.id().clone(), summary);
            }),
        }
    }
    nodes
}

#[test]
fn generated_checked_expression_coverage_includes_every_owned_root_family() {
    let template = r#"
const node factor: Dimensionless = 2.0;
param scale: Dimensionless(min: 1.0) = @factor;
unit step: Length = (@scale) m;
param x: Length(min: 0.0 m) = 1.0 step;
type Boxed { Boxed(value: Length(min: 0.0 m)), }
node boxed: Boxed = Boxed(value: @x);
node values: Length[Fin(SIZE)] = for p: Fin(SIZE) { @boxed.value };
assert close = @x ~= 2.0 m +/- 0.01 m;
plot curve = { mark: line { stroke_width: 2.0 }, encode: { x: @values }, title: "Curve" };
figure comparison = { plots: [curve], title: "Comparison" };
layer overlay = { plots: [curve], title: "Overlay", width: 400.0 };
"#;
    for size in 2..=6 {
        let source = template.replace("SIZE", &size.to_string());
        let mut old_ids = Vec::new();
        for padding in ["", "\n\n    "] {
            let project = crate::loader::LoadedProject::from_source(
                &format!("{padding}{source}"),
                "coverage.gcl",
            )
            .unwrap();
            let checked = ProjectCompiler::new(&project).check().unwrap();
            let dag = checked.tir().root();
            let nodes = checked_nodes(dag);
            let mut ids = std::collections::HashSet::new();
            dag.body_for_test()
                .owned_expression_roots()
                .for_each(|root| {
                    graphcal_compiler::hir::expr::visit_expr(root, &mut |expr| {
                        let id = expr.id();
                        assert_eq!(nodes[id].0, expr.span);
                        ids.insert(id.clone());
                    });
                });
            assert_eq!(ids.len(), nodes.len());
            assert!(
                ids.len() > 20,
                "coverage fixture must not become vacuous: {} rows",
                ids.len()
            );
            assert!(old_ids.iter().all(|id| !nodes.contains_key(id)));
            let tir = checked.tir();
            assert!(nodes.values().any(|(_, ty, _)| {
                matches!(ty, Some(checked_type) if checked_type
                    .materialized_shape(|axis| {
                        Ok::<_, graphcal_compiler::tir::materialized_shape::MaterializedShapeError>(
                            tir.index_def(axis).and_then(|index| index.concrete_cardinality()),
                        )
                    })
                    .unwrap()
                    .is_some_and(|shape| shape.total().get() == size))
            }));
            assert_eq!(dag.body_for_test().semantic().dynamic_unit_scales.len(), 1);
            assert!(nodes.values().any(|(_, _, contextual)| *contextual));
            old_ids = ids.into_iter().collect();
            let prepared = checked
                .prepare_with_host_fns(&graphcal_eval::host_fns::HostFunctionRegistry::new())
                .unwrap();
            let row = prepared.binding_builder().finish().unwrap();
            assert!(!prepared.evaluate(&row).unwrap().has_errors());
        }
    }
}

#[test]
fn pipeline_cost_baseline_observes_preparation_and_repeated_call_work() {
    let source = r"
type Packet { Packet(value: Length), }
dag worker {
    param x: Length;
    pub node out: Length = if @x > 0.0 m { @x -> km } else { 0.0 m -> m };
}
node first: Length = @worker(x: 1000.0 m)::out;
node other_output: Length = @worker(x: 2000.0 m)::out;
node packet: Packet = Packet(value: @first);
";
    let project = crate::loader::LoadedProject::from_source(source, "metrics.gcl").unwrap();
    let ((prepared, retained_constructors), preparation) =
        graphcal_eval::pipeline_metrics::measure(|| {
            let checked = ProjectCompiler::new(&project).check().unwrap();
            let mut retained = 0_usize;
            for dag in checked.tir().dag_registry().values() {
                for (_, body) in dag.bodies_for_test().roots() {
                    let mut count = |applies: bool| retained += usize::from(applies);
                    match body {
                        graphcal_compiler::tir::texpr::CheckedBody::Executable(body) => {
                            graphcal_compiler::tir::texpr::visit_tnodes(
                                body.as_node(),
                                &mut |node| {
                                    count(matches!(
                                        node,
                                        graphcal_compiler::tir::texpr::TNodeRef::Value(expr)
                                            if expr.application().is_some()
                                    ));
                                },
                            );
                        }
                        graphcal_compiler::tir::texpr::CheckedBody::Deferred(body) => {
                            graphcal_compiler::tir::texpr::visit_tnodes(
                                body.as_node(),
                                &mut |node| {
                                    count(matches!(
                                        node,
                                        graphcal_compiler::tir::texpr::TNodeRef::Value(expr)
                                            if expr.application().is_some()
                                    ));
                                },
                            );
                        }
                    }
                }
            }
            (
                checked
                    .prepare_with_host_fns(&graphcal_eval::host_fns::HostFunctionRegistry::new())
                    .unwrap(),
                retained,
            )
        });
    assert_eq!(
        retained_constructors, 1,
        "the actual constructor application must have been retained"
    );
    let row = prepared.binding_builder().finish().unwrap();
    let (first, first_counts) =
        graphcal_eval::pipeline_metrics::measure(|| prepared.evaluate(&row).unwrap());
    let (second, second_counts) =
        graphcal_eval::pipeline_metrics::measure(|| prepared.evaluate(&row).unwrap());
    assert!(!first.has_errors(), "{first:?}");
    assert!(!second.has_errors(), "{second:?}");
    assert_eq!(
        first_counts, second_counts,
        "prepared evaluation costs should be stable"
    );
    assert_eq!(
        preparation.imported_body_references, 0,
        "single-file fixture has no ordinary imports"
    );
    assert!(preparation.plan_constructions > 0, "{preparation:?}");
    assert_eq!(first_counts.plan_constructions, 0, "{first_counts:?}");
    assert_eq!(first_counts.imported_source_resolutions, 0);
    assert_eq!(
        first_counts.frame_executions, 3,
        "root and both calls must use the shared machine"
    );
    // Constructor applications are retained during checking and consumed here.
    // Presentation replay remains Phase D work.
    assert!(
        first_counts.constructor_fact_consumptions > 0,
        "{first_counts:?}"
    );
    assert!(
        first_counts.presentation_evaluations > 0,
        "{first_counts:?}"
    );
    eprintln!("preparation: {preparation:?}; repeated evaluation: {first_counts:?}");
}

#[test]
fn pipeline_cost_baseline_observes_ordinary_import_body_sharing() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("a.gcl", "pub node output: Dimensionless = 1.0;"),
            (
                "b.gcl",
                "import pipeline.a as a; pub node output: Dimensionless = 2.0;",
            ),
            (
                "main.gcl",
                "import pipeline.b as b; node output: Dimensionless = 3.0;",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let (prepared, counts) = graphcal_eval::pipeline_metrics::measure(|| {
        ProjectCompiler::new(&project).prepare().unwrap()
    });
    let row = prepared.binding_builder().finish().unwrap();
    assert!(!prepared.evaluate(&row).unwrap().has_errors());
    assert_eq!(counts.imported_body_references, 3, "{counts:?}");
    assert_eq!(
        counts.unshared_imported_bodies, 0,
        "ordinary imports use canonical immutable body handles: {counts:?}"
    );
    eprintln!("three-module chain preparation: {counts:?}");
}

#[test]
fn pipeline_cost_baseline_import_chains_share_canonical_bodies() {
    for size in [2_u64, 4, 8] {
        let files = (0..size).map(|index| {
            let body = match index {
                0 => "pub node output: Dimensionless = 1.0;".to_string(),
                _ => format!("import pipeline.layer{} as predecessor; pub node output: Dimensionless = 1.0;", index.saturating_sub(1)),
            };
            (format!("layer{index}.gcl"), body)
        }).collect::<Vec<_>>();
        let borrowed = files
            .iter()
            .map(|(path, source)| (path.as_str(), source.as_str()))
            .collect::<Vec<_>>();
        let (_directory, root) = write_pipeline_project(&borrowed, borrowed.last().unwrap().0);
        let project = crate::loader::load_project(&root, None, &fs()).unwrap();
        let (_, counts) = graphcal_eval::pipeline_metrics::measure(|| {
            ProjectCompiler::new(&project).prepare().unwrap()
        });
        assert_eq!(
            counts.imported_body_references,
            size.saturating_mul(size.saturating_sub(1)) / 2,
            "{counts:?}"
        );
        assert_eq!(
            counts.unshared_imported_bodies, 0,
            "{size}-module chain retains canonical body addresses: {counts:?}"
        );
        eprintln!("{size}-module chain: {counts:?}");
    }
}

#[test]
fn pipeline_cost_baseline_diamond_keeps_constant_pools_and_shared_call_bodies() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("leaf.gcl", "pub const node BASE: Dimensionless = 2.0;"),
            (
                "left.gcl",
                "import pipeline.leaf::{BASE}; pub const node LEFT: Dimensionless = @BASE + 1.0; pub node out: Dimensionless = @BASE;",
            ),
            (
                "right.gcl",
                "import pipeline.leaf::{BASE as VALUE}; pub const node RIGHT: Dimensionless = @VALUE + 2.0; pub node out: Dimensionless = @VALUE;",
            ),
            (
                "main.gcl",
                "import pipeline.left as left; import pipeline.right as right; const node TOTAL: Dimensionless = @left::LEFT + @right::RIGHT; node output: Dimensionless = @TOTAL + @left()::out + @right()::out;",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let (prepared, counts) = graphcal_eval::pipeline_metrics::measure(|| {
        ProjectCompiler::new(&project).prepare().unwrap()
    });
    assert_eq!(counts.imported_body_references, 6, "{counts:?}");
    assert_eq!(counts.unshared_imported_bodies, 0, "{counts:?}");
    let row = prepared.binding_builder().finish().unwrap();
    let result = prepared.evaluate(&row).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_quantity_value(&result, "output", 11.0);
}

#[test]
fn presentation_selects_pure_native_branch_once_and_rendering_never_replays() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let source = r#"
import plugin "graphcal:selector-counter" as probe { fn toggle() -> Bool; }
node measured: Length = if probe::toggle() { 1000.0 m -> km } else { 2.0 m -> m };
"#;
    let project = crate::loader::LoadedProject::from_source(source, "selector.gcl").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut host = graphcal_eval::host_fns::HostFunctionRegistry::new();
    host.register_for_test(
        graphcal_compiler::syntax::plugin::PluginPath::new("graphcal:selector-counter"),
        graphcal_compiler::syntax::function_name::FnName::expect_valid("toggle"),
        move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(graphcal_eval::host_fns::HostFnValue::F64(1.0))
        },
    );
    let prepared = ProjectCompiler::new(&project)
        .host_fns(&host)
        .prepare()
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "checking must not invoke a host"
    );
    let row = prepared.binding_builder().finish().unwrap();
    let result = prepared.evaluate(&row).unwrap();
    assert_quantity_value(&result, "measured", 1000.0);
    let (_, value) = result
        .nodes()
        .find(|(name, _)| **name == scoped_name("measured"))
        .unwrap();
    let Value::Quantity { display_unit, .. } = value.as_ref().unwrap() else {
        panic!("expected quantity");
    };
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "selector is evaluated exactly once"
    );
    assert_eq!(display_unit.as_ref().unwrap().label, "km");
    for _ in 0..4 {
        assert_eq!(
            value
                .as_ref()
                .unwrap()
                .format_display(&result.render, UnitLabel::Inline)
                .unwrap(),
            "1 [km]"
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "rendering has no host capability"
    );
}

#[test]
fn context_capabilities_are_phase_selected_and_checked_scopes_fail_closed() {
    let source = "type Bounded { Bounded(value: Dimensionless(min: 1.0)), } node x: Bounded = Bounded(value: 2.0);";
    let tir = compile_to_tir(source, "capabilities.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("capabilities.gcl", std::sync::Arc::new(source.to_string()));
    let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
    let provisional = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        &sources,
        cancellation.clone(),
    );
    assert!(provisional.host_fns().is_none());
    assert!(provisional.struct_field_constraints().is_none());
    assert!(provisional.execution_plan().is_err());

    let prepared =
        graphcal_eval::exec_plan::compile_with_cancellation(&tir, src, &sources, &cancellation)
            .unwrap();
    let plan = prepared.plan();
    let host = graphcal_eval::host_fns::HostFunctionRegistry::new();
    let context =
        graphcal_eval::eval_expr::EvalSession::checked(plan, src, &sources, &host, cancellation);
    assert!(std::ptr::eq(context.tir, plan.tir()));
    assert!(std::ptr::eq(
        context.struct_field_constraints().unwrap(),
        plan.program().facts().struct_field_constraints()
    ));
    assert!(!context.struct_field_constraints().unwrap().is_empty());
    assert!(std::ptr::eq(context.host_fns().unwrap(), &raw const host));
}

#[test]
fn pure_plugin_values_agree_across_source_orders_and_root_call_execution() {
    let mut host = graphcal_eval::host_fns::HostFunctionRegistry::new();
    host.register_for_test(
        graphcal_compiler::syntax::plugin::PluginPath::new("graphcal:pure-plan-test"),
        graphcal_compiler::syntax::function_name::FnName::expect_valid("twice"),
        |args| match &args[0] {
            graphcal_eval::host_abi::argument::HostArgument::Scalar(
                graphcal_eval::host_abi::HostScalar::Quantity(value),
            ) => Ok(graphcal_eval::host_fns::HostFnValue::F64(value.get() * 2.0)),
            other => panic!("expected checked scalar argument, got {other:?}"),
        },
    );
    for body in [
        "pub node a: Dimensionless = probe::twice(2.0); pub node z: Dimensionless = probe::twice(@a); pub node q: Dimensionless = probe::twice(3.0);",
        "pub node q: Dimensionless = probe::twice(3.0); pub node z: Dimensionless = probe::twice(@a); pub node a: Dimensionless = probe::twice(2.0);",
    ] {
        for declarations in [
            format!(
                "{body} node output: Dimensionless = @z; node independent: Dimensionless = @q;"
            ),
            format!(
                "dag inner {{ {body} }} node output: Dimensionless = @inner()::z; node independent: Dimensionless = @inner()::q;"
            ),
        ] {
            let source = format!(
                "import plugin \"graphcal:pure-plan-test\" as probe {{ fn twice(x: Dimensionless) -> Dimensionless; }} {declarations}"
            );
            let project =
                crate::loader::LoadedProject::from_source(&source, "pure-plans.gcl").unwrap();
            let prepared = ProjectCompiler::new(&project)
                .host_fns(&host)
                .prepare()
                .unwrap();
            let row = prepared.binding_builder().finish().unwrap();
            let (result, counts) =
                graphcal_eval::pipeline_metrics::measure(|| prepared.evaluate(&row).unwrap());
            assert!(!result.has_errors(), "{result:?}");
            assert_quantity_value(&result, "output", 8.0);
            assert_quantity_value(&result, "independent", 6.0);
            assert_eq!(counts.plan_constructions, 0);
            assert_eq!(counts.imported_source_resolutions, 0);
            assert!(counts.frame_executions > 0);
        }
    }
}

fn callable_plan_fixture() -> (
    graphcal_compiler::tir::typed::CheckedTir,
    graphcal_compiler::source_id::SourceId,
    graphcal_compiler::source_registry::SourceRegistry,
) {
    let source = "const node BASE: Dimensionless = 3.0; dag helper { const node LOCAL: Dimensionless = 20.0; pub node out: Dimensionless = @LOCAL; } node value: Dimensionless = @helper()::out + @BASE;";
    let tir = compile_to_tir(source, "call-plans.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("call-plans.gcl", std::sync::Arc::new(source.to_string()));
    assert!(
        tir.dag_registry().len() > 1,
        "fixture must contain a real callable body"
    );
    (tir, src, sources)
}

#[test]
fn plans_have_one_callable_for_every_dag_at_its_position() {
    let source =
        "dag unused { pub node out: Dimensionless = 1.0; } node value: Dimensionless = 2.0;";
    let tir = compile_to_tir(source, "uncalled.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("uncalled.gcl", std::sync::Arc::new(source.to_string()));
    let prepared = graphcal_eval::exec_plan::compile(&tir, src, &sources).unwrap();
    let owners = prepared
        .plan()
        .callables()
        .map(|callable| callable.scope().dag().dag_id().clone())
        .collect::<Vec<_>>();
    let positioned = tir
        .dag_registry()
        .positioned()
        .map(|(_, dag)| dag.dag_id().clone())
        .collect::<Vec<_>>();
    assert_eq!(owners, positioned, "the uncalled DAG has its callable too");
    assert_eq!(&owners[0], tir.root_dag_id());
}

#[test]
fn call_slots_resolve_to_the_callable_of_their_target() {
    let (tir, src, sources) = callable_plan_fixture();
    let prepared = graphcal_eval::exec_plan::compile(&tir, src, &sources).unwrap();
    let plan = prepared.plan();
    let targets = tir
        .root()
        .call_targets()
        .iter()
        .map(|(_, target)| target)
        .collect::<Vec<_>>();
    let helper = tir
        .dag_registry()
        .keys()
        .find(|owner| *owner != tir.root_dag_id())
        .unwrap();
    assert_eq!(targets, [helper]);
    assert_eq!(
        plan.callees_of(tir.root_dag_id())
            .unwrap()
            .iter()
            .map(|callee| callee.scope().dag().dag_id())
            .collect::<Vec<_>>(),
        targets
    );
}

#[test]
fn frozen_stores_record_the_imported_dags_their_bodies_call() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("leaf.gcl", "pub node out: Dimensionless = 2.0;"),
            (
                "mid.gcl",
                "import pipeline.leaf as leaf; dag local { pub node out: Dimensionless = 1.0; } pub node out: Dimensionless = @leaf()::out + @local()::out;",
            ),
        ],
        "mid.gcl",
    );
    let (tir, _) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let mid = tir.root_dag_id().clone();
    let leaf = tir
        .dag_registry()
        .keys()
        .find(|owner| owner.leaf().to_string() == "leaf")
        .unwrap()
        .clone();
    let store = tir.freeze_local_dag_store().unwrap();
    assert_eq!(store.len(), 2, "the root and its inline DAG");
    assert_eq!(
        store.external_callees().collect::<Vec<_>>(),
        [(&leaf, &mid)],
        "only the call into the imported module leaves the store"
    );
}

#[test]
fn instances_sharing_a_template_body_keep_its_callees() {
    let source = "dag helper { pub node out: Dimensionless = 10.0; } \
                  dag inner { param input: Dimensionless = 2.0; pub node value: Dimensionless = @input * @helper()::out; } \
                  include inner(input: 3.0) as first; \
                  include inner(input: 4.0) as second; \
                  dag other { pub node out: Dimensionless = 5.0; } \
                  include inner(input: @other()::out) as third; \
                  node output: Dimensionless = @first::value + @second::value + @third::value;";
    let tir = compile_to_tir(source, "shared-callees.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register(
        "shared-callees.gcl",
        std::sync::Arc::new(source.to_string()),
    );
    let prepared = graphcal_eval::exec_plan::compile(&tir, src, &sources).unwrap();
    let plan = prepared.plan();
    let callees = |dag: &graphcal_compiler::tir::typed::CheckedDag| {
        plan.callees_of(dag.dag_id())
            .unwrap()
            .iter()
            .map(|callee| callee.scope().dag().dag_id().clone())
            .collect::<Vec<_>>()
    };
    let template = tir
        .dag_registry()
        .values()
        .find(|dag| dag.dag_id().leaf().to_string() == "inner")
        .unwrap();
    let template_callees = callees(template);
    assert_eq!(template_callees.len(), 1);
    let instances = tir
        .dag_registry()
        .values()
        .filter(|dag| dag.is_semantic_instance())
        .collect::<Vec<_>>();
    assert_eq!(instances.len(), 3);
    let mut extended = 0;
    for instance in instances {
        // An instance's call targets extend its template's: every slot of a
        // shared template tree keeps its callee, and the calls of a rebound
        // default come after them.
        let targets = instance.call_targets();
        for (slot, target) in template.call_targets().iter() {
            assert_eq!(targets.target(slot), target);
        }
        let instance_callees = callees(instance);
        assert_eq!(
            instance_callees[..template_callees.len()],
            template_callees[..]
        );
        if targets.len() > template.call_targets().len() {
            extended += 1;
            assert_eq!(
                instance_callees[template_callees.len()..]
                    .iter()
                    .map(|dag| dag.leaf().to_string())
                    .collect::<Vec<_>>(),
                ["other"]
            );
        }
    }
    assert_eq!(extended, 1, "only the rebound default calls `other`");
    let result = compile_and_eval(source).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_quantity_value(&result, "output", 120.0);
}

#[test]
fn every_body_has_one_prepared_callable_with_retained_single_body_pools() {
    let (tir, src, sources) = callable_plan_fixture();
    let (prepared, counts) = graphcal_eval::pipeline_metrics::measure(|| {
        graphcal_eval::exec_plan::compile(&tir, src, &sources).unwrap()
    });
    let plan = prepared.plan();
    assert_eq!(
        counts.plan_constructions,
        u64::try_from(tir.dag_registry().len()).unwrap()
    );
    assert_eq!(plan.callables().count(), tir.dag_registry().len());
    assert_eq!(
        plan.callables().next().unwrap().scope().dag().dag_id(),
        tir.root_dag_id()
    );
    for dag in tir.dag_registry().values() {
        let callable = plan
            .callables()
            .find(|callable| callable.scope().dag().dag_id() == dag.dag_id())
            .unwrap();
        assert_eq!(
            callable
                .execution_dags()
                .iter()
                .map(|scope| scope.dag().dag_id())
                .collect::<Vec<_>>(),
            [dag.dag_id()]
        );
        let sealed = plan.program().dag(dag.dag_id()).unwrap();
        assert!(!sealed.const_values().is_empty());
        assert!(std::ptr::eq(
            sealed.const_values(),
            callable.scope().const_values()
        ));
    }
}

#[test]
fn shared_frames_agree_on_defaults_bindings_domains_and_assertions() {
    for binding in ["", "input: 3.0"] {
        let expected = if binding.is_empty() { 4.0 } else { 6.0 };
        let body = "param input: Dimensionless(min: 0.0, max: 5.0) = 2.0; pub node value: Dimensionless = @input * 2.0; pub assert positive = @value > 0.0; #[expected_fail] pub assert inverted = false;";
        let called = format!(
            "dag inner {{ {body} }} node output: Dimensionless = @inner({binding})::value;"
        );
        let included = format!(
            "dag inner {{ {body} }} include inner({binding}) as inner_instance; node output: Dimensionless = @inner_instance::value;"
        );
        let root = format!(
            "{} node output: Dimensionless = @value;",
            if binding.is_empty() {
                body.to_string()
            } else {
                body.replace("= 2.0;", "= 3.0;")
            }
        );
        for source in [&root, &called, &included] {
            let result = compile_and_eval(source).unwrap();
            assert!(!result.has_errors(), "{source}\n{result:?}");
            assert_quantity_value(&result, "output", expected);
        }
    }
    for body in [
        "param input: Dimensionless = 6.0; pub node output: Dimensionless(min: 0.0, max: 5.0) = @input;",
        "pub node output: Dimensionless = 2.0; pub assert fails = false;",
        "pub node output: Dimensionless = 2.0; #[expected_fail] pub assert unexpected = true;",
    ] {
        for source in [
            body.to_string(),
            format!("dag inner {{ {body} }} node called: Dimensionless = @inner()::output;"),
            format!("dag inner {{ {body} }} include inner() as inner_instance;"),
        ] {
            let result =
                compile_and_eval(&format!("{source} node independent: Dimensionless = 7.0;"))
                    .unwrap();
            assert!(result.has_errors(), "{source}\n{result:?}");
            assert_quantity_value(&result, "independent", 7.0);
        }
    }
}

#[test]
fn shared_frames_reject_dynamic_parameter_domain_violations_without_losing_independent_values() {
    let body =
        "param input: Dimensionless(min: 0.0, max: 5.0); pub node output: Dimensionless = @input;";
    for source in [
        "param input: Dimensionless(min: 0.0, max: 5.0) = @raw; node output: Dimensionless = @input;".to_string(),
        format!("dag inner {{ {body} }} node output: Dimensionless = @inner(input: @raw)::output;"),
        format!("dag inner {{ {body} }} include inner(input: @raw) as configured; node output: Dimensionless = @configured::output;"),
    ] {
        let result = compile_and_eval(&format!("param raw: Dimensionless = 6.0; {source} node independent: Dimensionless = 7.0;")).unwrap();
        assert!(result.has_errors(), "{source}\n{result:?}");
        assert_quantity_value(&result, "independent", 7.0);
    }
}

#[test]
fn dependency_availability_is_determined_once_per_declaration() {
    let source = "node a: Dimensionless = 1.0 + 2.0 * (3.0 - 4.0 / 5.0); \
                  node b: Dimensionless = if @a > 0.0 { @a * (@a + 1.0) } else { -@a }; \
                  node c: Dimensionless = abs(@a - @b) + sqrt(@b * 2.0);";
    let tir = compile_to_tir(source, "availability.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("availability.gcl", std::sync::Arc::new(source.to_string()));
    let prepared = graphcal_eval::exec_plan::compile(&tir, src, &sources).unwrap();
    let (runtime, counts) = graphcal_eval::pipeline_metrics::measure(|| {
        graphcal_eval::eval::runtime::run_eval_loop_with_bindings(
            prepared.plan(),
            &graphcal_eval::eval::bindings::RuntimeParameterBindings::new(),
            src,
            &sources,
            &graphcal_eval::host_fns::HostFunctionRegistry::new(),
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap()
    });
    assert!(runtime.errors.is_empty());
    assert_eq!(runtime.values.len(), 3);
    assert_eq!(counts.dependency_availability_checks, 3);
}

#[test]
fn prepared_imports_and_instance_constant_pools_borrow_canonical_values() {
    let source = "pub const node OUTER: Dimensionless = 2.0; dag helper { import pools::{OUTER}; const node LOCAL: Dimensionless = 3.0; pub node value: Dimensionless = @OUTER + @LOCAL; } include helper() as one; include helper() as two; node output: Dimensionless = @one::value + @two::value + @helper()::value;";
    let tir = compile_to_tir(source, "pools.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("pools.gcl", std::sync::Arc::new(source.to_string()));
    let (prepared, preparation) = graphcal_eval::pipeline_metrics::measure(|| {
        graphcal_eval::exec_plan::compile(&tir, src, &sources).unwrap()
    });
    let plan = prepared.plan();
    assert!(preparation.imported_source_resolutions > 0);
    assert!(plan.root().execution_dags().len() > 1);
    let mut constants = 0;
    let mut imports = 0;
    for callable in plan.callables() {
        for scope in callable.execution_dags() {
            for (key, value) in scope.const_values().iter() {
                let body = plan.declaration(key).unwrap().scope();
                let canonical = plan
                    .program()
                    .dag(body.dag().dag_id())
                    .unwrap()
                    .const_values()
                    .get(key)
                    .unwrap();
                assert!(std::ptr::eq(value, canonical));
                constants += 1;
            }
        }
        for import in callable.constant_imports() {
            let value = import.value.value();
            assert!(plan.tir().dag_registry().keys().any(|dag_id| {
                plan.program()
                    .dag(dag_id)
                    .unwrap()
                    .const_values()
                    .values()
                    .any(|canonical| std::ptr::eq(value, canonical))
            }));
            imports += 1;
        }
    }
    assert!(constants > 2 && imports > 0);
    let (runtime, counts) = graphcal_eval::pipeline_metrics::measure(|| {
        graphcal_eval::eval::runtime::run_eval_loop_with_bindings(
            plan,
            &graphcal_eval::eval::bindings::RuntimeParameterBindings::new(),
            src,
            &sources,
            &graphcal_eval::host_fns::HostFunctionRegistry::new(),
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap()
    });
    assert!(runtime.errors.is_empty());
    assert_eq!(counts.imported_source_resolutions, 0);
    assert_eq!(
        counts.frame_executions, 2,
        "one include-closure frame and one inline call"
    );
    assert_quantity_value(
        &compile_and_eval_named(source, "pools.gcl").unwrap(),
        "output",
        15.0,
    );
}

#[test]
fn selective_import_rejects_required_static_inputs() {
    for (declaration, import_item, expected_kind) in [
        (
            "pub(bind) type Element;",
            "type Element",
            graphcal_compiler::static_interface::StaticInputKind::Type,
        ),
        (
            "pub(bind) dim Quantity;",
            "dim Quantity",
            graphcal_compiler::static_interface::StaticInputKind::Dimension,
        ),
        (
            "pub(bind) index Axis;",
            "index Axis",
            graphcal_compiler::static_interface::StaticInputKind::Index,
        ),
    ] {
        let (_directory, root) = write_pipeline_project(
            &[
                ("lib.gcl", declaration),
                (
                    "main.gcl",
                    &format!(
                        "import pipeline.lib::{{ {import_item} }};\nnode output: Dimensionless = 1.0;"
                    ),
                ),
            ],
            "main.gcl",
        );
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportRequiredStaticInput { kind, .. }), .. }), .. })
                if kind == expected_kind
        ));
    }
}

#[test]
fn qualified_import_rejects_required_static_members() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub(bind) type Element;"),
            (
                "main.gcl",
                "import pipeline.lib as lib;\n\
                 type Local { Local(value: lib::Element) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::ImportRequiredStaticInput { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn qualified_import_rejects_transitive_required_static_dependencies() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) type Element;\n\
                 pub type Box { Box(value: Element) }\n\
                 pub type Wrapper { Wrapper(value: Box) }",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\n\
                 type Local { Local(value: lib::Wrapper) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportUnresolvedStaticDependency { dependency_kind: graphcal_compiler::static_interface::StaticInputKind::Type, dependency, .. }), .. }), .. }) if dependency.as_str() == "Element"
    ));
}

#[test]
fn qualified_import_resolves_ambiguous_static_dependencies_to_their_symbol() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) dim Basis;\n\
                 pub type Reading { Reading(value: Basis) }",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\n\
                 type Local { Local(value: lib::Reading) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportUnresolvedStaticDependency { dependency_kind: graphcal_compiler::static_interface::StaticInputKind::Dimension, dependency, .. }), .. }), .. }) if dependency.as_str() == "Basis"
    ));
}

#[test]
fn selective_import_rejects_transitive_required_static_dependencies() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) type Element;\n\
                 pub type Box { Box(value: Element) }\n\
                 pub type Wrapper { Wrapper(value: Box) }",
            ),
            (
                "main.gcl",
                "import pipeline.lib::{ type Wrapper };\nnode output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::ImportUnresolvedStaticDependency { dependency_kind: graphcal_compiler::static_interface::StaticInputKind::Type, dependency, .. }), .. }), .. }) if dependency.as_str() == "Element"
    ));
}

#[test]
fn selective_include_projects_effective_bound_static_target() {
    let source = "type Concrete { Concrete }\n\
                  dag target { pub(bind) type Slot; }\n\
                  include target(type Slot: Concrete)::{ type Slot as Effective };\n\
                  node value: Effective = Concrete;\n\
                  node output: Dimensionless = 1.0;";
    compile_and_eval(source).expect("the projected type is the effective binding target");
}

#[test]
fn cross_file_selective_include_projects_effective_bound_static_target() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub(bind) type Slot;"),
            (
                "main.gcl",
                "type Concrete { Concrete }\n\
                 include pipeline.lib(type Slot: Concrete)::{ type Slot as Effective };\n\
                 type Envelope { Envelope(value: Effective) }\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    compile_and_eval_project(&root, &HashMap::new(), None, &fs())
        .expect("cross-file projection uses the concrete binding target");
}

#[test]
fn selective_include_projects_closed_static_and_runtime_unit_categories() {
    let source = "dag target {\n\
                      pub type Record { Record }\n\
                      pub dim Distance = Length;\n\
                      pub index Axis = { Only };\n\
                      pub unit double_metre: Length = 2.0 m;\n\
                  }\n\
                  include target()::{\n\
                      type Record as EffectiveRecord,\n\
                      dim Distance as EffectiveDistance,\n\
                      index Axis as EffectiveAxis,\n\
                      unit double_metre as effective_double_metre,\n\
                  };\n\
                  type Envelope { Envelope(value: EffectiveRecord) }\n\
                  node distance: EffectiveDistance = 1.0 m;\n\
                  node indexed: Dimensionless[EffectiveAxis] = for i: EffectiveAxis { 1.0 };\n\
                  node scaled: Length = 1.0 effective_double_metre;";
    let result = compile_and_eval(source).expect("all typed projection markers resolve");
    assert_quantity_value(&result, "distance", 1.0);
    assert_quantity_value(&result, "scaled", 2.0);
}

#[test]
fn selective_include_unit_projection_resolves_in_unit_definitions() {
    // A projected unit alias is visible to the importer's own unit
    // definitions, not only to expressions.
    let source = "dag target {\n\
                      pub const unit double_metre: Length = 2.0 m;\n\
                  }\n\
                  include target()::{ unit double_metre as effective_double_metre };\n\
                  const unit quad_metre: Length = 2.0 effective_double_metre;\n\
                  node scaled: Length = 1.0 quad_metre;";
    let result = compile_and_eval(source).expect("projected unit alias resolves");
    assert_quantity_value(&result, "scaled", 4.0);
}

#[test]
fn renamed_include_projection_does_not_bind_the_source_name_in_declarations() {
    // `dim X as Y` / `unit X as Y` bind only `Y` in the importer's own
    // `dim` / `unit` declarations; `X` is as unknown as any undeclared name.
    let target = "dag target {\n\
                      pub dim Rate = Mass / Time;\n\
                      pub const unit double_metre: Length = 2.0 m;\n\
                  }\n";
    for (body, expected) in [
        (
            "include target()::{ dim Rate as R };\ndim D = Rate * Time;",
            "Rate",
        ),
        (
            "include target()::{ dim Rate as R };\nunit u: Rate = 2.0 kg/s;",
            "Rate",
        ),
    ] {
        match compile_and_eval(&format!("{target}{body}")) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind:
                            SemanticErrorKind::Dimension(DimensionError::UnknownDimension {
                                name, ..
                            }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), expected, "{body}");
            }
            other => panic!("expected UnknownDimension for {expected} in {body}, got {other:?}"),
        }
    }
    for body in [
        "include target()::{ unit double_metre as dm };\n\
         const unit quad_metre: Length = 2.0 double_metre;",
        "include target()::{ unit double_metre as dm };\n\
         unit quad_metre: Length = 2.0 double_metre;",
    ] {
        match compile_and_eval(&format!("{target}{body}")) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { name, .. }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), "double_metre", "{body}");
            }
            other => panic!("expected UnknownUnit for double_metre in {body}, got {other:?}"),
        }
    }
}

#[test]
fn selective_include_projects_specialized_adt_constructors() {
    let source = "dag target {\n\
                      pub(bind) dim Quantity;\n\
                      param amount: Quantity;\n\
                      pub type Reading { Missing, Present(value: Quantity) }\n\
                      pub node supplied: Reading = Present(value: @amount);\n\
                  }\n\
                  include target(amount: 2.0 m, dim Quantity: Length)::{\n\
                      type Reading,\n\
                      Missing as NoReading,\n\
                      Present as HasReading,\n\
                      supplied,\n\
                  };\n\
                  node local: Reading = HasReading(value: 2.0 m);\n\
                  node reading: Reading = @supplied;\n\
                  node output: Length = match @reading {\n\
                      HasReading(value: amount) => amount,\n\
                      NoReading => 0.0 m,\n\
                  };";
    let result = compile_and_eval(source)
        .expect("constructors retain the specialized nominal type identity");
    assert_quantity_value(&result, "output", 2.0);
}

const DERIVED_FROM_BINDABLE_DIM_DAG: &str = "dag blib {\n\
                                                   pub(bind) dim Q;\n\
                                                   pub dim QR = Q / Time;\n\
                                                   param q: Q;\n\
                                                   pub node rate: QR = @q / 2.0 s;\n\
                                               }\n";

fn expect_annotation_mismatch(source: &str) {
    match compile_and_eval(source) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected DimensionMismatchInAnnotation for\n{source}\ngot {other:?}"),
    }
}

#[test]
fn derived_dimension_over_bindable_dimension_is_accepted() {
    compile_and_eval(DERIVED_FROM_BINDABLE_DIM_DAG)
        .expect("a derived dimension may be defined over a required bindable dimension");

    let with_default = "dag blib {\n\
                            pub(bind) dim Q = Length;\n\
                            pub dim QR = Q / Time;\n\
                            param q: QR = 1.5 m / s;\n\
                            pub node rate: QR = @q;\n\
                        }\n\
                        include blib()::{rate, dim QR};\n\
                        node x: QR = @rate;";
    let result = compile_and_eval(with_default)
        .expect("a derived dimension over a defaulted bindable dimension is accepted");
    assert_quantity_value(&result, "x", 1.5);
}

#[test]
fn derived_dimension_over_bindable_dimension_follows_the_include_binding() {
    for include in [
        "include blib(dim Q: Length, q: 4.0 m)::{dim QR, rate};",
        "include blib(dim Q: Length, q: 4.0 m)::{rate};",
    ] {
        let source = format!(
            "{DERIVED_FROM_BINDABLE_DIM_DAG}{include}\n\
             node y: Length / Time = @rate;"
        );
        let result = compile_and_eval(&source)
            .unwrap_or_else(|error| panic!("`{include}` was rejected: {error:?}"));
        assert_quantity_value(&result, "y", 2.0);
    }

    for (projection, local) in [("dim QR", "QR"), ("dim QR as Speed", "Speed")] {
        let prefix = format!(
            "{DERIVED_FROM_BINDABLE_DIM_DAG}\
             include blib(dim Q: Length, q: 4.0 m)::{{{projection}}};\n"
        );
        let result = compile_and_eval(&format!("{prefix}node x: {local} = 1.0 m/s;"))
            .unwrap_or_else(|error| panic!("`{projection}` was not specialized: {error:?}"));
        assert_quantity_value(&result, "x", 1.0);
        expect_annotation_mismatch(&format!("{prefix}node x: {local} = 1.0 m;"));
    }
}

#[test]
fn derived_dimension_projection_from_file_module_follows_the_include_binding() {
    let library = "pub(bind) dim Q;\n\
                   pub dim QR = Q / Time;\n\
                   param q: Q;\n\
                   pub node rate: QR = @q / 2.0 s;\n";
    for (main, expected) in [
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{dim QR, rate};\n\
             node x: QR = @rate;\n\
             node y: Mass / Time = @x;\n",
            Some(2.0),
        ),
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{dim QR};\n\
             node y: QR = 1.0 m/s;\n",
            None,
        ),
    ] {
        let (_directory, root) =
            write_pipeline_project(&[("lib.gcl", library), ("main.gcl", main)], "main.gcl");
        let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
        match (result, expected) {
            (Ok(result), Some(value)) => assert_quantity_value(&result, "y", value),
            (
                Err(CompileError::Eval(RenderedSemanticError {
                    error:
                        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                            kind:
                                SemanticErrorKind::Dimension(
                                    DimensionError::DimensionMismatchInAnnotation { .. },
                                ),
                            ..
                        }),
                    ..
                })),
                None,
            ) => {}
            (other, _) => panic!("unexpected result for\n{main}\n{other:?}"),
        }
    }
}

/// A template whose bindable dimension port has a default, with a derived
/// dimension over it.
const DEFAULTED_BINDABLE_DIM_LIBRARY: &str = "pub(bind) dim Q = Length;\n\
                                              pub dim QR = Q / Time;\n\
                                              param q: Q;\n\
                                              pub node doubled: Q = @q * 2.0;\n\
                                              pub node rate: QR = @q / 2.0 s;\n";

#[test]
fn defaulted_bindable_dimension_follows_the_include_binding_in_inline_dags() {
    let library = format!("dag blib {{\n{DEFAULTED_BINDABLE_DIM_LIBRARY}}}\n");
    for (include, output, value) in [
        // Rebinding the defaulted port retypes every use and derived dimension.
        (
            "include blib(dim Q: Mass, q: 4.0 kg)::{doubled, rate};",
            "node x: Mass = @doubled;\nnode y: Mass / Time = @rate;",
            (8.0, 2.0),
        ),
        // Without a binding the default applies.
        (
            "include blib(q: 4.0 m)::{doubled, rate};",
            "node x: Length = @doubled;\nnode y: Length / Time = @rate;",
            (8.0, 2.0),
        ),
    ] {
        let source = format!("{library}{include}\n{output}");
        let result = compile_and_eval(&source)
            .unwrap_or_else(|error| panic!("`{include}` was rejected: {error:?}"));
        assert_quantity_value(&result, "x", value.0);
        assert_quantity_value(&result, "y", value.1);
    }
    // The rebound port rejects a value of the default dimension.
    assert!(
        compile_and_eval(&format!(
            "{library}include blib(dim Q: Mass, q: 4.0 m)::{{doubled}};\nnode x: Mass = @doubled;"
        ))
        .is_err()
    );
    // A projected derived dimension follows the rebinding too.
    let prefix = format!("{library}include blib(dim Q: Mass, q: 4.0 kg)::{{dim QR}};\n");
    let result = compile_and_eval(&format!("{prefix}node x: QR = 1.0 kg/s;"))
        .expect("the projected QR is Mass / Time");
    assert_quantity_value(&result, "x", 1.0);
    expect_annotation_mismatch(&format!("{prefix}node x: QR = 1.0 m/s;"));
}

#[test]
fn defaulted_bindable_dimension_follows_the_include_binding_in_file_modules() {
    for (main, expected) in [
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{doubled, rate};\n\
             node x: Mass = @doubled;\n\
             node y: Mass / Time = @rate;\n",
            Some((8.0, 2.0)),
        ),
        (
            "include pipeline.lib(q: 4.0 m)::{doubled, rate};\n\
             node x: Length = @doubled;\n\
             node y: Length / Time = @rate;\n",
            Some((8.0, 2.0)),
        ),
        (
            "include pipeline.lib(dim Q: Mass, q: 4.0 kg)::{doubled};\n\
             node x: Length = @doubled;\n",
            None,
        ),
    ] {
        let (_directory, root) = write_pipeline_project(
            &[
                ("lib.gcl", DEFAULTED_BINDABLE_DIM_LIBRARY),
                ("main.gcl", main),
            ],
            "main.gcl",
        );
        let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
        match (result, expected) {
            (Ok(result), Some((x, y))) => {
                assert_quantity_value(&result, "x", x);
                assert_quantity_value(&result, "y", y);
            }
            (
                Err(CompileError::Eval(RenderedSemanticError {
                    error:
                        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                            kind:
                                SemanticErrorKind::Dimension(
                                    DimensionError::DimensionMismatchInAnnotation { .. },
                                ),
                            ..
                        }),
                    ..
                })),
                None,
            ) => {}
            (other, _) => panic!("unexpected result for\n{main}\n{other:?}"),
        }
    }
}

#[test]
fn defaulted_bindable_dimension_rebinding_passes_through_nested_includes() {
    let source = "dag inner {\n\
                      pub(bind) dim P = Length;\n\
                      param p: P;\n\
                      pub node twice: P = @p * 2.0;\n\
                  }\n\
                  dag wrap {\n\
                      pub(bind) dim Q = Length;\n\
                      param q: Q;\n\
                      include inner(dim P: Q, p: @q)::{twice};\n\
                      pub node out: Q = @twice;\n\
                  }\n\
                  include wrap(dim Q: Mass, q: 3.0 kg)::{out};\n\
                  node r: Mass = @out;";
    let result = compile_and_eval(source).expect("the rebinding reaches the nested include");
    assert_quantity_value(&result, "r", 6.0);
}

#[test]
fn rebound_defaulted_dimension_port_types_projected_plot_channels() {
    let source = "dag lib {\n\
                      pub(bind) dim Q = Length;\n\
                      pub index I = { A, B };\n\
                      param q: Q;\n\
                      pub node xs: Q[I] = for i: I { @q };\n\
                      pub node ys: Dimensionless[I] = for i: I { 1.0 };\n\
                      pub plot p = { mark: line, encode: { x: @xs, y: @ys } };\n\
                  }\n\
                  include lib(dim Q: Mass, q: 3.0 kg)::{p};";
    let result = compile_and_eval(source).expect("the projected plot checks");
    let [plot] = result.plots.as_slice() else {
        panic!("expected one projected plot, got {:?}", result.plots);
    };
    let x = plot
        .encoding_meta
        .iter()
        .find(|(channel, _)| *channel == graphcal_compiler::syntax::ast::EncodingChannel::X)
        .map(|(_, metadata)| metadata);
    assert_eq!(
        x.and_then(|metadata| metadata.dimension_label.as_deref()),
        Some("Mass")
    );
}

#[test]
fn projected_type_fields_follow_bound_ports_through_unprojected_derived_dimensions() {
    // `QR` is not projected, yet the projected `Rate` names it in a field:
    // the field follows the include's binding of the port `QR` is defined over.
    for (port, binding, accepted, rejected) in [
        ("pub(bind) dim Q;", "dim Q: Length", "1.0 m/s", "1.0 kg/s"),
        (
            "pub(bind) dim Q = Length;",
            "dim Q: Mass",
            "1.0 kg/s",
            "1.0 m/s",
        ),
    ] {
        let program = |value: &str| {
            format!(
                "dag blib {{\n\
                     {port}\n\
                     pub dim QR = Q / Time;\n\
                     pub type Rate {{ Rate(v: QR) }}\n\
                 }}\n\
                 include blib({binding})::{{type Rate, Rate}};\n\
                 node r: Rate = Rate(v: {value});"
            )
        };
        compile_and_eval(&program(accepted))
            .unwrap_or_else(|error| panic!("`{port}` with `{binding}` rejected: {error:?}"));
        assert!(
            compile_and_eval(&program(rejected)).is_err(),
            "`{port}` with `{binding}` accepted {rejected}"
        );
    }
}

#[test]
fn template_bodies_may_use_derived_dimensions_over_defaulted_ports() {
    // A body typed by a derived dimension over a defaulted port does not
    // observe the port's default (no V007), whether or not it is bound.
    let library = format!("dag blib {{\n{DEFAULTED_BINDABLE_DIM_LIBRARY}}}\n");
    let source =
        format!("{library}include blib(q: 4.0 m)::{{rate}};\nnode y: Length / Time = @rate;");
    let result = compile_and_eval(&source).expect("no false V007 for a derived dimension");
    assert_quantity_value(&result, "y", 2.0);
}

#[test]
fn included_dag_dimensions_do_not_shadow_importer_dimensions() {
    let prefix = "dag blib {\n\
                      pub dim R = Length;\n\
                      param q: R;\n\
                      pub node rate: R = @q;\n\
                  }\n\
                  dim R = Mass;\n\
                  include blib(q: 4.0 m) as b;\n";
    let result = compile_and_eval(&format!("{prefix}node x: R = 1.0 kg;"))
        .expect("the importer's own dimension keeps its definition");
    assert_quantity_value(&result, "x", 1.0);
    expect_annotation_mismatch(&format!("{prefix}node x: R = 1.0 m;"));
}

#[test]
fn constructor_empty_parentheses_are_rejected() {
    for (source, expected_constructor) in [
        (
            "type Status { Nominal, Coast }\nnode status: Status = Coast();",
            "Coast",
        ),
        (
            "type Status { Nominal, Coast }\n\
             node status: Status = Coast;\n\
             node result: Dimensionless = match @status {\n\
                 Nominal => 0.0,\n\
                 Coast() => 1.0,\n\
             };",
            "Coast",
        ),
        (
            "type Reading { Missing, Present(value: Length) }\n\
             node reading: Reading = Present();",
            "Present",
        ),
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Struct(StructError::EmptyParenthesizedConstructor { constructor, .. }), .. }), .. }) if constructor.as_str() == expected_constructor
            ),
            "unexpected error: {error:?}"
        );
    }
}

#[test]
fn payload_constructor_empty_pattern_keeps_missing_field_diagnostic() {
    let source = "type Reading { Missing, Present(value: Length) }\n\
                  node reading: Reading = Present(value: 2.0 m);\n\
                  node output: Length = match @reading {\n\
                      Missing => 0.0 m,\n\
                      Present() => 1.0 m,\n\
                  };";
    let error = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Struct(StructError::MissingPatternFields { constructor, .. }), .. }), .. })
                if constructor.as_str() == "Present"
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn selective_include_rejects_dag_blueprint_projection() {
    let source = "dag target {\n\
                      pub dag child { pub node output: Dimensionless = 1.0; }\n\
                  }\n\
                  include target()::{ child };\n\
                  node output: Dimensionless = 1.0;";
    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::IncludeItemNotProjectable { name, .. }), .. }), .. }) if name.as_str() == "child"
    ));
}

#[test]
fn selective_include_rejects_constructor_of_rebound_owner_type() {
    let source = "type Replacement { Replacement }\n\
                  dag target { pub(bind) type Choice { Pick } }\n\
                  include target(type Choice: Replacement)::{ Pick };\n\
                  node output: Dimensionless = 1.0;";
    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::IncludeConstructorOwnerRebound { constructor, owner_type, .. }), .. }), .. }) if constructor.as_str() == "Pick" && owner_type.to_string().ends_with("Choice")
    ));
}

#[test]
fn selective_include_projects_structural_index_binding_target() {
    let source = "dag target { pub(bind) index Axis; }\n\
                  include target(index Axis: Fin(2))::{ index Axis as EffectiveAxis };\n\
                  node indexed: Dimensionless[EffectiveAxis] = for i: EffectiveAxis { 1.0 };";
    compile_and_eval(source).expect("the projected index has structural Fin(2) identity");
}

#[test]
fn structural_index_binding_cardinality_must_be_closed() {
    let program = |cardinality: &str| {
        format!(
            "dag target {{ pub(bind) index Axis; }}\n\
             include target(index Axis: Fin({cardinality}))::{{ index Axis as EffectiveAxis }};\n\
             node indexed: Dimensionless[EffectiveAxis] = for i: EffectiveAxis {{ 1.0 }};"
        )
    };
    compile_and_eval(&program("1 + 2 * 3")).expect("a closed cardinality binds");
    assert!(matches!(
        compile_and_eval(&program("N + 1")).unwrap_err(),
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::UnknownIndex { name, .. }), .. }), .. }) if name.to_string() == "N"
    ));
    assert!(matches!(
        compile_and_eval(&program("4294967296 * 4294967296")).unwrap_err(),
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(kind @ IndexError::NatOverflow { .. }), .. }), .. })
            if kind.to_string().contains("type-level Nat arithmetic overflow")
    ));
}

#[test]
fn include_rejects_required_static_inputs_as_binding_targets() {
    let source = "pub(bind) type Replacement;\n\
                  dag target { pub(bind) type Slot; }\n\
                  include target(type Slot: Replacement) as configured;\n\
                  node output: Dimensionless = 1.0;";
    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::InvalidStaticBindingTarget { kind: graphcal_compiler::static_interface::StaticInputKind::Type, name, target, .. }), .. }), .. }) if name.as_str() == "Slot" && target.as_str() == "Replacement"
    ));
}

#[test]
fn include_requires_every_required_static_input_category() {
    for (declaration, expected_kind) in [
        (
            "pub(bind) type Element;",
            graphcal_compiler::static_interface::StaticInputKind::Type,
        ),
        (
            "pub(bind) dim Quantity;",
            graphcal_compiler::static_interface::StaticInputKind::Dimension,
        ),
        (
            "pub(bind) index Axis;",
            graphcal_compiler::static_interface::StaticInputKind::Index,
        ),
    ] {
        let (_directory, root) = write_pipeline_project(
            &[
                ("lib.gcl", declaration),
                (
                    "main.gcl",
                    "include pipeline.lib() as configured;\nnode output: Dimensionless = 1.0;",
                ),
            ],
            "main.gcl",
        );
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::RequiredStaticInputNotBound { kind, .. }), .. }), .. })
                if kind == expected_kind
        ));
    }
}

#[test]
fn fixed_static_declarations_are_not_binding_ports() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub type Element { Element }"),
            (
                "main.gcl",
                "type LocalElement { LocalElement }\n\
                 include pipeline.lib(type Element: LocalElement) as configured;\n\
                 node output: Dimensionless = 1.0;",
            ),
        ],
        "main.gcl",
    );
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::DagInputCategoryMismatch {
                    expected: "type",
                    ..
                }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn bare_graph_declaration_refs_fail_before_dependency_planning() {
    let cases = [
        (
            "later runtime node",
            "node b_ref: Dimensionless = z_target * 3.0;\n\
             node z_target: Dimensionless = 2.0;",
            "z_target",
        ),
        (
            "earlier runtime node",
            "node a: Dimensionless = 2.0;\n\
             node b: Dimensionless = a * 3.0;",
            "a",
        ),
        (
            "parameter",
            "param input: Dimensionless = 2.0;\n\
             node output: Dimensionless = input;",
            "input",
        ),
        (
            "const node",
            "const node factor: Dimensionless = 2.0;\n\
             node output: Dimensionless = factor;",
            "factor",
        ),
        (
            "mixed bare and graph-reference cycle",
            "node a: Dimensionless = @b + 1.0;\n\
             node b: Dimensionless = a + 1.0;",
            "a",
        ),
    ];

    for (case, source, expected_name) in cases {
        let error = compile_and_eval_named(source, "test.gcl").unwrap_err();
        assert!(
            matches!(
                error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BareGraphDeclarationRef { name, .. }), .. }), .. })
                    if name.to_string() == expected_name
            ),
            "case: {case}"
        );
    }
}

#[test]
fn safety_attributes_reject_repetition_and_invalid_assumptions() {
    let repeated_expected_fail = "#[expected_fail]\n\
                                  #[expected_fail]\n\
                                  assert check = false;";
    let error = compile_and_eval_named(repeated_expected_fail, "test.gcl").unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::ExpectedFail,
                    ..
                }),
                ..
            }),
            ..
        })
    ));

    let invalid_assumes = [
        "assert guard = true;\n#[assumes]\nnode output: Dimensionless = 1.0;",
        "assert guard = true;\n#[assumes(guard, guard)]\nnode output: Dimensionless = 1.0;",
        "assert premise_a = true;\nassert premise_b = true;\n#[assumes(premise_a)]\n#[assumes(premise_b)]\nnode output: Dimensionless = 1.0;",
    ];
    for source in invalid_assumes {
        assert!(compile_and_eval_named(source, "test.gcl").is_err());
    }

    let valid = "assert premise_a = true;\n\
                 assert premise_b = true;\n\
                 #[assumes(premise_b, premise_a)]\n\
                 node output: Dimensionless = 1.0;";
    assert!(compile_and_eval_named(valid, "test.gcl").is_ok());
}

#[test]
fn repeated_expected_fail_is_rejected_on_file_and_inline_include_items() {
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub assert check = false;\n"),
            (
                "main.gcl",
                "include pipeline.lib()::{\n    #[expected_fail]\n    #[expected_fail]\n    check,\n};\n",
            ),
        ],
        "main.gcl",
    );
    let file_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        file_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::ExpectedFail,
                    ..
                }),
                ..
            }),
            ..
        })
    ));

    let inline = "dag checks { pub assert check = false; }\n\
                  include checks()::{\n\
                      #[expected_fail]\n\
                      #[expected_fail]\n\
                      check,\n\
                  };";
    let inline_error = compile_and_eval_named(inline, "test.gcl").unwrap_err();
    assert!(matches!(
        inline_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::ExpectedFail,
                    ..
                }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn repeated_hidden_is_rejected_on_plots_and_include_items() {
    let declaration = "node x: Dimensionless = 1.0;\n\
                       #[hidden]\n\
                       #[hidden]\n\
                       plot chart = { mark: point, encode: { x: @x } };";
    let declaration_error = compile_and_eval_named(declaration, "test.gcl").unwrap_err();
    assert!(matches!(
        declaration_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::Hidden,
                    ..
                }),
                ..
            }),
            ..
        })
    ));

    let producer = "pub node x: Dimensionless = 1.0;\n\
                    pub plot chart = { mark: point, encode: { x: @x } };\n";
    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", producer),
            (
                "main.gcl",
                "include pipeline.lib()::{ #[hidden] #[hidden] chart };\n",
            ),
        ],
        "main.gcl",
    );
    let include_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        include_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::RepeatedSingletonAttribute {
                    name: AttributeName::Hidden,
                    ..
                }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn time_scale_spellings_are_disjoint_from_graph_value_namespaces() {
    for scale in graphcal_compiler::semantic::time_scale::TimeScale::ALL {
        for declaration in [
            format!("param {scale}: Dimensionless = 1.0;"),
            format!("node {scale}: Dimensionless = 1.0;"),
            format!("const node {scale}: Dimensionless = 1.0;"),
        ] {
            let source = format!(
                "{declaration}\n\
                 node copied: Dimensionless = @{scale};\n\
                 node event: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");"
            );
            compile_and_eval_named(&source, "test.gcl").unwrap();
        }

        let aliased = format!(
            "dag producer {{ pub node value: Dimensionless = 1.0; }}\n\
             include producer()::{{ value as {scale} }};\n\
             node copied: Dimensionless = @{scale};\n\
             node event: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");"
        );
        compile_and_eval_named(&aliased, "test.gcl").unwrap();

        let constructor = format!(
            "type ScaleTag {{ {scale} }}\n\
             node tag: ScaleTag = {scale};\n\
             node event: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");"
        );
        compile_and_eval_named(&constructor, "test.gcl").unwrap();
    }

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub type ScaleTag { Only }\n"),
            (
                "main.gcl",
                "import pipeline.lib::{ type ScaleTag, Only as UTC };\n\
                 node tag: ScaleTag = UTC;\n\
                 node event: Datetime<UTC> = epoch<UTC>(\"2024-01-01T00:00:00\");\n",
            ),
        ],
        "main.gcl",
    );
    compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let bare_error = compile_and_eval_named(
        "node UTC: Dimensionless = 1.0;\nnode invalid: Dimensionless = UTC;",
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        bare_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BareGraphDeclarationRef { name, .. }), .. }), .. })
            if name.to_string() == "UTC"
    ));
}

#[test]
fn reserved_name_policy_covers_import_include_and_reexport_aliases() {
    let library = "pub dim CustomDim = Length;\n\
                   pub const unit custom_unit: Length = 2.0 m;\n\
                   pub type CustomType { CustomType }\n\
                   pub type Choice { Only }\n\
                   pub index CustomIndex = { One };\n\
                   pub const node constant: Dimensionless = 1.0;\n\
                   pub node runtime: Dimensionless = 2.0;\n";
    let invalid_imports = [
        ("dim CustomDim as Velocity", "Velocity"),
        ("unit custom_unit as m", "m"),
        ("type CustomType as Bool", "Bool"),
        ("index CustomIndex as Length", "Length"),
        ("constant as E", "E"),
        ("Only as E", "E"),
        ("Only as sum", "sum"),
        ("Only as scan", "scan"),
    ];

    for (item, expected_name) in invalid_imports {
        let main = format!("import pipeline.lib::{{ {item} }};\n");
        let (_directory, root) =
            write_pipeline_project(&[("lib.gcl", library), ("main.gcl", &main)], "main.gcl");
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. })
                if name.to_string() == expected_name
        ));
    }

    let invalid_includes = [
        "include pipeline.lib()::{ runtime as E };\n",
        "include pipeline.lib()::{ type Choice, Only as E };\n",
        "dag producer { pub node value: Dimensionless = 1.0; }\ninclude producer()::{ value as E };\n",
        "dag producer { pub type Choice { Only } }\ninclude producer()::{ type Choice, Only as E };\n",
    ];
    for main in invalid_includes {
        let error = if main.starts_with("dag") {
            compile_and_eval_named(main, "test.gcl").unwrap_err()
        } else {
            let (_directory, root) =
                write_pipeline_project(&[("lib.gcl", library), ("main.gcl", main)], "main.gcl");
            compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err()
        };
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "E"
        ));
    }

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub const node value: Dimensionless = 1.0;\n"),
            ("middle.gcl", "import pipeline.lib::{ pub value as E };\n"),
            ("main.gcl", "import pipeline.middle as middle;\n"),
        ],
        "main.gcl",
    );
    let reexport_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        reexport_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "E"
    ));

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub type Choice { Only }\n"),
            ("middle.gcl", "import pipeline.lib::{ pub Only as sum };\n"),
            ("main.gcl", "import pipeline.middle as middle;\n"),
        ],
        "main.gcl",
    );
    let constructor_reexport_error =
        compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        constructor_reexport_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "sum"
    ));

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub type Choice { Only }\n\
                 pub dag consumer {\n\
                     import pipeline.lib::{ Only as E };\n\
                     pub node output: Dimensionless = 1.0;\n\
                 }\n",
            ),
            ("main.gcl", "include pipeline.lib.consumer()::{ output };\n"),
        ],
        "main.gcl",
    );
    let self_import_error =
        compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        self_import_error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::BuiltinNameShadowed { name, .. }), .. }), .. }) if name.to_string() == "E"
    ));
}

#[test]
fn repeated_include_producers_are_rejected_before_metadata_maps() {
    let producer = "pub node x: Dimensionless = 1.0;\n\
                    pub assert okay = @x == 2.0;\n\
                    pub plot chart = { mark: point, encode: { x: @x } };\n";
    let file_selectors = [
        "okay as first_check, okay as second_check",
        "#[expected_fail] okay as first_check, #[expected_fail] okay as second_check",
        "#[hidden] chart as first_chart, chart as second_chart",
        "x as first_value, x as second_value",
    ];
    for selectors in file_selectors {
        let main = format!(
            "include pipeline.lib()::{{ {selectors} }};\n\
             #[assumes(first_check)]\n\
             node dependent: Dimensionless = 1.0;\n"
        );
        let (_directory, root) =
            write_pipeline_project(&[("lib.gcl", producer), ("main.gcl", &main)], "main.gcl");
        let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::DuplicateIncludeSelection { .. }),
                    ..
                }),
                ..
            })
        ));
    }

    for selectors in [
        "okay as first_check, okay as second_check",
        "#[hidden] chart as first_chart, chart as second_chart",
    ] {
        let source =
            format!("dag producer {{ {producer} }}\ninclude producer()::{{ {selectors} }};");
        let error = compile_and_eval_named(&source, "test.gcl").unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::DuplicateIncludeSelection { .. }),
                    ..
                }),
                ..
            })
        ));
    }
}

#[test]
fn unique_include_producers_remain_order_independent() {
    let producer = "dag producer {\n\
                        pub node output_a: Dimensionless = 1.0;\n\
                        pub node output_b: Dimensionless = 2.0;\n\
                    }\n";
    for selectors in [
        "output_a as x, output_b as y",
        "output_b as y, output_a as x",
    ] {
        let source = format!(
            "{producer}\n\
             include producer()::{{ {selectors} }};\n\
             node total: Dimensionless = @x + @y;"
        );
        compile_and_eval_named(&source, "test.gcl").unwrap();
    }
}

#[test]
fn lazy_attribute_is_rejected_on_declarations_and_include_items() {
    for source in [
        "#[lazy]\nnode output: Dimensionless = 1.0;",
        "#[lazy(guard)]\nnode output: Dimensionless = 1.0;",
        "#[lazy]\nassert output = true;",
        "#[lazy(guard)]\nassert output = true;",
    ] {
        let error = compile_and_eval_named(source, "test.gcl").unwrap_err();
        assert!(matches!(
            error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Attribute(AttributeError::LazyNotSupported),
                    ..
                }),
                ..
            })
        ));
    }

    let (_directory, root) = write_pipeline_project(
        &[
            ("lib.gcl", "pub node value: Dimensionless = 1.0;\n"),
            ("main.gcl", "include pipeline.lib()::{ #[lazy] value };\n"),
        ],
        "main.gcl",
    );
    let file_error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        file_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::LazyNotSupported),
                ..
            }),
            ..
        })
    ));

    let inline_error = compile_and_eval_named(
        "dag producer { pub node value: Dimensionless = 1.0; }\n\
         include producer()::{ #[lazy(guard)] value };",
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        inline_error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Attribute(AttributeError::LazyNotSupported),
                ..
            }),
            ..
        })
    ));
}

fn assert_missing_dag_bindings(
    error: CompileError,
    expected_dag_name: &str,
    expected_missing: &[&str],
) {
    match error {
        CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Graph(GraphError::MissingDagBindings {
                            missing,
                            dag_name,
                            ..
                        }),
                    ..
                }),
            ..
        }) => {
            assert_eq!(dag_name.to_string(), expected_dag_name);
            assert_eq!(
                missing.iter().map(ToString::to_string).collect::<Vec<_>>(),
                expected_missing
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            );
        }
        other => panic!("expected MissingDagBindings, got {other:?}"),
    }
}

#[test]
fn prepared_interface_contains_only_direct_hir_source_declarations() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param nested_input: Dimensionless = 2.0;\n\
                 pub node nested_output: Dimensionless = @nested_input;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as nested;\n\
                 param direct_input: Dimensionless = 3.0;\n\
                 node private_output: Dimensionless = @direct_input;\n\
                 pub node public_output: Dimensionless = @private_output;\n",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();

    let [parameter] = prepared.parameter_ports() else {
        panic!("expected exactly one direct entry parameter");
    };
    assert_eq!(parameter.name().as_str(), "direct_input");
    assert!(parameter.has_default());

    let outputs = prepared.output_ports();
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.name().as_str())
            .collect::<Vec<_>>(),
        ["private_output", "public_output"]
    );
    assert!(!outputs[0].is_public());
    assert!(outputs[1].is_public());
}

#[test]
fn empty_and_default_equivalent_includes_share_instance_semantics() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param input: Dimensionless = 2.0;\npub node output: Dimensionless = @input * 3.0;\nassert positive = @output > 0.0;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as defaults;\ninclude pipeline.lib(input: 2.0) as configured;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "defaults::output", 6.0);
    assert_quantity_value(&result, "configured::output", 6.0);
    assert_eq!(
        result
            .assertions
            .iter()
            .map(|(name, outcome, _)| (name.to_string(), outcome))
            .collect::<Vec<_>>(),
        [
            ("defaults::positive".to_string(), &AssertResult::Pass),
            ("configured::positive".to_string(), &AssertResult::Pass),
        ]
    );
}

#[test]
fn repeated_empty_includes_keep_distinct_output_names() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param input: Dimensionless = 2.0;\npub node output: Dimensionless = @input;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as first;\ninclude pipeline.lib() as second;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "first::output", 2.0);
    assert_quantity_value(&result, "second::output", 2.0);
}

#[test]
fn check_rejects_all_unbound_required_params_in_same_file_include() {
    let error = compile_to_tir(
        "dag scale {\n\
         param factor: Dimensionless;\n\
         param value: Dimensionless;\n\
         pub node out: Dimensionless = @factor * @value;\n\
         }\n\
         include scale()::{ out };\n",
        "test.gcl",
    )
    .expect_err("an include must bind every required param");

    assert_missing_dag_bindings(error, "scale", &["factor", "value"]);
}

#[test]
fn check_rejects_partially_bound_required_params_in_same_file_include() {
    let error = compile_to_tir(
        "dag scale {\n\
         param factor: Dimensionless;\n\
         param value: Dimensionless;\n\
         pub node out: Dimensionless = @factor * @value;\n\
         }\n\
         include scale(value: 3.0)::{ out };\n",
        "test.gcl",
    )
    .expect_err("a partial include must report its remaining required params");

    assert_missing_dag_bindings(error, "scale", &["factor"]);
}

#[test]
fn check_rejects_unbound_required_params_in_cross_file_include() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param factor: Dimensionless;\n\
                 pub node out: Dimensionless = @factor * 2.0;\n",
            ),
            ("main.gcl", "include pipeline.lib()::{ out };\n"),
        ],
        "main.gcl",
    );

    let error = compile_to_tir_project(&root, None, &fs())
        .expect_err("a cross-file include must bind every required param");
    assert_missing_dag_bindings(error, "pipeline.lib", &["factor"]);
}

#[test]
fn nested_configured_includes_keep_instance_local_bindings_and_assertions() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input;\nassert positive = @output > 0.0;\n",
            ),
            (
                "middle.gcl",
                "param input: Dimensionless;\ninclude pipeline.leaf(input: @input) as leaf;\npub node output: Dimensionless = @leaf::output;\n",
            ),
            (
                "main.gcl",
                "include pipeline.middle(input: 2.0) as first;\ninclude pipeline.middle(input: 5.0) as second;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "first::output", 2.0);
    assert_quantity_value(&result, "second::output", 5.0);
    assert_eq!(
        result
            .assertions
            .iter()
            .filter(|(_, outcome, _)| *outcome == AssertResult::Pass)
            .count(),
        2
    );
}

#[test]
fn pure_imports_reject_assertion_outcomes_and_runtime_unit_scales() {
    let (_directory, assertion_root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param rate: Dimensionless = 2.0;\npub assert rate_positive = @rate > 0.0;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib::{ rate_positive };\n#[assumes(rate_positive)]\nnode checked: Bool = true;\n",
            ),
        ],
        "main.gcl",
    );
    let assertion_error =
        compile_to_tir_project(&assertion_root, None, &fs()).expect_err("assert import must fail");
    assert!(
        matches!(
            assertion_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportAssertionItem { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected assertion-import error: {assertion_error:?}"
    );

    let (_directory, unit_root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\npub unit DoubleUSD: Money = 2.0 USD;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode price: lib::Money = 3.0 lib::EUR;\n",
            ),
        ],
        "main.gcl",
    );
    let unit_error =
        compile_to_tir_project(&unit_root, None, &fs()).expect_err("runtime unit import must fail");
    assert!(
        matches!(
            unit_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeUnit { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected runtime-unit import error: {unit_error:?}"
    );

    std::fs::write(
        &unit_root,
        "import pipeline.lib as lib;\nconst unit LocalEUR: lib::Money = 1.0 lib::EUR;\n",
    )
    .unwrap();
    let unit_definition_error = compile_to_tir_project(&unit_root, None, &fs())
        .expect_err("runtime unit in a static unit definition must fail");
    assert!(
        matches!(
            unit_definition_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeUnit { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected unit-definition import error: {unit_definition_error:?}"
    );

    std::fs::write(
        &unit_root,
        "import pipeline.lib::{ unit DoubleUSD };\nnode price: Money = 1.0 DoubleUSD;\n",
    )
    .unwrap();
    let constant_scale_runtime_error = compile_to_tir_project(&unit_root, None, &fs())
        .expect_err("plain units remain runtime even with a constant scale expression");
    assert!(
        matches!(
            constant_scale_runtime_error,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeUnit { .. }),
                    ..
                }),
                ..
            })
        ),
        "unexpected constant-scale runtime-unit error: {constant_scale_runtime_error:?}"
    );
}

#[test]
fn pure_module_import_can_call_an_instance_with_dynamic_units() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\nparam amount: Money = 100.0 EUR;\npub node converted: Money = @amount -> USD;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode result: lib::Money = @lib()::converted;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "result", 200.0);
}

#[test]
fn repeated_dag_calls_keep_dynamic_unit_scales_instance_scoped() {
    // Regression: expression evaluation must retain each concrete DAG scope;
    // resolving these scales against the root DAG would either fail or conflate calls.
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Money;\npub base unit USD: Money;\nparam rate: Dimensionless = 2.0;\npub unit EUR: Money = (@rate) USD;\nparam amount: Money = 100.0 EUR;\npub node converted: Money = @amount -> USD;\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode low: lib::Money = @lib(rate: 1.5)::converted;\nnode high: lib::Money = @lib(rate: 3.0)::converted;\n",
            ),
        ],
        "main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "low", 150.0);
    assert_quantity_value(&result, "high", 300.0);
}

#[test]
fn shared_modules_keep_equal_static_instances_and_dynamic_units_independent() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub(bind) dim Measure; param measured: Measure; pub node measurement: Measure = @measured; param scale: Dimensionless; pub unit step: Length = (@scale) m; param amount: Length = 3.0 step; pub node out: Length = @amount -> step;",
            ),
            (
                "main.gcl",
                "include pipeline.lib(dim Measure: Length, measured: 1.0 m, scale: 1.0) as first; include pipeline.lib(dim Measure: Length, measured: 2.0 m, scale: 2.0) as second; node low: Length = @first::out; node high: Length = @second::out; node first_measurement: Length = @first::measurement; node second_measurement: Length = @second::measurement;",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let checked = ProjectCompiler::new(&project).check().unwrap();
    let instances = checked.tir().root().semantic_instances();
    assert_eq!(instances.len(), 2);
    assert_eq!(
        instances[0].instance.specialization(),
        instances[1].instance.specialization()
    );
    assert_ne!(instances[0].instance.id(), instances[1].instance.id());
    let prepared = ProjectCompiler::new(&project).prepare().unwrap();
    let row = prepared.binding_builder().finish().unwrap();
    let result = prepared.evaluate(&row).unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_quantity_value(&result, "low", 3.0);
    assert_quantity_value(&result, "high", 6.0);
    assert_quantity_value(&result, "first_measurement", 1.0);
    assert_quantity_value(&result, "second_measurement", 2.0);
    for (name, expected_scale) in [("low", 1.0), ("high", 2.0)] {
        let value = result
            .nodes()
            .find(|(key, _)| *key == &scoped_name(name))
            .unwrap()
            .1
            .as_ref()
            .unwrap();
        match value {
            Value::Quantity {
                display_unit: Some(unit),
                ..
            } => assert!((unit.scale.get() - expected_scale).abs() < f64::EPSILON),
            other => panic!("expected instance-owned display unit, got {other:?}"),
        }
    }
}

/// A selective include item is materialized as a local alias in the
/// including DAG, and the include edge records that, so the exposed name
/// denotes the alias rather than the instance declaration; a whole-module
/// include member denotes the instance declaration.
#[test]
fn include_edges_record_which_exposed_values_have_local_alias_bodies() {
    use graphcal_compiler::ir::instance::ExposedValueBody;

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param factor: Dimensionless;\npub node output: Dimensionless = @factor * 2.0;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib(factor: 2.0)::{output as renamed};\ninclude pipeline.lib(factor: 5.0) as high;\nnode total: Dimensionless = @renamed + @high::output;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, _project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let body = tir.root().body_for_test();
    let bodies = body
        .semantic_instances()
        .iter()
        .flat_map(|record| {
            tir.dag_registry()
                .semantic_instance(record)
                .expect("materialized instance")
                .output_projections()
                .map(|resolved| {
                    (
                        record.instance.exposed_name(resolved.projection),
                        resolved.projection.body(),
                        resolved.target,
                    )
                })
        })
        .collect::<Vec<_>>();
    let renamed = scoped_name("renamed");
    let (_, renamed_body, _) = bodies
        .iter()
        .find(|(name, ..)| name == &renamed)
        .expect("the selected projection");
    assert_eq!(*renamed_body, ExposedValueBody::LocalAlias);
    let alias = body.bound_decl_identity(&renamed).unwrap();
    assert_eq!(alias.owner(), tir.root_dag_id());

    let member = member_name(&["high"], "output");
    let (_, member_body, member_target) = bodies
        .iter()
        .find(|(name, ..)| name == &member)
        .expect("the member projection");
    assert_eq!(*member_body, ExposedValueBody::Instance);
    assert_eq!(body.bound_decl_identity(&member), Some(member_target));

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_quantity_value(&result, "total", 14.0);
}

/// Unavailability reasons name declarations as the output does: instance
/// members by their scopes, a private include scope by its readable name,
/// and an inline DAG a call invokes by its scope below the root.
#[test]
fn unavailability_reasons_name_declarations_as_the_output_does() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "node pending: Dimensionless = todo {};\nnode bad: Dimensionless = 1.0 / 0.0;\npub node output: Dimensionless = @pending;\npub node broken: Dimensionless = @bad;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib() as l;\ninclude pipeline.lib()::{output as picked, broken as busted};\ndag helper {\n    node inner: Dimensionless = todo {};\n    pub node out: Dimensionless = @inner;\n}\nnode total: Dimensionless = @l::output + @picked;\nnode sad: Dimensionless = @l::broken + @busted;\nnode called: Dimensionless = @helper()::out;\nassert waits = @l::output > 0.0;\nassert fails = @sad > 0.0;\n",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let reason = |name: &str| {
        result
            .entries
            .iter()
            .find(|(candidate, _, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("missing `{name}`"))
            .1
            .as_ref()
            .expect_err("unavailable")
            .to_string()
    };
    assert_eq!(
        reason("total"),
        "BLOCKED — unfinished dependencies: lib::pending, l::pending"
    );
    assert_eq!(reason("sad"), "dependency failed: busted, l::broken");
    assert_eq!(reason("busted"), "dependency failed: lib::broken");
    assert_eq!(
        reason("l::output"),
        "BLOCKED — unfinished dependencies: l::pending"
    );
    assert_eq!(
        reason("called"),
        "BLOCKED — unfinished dependencies: helper::inner"
    );
    let assertion = |name: &str| {
        result
            .assertions
            .iter()
            .find(|(candidate, _, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("missing assertion `{name}`"))
            .1
            .clone()
    };
    let AssertResult::Blocked { reason } = assertion("waits") else {
        panic!("`waits` must be blocked");
    };
    assert_eq!(
        reason.to_string(),
        "BLOCKED — unfinished dependencies: l::pending"
    );
    let AssertResult::Error { message } = assertion("fails") else {
        panic!("`fails` must report its failed dependency");
    };
    assert_eq!(message, "dependency failed: sad");
}

#[test]
fn checked_tir_records_typed_template_instance_bindings() {
    use graphcal_compiler::resolved_name::ResolvedDeclName;

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "param factor: Dimensionless;\npub node output: Dimensionless = @factor * 2.0;\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib(factor: 2.0) as low;\ninclude pipeline.lib(factor: 5.0) as high;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let template = loaded_file_dag_id(&project, "lib.gcl");
    let template_param =
        ResolvedDeclName::for_test(template.clone(), DeclName::expect_valid("factor"));
    let records = tir
        .root()
        .semantic_instances()
        .iter()
        .filter(|record| record.instance.id().template() == &template)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_ne!(
        records[0].instance.id().owner(),
        records[1].instance.id().owner()
    );

    for record in records {
        let instance = &record.instance;
        assert_eq!(instance.id().parent(), tir.root_dag_id());
        assert!(record.value_bindings.contains_key(&template_param));
        let concrete = instance.value_port(&template_param).unwrap();
        assert_eq!(concrete.owner(), instance.id().owner());
        assert_eq!(concrete.as_str(), "factor");

        let output_name = ScopedName::in_scope(
            instance.id().scope().clone(),
            DeclName::expect_valid("output"),
        );
        let output = &tir.root().body_for_test().semantic().decl_bindings[&output_name];
        assert_eq!(output.owner(), instance.id().owner());

        // Projections are read resolved in the instance's own frame.
        let checked = tir
            .dag_registry()
            .semantic_instance(record)
            .expect("materialized instance");
        assert_eq!(checked.dag().dag_id(), instance.id().owner());
        let targets = checked
            .output_projections()
            .map(|projection| projection.target)
            .collect::<Vec<_>>();
        assert_eq!(targets, vec![concrete.clone(), output.clone()]);
    }
}

#[test]
fn nested_instances_retain_template_and_concrete_parent_identity() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\npub node output: Dimensionless = @input;\n",
            ),
            (
                "middle.gcl",
                "param input: Dimensionless;\ninclude pipeline.leaf(input: @input) as leaf;\npub node output: Dimensionless = @leaf::output;\n",
            ),
            (
                "main.gcl",
                "include pipeline.middle(input: 2.0) as first;\ninclude pipeline.middle(input: 5.0) as second;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let middle_template = loaded_file_dag_id(&project, "middle.gcl");
    let leaf_template = loaded_file_dag_id(&project, "leaf.gcl");
    let outer_owners = tir
        .root()
        .semantic_instances()
        .iter()
        .filter(|record| record.instance.id().template() == &middle_template)
        .map(|record| record.instance.id().owner().clone())
        .collect::<HashSet<_>>();
    let nested = tir
        .root()
        .semantic_instances()
        .iter()
        .flat_map(|outer| {
            tir.dag_registry()
                .get(outer.instance.id().owner())
                .expect("materialized outer instance")
                .semantic_instances()
                .iter()
                .map(|record| &record.instance)
        })
        .filter(|record| record.id().template() == &leaf_template)
        .collect::<Vec<_>>();

    assert_eq!(outer_owners.len(), 2);
    assert_eq!(nested.len(), 2);
    assert!(nested.iter().all(|record| {
        outer_owners.contains(record.id().parent())
            && record.id().owner().parent().as_ref() == Some(record.id().parent())
    }));
    assert_ne!(nested[0].id().owner(), nested[1].id().owner());
}

#[test]
fn project_type_store_keeps_imported_definitions_under_their_canonical_owner() {
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\npub index Axis = { A };\npub type Item { Item(value: Measure) }\n",
            ),
            (
                "main.gcl",
                "import pipeline.lib as lib;\nnode value: lib::Measure = 1.0 lib::u;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let dependency = loaded_file_dag_id(&project, "lib.gcl");
    let importer = tir.root_dag_id().clone();

    let dependency_dimension = graphcal_compiler::resolved_name::ResolvedDimName::for_test(
        dependency.clone(),
        graphcal_compiler::syntax::dimension::DimName::expect_valid("Measure"),
    );
    let importer_dimension = graphcal_compiler::resolved_name::ResolvedDimName::for_test(
        importer.clone(),
        graphcal_compiler::syntax::dimension::DimName::expect_valid("Measure"),
    );
    assert!(tir.dimension(&dependency_dimension).is_some());
    assert!(tir.dimension(&importer_dimension).is_none());

    let dependency_unit = graphcal_compiler::resolved_name::ResolvedUnitName::for_test(
        dependency.clone(),
        graphcal_compiler::syntax::dimension::UnitName::expect_valid("u"),
    );
    let importer_unit = graphcal_compiler::resolved_name::ResolvedUnitName::for_test(
        importer.clone(),
        graphcal_compiler::syntax::dimension::UnitName::expect_valid("u"),
    );
    assert!(tir.unit_info(&dependency_unit).is_some());
    assert!(tir.unit_info(&importer_unit).is_none());

    let dependency_index = graphcal_compiler::resolved_name::ResolvedIndexName::for_test(
        dependency.clone(),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    let importer_index = graphcal_compiler::resolved_name::ResolvedIndexName::for_test(
        importer.clone(),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    assert!(tir.declared_index_def(&dependency_index).is_some());
    assert!(tir.declared_index_def(&importer_index).is_none());

    let dependency_type = graphcal_compiler::resolved_name::ResolvedStructTypeName::for_test(
        dependency,
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Item"),
    );
    let importer_type = graphcal_compiler::resolved_name::ResolvedStructTypeName::for_test(
        importer,
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Item"),
    );
    assert!(tir.struct_type_def(&dependency_type).is_some());
    assert!(tir.struct_type_def(&importer_type).is_none());
}

#[test]
fn diamond_imports_install_one_canonical_shared_definition() {
    use graphcal_compiler::resolved_name::{ResolvedDimName, ResolvedUnitName};
    use graphcal_compiler::syntax::dimension::{DimName, UnitName};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "shared.gcl",
                "pub base dim SharedMeasure;\npub base unit su: SharedMeasure;\n",
            ),
            (
                "left.gcl",
                "import pipeline.shared as left_shared;\nconst node left_value: left_shared::SharedMeasure = 1.0 left_shared::su;\n",
            ),
            (
                "right.gcl",
                "import pipeline.shared as right_shared;\nconst node right_value: right_shared::SharedMeasure = 2.0 right_shared::su;\n",
            ),
            (
                "main.gcl",
                "import pipeline.left as left;\nimport pipeline.right as right;\nnode result: Dimensionless = 1.0;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let shared = loaded_file_dag_id(&project, "shared.gcl");
    let aliases = [
        loaded_file_dag_id(&project, "left.gcl"),
        loaded_file_dag_id(&project, "right.gcl"),
        tir.root_dag_id().clone(),
    ];
    let dimension = DimName::expect_valid("SharedMeasure");
    let unit = UnitName::expect_valid("su");

    assert!(
        tir.dimension(&ResolvedDimName::for_test(
            shared.clone(),
            dimension.clone()
        ))
        .is_some()
    );
    assert!(
        tir.unit_info(&ResolvedUnitName::for_test(shared, unit.clone()))
            .is_some()
    );
    for alias_owner in aliases {
        assert!(
            tir.dimension(&ResolvedDimName::for_test(
                alias_owner.clone(),
                dimension.clone(),
            ))
            .is_none()
        );
        assert!(
            tir.unit_info(&ResolvedUnitName::for_test(alias_owner, unit.clone()))
                .is_none()
        );
    }
}

#[test]
fn same_leaf_definitions_from_distinct_modules_keep_distinct_canonical_owners() {
    use graphcal_compiler::resolved_name::{ResolvedDimName, ResolvedUnitName};
    use graphcal_compiler::syntax::dimension::{DimName, UnitName};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "left.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\n",
            ),
            (
                "right.gcl",
                "pub base dim Measure;\npub base unit u: Measure;\n",
            ),
            (
                "main.gcl",
                "import pipeline.left as left;\nimport pipeline.right as right;\nnode left_value: left::Measure = 1.0 left::u;\nnode right_value: right::Measure = 2.0 right::u;\n",
            ),
        ],
        "main.gcl",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let left = loaded_file_dag_id(&project, "left.gcl");
    let right = loaded_file_dag_id(&project, "right.gcl");
    let root_owner = tir.root_dag_id().clone();
    let dimension = DimName::expect_valid("Measure");
    let unit = UnitName::expect_valid("u");

    for owner in [left, right] {
        assert!(
            tir.dimension(&ResolvedDimName::for_test(owner.clone(), dimension.clone()))
                .is_some()
        );
        assert!(
            tir.unit_info(&ResolvedUnitName::for_test(owner, unit.clone()))
                .is_some()
        );
    }
    assert!(
        tir.dimension(&ResolvedDimName::for_test(root_owner.clone(), dimension,))
            .is_none()
    );
    assert!(
        tir.unit_info(&ResolvedUnitName::for_test(root_owner, unit))
            .is_none()
    );
}

#[test]
fn checked_project_preparation_does_not_recompile_dependencies() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (_directory, root) = write_pipeline_project(
        &[
            (
                "lib.gcl",
                "import plugin \"graphcal:session-test\" as test {\n    fn count(x: Dimensionless) -> Dimensionless;\n}\npub node output: Dimensionless = test::count(2.0);\n",
            ),
            (
                "main.gcl",
                "include pipeline.lib()::{ output };\nnode result: Dimensionless = @output;\n",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let mut host_fns = graphcal_eval::host_fns::HostFunctionRegistry::new();
    host_fns.register_for_test(
        graphcal_compiler::syntax::plugin::PluginPath::new("graphcal:session-test"),
        graphcal_compiler::syntax::function_name::FnName::expect_valid("count"),
        move |arguments| {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            Ok(graphcal_eval::host_fns::HostFnValue::from_argument(
                &arguments[0],
            ))
        },
    );

    let checked = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .check()
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "static checking invoked a callable host function"
    );
    let prepared = checked.prepare_with_host_fns(&host_fns).unwrap();
    let row = prepared.binding_builder().finish().unwrap();
    let result = prepared.evaluate(&row).unwrap();
    assert_quantity_value(&result, "result", 2.0);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the explicit include should invoke its host function exactly once"
    );
}

#[test]
fn cancellation_is_observed_at_every_pipeline_checkpoint() {
    use std::collections::HashMap;

    let source = "node values: Dimensionless[Fin(32)] = \
                  for i: Fin(32) { 1.0 };";
    let project = crate::loader::LoadedProject::from_source(source, "cancel.gcl").unwrap();
    let first_success = (0..512).find(|successful_checkpoints| {
        let cancellation =
            graphcal_compiler::cancellation::CancellationToken::cancel_after_successful_checkpoints(
                *successful_checkpoints,
            );
        ProjectCompiler::new(&project)
            .cancellation(&cancellation)
            .eval(&HashMap::new())
            .is_ok()
    });
    let Some(checkpoint_count) = first_success else {
        panic!("bounded project did not complete within the checkpoint sweep");
    };
    assert!(
        checkpoint_count > 1,
        "pipeline must expose internal checkpoints"
    );

    for successful_checkpoints in 0..checkpoint_count {
        let cancellation =
            graphcal_compiler::cancellation::CancellationToken::cancel_after_successful_checkpoints(
                successful_checkpoints,
            );
        let error = ProjectCompiler::new(&project)
            .cancellation(&cancellation)
            .eval(&HashMap::new())
            .expect_err("every pre-completion cancellation point must unwind");
        assert!(
            matches!(error, Outcome::Cancelled),
            "checkpoint {successful_checkpoints} produced the wrong outcome: {error:?}"
        );
    }
}

#[test]
fn cancellation_stops_an_in_flight_evaluation() {
    use std::{
        collections::HashMap,
        sync::{Arc, Barrier, mpsc},
        thread,
        time::Duration,
    };

    let source = "node values: Dimensionless[Fin(1000000)] = \
                  for i: Fin(1000000) { 1.0 };";
    let project = crate::loader::LoadedProject::from_source(source, "cancel.gcl").unwrap();
    let cancellation_source = graphcal_compiler::cancellation::CancellationSource::new();
    let cancellation = cancellation_source.token();
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        worker_barrier.wait();
        let result = ProjectCompiler::new(&project)
            .cancellation(&cancellation)
            .eval(&HashMap::new());
        sender.send(result).unwrap();
    });

    barrier.wait();
    // Let the worker enter the compile/eval pipeline before requesting
    // cancellation. The exact stage is deliberately irrelevant: every stage
    // shares the same token and must unwind to this boundary.
    thread::sleep(Duration::from_millis(20));
    cancellation_source.cancel();

    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("cancelled evaluation should stop promptly");
    let error = result.expect_err("the long-running evaluation should be cancelled");
    assert!(
        matches!(error, Outcome::Cancelled),
        "unexpected outcome: {error:?}"
    );
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_rocket_milestone() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "dry_mass") - 1200.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "fuel_mass") - 2800.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "isp") - 320.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "g0") - 9.80665).abs() < 1e-10);

    let v_exhaust = find_value(&result, "v_exhaust");
    assert!(
        (v_exhaust - 320.0 * 9.80665).abs() < 0.001,
        "v_exhaust = {v_exhaust}"
    );

    let mass_ratio = find_value(&result, "mass_ratio");
    assert!(
        (mass_ratio - (4000.0 / 1200.0)).abs() < 1e-6,
        "mass_ratio = {mass_ratio}"
    );

    let delta_v = find_value(&result, "delta_v");
    let expected_delta_v = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
    assert!(
        (delta_v - expected_delta_v).abs() < 0.001,
        "delta_v = {delta_v}, expected = {expected_delta_v}"
    );
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_constants_ksr() {
    let source = include_str!("../../../tests/fixtures/valid/constants.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "g0") - 9.80665).abs() < f64::EPSILON);
    assert!((find_value(&result, "two_g0") - 19.6133).abs() < 1e-10);
    assert!((find_value(&result, "half_pi") - std::f64::consts::FRAC_PI_2).abs() < f64::EPSILON);
    assert!((find_value(&result, "sqrt2") - std::f64::consts::SQRT_2).abs() < f64::EPSILON);

    let circumference = find_value(&result, "circumference");
    let expected = 2.0 * std::f64::consts::PI * 100.0;
    assert!(
        (circumference - expected).abs() < 1e-10,
        "circumference = {circumference}"
    );

    let area = find_value(&result, "area");
    let expected_area = std::f64::consts::PI * 100.0_f64.powf(2.0);
    assert!((area - expected_area).abs() < 1e-10, "area = {area}");
}

#[test]
fn inline_dag_call_with_failing_assert_fails_calling_node() {
    // #812: inline invocation of an assert-carrying dag evaluates the dag's
    // asserts; a failure fails the calling expression (fault-isolated).
    let result = compile_and_eval(
        "dag checked {\n\
             param v: Dimensionless;\n\
             pub node out: Dimensionless = @v * 2.0;\n\
             assert v_positive = @v > 0.0;\n\
         }\n\
         node y: Dimensionless = @checked(v: -3.0)::out;\n\
         node independent: Dimensionless = 1.0;",
    )
    .unwrap();
    let node_result = |name: &str| {
        result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .clone()
    };
    match node_result("y") {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert_eq!(
                message,
                "assertion `v_positive` failed in inline call of dag `checked` \
                 (assertion evaluated to false)"
            );
        }
        other => panic!("expected eval failure for `y`, got {other:?}"),
    }
    assert!(
        node_result("independent").is_ok(),
        "independent node must not be affected"
    );
}

#[test]
fn inline_dag_call_with_passing_assert_succeeds() {
    let result = compile_and_eval(
        "dag checked {\n\
             param v: Dimensionless;\n\
             pub node out: Dimensionless = @v * 2.0;\n\
             assert v_positive = @v > 0.0;\n\
         }\n\
         node y: Dimensionless = @checked(v: 3.0)::out;",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 6.0).abs() < f64::EPSILON);
    // The dag's assert is internal to the instantiation — no spurious
    // top-level assertion report.
    assert!(result.assertions.is_empty());
}

#[test]
fn inline_dag_call_respects_expected_fail() {
    // An #[expected_fail] assert that fails inside the inline instantiation
    // is an expected failure → Pass → no error. One that passes unexpectedly
    // inverts to a failure → the calling node errors.
    let source_template = |v: &str| {
        format!(
            "dag checked {{\n\
                 param v: Dimensionless;\n\
                 pub node out: Dimensionless = @v * 2.0;\n\
                 #[expected_fail]\n\
                 assert is_neg = @v < 0.0;\n\
             }}\n\
             node y: Dimensionless = @checked(v: {v})::out;"
        )
    };

    let result = compile_and_eval(&source_template("3.0")).unwrap();
    assert!(
        result.nodes().next().unwrap().1.is_ok(),
        "expected failure occurred → no error: {:?}",
        result.nodes().next().unwrap().1
    );

    let result = compile_and_eval(&source_template("-3.0")).unwrap();
    match &result.nodes().next().unwrap().1 {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert!(
                message.contains("assertion passed but was marked #[expected_fail]"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected eval failure for unexpected pass, got {other:?}"),
    }
}

#[test]
fn assert_literal_negative_zero_tolerance_is_rejected() {
    let error = compile_and_eval(
        "param measured: Length = 1.0 m;\n\
         assert exact = @measured ~= 1.0 m +/- -0.0 m;",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Attribute(AttributeError::NegativeTolerance { value }), .. }), .. }) if value == 0.0 && value.is_sign_negative()
    ));
}

/// Helper: find a named value and return it (for indexed value tests).
fn find_entry(result: &EvalResult, name: &str) -> Value {
    result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .clone()
}

/// Helper: extract indexed entries as `Vec<(variant, si_value)>`.
fn indexed_si_values(value: &Value) -> Vec<(&str, f64)> {
    match value {
        Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(k, v)| {
                (
                    k.as_named()
                        .expect("helper is only used with named indexes")
                        .as_str(),
                    v.si_value().unwrap().get(),
                )
            })
            .collect(),
        _ => panic!("expected indexed value, got {value:?}"),
    }
}

#[test]
fn invalid_datetime_timezone_is_rejected_in_every_expression_owner() {
    let cases = [
        (
            "param",
            r#"param bad: Datetime = datetime("2024-11-05T10:00", "Not/A_Timezone");"#,
        ),
        (
            "node",
            r#"node bad: Datetime = datetime("2024-11-05T10:00", "Not/A_Timezone");"#,
        ),
        (
            "const node",
            r#"const node bad: Datetime = datetime("2024-11-05T10:00", "Not/A_Timezone");"#,
        ),
        (
            "nested",
            r#"node bad: Datetime = to_utc(datetime("2024-11-05T10:00", "Not/A_Timezone"));"#,
        ),
    ];

    for (context, source) in cases {
        let error = compile_to_tir(source, "test.gcl").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown timezone `Not/A_Timezone`"),
            "{context} did not reject the timezone during checking: {error}"
        );
    }
}

#[test]
fn datetime_constructors_reject_conflicting_or_missing_interpretation_sources() {
    let cases = [
        r#"node bad: Datetime = datetime("2024-11-05T12:00:00");"#,
        r#"node bad: Datetime = datetime("2024-11-05T12:00:00 TT");"#,
        r#"node bad: Datetime = datetime("2024-11-05T12:00:00Z", "UTC");"#,
        r#"node bad: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00Z");"#,
        r#"node bad: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00 TT");"#,
    ];
    for source in cases {
        let error = compile_to_tir(source, "test.gcl").unwrap_err();
        assert!(
            error.to_string().contains("invalid datetime literal"),
            "constructor contract was not checked: {error}"
        );
    }
}

#[test]
fn dst_gaps_and_folds_are_rejected_during_checking() {
    let gap = compile_to_tir(
        r#"const node gap: Datetime = datetime("2024-03-10T02:30:00", "America/New_York");"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        gap,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::NonexistentCivilDateTime { .. }),
                ..
            }),
            ..
        })
    ));

    let fold = compile_to_tir(
        r#"
node fold: Datetime = if true {
    datetime("2024-11-03T01:30:00", "America/New_York")
} else {
    datetime("2024-11-03T01:30:00Z")
};
"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        fold,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::RepeatedCivilDateTime { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn positional_epoch_scale_is_rejected_during_checking() {
    let error = compile_to_tir(
        r#"node bad: Datetime<TT> = epoch("2024-11-05T12:00:00", TT);"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("epoch requires exactly one static time-scale argument"),
        "unexpected error: {error}"
    );
}

#[test]
fn epoch_constructor_rejects_embedded_scale_suffix_during_checking() {
    let err = compile_to_tir(
        r#"node bad: Datetime<TT> = epoch<TT>("2000-01-01T12:00:00 TT");"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("invalid datetime literal"),
        "unexpected error: {err}"
    );
}

// --- Comparison and boolean operator tests ---

#[test]
fn eval_boolean_and_eagerly_evaluates_the_right_operand() {
    assert_node_error("node y: Bool = false && sqrt(-1.0) > 0.0;", "y", "NaN");
}

#[test]
fn eval_boolean_or_eagerly_evaluates_the_right_operand() {
    assert_node_error("node y: Bool = true || sqrt(-1.0) > 0.0;", "y", "NaN");
}

// --- Override tests ---

/// A closed value parsed from its own text, as a `--param` value is.
fn parse_expr(s: &str) -> ExternalValue {
    ExternalValue::parse("<--param>", s).unwrap()
}

/// A value expression synthesized without text of its own (JSON, editor).
fn parse_synthesized_expr(s: &str) -> graphcal_compiler::desugar::desugared_ast::Expr {
    parse_expr(s).expr().clone()
}

#[test]
fn override_param_changes_result() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    // Default isp=320 s, override to 450 s => higher delta_v
    let default = compile_and_eval_named(source, "test.gcl").unwrap();
    let default_dv = find_value(&default, "delta_v");

    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("isp"), parse_expr("450.0 s"));
    let overridden = compile_and_eval_with_overrides(source, "test.gcl", &overrides).unwrap();
    let new_dv = find_value(&overridden, "delta_v");

    assert!(new_dv > default_dv, "higher isp should give higher delta_v");
}

#[test]
fn override_with_wrong_dimension_errors() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    // isp expects Time, not Mass
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("isp"), parse_expr("450.0 kg"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    assert!(result.is_err());
}

#[test]
fn override_node_errors() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("delta_v"), parse_expr("100.0 m/s"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    match result {
        Err(CompileError::Binding(BindingError::NotAParam { name, actual_kind })) => {
            assert_eq!(name.as_str(), "delta_v");
            assert_eq!(actual_kind.to_string(), "node");
        }
        other => panic!("expected OverrideNotAParam, got {other:?}"),
    }
}

#[test]
fn override_const_errors() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("g0"), parse_expr("10.0 m/s^2"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    match result {
        Err(CompileError::Binding(BindingError::NotAParam { name, actual_kind })) => {
            assert_eq!(name.as_str(), "g0");
            assert_eq!(actual_kind.to_string(), "const");
        }
        other => panic!("expected OverrideNotAParam, got {other:?}"),
    }
}

#[test]
fn override_unknown_param_errors() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("nonexistent"), parse_expr("100"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    match result {
        Err(CompileError::Binding(BindingError::UnknownParam { name })) => {
            assert_eq!(name.as_str(), "nonexistent");
        }
        other => panic!("expected OverrideUnknownParam, got {other:?}"),
    }
}

#[test]
fn recursive_schema_graph_supports_finite_bindings_and_lazy_transport_rejection() {
    let source = r"
        pub type List {
            Nil,
            Cons(head: Int, tail: List),
        }
        param list: List;
        pub node echo: List = @list;
        pub node accepted: Bool = true;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "recursive.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let ModelValueSchema::Algebraic(list_id) = prepared.parameter_ports()[0].value_schema() else {
        panic!("List parameter must reference an algebraic definition");
    };
    let list_definition = prepared
        .schema_graph()
        .definition(list_id)
        .expect("List definition");
    let cons = &list_definition.constructors()[1];
    assert_eq!(cons.name().as_str(), "Cons");
    assert!(matches!(
        cons.fields()[1].value(),
        ModelValueSchema::Algebraic(tail_id) if tail_id == list_id
    ));
    assert!(
        prepared
            .schema_graph()
            .contains_recursive_type(prepared.parameter_ports()[0].value_schema())
    );

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(
            &DeclName::expect_valid("list"),
            &parse_expr("Cons(head: 1, tail: Cons(head: 2, tail: Nil))"),
        )
        .unwrap();
    let row = bindings.finish().unwrap();
    let model = prepared.model(&[DeclName::expect_valid("echo")]).unwrap();
    let ModelRowOutcome::Success(values) = prepared.evaluate_model_row(&row, &model).unwrap()
    else {
        panic!("finite recursive binding unexpectedly failed");
    };
    assert!(matches!(values.as_slice(), [Value::Struct { .. }]));

    let error = prepared
        .tenax_v2_model(&[DeclName::expect_valid("accepted")])
        .expect_err("Tenax v2 must reject recursion at its own boundary");
    assert!(matches!(
        error,
        ModelDefinitionError::RecursiveInputTypeUnsupported { name, .. }
            if name.as_str() == "list"
    ));
}

#[test]
fn mutually_recursive_model_definitions_form_a_finite_schema_graph() {
    let source = r"
        pub type Left {
            End,
            ToRight(value: Right),
        }
        pub type Right {
            ToLeft(value: Left),
        }
        pub node result: Left = End;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "mutual.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let ModelValueSchema::Algebraic(left_id) = prepared.output_ports()[0].value_schema() else {
        panic!("Left output must be algebraic");
    };
    assert_eq!(prepared.schema_graph().definitions().len(), 2);
    assert!(
        prepared
            .schema_graph()
            .contains_recursive_type(prepared.output_ports()[0].value_schema())
    );
    assert!(prepared.schema_graph().definition(left_id).is_some());
    assert!(
        prepared
            .evaluate(&prepared.binding_builder().finish().unwrap())
            .is_ok()
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end row-binding test covers every recursive value family"
)]
fn prepared_project_binds_complete_recursive_values_and_reuses_the_plan() {
    let source = r"
        pub type Choice {
            A,
            B(value: Int),
        }
        pub index Axis = { X, Y };
        param distance: Length;
        param sample_count: Int;
        param enabled: Bool;
        param axis_key: Key<Axis>;
        param when: Datetime;
        param impedance: Complex<Length>;
        param choice: Choice;
        param samples: Int[Axis];
        param choices: Choice[Axis];
        node total: Int = @samples[Axis#X] + @samples[Axis#Y];
        pub node accepted: Bool = @total == 3;
        pub node echo: Choice = @choice;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "prepared.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    assert_eq!(prepared.parameter_ports().len(), 9);
    assert!(matches!(
        prepared.parameter_ports()[3].value_schema(),
        ModelValueSchema::Key(index)
            if matches!(index.kind(), ModelIndexKind::Named { .. })
    ));

    let mut first = prepared.binding_builder();
    first
        .bind_expression(&DeclName::expect_valid("distance"), &parse_expr("2.0 m"))
        .unwrap();
    first
        .bind_expression(&DeclName::expect_valid("sample_count"), &parse_expr("5"))
        .unwrap();
    first
        .bind_expression(&DeclName::expect_valid("enabled"), &parse_expr("true"))
        .unwrap();
    first
        .bind_expression(&DeclName::expect_valid("axis_key"), &parse_expr("Axis#X"))
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("when"),
            &parse_expr("datetime(\"2025-01-02T03:04:05Z\")"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("impedance"),
            &parse_expr("complex(3.0 m, 4.0 m)"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("choice"),
            &parse_expr("B(value: 2)"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("samples"),
            &parse_expr("{ Axis#X: 1, Axis#Y: 2 }"),
        )
        .unwrap();
    first
        .bind_expression(
            &DeclName::expect_valid("choices"),
            &parse_expr("{ Axis#X: A, Axis#Y: B(value: 9) }"),
        )
        .unwrap();
    let first_row = first.finish().unwrap();
    let first_result = prepared.evaluate(&first_row).unwrap();
    assert_eq!(find_int_value(&first_result, "total"), 3);
    let recursive_model = prepared.model(&[DeclName::expect_valid("echo")]).unwrap();
    let ModelValueSchema::Algebraic(type_id) = recursive_model.outputs()[0].value_schema() else {
        panic!("echo schema was not algebraic");
    };
    let definition = recursive_model
        .schema_graph()
        .definition(type_id)
        .expect("algebraic schema reference must have a definition");
    assert_eq!(definition.constructors().len(), 2);
    assert_eq!(
        definition.constructors()[1].fields()[0].value(),
        &ModelValueSchema::Int
    );
    let ModelRowOutcome::Success(recursive_outputs) = prepared
        .evaluate_model_row(&first_row, &recursive_model)
        .unwrap()
    else {
        panic!("recursive model row unexpectedly failed");
    };
    assert!(matches!(
        recursive_outputs.as_slice(),
        [Value::Struct { .. }]
    ));

    let mut second = prepared.binding_builder();
    second
        .bind_expression(&DeclName::expect_valid("distance"), &parse_expr("4.0 m"))
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("sample_count"), &parse_expr("6"))
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("enabled"), &parse_expr("false"))
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("axis_key"), &parse_expr("Axis#Y"))
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("when"),
            &parse_expr("datetime(\"2026-01-02T03:04:05Z\")"),
        )
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("impedance"),
            &parse_expr("complex(5.0 m, 12.0 m)"),
        )
        .unwrap();
    second
        .bind_expression(&DeclName::expect_valid("choice"), &parse_expr("A"))
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("samples"),
            &parse_expr("{ Axis#X: 10, Axis#Y: 20 }"),
        )
        .unwrap();
    second
        .bind_expression(
            &DeclName::expect_valid("choices"),
            &parse_expr("{ Axis#X: B(value: 8), Axis#Y: A }"),
        )
        .unwrap();
    let second_result = prepared.evaluate(&second.finish().unwrap()).unwrap();
    assert_eq!(find_int_value(&second_result, "total"), 30);
}

#[test]
fn prepared_project_binds_coordinate_and_finite_keys() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s); \
         param at: Key<TimeStep>; param slot: Key<Fin(3)>;",
        "key-bindings.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    assert!(matches!(
        prepared.parameter_ports()[0].value_schema(),
        ModelValueSchema::Key(index)
            if matches!(index.kind(), ModelIndexKind::Coordinate { .. })
    ));
    assert!(matches!(
        prepared.parameter_ports()[1].value_schema(),
        ModelValueSchema::Key(index)
            if matches!(index.kind(), ModelIndexKind::Finite { index } if index.cardinality().get() == 3)
    ));

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(
            &DeclName::expect_valid("at"),
            &parse_expr("nearest_key(TimeStep, 1.1 s)"),
        )
        .unwrap();
    bindings
        .bind_expression(
            &DeclName::expect_valid("slot"),
            &parse_expr("fin_key(Fin(3), 2)"),
        )
        .unwrap();
    let result = prepared.evaluate(&bindings.finish().unwrap()).unwrap();
    assert!(result.params().all(|(_, value)| value.is_ok()));
}

#[test]
fn prepared_binding_literals_use_the_expected_numeric_type_exactly() {
    let project = crate::loader::LoadedProject::from_source(
        "param ratio: Dimensionless; param sample_count: Int;",
        "numeric-bindings.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(&DeclName::expect_valid("ratio"), &parse_expr("1"))
        .unwrap();
    bindings
        .bind_expression(&DeclName::expect_valid("sample_count"), &parse_expr("2.0"))
        .unwrap();
    let result = prepared.evaluate(&bindings.finish().unwrap()).unwrap();
    assert!((find_value(&result, "ratio") - 1.0).abs() < f64::EPSILON);
    assert_eq!(find_int_value(&result, "sample_count"), 2);
}

#[test]
fn prepared_bindings_reject_computations_and_cross_plan_positions() {
    let first_project = crate::loader::LoadedProject::from_source(
        "param x: Int; pub node positive: Bool = @x > 0;",
        "first.gcl",
    )
    .unwrap();
    let second_project = crate::loader::LoadedProject::from_source(
        "param x: Int; pub node positive: Bool = @x > 0;",
        "second.gcl",
    )
    .unwrap();
    let first = prepare_from_project(&first_project).unwrap();
    let second = prepare_from_project(&second_project).unwrap();

    let mut bindings = first.binding_builder();
    let error = bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr("1 + 2"))
        .unwrap_err();
    assert!(error.to_string().contains("not a closed value"));

    let mut wrong_plan = second.binding_builder();
    let error = wrong_plan
        .bind_integer(first.parameter_ports()[0].position(), 1)
        .unwrap_err();
    assert!(error.to_string().contains("another prepared project"));
}

fn binding_error_code(error: &CompileError) -> String {
    miette::Diagnostic::code(error)
        .expect("binding diagnostics have codes")
        .to_string()
}

#[test]
fn rejected_expression_bindings_report_typed_binding_errors() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index I = { a, b };\nparam n: Dimensionless = 1.0;\nparam x: Length = 1.0 m;\n\
         param d: Length(min: 0.0 m) = 1.0 m;\nparam k: Int = 1;\nparam values: Length[I];",
        "bindings.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let bind = |name: &str, value: &str| {
        prepared
            .binding_builder()
            .bind_expression(&DeclName::expect_valid(name), &parse_expr(value))
            .unwrap_err()
    };

    let error = bind("x", "@n");
    assert!(
        matches!(&error, CompileError::Binding(BindingError::NotClosed { name, .. }) if name.as_str() == "x"),
        "{error:?}"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O006");
    // Value diagnostics are drawn against the value's own text.
    let source = error.named_source().expect("located in the value");
    assert_eq!(source.name(), "<--param>");
    assert_eq!(source.inner().as_str(), "@n");
    // Declaration diagnostics stay in the entry source.
    let error = bind("d", "-1.0 m");
    assert_eq!(
        error.named_source().map(miette::NamedSource::name),
        Some("bindings.gcl")
    );
    assert!(ExternalValue::parse("<--param>", "1.0 +").is_err());

    let error = bind("k", "1.5");
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::InvalidLiteral {
                reason: crate::binding_error::BindingLiteralError::InexactInt,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O005");

    let error = bind("d", "-1.0 m");
    assert!(
        matches!(&error, CompileError::Binding(BindingError::DomainViolation { name, .. }) if name.as_str() == "d"),
        "{error:?}"
    );
    assert_eq!(error.to_string(), "below minimum (0 m)");
    assert_eq!(binding_error_code(&error), "graphcal::O010");

    // The value's kind is checked before its shape: a map for a scalar is a
    // kind mismatch, not an incomplete map.
    let error = bind("x", "{I#a: 1.0 m}");
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::KindMismatch {
                actual: crate::binding_error::BindingValueKind::MapLiteral,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "cannot bind a map literal to `x` of type `Length`"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O007");

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr("2.0 m"))
        .unwrap();
    let error = bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr("3.0 m"))
        .unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Binding(BindingError::BoundMoreThanOnce { .. })
        ),
        "{error:?}"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O009");
}

#[test]
fn typed_scalar_bindings_report_typed_binding_errors() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index Mode = { fast, slow };\nparam x: Length = 1.0 m;\n\
         param mode: Key<Mode> = Mode#fast;\nparam step: Key<Fin(2)>;",
        "scalars.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let ports = prepared.parameter_ports();
    let position = |name: &str| {
        ports
            .iter()
            .find(|port| port.name().as_str() == name)
            .unwrap()
            .position()
    };

    let error = prepared
        .binding_builder()
        .bind_integer(position("x"), 1)
        .unwrap_err();
    assert_eq!(error.to_string(), "cannot bind Int to `x` of type `Length`");
    assert_eq!(binding_error_code(&error), "graphcal::O007");

    let error = prepared
        .binding_builder()
        .bind_quantity(position("x"), f64::INFINITY)
        .unwrap_err();
    assert_eq!(binding_error_code(&error), "graphcal::O008");

    let error = prepared
        .binding_builder()
        .bind_named_key(position("mode"), &IndexVariantName::expect_valid("medium"))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown category `medium` for index `Mode`"
    );
    assert_eq!(binding_error_code(&error), "graphcal::O012");

    let error = prepared
        .binding_builder()
        .bind_named_key(position("step"), &IndexVariantName::expect_valid("first"))
        .unwrap_err();
    assert_eq!(binding_error_code(&error), "graphcal::O011");
}

/// P3-3: binding a coordinate-indexed value entry by entry is out of scope;
/// the structured binding reports that clearly instead of failing elsewhere.
#[test]
fn structured_binding_of_coordinate_indexed_entries_is_rejected_clearly() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index T = range(0.0 s, 1.0 s, step: 0.5 s);\nparam v: Length[T];",
        "coordinates.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let value = StructuredValueExpr::Indexed {
        entries: (0..3)
            .map(|_| StructuredValueExpr::Literal(parse_synthesized_expr("1.0 m")))
            .collect(),
    };

    let error = prepared
        .binding_builder()
        .bind_structured_expression(&DeclName::expect_valid("v"), &value)
        .unwrap_err();

    assert!(
        matches!(error.kind(), StructuredBindingErrorKind::CoordinateEntries),
        "{error:?}"
    );
    assert!(error.path().is_empty());
    assert_eq!(
        error.to_string(),
        "coordinate-indexed values cannot be bound entry by entry"
    );
}

#[test]
fn external_map_bindings_reject_entries_deeper_than_the_declared_schema() {
    let project = crate::loader::LoadedProject::from_source(
        "pub index Axis = { X }; pub index Extra = { Only }; param samples: Int[Axis];",
        "model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let input = miette::NamedSource::new(
        "input.json",
        std::sync::Arc::new("{\n  \"samples\": {}\n}".to_string()),
    );
    let mut bindings = prepared.binding_builder();

    let error = bindings
        .bind_external_expression(
            &DeclName::expect_valid("samples"),
            &parse_synthesized_expr("{ (Axis#X, Extra#Only): 1 }"),
            &input,
            (4usize, 9usize).into(),
        )
        .unwrap_err();

    match error {
        CompileError::ExternalBinding { name, reason, .. } => {
            assert_eq!(name.as_str(), "samples");
            assert!(
                reason.contains("map entry has more keys than the declared value has indexed axes"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected external binding diagnostic, got {other:?}"),
    }
}

#[test]
fn external_binding_errors_retain_boundary_source_and_parameter() {
    let project =
        crate::loader::LoadedProject::from_source("param sample_count: Int;", "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let input = miette::NamedSource::new(
        "input.json",
        std::sync::Arc::new("{\n  \"sample_count\": true\n}".to_string()),
    );
    let mut bindings = prepared.binding_builder();

    let error = bindings
        .bind_external_expression(
            &DeclName::expect_valid("sample_count"),
            &parse_synthesized_expr("true"),
            &input,
            (4usize, 7usize).into(),
        )
        .unwrap_err();

    match error {
        CompileError::ExternalBinding {
            name, reason, span, ..
        } => {
            assert_eq!(name.as_str(), "sample_count");
            assert!(
                reason.contains("declared Int, inferred Bool"),
                "unexpected reason: {reason}"
            );
            assert_eq!(span.offset(), 4);
            assert_eq!(span.len(), 7);
        }
        other => panic!("expected external binding diagnostic, got {other:?}"),
    }
}

#[test]
fn tenax_v2_projection_is_strict_and_preserves_typed_domains() {
    let source = r"
        pub index Mode = { Zulu, Alpha };
        param length: Length(min: 0.0 m, max: 10.0 m);
        param ratio: Dimensionless(min: -1.0, max: 1.0);
        param sample_count: Int(min: -2, max: 2);
        param mode: Key<Mode>;
        pub node selected: Bool = true;
    ";
    let project = crate::loader::LoadedProject::from_source(source, "model.gcl").unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let model = prepared
        .tenax_v2_model(&[DeclName::expect_valid("selected")])
        .unwrap();
    assert_eq!(model.inputs().len(), 4);
    assert_eq!(
        model.inputs()[0].kind().continuous(),
        Some((0.0, 10.0, Some("m"), 1.0))
    );
    assert_eq!(
        model.inputs()[1].kind().continuous(),
        Some((-1.0, 1.0, None, 1.0))
    );
    assert_eq!(model.inputs()[2].kind().integer(), Some((-2, 2)));
    assert_eq!(
        model.inputs()[3]
            .kind()
            .categories()
            .unwrap()
            .iter()
            .map(IndexVariantName::as_str)
            .collect::<Vec<_>>(),
        ["Alpha", "Zulu"]
    );
}

#[test]
fn tenax_v2_limits_do_not_restrict_the_generic_model_interface() {
    let project = crate::loader::LoadedProject::from_source(
        "param enabled: Bool; pub node selected: Bool = @enabled;",
        "generic-model.gcl",
    )
    .unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let outputs = [DeclName::expect_valid("selected")];
    assert!(prepared.model(&outputs).is_ok());
    assert!(matches!(
        prepared.tenax_v2_model(&outputs),
        Err(ModelDefinitionError::UnsupportedInputType { .. })
    ));
}

/// Evaluate `pipeline.main`'s `ok` model output for one bound `x`.
fn evaluate_included_model_row(leaf: &str, x: &str) -> ModelRowOutcome {
    let (_directory, root) = write_pipeline_project(
        &[
            ("leaf.gcl", leaf),
            (
                "main.gcl",
                "param x: Dimensionless(min: -10.0, max: 10.0);\n\
                 include pipeline.leaf(input: @x) as leaf;\n\
                 pub node ok: Bool = @x < 100.0;",
            ),
        ],
        "main.gcl",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let model = prepared.model(&[DeclName::expect_valid("ok")]).unwrap();
    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(&DeclName::expect_valid("x"), &parse_expr(x))
        .unwrap();
    let row = bindings.finish().unwrap();
    prepared.evaluate_model_row(&row, &model).unwrap()
}

#[test]
fn model_row_fails_when_an_included_assertion_fails() {
    let leaf = "param input: Dimensionless;\n\
                pub node output: Dimensionless = @input;\n\
                assert positive = @output > 0.0;";
    assert_eq!(
        evaluate_included_model_row(leaf, "1.0"),
        ModelRowOutcome::Success(vec![Value::Bool(true)])
    );
    match evaluate_included_model_row(leaf, "-1.0") {
        ModelRowOutcome::Failure(failure) => assert!(
            failure.message().contains("positive"),
            "unexpected failure: {failure}"
        ),
        ModelRowOutcome::Success(values) => panic!("expected assertion failure, got {values:?}"),
    }
}

#[test]
fn model_row_reports_runtime_errors_inside_included_dags_as_row_failures() {
    let leaf = "param input: Dimensionless;\n\
                node reciprocal: Dimensionless = 1.0 / @input;\n\
                pub node output: Dimensionless = @input;";
    assert_eq!(
        evaluate_included_model_row(leaf, "1.0"),
        ModelRowOutcome::Success(vec![Value::Bool(true)])
    );
    match evaluate_included_model_row(leaf, "0.0") {
        ModelRowOutcome::Failure(failure) => assert!(
            failure.message().contains("division by zero"),
            "unexpected failure: {failure}"
        ),
        ModelRowOutcome::Success(values) => panic!("expected runtime failure, got {values:?}"),
    }
}

/// A project whose root includes `pipeline.leaf` under `include`, where
/// `leaf` fails in its private `reciprocal` when `x` is zero.
fn write_private_failure_project(include: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\n\
                 node reciprocal: Dimensionless = 1.0 / @input;\n\
                 pub node output: Dimensionless = @input;",
            ),
            (
                "main.gcl",
                &format!(
                    "param x: Dimensionless(min: -10.0, max: 10.0);\n\
                     {include}\n\
                     pub node ok: Bool = @x < 100.0;"
                ),
            ),
        ],
        "main.gcl",
    )
}

#[test]
fn private_include_failures_are_labelled_by_the_include_scope() {
    // B2: a failure private to an included DAG is named by the scope the
    // root gives the include (its alias, or the module name of an anonymous
    // include), in both the model-row and the full evaluation result,
    // never by the internal identity or by the module name behind an alias.
    for (include, label) in [
        ("include pipeline.leaf(input: @x) as l2;", "l2::reciprocal"),
        (
            "include pipeline.leaf(input: @x)::{ output as o };",
            "leaf::reciprocal",
        ),
    ] {
        let (_directory, root) = write_private_failure_project(include);
        let project = crate::loader::load_project(&root, None, &fs()).unwrap();
        let prepared = prepare_from_project(&project).unwrap();
        let model = prepared.model(&[DeclName::expect_valid("ok")]).unwrap();
        let mut bindings = prepared.binding_builder();
        bindings
            .bind_expression(&DeclName::expect_valid("x"), &parse_expr("0.0"))
            .unwrap();
        let row = bindings.finish().unwrap();
        match prepared.evaluate_model_row(&row, &model).unwrap() {
            ModelRowOutcome::Failure(failure) => assert!(
                failure
                    .message()
                    .starts_with(&format!("{label}: division by zero")),
                "unexpected failure for `{include}`: {failure}"
            ),
            ModelRowOutcome::Success(values) => {
                panic!("expected a runtime failure for `{include}`, got {values:?}")
            }
        }

        let result = prepared.evaluate(&row).unwrap();
        let failed = result
            .entries
            .iter()
            .filter(|(_, value, _)| value.is_err())
            .map(|(name, _, _)| name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(failed, [label], "for `{include}`");
    }
}

#[test]
fn inclusive_bounds_are_ordered_intervals() {
    assert!(InclusiveBounds::try_new(Some(1.0), Some(2.0)).is_ok());
    assert!(InclusiveBounds::try_new(Some(2.0), Some(2.0)).is_ok());
    assert!(InclusiveBounds::<f64>::try_new(None, None).is_ok());
    let lower_only = InclusiveBounds::try_new(Some(3_i64), None).unwrap();
    assert_eq!((lower_only.lower(), lower_only.upper()), (Some(&3), None));
    assert_eq!(
        InclusiveBounds::try_new(Some(2.0), Some(1.0)),
        Err(InclusiveBoundsError::Inverted)
    );
    assert_eq!(
        InclusiveBounds::try_new(Some(f64::NAN), Some(1.0)),
        Err(InclusiveBoundsError::Unordered)
    );
    assert_eq!(
        InclusiveBounds::try_new(None, Some(f64::NAN)),
        Err(InclusiveBoundsError::Unordered)
    );
}

#[test]
fn required_param_without_override_errors() {
    let source = "param x: Dimensionless;\nnode y: Dimensionless = @x + 1.0;";
    let result = compile_and_eval_with_overrides(source, "test.gcl", &HashMap::new());
    match result {
        Err(CompileError::Binding(BindingError::RequiredParamNotProvided { name, .. })) => {
            assert_eq!(name.as_str(), "x");
        }
        other => panic!("expected RequiredParamNotProvided, got {other:?}"),
    }
}

#[test]
fn required_param_with_override_succeeds() {
    let source = "param x: Dimensionless;\nnode y: Dimensionless = @x + 1.0;";
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("x"), parse_expr("42.0"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 43.0).abs() < f64::EPSILON, "y = {y}, expected 43.0");
}
// --- Module import tests ---#[test]#[test]// --- Runtime arithmetic error tests ---

/// Helper: assert that a specific node in the result has a `NodeUnavailable::EvalFailed`
/// whose message contains `needle`.
fn assert_node_error(source: &str, node_name: &str, needle: &str) {
    let result = compile_and_eval(source).unwrap();
    let (_, node_result, _) = result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == node_name)
        .unwrap_or_else(|| panic!("node `{node_name}` not found"));
    match node_result {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert!(
                message.contains(needle),
                "expected error containing {needle:?}, got {message:?}"
            );
        }
        Err(other) => panic!("expected EvalFailed containing {needle:?}, got {other:?}"),
        Ok(val) => panic!("expected error for `{node_name}`, got value {val:?}"),
    }
}

#[test]
fn eval_division_by_zero() {
    assert_node_error(
        "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x / 0.0;",
        "y",
        "division by zero",
    );
}

#[test]
fn eval_zero_divided_by_zero() {
    assert_node_error(
        "param x: Dimensionless = 0.0;\nnode y: Dimensionless = @x / 0.0;",
        "y",
        "division by zero",
    );
}

#[test]
fn eval_sqrt_negative() {
    assert_node_error("node y: Dimensionless = sqrt(-1.0);", "y", "NaN");
}

#[test]
fn eval_ln_zero() {
    assert_node_error("node y: Dimensionless = ln(0.0);", "y", "infinite");
}

#[test]
fn eval_ln_negative() {
    assert_node_error("node y: Dimensionless = ln(-1.0);", "y", "NaN");
}

#[test]
fn eval_exp_overflow() {
    assert_node_error("node y: Dimensionless = exp(1000.0);", "y", "infinite");
}

#[test]
fn eval_power_negative_base_frac_exp() {
    assert_node_error("node y: Dimensionless = (-1.0) ^ 0.5;", "y", "NaN");
}

// --- Error containment tests ---

// --- Integer type tests ---

/// Helper: find a named Int value.
fn find_int_value(result: &EvalResult, name: &str) -> i64 {
    let val = result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
    match val {
        Value::Int(i) => *i,
        other => panic!("expected Int for `{name}`, got {other:?}"),
    }
}

#[test]
fn eval_nonzero_multiplication_underflow_is_an_error() {
    assert_node_error(
        "param tiny: Length = 1.0e-300 m;\nnode area: Area = @tiny * @tiny;",
        "area",
        "arithmetic operation underflowed to zero",
    );
}

#[test]
fn eval_int_division_by_zero() {
    assert_node_error(
        "param x: Int = 10;\nnode y: Int = @x / 0;",
        "y",
        "integer division by zero",
    );
}

#[test]
fn eval_int_modulo_by_zero() {
    assert_node_error(
        "param x: Int = 10;\nnode y: Int = @x % 0;",
        "y",
        "integer modulo by zero",
    );
}

#[test]
fn eval_int_with_unit_parse_error() {
    // `10 km` should be a parse error
    let err = compile_and_eval("param x: Length = 10 km;");
    assert!(err.is_err());
}

// --- Instantiated import tests ---

#[test]
fn project_instantiated_import_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // dry_mass overridden to 800 kg, fuel_mass default 2800 kg, isp default 320 s
    // delta_v = 320 * 9.80665 * ln((800 + 2800) / 800) = 3138.128 * ln(4.5)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}
#[test]
fn project_instantiated_import_graph_ref() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_graph_ref/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // my_mass = 800 kg, passed as dry_mass binding via @my_mass
    // delta_v = 320 * 9.80665 * ln(3600/800)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}

fn write_nested_file_include_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/demo");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("leaf.gcl"),
        "param x: Dimensionless;\n\
         node private_value: Dimensionless = @x + 1.0;\n\
         pub node doubled: Dimensionless = @x * 2.0;\n\
         pub assert positive = @doubled > 0.0;\n\
         pub plot chart = {\n\
             mark: line,\n\
             encode: { x: @x, y: @doubled },\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("middle.gcl"),
        "param x: Dimensionless;\n\
         include demo.leaf(x: @x) as leaf;\n\
         pub node out: Dimensionless = @leaf::doubled;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("upper.gcl"),
        "param x: Dimensionless;\n\
         include demo.middle(x: @x) as middle;\n\
         pub node out: Dimensionless = @middle::out;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn nested_instantiated_file_include_evaluates_immediate_scope_binding() {
    let (_dir, root) = write_nested_file_include_project(
        "include demo.middle(x: 3.0) as middle;\n\
         pub node result: Dimensionless = @middle::out;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 6.0).abs() < f64::EPSILON);
}

fn write_package_root_child_include_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\nsource_dir = \"src\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("defaults.gcl"),
        "pub node output: Dimensionless = 2.0;\n",
    )
    .unwrap();
    let root = dir.path().join("src/app.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn include_instance_can_share_its_display_path_with_a_source_module() {
    let (_dir, root) = write_package_root_child_include_project(
        "include app.defaults() as defaults;\n\
         pub node result: Dimensionless = @defaults::output;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn aliased_include_does_not_bind_the_source_module_name() {
    let (_dir, root) = write_package_root_child_include_project(
        "include app.defaults() as configured;\n\
         pub node leaked: Dimensionless = @defaults()::output;\n",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }), .. }), .. })
                if kind.to_string() == "unknown module `src.app.defaults`"
        ),
        "unexpected alias-leak diagnostic: {error:?}"
    );
}

#[test]
fn nested_instantiated_file_include_keeps_multiple_instances_isolated() {
    let (_dir, root) = write_nested_file_include_project(
        "include demo.middle(x: 3.0) as first;\n\
         include demo.middle(x: 5.0) as second;\n\
         pub node result: Dimensionless = @first::out + @second::out;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 16.0).abs() < f64::EPSILON);
}

#[test]
fn nested_instantiated_file_include_resolves_aliased_sibling_outputs_in_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/example");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"example\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("axes.gcl"),
        "pub index Item = { One, Two };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("a.gcl"),
        "import example.axes::{ index Item };\n\
         param input: Dimensionless[Item];\n\
         pub node a_values: Dimensionless[Item] = @input;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("b.gcl"),
        "import example.axes::{ index Item };\n\
         param input: Dimensionless[Item];\n\
         pub node b_values: Dimensionless[Item] =\n\
             for item: Item { @input[item] * 2.0 };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("c.gcl"),
        "import example.axes::{ index Item };\n\
         param left: Dimensionless[Item];\n\
         param right: Dimensionless[Item];\n\
         pub node combined: Dimensionless[Item] =\n\
             for item: Item { @left[item] + @right[item] };\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("middle.gcl"),
        "import example.axes::{ index Item };\n\
         param input: Dimensionless[Item];\n\
         include example.a(input: @input) as a;\n\
         include example.b(input: @a::a_values) as b;\n\
         include example.c(left: @a::a_values, right: @b::b_values) as c;\n\
         pub node result: Dimensionless[Item] = @c::combined;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import example.axes::{ index Item };\n\
         node input: Dimensionless[Item] = {\n\
             Item#One: 1.0,\n\
             Item#Two: 2.0,\n\
         };\n\
         include example.middle(input: @input)::{ pub result };\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert_eq!(
        indexed_si_values(&find_entry(&result, "result")),
        vec![("One", 3.0), ("Two", 6.0)]
    );
}

#[test]
fn nested_instantiated_file_include_reexports_requested_plot() {
    let (dir, root) = write_nested_file_include_project(
        "include demo.middle(x: 3.0)::{ out, chart };\n\
         pub node result: Dimensionless = @out;\n",
    );
    std::fs::write(
        dir.path().join("src/demo/middle.gcl"),
        "param x: Dimensionless;\n\
         include demo.leaf(x: @x)::{ doubled, pub chart };\n\
         pub node out: Dimensionless = @doubled;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - 6.0).abs() < f64::EPSILON);
    assert_eq!(result.plots.len(), 1);
    assert_eq!(result.plots[0].name.to_string(), "chart");
}

#[test]
fn requested_plot_specializes_its_required_index_axis() {
    let result = compile_and_eval(
        "index Axis = { One, Two };\n\
         node input: Dimensionless[Axis] = { Axis#One: 1.0, Axis#Two: 2.0 };\n\
         dag chart {\n\
             pub(bind) index Item;\n\
             param input: Dimensionless[Item];\n\
             pub plot output = {\n\
                 mark: line,\n\
                 encode: { x: @input, y: @input },\n\
             };\n\
         }\n\
         include chart(index Item: Axis, input: @input)::{ output };\n",
    )
    .unwrap();

    let plot = result.plots.first().expect("requested plot must render");
    assert_eq!(plot.name.to_string(), "output");
    let (_, values) = plot.encodings.first().expect("plot must contain x data");
    let graphcal_eval::eval::types::PlotFieldValue::Numbers(values) = values else {
        panic!("expected numeric plot data, got {values:?}");
    };
    assert_eq!(values.as_slice(), [1.0, 2.0]);
}

#[test]
fn requested_plot_keeps_instance_owned_dynamic_unit_presentation() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/demo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("fx.gcl"),
        "pub base dim Money;\n\
         pub base unit USD: Money;\n\
         param rate: Dimensionless = 2.0;\n\
         pub unit EUR: Money = (@rate) USD;\n\
         param amount: Money = 3.0 EUR;\n\
         pub plot chart = { mark: bar, encode: { x: 1.0, y: @amount } };\n",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(&root, "include demo.fx()::{ chart };\n").unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let plot = result.plots.first().expect("requested plot must render");
    let (_, values) = plot
        .encodings
        .iter()
        .find(|(channel, _)| *channel == graphcal_compiler::syntax::ast::EncodingChannel::Y)
        .expect("plot must contain y data");
    let graphcal_eval::eval::types::PlotFieldValue::Numbers(values) = values else {
        panic!("expected numeric plot data, got {values:?}");
    };
    assert_eq!(values.as_slice(), [3.0]);
}

#[test]
fn requested_instance_plot_reports_its_failed_instance_dependency() {
    // The plot body is the template's; the failed dependency is keyed by the
    // instance's own declaration, so the report must resolve the body's
    // reference through the instance frame (#842 inside an include).
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/demo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("div.gcl"),
        "pub index Step = { A, B };\n\
         param values: Dimensionless[Step] = { Step#A: 1.0, Step#B: 0.0 };\n\
         node inv: Dimensionless[Step] = for s: Step { 1.0 / @values[s] };\n\
         pub plot chart = {\n\
             mark: line,\n\
             encode: { x: for s: Step { @values[s] }, y: for s: Step { @inv[s] } },\n\
         };\n",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(&root, "include demo.div()::{ chart };\n").unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!(result.plots.is_empty());
    let [error] = result.plot_errors.as_slice() else {
        panic!("expected one plot error, got {:?}", result.plot_errors);
    };
    assert_eq!(error.name.to_string(), "chart");
    let graphcal_eval::eval::PlotUnavailable::Evaluation(NodeUnavailable::EvalFailed { message }) =
        &error.reason
    else {
        panic!("expected an evaluation failure, got {:?}", error.reason);
    };
    assert!(
        // The dependency is named as the output names it: the private include
        // scope takes its readable name.
        message.starts_with("dependency failed: div::inv (")
            && message.contains("division by zero"),
        "expected the failed instance dependency with its root cause: {message}"
    );
}

#[test]
fn composition_of_an_unavailable_requested_instance_plot_reports_the_plot() {
    // A figure names a requested instance plot by its local alias. When that
    // plot is unavailable, the figure is blocked by it and reports it by that
    // alias, not by the instance's plot declaration identity; the alias binds
    // no declaration identity, so it must not be looked up as one (this used
    // to abort evaluation with X001).
    let (_directory, root) = write_pipeline_project(
        &[
            (
                "leaf.gcl",
                "param input: Dimensionless;\n\
                 node reciprocal: Dimensionless = 1.0 / @input;\n\
                 pub plot chart = { mark: point, encode: { x: @input, y: @reciprocal } };\n",
            ),
            (
                "main.gcl",
                "include pipeline.leaf(input: 0.0)::{ chart as ch };\n\
                 figure summary = { plots: [ch] };\n",
            ),
        ],
        "main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!(result.plots.is_empty());
    assert!(result.figures.is_empty());
    let names = result
        .plot_errors
        .iter()
        .map(|error| error.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, ["ch", "summary"]);
    let graphcal_eval::eval::PlotUnavailable::ComposedPlots(
        graphcal_eval::eval::ComposedPlotsUnavailable::Failed { failed_plots },
    ) = &result.plot_errors[1].reason
    else {
        panic!(
            "expected the figure to be blocked by its plot, got {:?}",
            result.plot_errors[1].reason
        );
    };
    let [plot] = failed_plots.as_slice() else {
        panic!("expected one failed plot, got {failed_plots:?}");
    };
    assert_eq!(plot.as_str(), "ch");
    assert_eq!(
        result.plot_errors[1].reason.to_string(),
        "dependency failed: ch"
    );
}

#[test]
fn compositions_name_unavailable_plots_as_the_root_does() {
    // Figures and layers report the plots blocking them by their names in the
    // root, for root plots and requested inline-DAG instance plots alike,
    // never by declaration identities such as `main.<include@N>.inner`.
    let result = compile_and_eval_named(
        "dag lib { pub plot inner = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } }; }\n\
         include lib()::{ inner as shown };\n\
         plot broken = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } };\n\
         node missing: Dimensionless = todo {};\n\
         plot pending = { mark: point, encode: { y: @missing } };\n\
         figure comparison = { plots: [broken], title: \"C\" };\n\
         layer overlay = { plots: [shown, broken] };\n\
         figure mixed = { plots: [pending, shown] };\n",
        "main.gcl",
    )
    .unwrap();
    let reasons = result
        .plot_errors
        .iter()
        .map(|error| (error.name.to_string(), error.reason.to_string()))
        .collect::<HashMap<_, _>>();
    assert_eq!(reasons["comparison"], "dependency failed: broken");
    assert_eq!(reasons["overlay"], "dependency failed: broken, shown");
    assert!(
        reasons["mixed"].ends_with("; dependency failed: shown"),
        "{}",
        reasons["mixed"]
    );
    let mixed = &result
        .plot_errors
        .iter()
        .find(|error| error.name.to_string() == "mixed")
        .unwrap()
        .reason;
    assert!(mixed.is_incomplete());
    assert!(mixed.has_failure());
}

#[test]
fn three_level_instantiated_file_include_preserves_assertion_instance_path() {
    let (_dir, root) = write_nested_file_include_project(
        "include demo.upper(x: -1.0) as upper;\n\
         pub node result: Dimensionless = @upper::out;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "result") - (-2.0)).abs() < f64::EPSILON);
    assert!(result.assertions.iter().any(|(name, outcome, _)| {
        name.to_string() == "upper.middle.leaf::positive"
            && matches!(
                outcome,
                graphcal_eval::eval::types::AssertResult::Fail { .. }
            )
    }));
    assert!(
        result
            .output_surface()
            .contains(&member_name(&["upper"], "out"))
    );
    assert!(
        !result
            .output_surface()
            .contains(&member_name(&["upper", "middle"], "out"))
    );
    assert!(
        !result
            .output_surface()
            .contains(&member_name(&["upper", "middle", "leaf"], "private_value"))
    );
}

fn write_same_leaf_include_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/app");
    std::fs::create_dir_all(package_dir.join("analysis")).unwrap();
    std::fs::create_dir_all(package_dir.join("presentation")).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("analysis/shared.gcl"),
        "param input: Dimensionless;\npub node output: Dimensionless = @input + 1.0;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("presentation/shared.gcl"),
        "param input: Dimensionless;\npub node output: Dimensionless = @input * 2.0;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

#[test]
fn project_selective_includes_allow_distinct_modules_with_same_leaf_name() {
    let (_dir, root) = write_same_leaf_include_project(
        "include app.analysis.shared(input: 2.0)::{ output as analyzed };\n\
         include app.presentation.shared(input: 5.0)::{ output as rendered };\n\
         node combined: Dimensionless = @analyzed + @rendered;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "combined") - 13.0).abs() < f64::EPSILON);

    let internal_input_owners = result
        .entries
        .iter()
        .filter(|(name, _, _)| name.leaf().as_str() == "input" && name.is_qualified())
        .map(|(name, _, _)| name.qualifier().to_vec())
        .collect::<HashSet<_>>();
    assert_eq!(internal_input_owners.len(), 2);
}

#[test]
fn module_aliases_and_same_named_nodes_are_duplicate_names() {
    // B4 regression: an include alias is a Term name exactly like an import
    // alias, including when it instantiates a local inline DAG.
    let velocity = "dag velocity { param r: Dimensionless; pub node v: Dimensionless = @r; }\n";
    for alias_decl in [
        "include velocity(r: 1.0) as parking;",
        "import velocity as parking;",
    ] {
        let source = format!("{velocity}{alias_decl}\nnode parking: Dimensionless = 2.0;\n");
        match compile_and_eval(&source) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind: SemanticErrorKind::Name(NameError::DuplicateName { name, .. }),
                        ..
                    }),
                ..
            })) => {
                assert_eq!(name.to_string(), "parking", "{alias_decl}");
            }
            other => panic!("expected N001 for `{alias_decl}`, got {other:?}"),
        }
    }
}

#[test]
fn project_selective_includes_still_reject_duplicate_local_names() {
    let (_dir, root) = write_same_leaf_include_project(
        "include app.analysis.shared(input: 2.0)::{ output as duplicate };\n\
         include app.presentation.shared(input: 5.0)::{ output as duplicate };\n",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();

    match project.build_module_resolver() {
        Err(graphcal_compiler::resolve::error::ModuleResolveError::DuplicateImportName {
            namespace,
            name,
            ..
        }) => {
            assert_eq!(
                namespace,
                graphcal_compiler::resolve::namespace::Namespace::Term
            );
            assert_eq!(name.as_str(), "duplicate");
        }
        other => panic!("expected a duplicate local declaration diagnostic, got {other:?}"),
    }
}

#[test]
fn project_module_includes_still_reject_duplicate_default_aliases() {
    let (_dir, root) = write_same_leaf_include_project(
        "include app.analysis.shared(input: 2.0);\n\
         include app.presentation.shared(input: 5.0);\n",
    );

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(ModuleError::DuplicateModuleName { name, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.as_str(), "shared");
        }
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Module(kind @ ModuleError::ModuleResolution { .. }),
                    ..
                }),
            ..
        })) => {
            let message = kind.to_string();
            assert!(
                message.contains("duplicate module") && message.contains("shared"),
                "unexpected duplicate-alias diagnostic: {message}",
            );
        }
        other => panic!("expected a duplicate module alias diagnostic, got {other:?}"),
    }
}

#[test]
fn project_selective_includes_allow_multiple_instances_of_same_module() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("shared.gcl"),
        "param input: Dimensionless;\npub node output: Dimensionless = @input * 2.0;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include app.shared(input: 2.0)::{ output as first };\n\
         include app.shared(input: 3.0)::{ output as other };\n\
         node combined: Dimensionless = @first + @other;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "combined") - 10.0).abs() < f64::EPSILON);
}

#[test]
fn project_import_preserves_structural_finite_index_identity() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("lib.gcl"),
        "pub const node values: Dimensionless[Fin(2)] = table[Fin(2)] { 1.0; 2.0; };\n",
    )
    .unwrap();
    let root = source_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import app.lib::{ values };\nconst node copied: Dimensionless[Fin(2)] = @values;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let copied = result
        .consts()
        .find(|(name, _)| name.to_string() == "copied")
        .and_then(|(_, value)| value.as_ref().ok())
        .expect("copied imported finite-indexed value");
    assert!(matches!(copied, Value::Indexed { entries, .. } if entries.len() == 2));
}

#[test]
fn project_selective_import_item_rejects_unknown_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/attr");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"attr\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub node x: Dimensionless = 1.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import attr.lib::{ #[bogus] x };\nnode y: Dimensionless = @x;\n",
    )
    .unwrap();

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Attribute(AttributeError::UnknownAttribute { name, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.as_str(), "bogus");
        }
        other => panic!("expected UnknownAttribute, got {other:?}"),
    }
}

#[test]
fn project_qualified_index_type_annotation_and_variant_arg() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/mission");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"mission\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub index Phase = { Burn, Coast };\n\
         pub dim GravityAccel = Length / Time^2;\n\
         pub node thrust: Dimensionless[Phase] = { Phase#Burn: 3.0, Phase#Coast: 5.0 };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import mission.lib as lib;\n\
         node thrust: Dimensionless[lib::Phase] = { lib::Phase#Burn: 3.0, lib::Phase#Coast: 5.0 };\n\
         node burn: Dimensionless = @thrust[lib::Phase#Burn];\n\
         node accel: lib::GravityAccel = 9.80665 m/s^2;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    assert!((find_value(&result, "burn") - 3.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "accel") - 9.80665).abs() < f64::EPSILON);
}

fn write_same_leaf_index_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Warm, Cold };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_same_variant_index_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_range_index_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Step = range(0.0 s, 1.0 s, step: 1.0 s);\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Step = range(0.0 s, 2.0 s, step: 1.0 s);\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_constructor_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Action { Pick(distance: Length), Idle }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Command { Pick(duration: Time), Idle }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_struct_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Pick(distance: Length), Idle }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Pick(duration: Time), Idle }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(distance: Length) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(duration: Time) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(distance: Length(min: 1.0 m)) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(duration: Time(min: 1.0 s)) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_same_field_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(value: Length(min: 1.0 m)) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(value: Length(min: 10.0 m)) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_custom_unit_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/record_scope");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"record_scope\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("schema.gcl"),
        "pub base dim Currency;\n\
         pub base unit credit: Currency;\n\
         pub dim BaseRate = Length / Time;\n\
         pub dim WeightedRate = BaseRate * Mass;\n\
         pub dim ScaledRate = WeightedRate / Time;\n\
         pub type Price { Price(amount: Currency(min: 0.0 credit)) }\n\
         pub type Basket { Basket(price: Price) }\n\
         pub type Receipt { Receipt(basket: Basket) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_imported_unit_dependency_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/unit_dependencies");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"unit_dependencies\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("foundation.gcl"),
        "pub base dim Score;\n\
         pub base unit point: Score;\n\
         pub type Measurement { Measurement(value: Score) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("rates.gcl"),
        "import unit_dependencies.foundation as foundation;\n\
         pub dim ScoreRate = foundation::Score / Time;\n\
         pub const unit point_per_second: ScoreRate = 1.0 foundation::point / s;\n\
         pub type RateMeasurement { RateMeasurement(value: ScoreRate) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn loaded_file_dag_id(
    project: &crate::loader::LoadedProject,
    file_name: &str,
) -> graphcal_compiler::dag_id::DagId {
    project
        .files()
        .iter()
        .find(|file| file.path().file_name().and_then(|name| name.to_str()) == Some(file_name))
        .map_or_else(
            || panic!("loaded file `{file_name}` not found"),
            |file| file.dag_id().clone(),
        )
}

// #1087: imported type constraints retain the defining module's unit scope.
#[test]
fn imported_record_field_constraint_uses_defining_unit_scope_through_module_alias() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema as schema;\n\
         base dim ConsumerCurrency;\n\
         base unit credit: ConsumerCurrency;\n\
         param price: schema::Price;\n\
         node result: Dimensionless = 1.0;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn imported_record_field_constraint_uses_defining_unit_scope_through_selective_type_import() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Price};\n\
         base dim ConsumerCurrency;\n\
         base unit credit: ConsumerCurrency;\n\
         param price: Price;\n\
         node result: Dimensionless = 1.0;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn prepared_imported_record_binding_uses_canonical_nested_constructors_and_units() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Receipt};\n\
         param receipt: Receipt;\n\
         pub node accepted: Bool = true;\n",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();

    let mut bindings = prepared.binding_builder();
    bindings
        .bind_expression(
            &graphcal_compiler::syntax::decl_name::DeclName::expect_valid("receipt"),
            &parse_expr("Receipt(basket: Basket(price: Price(amount: 1.0 credit)))"),
        )
        .unwrap();
    let result = prepared.evaluate(&bindings.finish().unwrap()).unwrap();
    assert!(
        result
            .nodes()
            .any(|(name, value)| name.leaf().as_str() == "accepted"
                && matches!(value, Ok(Value::Bool(true))))
    );
}

#[test]
fn prepared_imported_record_binding_enforces_nested_definition_site_constraint() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Receipt};\n\
         param receipt: Receipt;\n\
         pub node accepted: Bool = true;\n",
    );
    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    let prepared = prepare_from_project(&project).unwrap();
    let mut bindings = prepared.binding_builder();

    let error = bindings
        .bind_expression(
            &graphcal_compiler::syntax::decl_name::DeclName::expect_valid("receipt"),
            &parse_expr("Receipt(basket: Basket(price: Price(amount: -1.0 credit)))"),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("Price.amount") && error.to_string().contains("below minimum"),
        "unexpected error: {error}"
    );
}

#[test]
fn selectively_imported_dimension_retains_transitive_definition_site_dependencies() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{dim ScaledRate};\n\
         param rate: ScaledRate;\n\
         pub node accepted: Bool = true;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn module_imported_dimension_retains_transitive_definition_site_dependencies() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema as schema;\n\
         param rate: schema::ScaledRate;\n\
         pub node accepted: Bool = true;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_record_retains_transitive_definition_site_field_types() {
    let (_dir, root) = write_custom_unit_constrained_record_type_project(
        "import record_scope.schema::{type Receipt};\n\
         param receipt: Receipt;\n\
         pub node accepted: Bool = true;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_base_unit_retains_custom_dimension_dependency() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.foundation::{type Measurement, unit point};\n\
         param measurement: Measurement;\n\
         assert nonnegative = @measurement.value >= 0.0 point;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_derived_unit_retains_aliased_dependencies() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.rates::{type RateMeasurement, unit point_per_second};\n\
         param rate: RateMeasurement;\n\
         assert nonnegative = @rate.value >= 0.0 point_per_second;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn module_imported_unit_retains_custom_dimension_dependency() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.rates as rates;\n\
         param rate: rates::RateMeasurement;\n\
         assert nonnegative = @rate.value >= 0.0 rates::point_per_second;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn selectively_imported_unit_does_not_expose_its_backing_dimension_name() {
    let (_dir, root) = write_imported_unit_dependency_project(
        "import unit_dependencies.foundation::{unit point};\n\
         param score: Score;\n\
         assert nonnegative = @score >= 0.0 point;\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.to_string(), "Score");
        }
        other => panic!("expected UnknownDimension for Score, got {other:?}"),
    }
}

#[test]
fn project_constructor_call_uses_resolved_owner_with_same_leaf_constructors() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node command: b::Command = b::Pick(duration: 3.0 s);\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_match_pattern_uses_resolved_constructor_and_binding() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node distance: Length = match @action {\n\
             a::Pick(distance: d) => d,\n\
             a::Idle => 0.0 m,\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_struct_type_uses_resolved_owner_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_struct_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Item = a::Pick(distance: 2.0 m);\n\
         node command: b::Item = b::Pick(duration: 3.0 s);\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_struct_type_rejects_same_leaf_wrong_owner_constructor() {
    let (_dir, root) = write_same_leaf_struct_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node bad: a::Item = b::Pick(duration: 3.0 s);\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected DimensionMismatchInAnnotation, got {other:?}"),
    }
}

#[test]
fn project_field_access_uses_resolved_struct_type_def_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node distance: Length = @item.distance;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn eval_constructor_calls_preserve_same_leaf_struct_owners() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node command: b::Command = b::Pick(duration: 3.0 s);\n",
    );

    let (_tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let owner_of_struct = |name: &str| {
        let value = result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("node `{name}` failed: {e}"));
        let Value::Struct {
            type_name,
            constructor,
            ..
        } = value
        else {
            panic!("expected struct value for `{name}`, got {value:?}");
        };
        assert_eq!(constructor.as_str(), "Pick");
        type_name.resolved().clone()
    };

    assert_eq!(owner_of_struct("action").owner(), &a_id);
    assert_eq!(owner_of_struct("command").owner(), &b_id);
}

#[test]
fn eval_constructor_match_uses_resolved_owner_and_binding() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node command: b::Command = b::Pick(duration: 3.0 s);\n\
         node distance: Length = match @action {\n\
             a::Pick(distance: d) => d,\n\
             a::Idle => 0.0 m,\n\
         };\n\
         node duration: Time = match @command {\n\
             b::Pick(duration: t) => t,\n\
             b::Idle => 0.0 s,\n\
         };\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "distance") - 2.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "duration") - 3.0).abs() < f64::EPSILON);
}

#[test]
fn eval_field_access_uses_resolved_struct_type_def_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node other: b::Item = b::Item(duration: 3.0 s);\n\
         node distance: Length = @item.distance;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "distance") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn eval_constructor_match_rejects_runtime_owner_mismatch_with_same_leaf_constructor() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a::Action = a::Pick(distance: 2.0 m);\n\
         node distance: Length = match @action {\n\
             a::Pick(distance: d) => d,\n\
             a::Idle => 0.0 m,\n\
         };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name("distance"))
        .unwrap()
        .clone();
    let expr = tir
        .declaration_body(&expr_key)
        .unwrap()
        .runtime_expression()
        .unwrap();
    let b_owner = graphcal_compiler::resolved_name::ResolvedName::for_test(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Command"),
    );
    let fields = vec![(
        graphcal_compiler::syntax::type_name::FieldName::expect_valid("distance"),
        graphcal_compiler::semantic::checked_type::CheckedType::Quantity(
            graphcal_compiler::dimension::Dimension::dimensionless(),
        ),
        graphcal_eval::eval_expr::RuntimeValue::quantity(9.0).unwrap(),
    )];
    let values = HashMap::from([(
        tir.root()
            .body_for_test()
            .lookup_decl_identity(&scoped_name("action"))
            .into_bound()
            .unwrap(),
        graphcal_eval::eval_expr::RuntimeValue::Struct(
            graphcal_eval::runtime_value::StructValue::for_test(
                b_owner,
                graphcal_compiler::syntax::type_name::ConstructorName::expect_valid("Pick"),
                fields,
            ),
        ),
    )]);
    let src = project.root_file().source_id();
    let sources = project.sources();
    let ctx = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .for_decl(&expr_key);

    let err = graphcal_eval::eval_expr::eval_root(&ctx.executable(expr).unwrap(), &values, &ctx)
        .unwrap_err();
    // A value of another owner contradicts the checked type: no arm is
    // selected by leaf name, and the violation is an internal error.
    match err {
        Outcome::Failed(SemanticError::Internal(internal)) => {
            assert!(
                internal.message().contains("no match arm for variant"),
                "{}",
                internal.message()
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn eval_struct_field_constraints_use_resolved_owner_with_same_leaf_types_and_fields() {
    let (_dir, root) = write_same_leaf_same_field_constrained_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_ok: a::Item = a::Item(value: 2.0 m);\n\
         node b_bad: b::Item = b::Item(value: 2.0 m);\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let a_ok = result
        .nodes()
        .find(|(n, _)| n.to_string() == "a_ok")
        .expect("node a_ok")
        .1
        .as_ref();
    assert!(
        a_ok.is_ok(),
        "a_ok should satisfy a::Item's constraint: {a_ok:?}"
    );
    let b_bad = result
        .nodes()
        .find(|(n, _)| n.to_string() == "b_bad")
        .expect("node b_bad")
        .1
        .as_ref();
    match b_bad {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert!(message.contains("minimum"), "{message}");
        }
        other => panic!("expected b_bad constraint failure, got {other:?}"),
    }
}

#[test]
fn project_declared_type_preserves_same_leaf_index_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");

    let graphcal_compiler::semantic::checked_type::CheckedType::Indexed { index, .. } =
        root_decl_type(&tir, "series").declared()
    else {
        panic!("expected indexed declared type for `series`");
    };
    assert_eq!(index.display_name().to_string(), "Phase");
    assert_eq!(
        index
            .declared_resolved()
            .map(graphcal_compiler::resolved_name::ResolvedName::owner,),
        Some(&a_id)
    );
}

#[test]
fn project_declared_type_preserves_same_leaf_struct_owner() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node other: b::Item = b::Item(duration: 3.0 s);\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");

    let graphcal_compiler::semantic::checked_type::CheckedType::Struct(item, _) =
        root_decl_type(&tir, "item").declared()
    else {
        panic!("expected struct declared type for `item`");
    };
    let graphcal_compiler::semantic::checked_type::CheckedType::Struct(other, _) =
        root_decl_type(&tir, "other").declared()
    else {
        panic!("expected struct declared type for `other`");
    };
    assert_eq!(item.name().as_str(), "Item");
    assert_eq!(other.name().as_str(), "Item");
    assert_eq!(item.resolved().owner(), &a_id);
    assert_eq!(other.resolved().owner(), &b_id);
}

#[test]
fn project_struct_field_constraints_preserve_same_leaf_struct_owner() {
    let (_dir, root) = write_same_leaf_constrained_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a::Item = a::Item(distance: 2.0 m);\n\
         node other: b::Item = b::Item(duration: 3.0 s);\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let src = project.root_file().source_id();
    let sources = project.sources();
    let constraints = graphcal_eval::execution_check::resolve_struct_field_constraints(
        &tir,
        &HashMap::new(),
        src,
        sources,
    )
    .unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");

    assert!(constraints.keys().any(|key| {
        key.owning_type.resolved().owner() == &a_id
            && key.owning_type.name().as_str() == "Item"
            && key.constructor.as_str() == "Item"
            && key.field.as_str() == "distance"
    }));
    assert!(constraints.keys().any(|key| {
        key.owning_type.resolved().owner() == &b_id
            && key.owning_type.name().as_str() == "Item"
            && key.constructor.as_str() == "Item"
            && key.field.as_str() == "duration"
    }));
    assert!(
        constraints
            .keys()
            .all(|key| !key.owning_type.resolved().owner().segments().is_empty())
    );
}

#[test]
fn nat_generic_arguments_work_in_types_constructors_and_nested_expressions() {
    let source = r"
pub type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

pub type Matrix<M: Nat, N: Nat> {
    Matrix(value: Fixed<M * N + 1>),
}

param value: Matrix<2, 3> = Matrix<2, 3>(
    value: Fixed<7>(value: 1.0),
);
";

    let result = compile_and_eval(source).unwrap();
    let value = result
        .params()
        .find(|(name, _)| name.to_string() == "value")
        .expect("value param")
        .1
        .as_ref();
    assert!(value.is_ok(), "Nat-generic value failed: {value:?}");
}

#[test]
fn sorted_generic_defaults_are_substituted_in_type_and_constructor_positions() {
    let source = r"
pub index Component = { X, Y, Z };
pub type Marker { Marker }
pub type Fixed<
    D: Dim,
    I: Index = Component,
    F: Type = Marker,
    N: Nat = 3,
> {
    Fixed(value: D),
}

pub type AllDefaults<
    D: Dim = Dimensionless,
    I: Index = Component,
    F: Type = Marker,
    N: Nat = 3,
> {
    AllDefaults(value: D),
}

param value: Fixed<Dimensionless> = Fixed<Dimensionless>(value: 1.0);
param all_defaults: AllDefaults = AllDefaults(value: 2.0);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn dependent_defaults_compose_across_all_generic_sorts() {
    let source = r"
pub index Phase = { A };
pub type Marker { Marker }
pub type Fixed<N: Nat> { Fixed(value: Dimensionless) }
pub type Bundle<
    D: Dim,
    I: Index,
    T: Type,
    N: Nat,
    E: Dim = D * D,
    J: Index = I,
    U: Type = T,
    M: Nat = N + 1,
> {
    Bundle(scalar: E, payload: U, series: E[J], fixed: Fixed<M>),
}

param value: Bundle<Length, Phase, Marker, 2> = Bundle<Length, Phase, Marker, 2>(
    scalar: 1.0 m^2,
    payload: Marker,
    series: { Phase#A: 2.0 m^2 },
    fixed: Fixed<3>(value: 1.0),
);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn dependent_nat_defaults_compose_through_nested_type_defaults() {
    let source = r"
pub type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

pub type Holder<N: Nat, M: Nat = N + 1, F: Type = Fixed<M>> {
    Holder(value: F),
}

param value: Holder<2> = Holder<2>(value: Fixed<3>(value: 1.0));
";

    compile_and_eval(source).unwrap();
}

#[test]
fn generic_defaults_may_reference_only_earlier_parameters() {
    let error = compile_and_eval(
        "pub type Invalid<N: Nat = M, M: Nat = 3> { Invalid(value: Dimensionless) }",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("default for generic parameter `N` may reference only earlier generic parameters; `M` is not earlier"),
        "unexpected error: {error}"
    );
}

#[test]
fn generic_defaults_must_form_a_trailing_parameter_suffix() {
    let error =
        compile_and_eval("pub type Invalid<N: Nat = 3, M: Nat> { Invalid(value: Dimensionless) }")
            .unwrap_err();
    assert!(
        error.to_string().contains(
            "generic parameter `M` without a default cannot follow defaulted parameter `N`"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn nested_type_generic_arguments_apply_inner_sorted_defaults() {
    let source = r"
pub type Fixed<D: Dim, N: Nat = 3> {
    Fixed(value: D),
}

pub type Holder<F: Type> {
    Holder(value: F),
}

param value: Holder<Fixed<Dimensionless>> = Holder<Fixed<Dimensionless>>(
    value: Fixed<Dimensionless>(value: 1.0),
);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn multi_declaration_supports_finite_shared_axes() {
    let source = r"
param x: Dimensionless[Fin(2)],
param y: Dimensionless[Fin(2)]
  = table[Fin(2), (_, _)] {
      : _, _;
      1.0, 2.0;
      3.0, 4.0;
  };
";
    let result = compile_and_eval(source).unwrap();
    for name in ["x", "y"] {
        let value = result
            .params()
            .find(|(candidate, _)| candidate.to_string() == name)
            .and_then(|(_, value)| value.as_ref().ok())
            .expect("finite multi-decl param");
        assert!(matches!(value, Value::Indexed { entries, .. } if entries.len() == 2));
    }
}

#[test]
fn finite_indexes_remain_distinct_from_nat_generic_arguments() {
    let source = r"
pub type Vector<N: Nat, D: Dim> {
    Vector(values: D[Fin(N)]),
}
pub type IndexedVector<I: Index, D: Dim> {
    IndexedVector(values: D[I]),
}
pub type DefaultIndexed<I: Index = Fin(2)> {
    DefaultIndexed(values: Dimensionless[I]),
}

param defaulted: DefaultIndexed = DefaultIndexed(
    values: table[Fin(2)] { 7.0; 8.0; },
);
param vector: Vector<3, Dimensionless> = Vector<3, Dimensionless>(
    values: table[Fin(3)] { 1.0; 2.0; 3.0; },
);
param indexed: IndexedVector<Fin(3), Dimensionless> =
    IndexedVector<Fin(3), Dimensionless>(
        values: table[Fin(3)] { 4.0; 5.0; 6.0; },
    );
";

    compile_and_eval(source).unwrap();
}

#[test]
fn generic_arguments_reject_sort_crossing() {
    let cases = [
        (
            "pub type ByIndex<I: Index> { ByIndex(value: Dimensionless) }\nparam value: ByIndex<3>;",
            "expected Index, found Nat `3`",
        ),
        (
            "pub type ByNat<N: Nat> { ByNat(value: Dimensionless) }\nparam value: ByNat<Fin(3)>;",
            "sort `Nat`",
        ),
        (
            "pub index Phase = { A };\npub type ByNat<N: Nat> { ByNat(value: Dimensionless) }\nparam value: ByNat<Phase>;",
            "sort `Nat`",
        ),
        (
            "pub index Phase = { A };\npub type ByType<T: Type> { ByType(value: T) }\nparam value: ByType<Dimensionless[Phase]>;",
            "sort `Type`",
        ),
    ];

    for (source, expected) in cases {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn bare_nat_index_positions_are_rejected() {
    for source in [
        "param value: Dimensionless[3];",
        "pub type Sized<N: Nat> { Sized(values: Dimensionless[N]) }",
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains("expected Index, found Nat"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn finite_index_cardinality_is_validated_before_allocation() {
    for (source, expected) in [
        (
            "param value: Dimensionless[Fin(0)];",
            "Fin(0) is not allowed",
        ),
        (
            "param value: Dimensionless[Fin(1000001)];",
            "exceeds the practical limit",
        ),
        (
            "pub type Vector<N: Nat> { Vector(values: Dimensionless[Fin(N)]) }\nparam value: Vector<0>;",
            "Fin(0) is not allowed",
        ),
        (
            "pub type Vector<N: Nat> { Vector(values: Dimensionless[Fin(N)]) }\nparam value: Vector<1000001>;",
            "exceeds the practical limit",
        ),
        (
            "pub type InvalidDefault<I: Index = Fin(0)> { InvalidDefault(value: Dimensionless) }",
            "Fin(0) is not allowed",
        ),
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn non_empty_function_generic_arguments_are_rejected() {
    let error = compile_and_eval("node value: Dimensionless = sqrt<3>(4.0);").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("function `sqrt` does not accept generic arguments"),
        "unexpected error: {error}"
    );
}

#[test]
fn project_generic_struct_defaults_preserve_same_leaf_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    let module_source = "pub type Marker { Marker }\n\
         pub type Wrap<D: Dim, F: Type = Marker> { Wrap(value: D) }\n";
    std::fs::write(root_dir.join("a.gcl"), module_source).unwrap();
    std::fs::write(root_dir.join("b.gcl"), module_source).unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_wrap: a::Wrap<Length> = a::Wrap<Length>(value: 1.0 m);\n\
         node b_wrap: b::Wrap<Time> = b::Wrap<Time>(value: 1.0 s);\n",
    )
    .unwrap();

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let marker_owner = |decl: &str| {
        let graphcal_compiler::tir::typed::ResolvedDeclType::Value(
            graphcal_compiler::tir::typed::ResolvedValueType::Struct {
                name: wrap,
                generic_args,
                ..
            },
        ) = root_decl_type(&tir, decl).resolved()
        else {
            panic!("expected generic struct annotation for `{decl}`");
        };
        assert_eq!(wrap.as_str(), "Wrap");
        let graphcal_compiler::tir::typed::ResolvedGenericArg::Type(
            graphcal_compiler::tir::typed::ResolvedValueType::Struct {
                name: marker_resolved,
                ..
            },
        ) = &generic_args[1]
        else {
            panic!(
                "expected default marker type arg for `{decl}`, got {:?}",
                generic_args[1]
            );
        };
        assert_eq!(marker_resolved.as_str(), "Marker");
        marker_resolved.owner().clone()
    };

    assert_eq!(marker_owner("a_wrap"), a_id);
    assert_eq!(marker_owner("b_wrap"), b_id);
}

#[test]
fn project_index_access_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n\
         node burn: Dimensionless = @series[a::Phase#Burn];\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_index_access_rejects_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n\
         node bad: Dimensionless = @series[b::Phase#Warm];\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_for_comp_rejects_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = for p: b::Phase { 1.0 };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Dimension(DimensionError::DimensionMismatchInAnnotation {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected DimensionMismatchInAnnotation, got {other:?}"),
    }
}

#[test]
fn project_map_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             a::Phase#Coast: 2.0,\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_map_literal_rejects_same_leaf_wrong_owner_key() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             b::Phase#Warm: 2.0,\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_map_literal_missing_variants_uses_resolved_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::MissingVariants { missing, .. }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(missing.len(), 1);
            assert_eq!(
                missing[0]
                    .as_named()
                    .expect("Phase is a named index")
                    .as_str(),
                "Coast"
            );
        }
        other => panic!("expected MissingVariants, got {other:?}"),
    }
}

#[test]
fn project_table_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = table[a::Phase] {\n\
             Burn: 1.0;\n\
             Coast: 2.0;\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_variant_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             a::Phase#Coast: 2.0,\n\
         };\n\
         node burn: Dimensionless = @series[a::Phase#Burn];\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_label_match_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => 1.0,\n\
                 a::Phase#Coast => 2.0,\n\
             }\n\
         };\n\
         node burn_code: Dimensionless = @code[a::Phase#Burn];\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "burn_code") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn project_label_match_rejects_same_leaf_wrong_owner_pattern() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => 1.0,\n\
                 b::Phase#Warm => 2.0,\n\
             }\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(IndexError::IndexMismatch { .. }),
                    ..
                }),
            ..
        })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_expected_fail_keys_accept_resolved_index_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_checks: Bool[a::Phase] = {\n\
             a::Phase#Burn: false,\n\
             a::Phase#Coast: true,\n\
         };\n\
         #[expected_fail(a::Phase#Burn)]\n\
         assert a_expected = @a_checks;\n\
         node b_checks: Bool[b::Phase] = {\n\
             b::Phase#Burn: false,\n\
             b::Phase#Coast: true,\n\
         };\n\
         #[expected_fail(b::Phase#Burn)]\n\
         assert b_expected = @b_checks;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let assert_result = |name: &str| {
        result
            .assertions
            .iter()
            .find(|(assert_name, _, _)| assert_name.to_string() == name)
            .unwrap_or_else(|| panic!("assertion `{name}` not found"))
            .1
            .clone()
    };
    assert_eq!(assert_result("a_expected"), AssertResult::Pass);
    assert_eq!(assert_result("b_expected"), AssertResult::Pass);
}

#[test]
fn included_expected_fail_resolves_index_in_its_defining_module() {
    // Regression for #1200: include assembly renames the assertion, but its
    // expected-fail index key remains scoped to and sourced from lib.gcl.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/include_expected_fail_scope/src/repro/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let (_, assertion, _) = result
        .assertions
        .iter()
        .find(|(name, _, _)| name.to_string().contains("check"))
        .expect("included assertion not found");
    assert_eq!(*assertion, AssertResult::Pass);
}

#[test]
fn project_expected_fail_keys_reject_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node b_checks: Bool[b::Phase] = {\n\
             b::Phase#Burn: false,\n\
             b::Phase#Coast: true,\n\
         };\n\
         #[expected_fail(a::Phase#Burn)]\n\
         assert b_expected = @b_checks;\n",
    );

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Attribute(AttributeError::ExpectedFailKeyIndexMismatch {
                            ..
                        }),
                    ..
                }),
            ..
        })) => {}
        other => {
            panic!(
                "expected ExpectedFailKeyIndexMismatch for foreign expected_fail key, got {other:?}"
            )
        }
    }
}

#[test]
fn eval_index_collections_preserve_same_leaf_owners_across_runtime_boundaries() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a::{ index Phase };\n\
         import collide.a as a;\n\
         import collide.b as b;\n\
         dag pick_a {\n\
             import collide.a::{ index Phase };\n\
             param series: Dimensionless[Phase];\n\
             pub node burn: Dimensionless = @series[Phase#Burn];\n\
             pub node echoed: Dimensionless[Phase] = for p: Phase { @series[p] };\n\
         }\n\
         node map_a: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 10.0,\n\
             a::Phase#Coast: 20.0,\n\
         };\n\
         node map_b: Dimensionless[b::Phase] = {\n\
             b::Phase#Burn: 1.0,\n\
             b::Phase#Coast: 2.0,\n\
         };\n\
         node table_a: Dimensionless[Phase] = table[Phase] {\n\
             Burn: 3.0;\n\
             Coast: 4.0;\n\
         };\n\
         node for_a: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => @map_a[p],\n\
                 a::Phase#Coast => @pick_a(series: @map_a)::echoed[p],\n\
             }\n\
         };\n\
         node scan_a: Dimensionless[a::Phase] = scan(@map_a, 0.0, |acc, val| acc + val);\n\
         node total: Dimensionless = @pick_a(series: @map_a)::burn\n\
             + @table_a[Phase#Burn]\n\
             + @for_a[a::Phase#Coast]\n\
             + @scan_a[a::Phase#Coast];\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "total") - 63.0).abs() < f64::EPSILON);

    let owner_of_indexed = |name: &str| {
        let value = result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("node `{name}` failed: {e}"));
        let Value::Indexed { index_name, .. } = value else {
            panic!("expected indexed value for `{name}`, got {value:?}");
        };
        index_name
            .declared_resolved()
            .cloned()
            .unwrap_or_else(|| panic!("expected declared index for `{name}`"))
    };

    let a_owner = owner_of_indexed("map_a");
    let b_owner = owner_of_indexed("map_b");
    assert_ne!(a_owner, b_owner);
    assert_eq!(owner_of_indexed("table_a"), a_owner);
    assert_eq!(owner_of_indexed("for_a"), a_owner);
    assert_eq!(owner_of_indexed("scan_a"), a_owner);
}

#[test]
fn eval_unfold_uses_resolved_explicit_range_index_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_range_index_project(
        "import collide.b as b;\n\
         import collide.a as a;\n\
         node y: Dimensionless[a::Step] = unfold(a::Step, 0.0, |prev_y, prev_t, t| prev_y + 1.0);\n",
    );

    let (_tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_owner = loaded_file_dag_id(&project, "a.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let value = result
        .nodes()
        .find(|(name, _)| name.to_string() == "y")
        .expect("node y")
        .1
        .as_ref()
        .expect("node y value");
    let Value::Indexed {
        index_name,
        entries,
        ..
    } = value
    else {
        panic!("expected indexed value for `y`, got {value:?}");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(
        index_name
            .declared_resolved()
            .map(graphcal_compiler::resolved_name::ResolvedName::owner),
        Some(&a_owner)
    );
}

#[test]
fn eval_index_access_rejects_runtime_owner_mismatch_with_same_leaf_variant() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a::Phase] = {\n\
             a::Phase#Burn: 1.0,\n\
             a::Phase#Coast: 2.0,\n\
         };\n\
         node burn: Dimensionless = @series[a::Phase#Burn];\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name("burn"))
        .unwrap()
        .clone();
    let expr = tir
        .declaration_body(&expr_key)
        .unwrap()
        .runtime_expression()
        .unwrap();
    let b_owner = graphcal_compiler::resolved_name::ResolvedName::for_test(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Phase"),
    );
    let b_axis = graphcal_eval::runtime_value::IndexAxis::resolve(
        &tir,
        &graphcal_compiler::semantic::checked_type::IndexTypeRef::from_resolved(b_owner),
    )
    .unwrap();
    let entries = graphcal_eval::runtime_value::IndexedValue::for_test(
        b_axis,
        vec![
            graphcal_eval::eval_expr::RuntimeValue::quantity(99.0).unwrap(),
            graphcal_eval::eval_expr::RuntimeValue::quantity(100.0).unwrap(),
        ],
    );
    let values = HashMap::from([(
        tir.root()
            .body_for_test()
            .lookup_decl_identity(&scoped_name("series"))
            .into_bound()
            .unwrap(),
        graphcal_eval::eval_expr::RuntimeValue::Indexed(entries),
    )]);
    let src = project.root_file().source_id();
    let sources = project.sources();
    let ctx = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .for_decl(&expr_key);

    let err = graphcal_eval::eval_expr::eval_root(&ctx.executable(expr).unwrap(), &values, &ctx)
        .unwrap_err();
    // A key of another owner contradicts the checked type: no entry is
    // selected by leaf name, and the violation is an internal error.
    match err {
        Outcome::Failed(SemanticError::Internal(internal)) => {
            assert!(
                internal.message().contains("checked index entry"),
                "{}",
                internal.message()
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

#[test]
fn eval_label_match_rejects_runtime_owner_mismatch_with_same_leaf_variant() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a::Phase] = for p: a::Phase {\n\
             match p {\n\
                 a::Phase#Burn => 1.0,\n\
                 a::Phase#Coast => 2.0,\n\
             }\n\
         };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .body_for_test()
        .bound_decl_identity(&scoped_name("code"))
        .unwrap()
        .clone();
    let expr = tir
        .declaration_body(&expr_key)
        .unwrap()
        .runtime_expression()
        .unwrap();
    let root = expr.executable().unwrap();
    let tree = root.root();
    let graphcal_compiler::tir::typed::scoped_node::NodeKind::For {
        bindings,
        body: match_expr,
    } = tree.kind()
    else {
        panic!("expected `code` to be a for-comprehension, got {tree:?}");
    };
    let [binding] = bindings.as_slice() else {
        panic!("expected one for-comprehension binding, got {bindings:?}");
    };
    let binding = &binding.binding;
    let b_owner = graphcal_compiler::resolved_name::ResolvedName::for_test(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Phase"),
    );
    let values = HashMap::new();
    let local_values = graphcal_eval::eval_expr::HirLocalValueMap::from_bindings(vec![(
        binding.local.id,
        graphcal_eval::runtime_presentation::EvaluatedRuntimeValue::plain(
            graphcal_eval::eval_expr::RuntimeValue::Key(
                graphcal_eval::runtime_value::KeyValue::for_entry(
                    graphcal_eval::runtime_value::IndexAxis::resolve(
                        &tir,
                        &graphcal_compiler::semantic::checked_type::IndexTypeRef::from_resolved(
                            b_owner,
                        ),
                    )
                    .unwrap(),
                    &graphcal_compiler::syntax::index_name::IndexEntryKey::named(
                        graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid(
                            "Burn",
                        ),
                    ),
                )
                .unwrap(),
            ),
        ),
    )]);
    let src = project.root_file().source_id();
    let sources = project.sources();
    let ctx = graphcal_eval::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .for_decl(&expr_key);

    let err =
        graphcal_eval::eval_expr::eval_subtree_for_test(match_expr, &values, &local_values, &ctx)
            .unwrap_err();
    // A value of another owner contradicts the checked type: no arm is
    // selected by leaf name, and the violation is an internal error.
    match err {
        Outcome::Failed(SemanticError::Internal(internal)) => {
            assert!(
                internal.message().contains("no match arm for label"),
                "{}",
                internal.message()
            );
        }
        other => panic!("expected InternalError, got {other:?}"),
    }
}

// ---- Bare module path eval tests ----
mod prop {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn division_of_finite_nonzero_is_finite_or_reports_range_loss(
            a in proptest::num::f64::NORMAL,
            b in proptest::num::f64::NORMAL,
        ) {
            prop_assume!(b != 0.0 && a.is_finite() && b.is_finite());
            let source = format!(
                "param x: Dimensionless = {a:e};\nparam y: Dimensionless = {b:e};\nnode z: Dimensionless = @x / @y;"
            );
            let r = compile_and_eval(&source).unwrap();
            let z_result = &r.entries.iter()
                .find(|(n, _, _)| n.to_string() == "z")
                .unwrap().1;
            match z_result {
                Ok(val) => {
                    let z = val.si_value().unwrap().get();
                    prop_assert!(z.is_finite(), "division produced non-finite: {z}");
                }
                Err(NodeUnavailable::EvalFailed { message }) => {
                    // Complete underflow and overflow are both surfaced rather
                    // than silently becoming zero or infinity.
                    prop_assert!(
                        message.contains("underflow")
                            || message.contains("overflow")
                            || message.contains("infinite"),
                        "unexpected error: {message}"
                    );
                }
                Err(e) => prop_assert!(false, "unexpected error type: {e:?}"),
            }
        }

        #[test]
        fn sqrt_of_positive_is_finite(a in 0.0f64..1e150) {
            let source = format!(
                "param x: Dimensionless = {a:e};\nnode y: Dimensionless = sqrt(@x);"
            );
            let result = compile_and_eval(&source).unwrap();
            let y = find_value(&result, "y");
            prop_assert!(y.is_finite(), "sqrt produced non-finite: {y}");
        }

        #[test]
        fn exp_of_small_is_finite(a in -700.0f64..700.0) {
            let source = format!(
                "param x: Dimensionless = {a:e};\nnode y: Dimensionless = exp(@x);"
            );
            let result = compile_and_eval(&source).unwrap();
            let y = find_value(&result, "y");
            prop_assert!(y.is_finite(), "exp produced non-finite: {y}");
        }
    }
}

// --- Partial overrides / partial bindings tests ---

#[test]
fn cli_partial_override_uses_defaults() {
    // When overrides are provided for some params, the rest fall back to defaults.
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("isp"), parse_expr("450.0 s"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    assert!(
        result.is_ok(),
        "partial overrides should fall back to defaults: {result:?}"
    );
}

#[test]
fn import_partial_binding_uses_defaults() {
    // Parameterized import with partial binding falls back to defaults.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        result.is_ok(),
        "partial import binding should fall back to defaults: {result:?}"
    );
}

// --- Required param (no default) import tests ---

#[test]
fn project_required_param_import() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/required_param_import/src/library/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // radius = 6371 km, circumference = 2 * PI * radius
    let expected = 2.0 * std::f64::consts::PI * 6_371_000.0; // in metres (SI)
    let circumference = find_value(&result, "circumference");
    assert!(
        (circumference - expected).abs() < 1.0,
        "circumference = {circumference}, expected = {expected}"
    );
}

// --- Injectable index tests ---

#[test]
fn include_required_index_accepts_structural_finite_axis() {
    // Regression for #1077: structural axes need no fabricated named-index alias.
    let source = include_str!("../../../tests/fixtures/valid/finite_index_dag_binding.gcl");
    let result = compile_and_eval(source).unwrap();
    let Value::Indexed { entries, .. } = find_entry(&result, "out") else {
        panic!("expected finite-indexed output");
    };
    let values = entries
        .values()
        .map(|value| value.si_value().unwrap().get())
        .collect::<Vec<_>>();
    assert_eq!(values, vec![2.0, 4.0, 6.0]);
}

#[test]
fn project_injectable_index_kind_mismatch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/invalid/multi/injectable_index_kind_mismatch/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Module(ModuleError::IndexKindMismatch {
                            dep_index,
                            bound_index,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(dep_index.to_string(), "Phase");
            assert_eq!(bound_index.to_string(), "TimeStep");
        }
        other => panic!("expected IndexKindMismatch, got {other:?}"),
    }
}

#[test]
fn project_injectable_index_basic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/injectable_index_basic/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // total = sum(10.0 + 20.0) = 30.0
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 30.0).abs() < 1e-10,
        "result = {result_val}, expected 30.0"
    );
}

#[test]
fn project_instantiated_import_type_binding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_type_binding/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // origin_size = 1.0 m (the lib's `Widget { size: 1.0 m }` rewritten to
    // `MyWidget { size: 1.0 m }` after type substitution)
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 1.0).abs() < 1e-10,
        "result = {result_val}, expected 1.0"
    );
}

#[test]
fn project_instantiated_import_dim_binding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_dim_binding/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // result = 10.0 m/s; the lib's `v: Speed = 10.0 m/s` has its type_ann
    // rewritten Speed -> Velocity so main's Velocity dimension resolves.
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 10.0).abs() < 1e-10,
        "result = {result_val}, expected 10.0"
    );
}

#[test]
fn project_pub_import_reexport_selective() {
    // Issue #452: selective `import "X" { pub item }` re-exports the
    // item at the importer's visible surface, so a transitive importer
    // can reach it via the intermediate file.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/pub_import_reexport_selective/src/middle/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // result = 9.80665 m/s^2 (in the base unit, value 9.80665).
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 9.806_65).abs() < 1e-10,
        "result = {result_val}, expected 9.80665"
    );
}

#[test]
fn project_include_overrides_index_no_param_binding_v005() {
    // V005: overriding `Phase` orphans the `cost` default (which mentions
    // `Phase#Design` / `Phase#Build`) because the importer forgot to
    // re-bind `cost` in the same include statement.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/include_overrides_index_no_param_binding/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Visibility(VisibilityError::IncludeMustReconcileOverride {
                            overridden,
                            overridden_kind,
                            orphan_decl,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(overridden.as_str(), "Phase");
            assert_eq!(overridden_kind.to_string(), "index");
            assert_eq!(orphan_decl.as_str(), "cost");
        }
        other => panic!("expected IncludeMustReconcileOverride, got {other:?}"),
    }
}

#[test]
fn project_include_overrides_index_with_param_binding_ok() {
    // Positive companion to project_include_overrides_index_no_param_binding_v005:
    // supplying a fresh `cost` binding satisfies A8.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/multi/include_overrides_index_with_param_binding/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let result_val = find_value(&result, "result");
    // total = 10 + 20 = 30
    assert!(
        (result_val - 30.0).abs() < 1e-10,
        "result = {result_val}, expected 30.0"
    );
}

#[test]
fn template_closure_error_renders_against_dependency_source() {
    // Option A checks reusable bodies with bindable Static ports rigid. The
    // V007 diagnostic belongs to the dependency body and must retain that
    // file's source and an in-bounds span.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/invalid/multi/merged_dep_body_dim_mismatch/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(rendered)) => {
            let SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Visibility(VisibilityError::TemplateBodyDependsOnStaticDefault {
                        body_name,
                        port_name,
                        ..
                    }),
                primary: span,
                ..
            }) = &rendered.error
            else {
                panic!("expected V007, got {:?}", rendered.error);
            };
            let src = rendered.named_source();
            assert_eq!(body_name.as_str(), "v");
            assert_eq!(port_name.as_str(), "Speed");
            assert!(
                src.name().ends_with("lib.gcl"),
                "diagnostic must name the dependency file, got `{}`",
                src.name()
            );
            // The span must index into the source it renders against.
            let len = src.inner().len();
            assert!(
                span.offset() + span.len() <= len,
                "span (offset {}, len {}) is out of bounds for `{}` of length {len}",
                span.offset(),
                span.len(),
                src.name(),
            );
        }
        other => panic!("expected V007 against the dependency source, got {other:?}"),
    }
}

#[test]
fn project_selective_include_leaks_private_type_v006() {
    // V006: selective `{ pub origin }` re-exports a declaration whose effective
    // signature names importer-private `PrivateInner` after substitution.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/pub_include_leaks_private_type/src/container/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Visibility(VisibilityError::GenericsLeakage {
                            reexport_name,
                            leaked_name,
                            leaked_kind,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(reexport_name.as_str(), "origin");
            assert_eq!(leaked_name.as_str(), "PrivateInner");
            assert_eq!(leaked_kind.noun(), "type");
        }
        other => panic!("expected GenericsLeakage, got {other:?}"),
    }
}

#[test]
fn project_selective_include_rejects_private_generic_default_binding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/pub_include_leaks_private_generic_default/src/generic_default/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Visibility(VisibilityError::GenericsLeakage {
                            reexport_name,
                            leaked_name,
                            leaked_kind,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(reexport_name.as_str(), "Wrapper");
            assert_eq!(leaked_name.as_str(), "PrivateElement");
            assert_eq!(leaked_kind.noun(), "type");
        }
        other => panic!("expected generic-default GenericsLeakage, got {other:?}"),
    }
}

#[test]
fn project_selective_include_with_public_type_binding_ok() {
    // Positive companion to project_selective_include_leaks_private_type_v006:
    // binding `Element` to a `pub` importer-local type satisfies A9.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/pub_include_with_public_type_binding/src/container/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        result.is_ok(),
        "selectively re-exporting an output with a `pub` type binding should compile: {result:?}"
    );
}

#[test]
fn project_injectable_index_expected_fail() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/injectable_index_expected_fail/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // The within_limit assertion should pass overall because Overdrive is marked expected_fail.
    let assert_result = result
        .assertions
        .iter()
        .find(|(name, _, _)| name.to_string().contains("within_limit"))
        .expect("within_limit assertion not found");
    assert!(
        matches!(assert_result.1, AssertResult::Pass),
        "expected Pass, got {:?}",
        assert_result.1
    );
}

// ---- Inline DAG tests (Phase 5) ----

fn setup_inline_semantics_project(
    files: &[(&str, &str)],
    root_rel: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"sem\"\n",
    )
    .expect("write manifest");
    for (rel, source) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture parent");
        }
        std::fs::write(path, source).expect("write fixture source");
    }
    let root = dir.path().join(root_rel);
    (dir, root)
}

#[test]
fn imported_generic_field_obligation_is_checked_in_the_consumer() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub type Box<D: Dim> { Box(x: D(min: 0.5 m)) }\n",
            ),
            (
                "src/sem/main.gcl",
                "import sem.lib as lib;\n\
                 node bad: lib::Box<Time> = lib::Box<Time>(x: 1.0 s);\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn generic_argument_constraint_in_default_is_rejected_end_to_end() {
    let error = compile_and_eval(
        r"
type Wrapper<T: Type> { Wrapper(value: T) }
type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
node bad: Bad = Bad(value: Wrapper<Length>(value: -1.0 m));
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn generic_argument_constraint_in_inline_dag_type_is_rejected_end_to_end() {
    let error = compile_and_eval(
        r"
dag nested {
    type Wrapper<T: Type> { Wrapper(value: T) }
    type Bad<T: Type = Wrapper<Length(min: 0.0 m)>> { Bad(value: T) }
    node bad: Bad = Bad(value: Wrapper<Length>(value: -1.0 m));
}
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn selective_import_wrong_category_preserves_marker_and_alternatives() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node JPY: Length = 1.0 m;\n\
                 pub const unit JPY: Length = 1.0 m;\n",
            ),
            ("src/sem/main.gcl", "import sem.lib::{ dim JPY };\n"),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Module(ModuleError::ImportCategoryMismatch { mismatch, .. }),
                ..
            }),
        ..
    }) = error
    else {
        panic!("expected category mismatch, got {error:?}");
    };
    assert_eq!(mismatch.name().as_str(), "JPY");
    assert_eq!(
        mismatch.expected(),
        graphcal_compiler::syntax::ast::ImportItemNamespace::Dimension
    );
    assert_eq!(
        mismatch.alternatives().as_slice(),
        &[
            graphcal_compiler::syntax::ast::ImportItemNamespace::Term,
            graphcal_compiler::syntax::ast::ImportItemNamespace::Unit,
        ]
    );
}

#[test]
fn inline_dag_required_coordinate_index_rejects_dimension_mismatch() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include pass_through(
    index Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Index(IndexError::IndexBindingDimensionMismatch {
                            dep_index,
                            expected_dim,
                            bound_index,
                            found_dim,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(dep_index.as_str(), "Step");
            assert_eq!(expected_dim.to_string(), "Time");
            assert_eq!(bound_index.to_string(), "DistanceStep");
            assert_eq!(found_dim.to_string(), "Length");
        }
        other => panic!("expected IndexBindingDimensionMismatch, got {other:?}"),
    }
}

#[test]
fn inline_dag_required_coordinate_index_rejects_named_index() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index Phase = { Start, End };
include pass_through(
    index Step: Phase,
    samples: {
        Phase#Start: 1.0 m,
        Phase#End: 2.0 m,
    },
) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Module(ModuleError::IndexKindMismatch {
                            dep_index,
                            dep_kind,
                            bound_index,
                            bound_kind,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(dep_index.to_string(), "Step");
            assert_eq!(dep_kind.to_string(), "coordinate");
            assert_eq!(bound_index.to_string(), "Phase");
            assert_eq!(bound_kind.to_string(), "named");
        }
        other => panic!("expected IndexKindMismatch, got {other:?}"),
    }
}

#[test]
fn inline_dag_required_index_missing_binding_uses_index_diagnostic() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

include pass_through(samples: 1.0 m) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Index(IndexError::RequiredStaticInputNotBound {
                            name, ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.as_str(), "Step");
        }
        other => panic!("expected RequiredStaticInputNotBound, got {other:?}"),
    }
}

#[test]
fn inline_dag_coordinate_contract_applies_simultaneous_dimension_binding() {
    let source = r"
dag pass_through {
    pub(bind) dim AxisDim;
    pub(bind) index Step: AxisDim;
    param samples: AxisDim[Step];
    pub node result: AxisDim[Step] = @samples;
}

pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include pass_through(
    dim AxisDim: Time,
    index Step: TimeStep,
    samples: for time: TimeStep { coord(time) },
) as output;
";

    compile_and_eval(source).unwrap();
}

#[test]
fn nested_dag_can_forward_compatible_required_coordinate_index() {
    let source = r"
dag inner {
    pub(bind) index InnerStep: Time;
    pub node n: Int = count(for step: InnerStep { step });
}

dag wrapper {
    pub(bind) index OuterStep: Time;
    include inner(index InnerStep: OuterStep) as inner_run;
    pub node n: Int = 1;
}

pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include wrapper(index OuterStep: TimeStep) as output;
";

    compile_and_eval(source).unwrap();
}

#[test]
fn file_dag_coordinate_contract_applies_simultaneous_dimension_binding() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                r"
pub(bind) dim AxisDim;
pub(bind) index Step: AxisDim;
param samples: AxisDim[Step];
pub node result: AxisDim[Step] = @samples;
",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include sem.library(
    dim AxisDim: Time,
    index Step: TimeStep,
    samples: for time: TimeStep { coord(time) },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
}

#[test]
fn cross_file_inline_dag_rejects_coordinate_dimension_mismatch() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                r"
pub dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}
",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include sem.library.pass_through(
    index Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        matches!(
            result,
            Err(CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Index(
                        IndexError::IndexBindingDimensionMismatch { .. }
                    ),
                    ..
                }),
                ..
            }))
        ),
        "expected coordinate dimension mismatch, got {result:?}"
    );
}

#[test]
fn file_dag_index_dimension_diagnostic_uses_importer_source_span() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                "pub(bind) index Step: Time;\nparam samples: Length[Step];\n",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include sem.library(
    index Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    let diagnostic: &dyn miette::Diagnostic = &error;
    let mut rendered = String::new();
    miette::NarratableReportHandler::new()
        .render_report(&mut rendered, diagnostic)
        .unwrap();

    assert!(rendered.contains("index Step: DistanceStep"), "{rendered}");
    assert!(!rendered.contains("OutOfBounds"), "{rendered}");
}

#[test]
fn inline_dag_rejects_parent_type_without_import() {
    let source = "\
pub dim Speed = Length / Time;

// `Speed` is deliberately not imported by the DAG body.
dag analyze {
    param v: Speed;
    pub node out: Speed = @v;
}

param input: Speed = 1.0 m/s;
node result: Speed = @analyze(v: @input)::out;
";

    let result = compile_and_eval_named(source, "test.gcl");
    assert!(
        result.is_err(),
        "inline DAG body should not inherit parent type-system names: {result:?}"
    );
}

#[test]
fn inline_dag_body_import_drives_dependency_loading() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 3.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
dag scaled {
    import sem.lib::{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}
node result: Dimensionless = @scaled(x: 2.0)::out;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    assert_eq!(
        project.files().len(),
        2,
        "inline DAG body import should load its dependency"
    );
}

#[test]
fn inline_dag_body_value_import_is_available_to_call() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 3.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
import sem.lib as lib;

dag scaled {
    import sem.lib::{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}
node result: Dimensionless = @scaled(x: 2.0)::out;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 6.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn include_inside_inline_dag_body_is_available_to_call() {
    let source = "\
dag inner {
    param x: Dimensionless;
    pub node y: Dimensionless = @x + 1.0;
}

dag wrapper {
    param z: Dimensionless;
    include inner(x: @z)::{ y };
    pub node out: Dimensionless = @y + 1.0;
}

node result: Dimensionless = @wrapper(z: 2.0)::out;
";

    let result = compile_and_eval(source).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 4.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn nested_inline_dag_is_compilable_from_parent_body() {
    let source = "\
dag wrapper {
    dag inner {
        pub node result: Dimensionless = 4.0;
    }
    pub node out: Dimensionless = @inner()::result + 1.0;
}

node result: Dimensionless = @wrapper()::out;
";

    let result = compile_and_eval(source).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 5.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn same_file_inline_dag_include_rejects_private_selected_output() {
    let source = "\
dag stage {
    param thrust: Force;
    node internal_scratch: Force = @thrust * 2.0;
    pub node reported: Force = @thrust;
}

include stage(thrust: 10.0 N)::{ internal_scratch };
node used: Force = @internal_scratch;
";

    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Visibility(VisibilityError::ImportPrivateItem { name, .. }), .. }), .. })
            if name.as_str() == "internal_scratch"
    ));
}

#[test]
fn same_file_inline_dag_include_alias_hides_private_output() {
    let source = "\
dag stage {
    param thrust: Force;
    node internal_scratch: Force = @thrust * 2.0;
    pub node reported: Force = @thrust;
}

include stage(thrust: 10.0 N) as inst;
node used: Force = @inst::internal_scratch;
";

    let error = compile_and_eval(source).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Visibility(VisibilityError::ImportPrivateItem { name, .. }), .. }), .. })
            if name.as_str() == "internal_scratch"
    ));
}

#[test]
fn inline_dag_include_and_call_share_body_import_semantics() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 2.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
dag scaled {
    import sem.lib::{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}

include scaled(x: 3.0)::{ out as included };
node called: Dimensionless = @scaled(x: 3.0)::out;
node difference: Dimensionless = @included - @called;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let included = find_value(&result, "included");
    let called = find_value(&result, "called");
    let difference = find_value(&result, "difference");
    assert!((included - 6.0).abs() < 1e-10, "included = {included}");
    assert!((called - 6.0).abs() < 1e-10, "called = {called}");
    assert!(difference.abs() < 1e-10, "difference = {difference}");
}

#[test]
fn inline_dag_include_selected_adt_output_uses_producer_scope() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}

pub dag make_payload {
    import sem.lib::{ type Payload, Payload };

    pub node out: Payload = Payload(x: 1.0);
}
",
            ),
            (
                "src/sem/main.gcl",
                "include sem.lib.make_payload()::{ out };\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn inline_dag_include_adt_param_default_uses_producer_scope() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}

pub dag use_default {
    import sem.lib::{ type Payload, Payload };

    param p: Payload = Payload(x: 1.0);
    pub node y: Dimensionless = @p.x;
}
",
            ),
            (
                "src/sem/main.gcl",
                "include sem.lib.use_default()::{ y };\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 1.0).abs() < 1e-10, "y = {y}");
}

#[test]
fn consumer_still_needs_import_for_explicit_adt_names() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}
",
            ),
            (
                "src/sem/main.gcl",
                "\
import sem.lib;
node p: Payload = Payload(x: 1.0);
",
            ),
        ],
        "src/sem/main.gcl",
    );

    assert!(
        compile_to_tir_project(&root, None, &fs()).is_err(),
        "consumer-written Payload names must still require an import"
    );
}

#[test]
fn inline_dag_basic_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/inline_dag_basic/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let val = find_value(&result, "final_result");
    assert!((val - 20.0).abs() < 1e-10, "expected 20.0, got {val}");
}

#[test]
fn inline_dag_include_selects_effective_param_value() {
    let source = "\
dag default_dag {
    param x: Dimensionless = 1.0;
}

dag bound_dag {
    param x: Dimensionless = 1.0;
}

include default_dag()::{ x as default_x };
include bound_dag(x: 2.0)::{ x as bound_x };
node combined: Dimensionless = @default_x + @bound_x;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "default_x") - 1.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_x") - 2.0).abs() < 1e-10);
    assert!((find_value(&result, "combined") - 3.0).abs() < 1e-10);
}

#[test]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "Graphcal source uses `{result}` as a brace-list selector, not a format arg"
)]
fn inline_dag_recursive_error() {
    // Direct recursion: dag includes itself.
    let source = r"
dag recursive {
    param x: Dimensionless;
    include recursive(x: 1.0)::{result};
    node result: Dimensionless = @x;
}
include recursive(x: 1.0)::{result};
";
    let result = compile_and_eval(source);
    assert!(result.is_err(), "recursive DAG should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("recursive DAG instantiation"),
        "error should mention recursive DAG: {err_msg}"
    );
}

#[test]
fn include_closure_cycles_are_rejected_at_the_including_declaration() {
    // Regression: a cycle through an include port binding used to pass
    // `check` and fail at evaluation with an internal error.
    let source = "dag lib { param x: Dimensionless; pub node out: Dimensionless = @x + 1.0; }\n\
                  include lib(x: @a) as inst;\n\
                  node a: Dimensionless = @inst::out;";
    match compile_and_eval(source) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Graph(GraphError::CyclicDependency { name, .. }),
                    primary: span,
                    ..
                }),
            ..
        })) => {
            assert_eq!(name.to_string(), "a");
            assert_eq!(span.offset(), source.find("node a").unwrap());
        }
        other => panic!("expected a cyclic dependency, got {other:?}"),
    }
}

/// Message and label start of the G009 a recursive inline-DAG source reports.
fn recursive_dag_error(source: &str) -> (String, usize) {
    match compile_and_eval(source) {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Graph(kind @ GraphError::RecursiveDagInstantiation { .. }),
                    primary: span,
                    ..
                }),
            ..
        })) => (kind.to_string(), span.offset()),
        other => panic!("expected a recursive DAG instantiation error, got {other:?}"),
    }
}

#[test]
fn mutually_recursive_inline_dags_report_the_cycle_from_the_first_template() {
    let source =
        "dag first { include second() as next; }\ndag second { include first() as next; }\n";
    assert_eq!(
        recursive_dag_error(source),
        (
            "recursive DAG instantiation: first -> second -> first".to_string(),
            0
        )
    );
}

#[test]
fn nested_recursive_inline_dag_is_named_by_its_path_in_the_file() {
    let source = "dag outer {\n  dag inner { include inner() as again; }\n}\n";
    assert_eq!(
        recursive_dag_error(source),
        (
            "recursive DAG instantiation: outer.inner -> outer.inner".to_string(),
            source.find("dag inner").unwrap()
        )
    );
}

#[test]
fn inline_dag_from_source() {
    // Test inline DAG from in-memory source.
    let source = "
dag add_velocities {
    param a: Velocity;
    param b: Velocity;
    pub node combined: Velocity = @a + @b;
}

param v1: Velocity = 10.0 m/s;
param v2: Velocity = 5.0 m/s;
include add_velocities(a: @v1, b: @v2)::{combined as total};
node result: Velocity = @total;
";
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!((val - 15.0).abs() < 1e-10, "expected 15.0, got {val}");
}

// ---- Cross-file DAG tests (Phase 6.10) ----
// ---- Cross-file qualified inline dag calls (issue #467) ----#[test]#[test]// ---- Bare module path DAG reference tests ----
// ---- Inline DAG invocation (issue #451) ----

#[test]
fn eval_inline_dag_call_basic() {
    let source = "\
dag scale {
    param factor: Dimensionless;
    param v: Length;
    pub node result: Length = @v * @factor;
}

param src: Length = 10.0 m;
node doubled: Length = @scale(factor: 2.0, v: @src)::result;
";
    let result = compile_and_eval(source).unwrap();
    let doubled = find_value(&result, "doubled");
    assert!(
        (doubled - 20.0).abs() < 1e-10,
        "expected 20.0, got {doubled}"
    );
}

#[test]
fn eval_inline_dag_call_binds_reordered_args_by_name() {
    let source = "\
dag combine {
    param a: Dimensionless;
    param b: Dimensionless;
    pub node result: Dimensionless = @a * 10.0 + @b;
}

node combined: Dimensionless = @combine(b: 2.0, a: 1.0)::result;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "combined") - 12.0).abs() < 1e-10);
}

#[test]
fn eval_include_binds_reordered_args_by_name() {
    let source = "\
dag combine {
    param a: Dimensionless;
    param b: Dimensionless;
    pub node result: Dimensionless = @a * 10.0 + @b;
}

include combine(b: 2.0, a: 1.0) as combined;
node result: Dimensionless = @combined::result;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "result") - 12.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_projects_effective_param_value() {
    let source = "\
dag config {
    param factor: Dimensionless = 2.0;
}

node default_factor: Dimensionless = @config()::factor;
node bound_factor: Dimensionless = @config(factor: 3.0)::factor;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "default_factor") - 2.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_factor") - 3.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_chains_through_body_nodes() {
    // An inline call where the dag body has an intermediate node; tests that
    // earlier nodes are evaluated and visible to later ones.
    let source = "\
dag two_step {
    param v: Length;
    node mid: Length = @v * 2.0;
    pub node result: Length = @mid + 1.0 m;
}

param src: Length = 3.0 m;
node out: Length = @two_step(v: @src)::result;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    // (3 * 2) + 1 = 7
    assert!((out - 7.0).abs() < 1e-10, "expected 7.0, got {out}");
}

#[test]
fn eval_inline_dag_call_imports_parent_const_with_alias() {
    let source = "\
pub const node seed_len: Length = 3.0 m;

dag scaled {
    import test::{seed_len as imported_seed};

    param factor: Dimensionless;
    pub node result: Length = @imported_seed * @factor;
}

node out: Length = @scaled(factor: 4.0)::result;
";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let out = find_value(&result, "out");
    assert!((out - 12.0).abs() < 1e-10, "expected 12.0, got {out}");
}

#[test]
fn cached_inline_template_refines_parent_const_types_for_repeated_instances() {
    let source = "\
pub const node radius: Length = 10.0 m;

dag shifted {
    import test::{radius};

    param altitude: Length;
    pub node result: Length = @radius + @altitude;
}

include shifted(altitude: 1.0 m)::{ result as first };
include shifted(altitude: 2.0 m)::{ result as shifted_again };
";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    assert!((find_value(&result, "first") - 11.0).abs() < 1e-10);
    assert!((find_value(&result, "shifted_again") - 12.0).abs() < 1e-10);
}

#[test]
fn eval_qualified_inline_dag_call_imports_parent_const_with_alias() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/inline_dag_call_cross_file_parent_const/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let earth_half = find_value(&result, "earth_half");
    assert!(
        (earth_half - 3_185_500.0).abs() < 1e-10,
        "expected 3185500.0, got {earth_half}"
    );
    assert!((find_value(&result, "default_factor") - 1.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_factor") - 0.5).abs() < 1e-10);
}

#[test]
fn imported_file_and_inline_dag_aliases_are_both_directly_callable() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/callable_module_aliases/src/callable/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "file_result") - 4.0).abs() < 1e-10);
    assert!((find_value(&result, "file_param_result") - 7.0).abs() < 1e-10);
    assert!((find_value(&result, "inline_result") - 9.0).abs() < 1e-10);
    assert!((find_value(&result, "selected_result") - 12.0).abs() < 1e-10);
    assert!((find_value(&result, "unaliased_file_result") - 10.0).abs() < 1e-10);
    assert!((find_value(&result, "unaliased_inline_result") - 18.0).abs() < 1e-10);
    assert!((find_value(&result, "nested_scope_result") - 40.0).abs() < 1e-10);
}

#[test]
fn absolute_inline_call_path_reports_imported_name_diagnostic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/callable_module_alias_absolute_path/src/callable/main.gcl",
    );
    let err = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        err,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Module(ModuleError::UnknownModule { name, .. }), .. }), .. }) if name.as_str() == "callable"
    ));
}

#[test]
fn eval_inline_dag_namespace_alias_at_field() {
    // Issue #518: `include foo() as bar; @bar::member` was N002.
    // Two instances confirm distinct namespaces.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/inline_dag_namespace/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let doubled = find_value(&result, "doubled_result");
    let tripled = find_value(&result, "tripled_result");
    assert!((doubled - 20.0).abs() < 1e-10, "doubled = {doubled}");
    assert!((tripled - 30.0).abs() < 1e-10, "tripled = {tripled}");
}

#[test]
fn field_access_on_value_is_not_hijacked_by_include_alias() {
    // `.` after a graph reference is always struct field access; `::` is the
    // only namespace-member boundary. A value can no longer share an include
    // alias's name (both are Term names; see
    // `module_aliases_and_same_named_nodes_are_duplicate_names`), so the value
    // and the alias expose the same member name instead.
    let source = "dag d { pub node out: Dimensionless = 8.0; }\n\
                  include d() as inst;\n\
                  type S { S(out: Dimensionless) }\n\
                  node value: S = S(out: 1.0);\n\
                  node y: Dimensionless = @value.out;\n\
                  node z: Dimensionless = @inst::out;";
    let result = compile_and_eval(source).unwrap();
    let y = find_value(&result, "y");
    let z = find_value(&result, "z");
    assert!((y - 1.0).abs() < 1e-10, "y = {y}");
    assert!((z - 8.0).abs() < 1e-10, "z = {z}");
}

#[test]
fn dot_member_access_on_include_alias_is_rejected_everywhere() {
    // `.` is always field access, so `@inst.LIM` is an unknown graph
    // reference in nodes, plot encodings, and type-annotation domain bounds
    // alike; the namespace member is spelled `@inst::LIM`.
    let prelude = "index Step = { A, B };\n\
                   dag d { pub const node LIM: Dimensionless = 5.0; }\n\
                   include d() as inst;\n";
    for body in [
        "node y: Dimensionless = @inst.LIM;",
        "plot p = { mark: point, encode: { x: for s: Step { @inst.LIM }, \
         y: for s: Step { @inst.LIM } }, title: \"t\" };",
        "param p: Dimensionless(max: @inst.LIM) = 3.0;",
    ] {
        let error = compile_and_eval(&format!("{prelude}{body}")).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::UnknownGraphRef { name, .. }), .. }), .. })
                    if name == &scoped_name("inst")
            ),
            "`{body}`: unexpected error: {error:?}"
        );
    }
}

#[test]
fn eval_cross_file_include_namespace_alias_at_field() {
    // Issue #518: `include path(...) as alias; @alias::member` across files.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_module/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let dv = find_value(&result, "dv");
    assert!(dv > 0.0, "dv should be positive, got {dv}");
}

#[test]
fn eval_import_namespace_alias_at_field() {
    // Issue #518: `import path as alias; @alias::const_member` across files.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/module_import_alias/src/constants/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let g = find_value(&result, "g");
    assert!((g - 9.806_65).abs() < 1e-10, "g = {g}");
}

#[test]
fn eval_qualified_const_refs_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub const node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub const node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         const node combined: Dimensionless = @a::shared + @b::shared;\n\
         const node shared: Dimensionless = @combined + 1.0;\n\
         node out: Dimensionless = @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 6.0).abs() < 1e-10, "out = {out}");
}

#[test]
fn eval_included_struct_array_field_access_uses_imported_type_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/repro");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"repro\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("types.gcl"),
        "pub type Item {\n    Item(mass: Mass),\n}\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("data.gcl"),
        "import repro.types::{ type Item, Item };\n\
         pub index ItemId = { Only };\n\
         pub node items: Item[ItemId] = {\n\
             ItemId#Only: Item(mass: 1.0 kg),\n\
         };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import repro.types::{ type Item };\n\
         import repro.data::{ index ItemId };\n\
         include repro.data()::{ items };\n\
         node first_mass: Mass = @items[ItemId#Only].mass;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let first_mass = find_value(&result, "first_mass");
    assert!(
        (first_mass - 1.0).abs() < 1e-10,
        "first_mass = {first_mass}"
    );
}

#[test]
fn eval_qualified_runtime_refs_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a() as a;\n\
         include collide.b() as b;\n\
         node total: Dimensionless = @a::shared + @b::shared;\n\
         node shared: Dimensionless = @total + 1.0;\n\
         node out: Dimensionless = @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 6.0).abs() < 1e-10, "out = {out}");
}

#[test]
fn eval_qualified_params_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "param shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "param shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a() as a;\n\
         include collide.b() as b;\n\
         param shared: Dimensionless = 100.0;\n\
         node total: Dimensionless = @a::shared + @b::shared + @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 105.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_selective_import_aliases_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub const node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub const node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a::{ shared as a_shared };\n\
         import collide.b::{ shared as b_shared };\n\
         const node shared: Dimensionless = 100.0;\n\
         node total: Dimensionless = @a_shared + @b_shared + @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 105.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_overrides_reject_included_implementation_params() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "param shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "param shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a()::{ shared as a_shared };\n\
         include collide.b()::{ shared as b_shared };\n\
         node total: Dimensionless = @a_shared + @b_shared;\n",
    )
    .unwrap();

    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("a_shared"), parse_expr("20.0"));
    overrides.insert(DeclName::expect_valid("b_shared"), parse_expr("30.0"));
    let result = compile_and_eval_project(&root, &overrides, None, &fs());
    match result {
        Err(CompileError::Binding(BindingError::NotAParam {
            name,
            actual_kind:
                graphcal_compiler::declaration_category::DeclCategory::Value(
                    graphcal_compiler::declaration_category::ValueDeclCategory::Node,
                ),
        })) => {
            assert!(name.as_str() == "a_shared" || name.as_str() == "b_shared");
        }
        other => panic!("expected included implementation override rejection, got {other:?}"),
    }
}

#[test]
fn eval_include_dep_with_aliased_module_import() {
    // End-to-end coverage for including a template that itself imports another
    // module under an alias (`import lib as mission;` + `@mission::C`). The
    // checked semantic instance must retain the template's canonical imported
    // value target.
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub const node C: Dimensionless = 7.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("dep.gcl"),
        "import collide.lib as mission;\n\
         pub node out: Dimensionless = @mission::C * 2.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.dep()::{ out as dep_out };\n\
         node total: Dimensionless = @dep_out + 1.0;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 15.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_inline_dag_include_cross_file_self_import() {
    // Cross-file `include` of a DAG whose body has `import <self>::{...}`
    // (resolved against the DAG's parent file). The checked instance must keep
    // that canonical parent import available during evaluation.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/inline_dag_include_cross_file_self_import/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!(
        (out - 3_185_500.0).abs() < 1e-10,
        "expected 3185500.0, got {out}"
    );
}

#[test]
fn eval_inline_dag_call_in_for_comp_with_loop_var() {
    // Motivating shape: inline call inside a `for` whose arg references the
    // loop variable via an indexed graph ref.
    let source = "\
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist: Length[Region] = { Region#A: 1.0 m, Region#B: 2.0 m };
node distances: Length[Region] = for r: Region { @id_len(v: @dist[r])::result };
";
    let result = compile_and_eval(source).unwrap();
    // distances is indexed, look it up by cell.
    let distances_entry = result
        .nodes()
        .find(|(n, _)| n.to_string() == "distances")
        .expect("distances node")
        .1
        .as_ref()
        .expect("distances value");
    match distances_entry {
        graphcal_eval::eval::types::Value::Indexed { entries, .. } => {
            let mut seen: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
            for (variant, value) in entries {
                seen.insert(variant.to_string(), value.si_value().unwrap().get());
            }
            assert!((seen["A"] - 1.0).abs() < 1e-10);
            assert!((seen["B"] - 2.0).abs() < 1e-10);
        }
        other => panic!("expected Indexed, got {other:?}"),
    }
}

#[test]
fn eval_inline_dag_call_in_match_arm_fresh_instances_per_syntactic_site() {
    // Each arm has a distinct syntactic call site; the eval-time selection
    // picks one arm's value but every call site semantically is a fresh
    // instantiation. This test exercises the motivating for/match shape.
    let source = "\
pub index Source = { Primary, Secondary };
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist_primary: Length[Region] = { Region#A: 1.0 m, Region#B: 2.0 m };
param dist_secondary: Length[Region] = { Region#A: 10.0 m, Region#B: 20.0 m };

node effective: Length[Source, Region] = for s: Source, r: Region {
    match s {
        Source#Primary   => @id_len(v: @dist_primary[r])::result,
        Source#Secondary => @id_len(v: @dist_secondary[r])::result,
    }
};
";
    let result = compile_and_eval(source).unwrap();
    let entry = result
        .nodes()
        .find(|(n, _)| n.to_string() == "effective")
        .expect("effective node")
        .1
        .as_ref()
        .expect("effective value");
    // Nested Indexed: outer Source, inner Region.
    let graphcal_eval::eval::types::Value::Indexed { entries: outer, .. } = entry else {
        panic!("expected Indexed, got {entry:?}");
    };
    let mut cells: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();
    for (svar, sval) in outer {
        let graphcal_eval::eval::types::Value::Indexed { entries: inner, .. } = sval else {
            panic!("expected inner Indexed, got {sval:?}");
        };
        for (rvar, rval) in inner {
            cells.insert(
                (svar.to_string(), rvar.to_string()),
                rval.si_value().unwrap().get(),
            );
        }
    }
    assert!((cells[&("Primary".into(), "A".into())] - 1.0).abs() < 1e-10);
    assert!((cells[&("Primary".into(), "B".into())] - 2.0).abs() < 1e-10);
    assert!((cells[&("Secondary".into(), "A".into())] - 10.0).abs() < 1e-10);
    assert!((cells[&("Secondary".into(), "B".into())] - 20.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_composition_fixture() {
    let source = include_str!("../../../tests/fixtures/valid/inline_dag_call_composition/main.gcl");
    let result = compile_and_eval(source).unwrap();
    // ((3 * 2) + 1) m = 7 m
    let y = find_value(&result, "y");
    assert!((y - 7.0).abs() < 1e-10, "expected 7.0, got {y}");
}

#[test]
fn eval_inline_dag_call_in_for_fixture() {
    let source = include_str!("../../../tests/fixtures/valid/inline_dag_call_in_for/main.gcl");
    let _result = compile_and_eval(source).unwrap();
}

#[test]
fn eval_inline_dag_call_in_match_fixture() {
    let source = include_str!("../../../tests/fixtures/valid/inline_dag_call_in_match/main.gcl");
    let _result = compile_and_eval(source).unwrap();
}

#[test]
fn eval_inline_dag_call_forward_reference_within_body() {
    // MVP walked the dag body in source order, which made this fail at eval
    // because `b` was evaluated before `a` was bound. The compile-pipeline
    // refactor runs the body in topological order.
    let source = "\
dag forward {
    param v: Length;
    pub node b: Length = @a * 2.0;
    node a: Length = @v + 1.0 m;
}

param src: Length = 3.0 m;
node out: Length = @forward(v: @src)::b;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    // (3 + 1) * 2 = 8
    assert!((out - 8.0).abs() < 1e-10, "expected 8.0, got {out}");
}

#[test]
fn eval_cross_file_inline_dag_nested_call_uses_canonical_target_with_same_leaf_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub dag helper {\n\
             pub node result: Dimensionless = 2.0;\n\
         }\n\
         pub dag wrapper {\n\
             pub node result: Dimensionless = @helper()::result + 10.0;\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub dag helper {\n\
             pub node result: Dimensionless = 100.0;\n\
         }\n\
         pub dag wrapper {\n\
             pub node result: Dimensionless = @helper()::result + 1000.0;\n\
         }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         dag helper {\n\
             pub node result: Dimensionless = 10000.0;\n\
         }\n\
         node out_a: Dimensionless = @a.wrapper()::result;\n\
         node out_b: Dimensionless = @b.wrapper()::result;\n\
         node out_local: Dimensionless = @helper()::result;\n\
         node total: Dimensionless = @out_a + @out_b + @out_local;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out_a = find_value(&result, "out_a");
    let out_b = find_value(&result, "out_b");
    let out_local = find_value(&result, "out_local");
    let total = find_value(&result, "total");
    assert!((out_a - 12.0).abs() < 1e-10, "out_a = {out_a}");
    assert!((out_b - 1100.0).abs() < 1e-10, "out_b = {out_b}");
    assert!(
        (out_local - 10000.0).abs() < 1e-10,
        "out_local = {out_local}"
    );
    assert!((total - 11112.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_public_values_preserve_same_leaf_imported_index_owners() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series_a: Dimensionless[a::Phase] = for p: a::Phase { 1.0 };\n\
         node series_b: Dimensionless[b::Phase] = for p: b::Phase { 2.0 };\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let indexed_owner = |name: &str| {
        let value = result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
        let Value::Indexed {
            index_name,
            entries,
            ..
        } = value
        else {
            panic!("expected indexed value for `{name}`, got {value:?}");
        };
        assert_eq!(index_name.display_name().to_string(), "Phase");
        assert_eq!(entries.len(), 2);
        index_name
            .declared_resolved()
            .cloned()
            .unwrap_or_else(|| panic!("expected declared index for `{name}`"))
    };

    assert_ne!(indexed_owner("series_a"), indexed_owner("series_b"));
}

#[test]
fn eval_inline_dag_call_indexed_output_projection() {
    // Projected output is itself indexed; the call site reads one cell.
    let source = "\
pub index Region = { A, B };

dag doubler {
    import input::{ index Region };

    param v: Length[Region];
    pub node result: Length[Region] = for r: Region { @v[r] * 2.0 };
}

param dist: Length[Region] = { Region#A: 1.0 m, Region#B: 3.0 m };
node out_a: Length = @doubler(v: @dist)::result[Region#A];
node out_b: Length = @doubler(v: @dist)::result[Region#B];
";
    let result = compile_and_eval(source).unwrap();
    let a = find_value(&result, "out_a");
    let b = find_value(&result, "out_b");
    assert!((a - 2.0).abs() < 1e-10, "expected 2.0, got {a}");
    assert!((b - 6.0).abs() < 1e-10, "expected 6.0, got {b}");
}

// ---- Typed Int and Datetime domain constraints (#958) ----

#[test]
fn datetime_struct_field_bound_requires_the_field_scale() {
    let error = compile_and_eval(
        r#"
type EventSpec {
    EventSpec(at: Datetime<TT>(min: datetime("2024-01-01T00:00:00Z"))),
}
node event: EventSpec = EventSpec(at: epoch<TT>("2024-06-01T00:00:00"));
"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(
                    DomainError::DatetimeDomainBoundTypeMismatch { .. }
                ),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn domain_bounds_reject_runtime_graph_references() {
    let error = compile_to_tir(
        r#"
param start: Datetime = datetime("2024-01-01T00:00:00Z");
param event: Datetime(min: @start) = datetime("2024-06-01T00:00:00Z");
"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Name(NameError::GraphRefInConst { .. }),
                ..
            }),
            ..
        })
    ));
}

// ---- Domain constraints on struct/union member fields (#450 Pos 1+2) ----

#[test]
fn struct_field_const_violation_is_compile_time() {
    let source = "
type Spec { Spec(mass: Mass(min: 100.0 kg, max: 2000.0 kg)) }
const node SAT: Spec = Spec(mass: 5000.0 kg);
";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Domain(DomainError::DomainViolation {
                        name, violation, ..
                    }),
                ..
            }),
        ..
    }) = err
    else {
        panic!("expected DomainViolation, got {err:?}");
    };
    assert_eq!(name.to_string(), "SAT.mass");
    assert!(
        violation.contains("above maximum"),
        "violation = {violation}"
    );
}

#[test]
fn generic_field_dimension_mismatch_is_rejected_before_evaluation() {
    let error = compile_and_eval(
        r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node bad: Box<Time> = Box<Time>(x: 1.0 s);
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn invalid_generic_static_fin_key_is_rejected_before_evaluation() {
    let error = compile_and_eval(
        r"
type T<N: Nat> { T(x: Int(min: to_int(key(Fin(N), 0)))) }
node bad: T<0> = T<0>(x: 0);
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::InvalidFiniteIndexCardinality { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn struct_field_min_exceeds_max_at_compile_time() {
    let source = "type Foo { Foo(x: Mass(min: 100.0 kg, max: 50.0 kg)) }";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainMinExceedsMax { name, .. }),
                ..
            }),
        ..
    }) = err
    else {
        panic!("expected DomainMinExceedsMax, got {err:?}");
    };
    assert_eq!(name.to_string(), "Foo.x");
}

#[test]
fn struct_field_invalid_target_at_compile_time() {
    let source = "type Foo { Foo(x: Bool(min: 0.0)) }";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::InvalidDomainTarget { .. }),
                    ..
                }),
                ..
            })
        ),
        "expected InvalidDomainTarget, got {err:?}"
    );
}

#[test]
fn struct_field_dim_mismatch_at_compile_time() {
    let source = "type Foo { Foo(x: Length(min: 1.0 s)) }";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { name, .. }),
                ..
            }),
        ..
    }) = err
    else {
        panic!("expected DomainDimensionMismatch, got {err:?}");
    };
    assert_eq!(name.to_string(), "Foo.x");
}

// ---- Position 4: domain constraint on a generic type argument ----

#[test]
fn generic_type_arg_constraint_rejected() {
    let source = "
pub type Eci { Eci }
pub type Vec3<D: Dim, F: Type> { Vec3(x: D, y: D, z: D) }
param p: Vec3<Length(min: 0.0 m), Eci> = Vec3<Length, Eci>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                    ..
                }),
                ..
            })
        ),
        "expected GenericTypeArgDomainConstraint, got {err:?}"
    );
}

// ---- Position 3: regression — include'd DAG already validates ----

#[test]
fn included_dag_param_constraint_runtime_violation() {
    let source = "
dag bumper {
    param v: Velocity(max: 100.0 m/s);
    pub node out: Velocity = @v * 2.0;
}
param speed: Velocity = 1000.0 m/s;
include bumper(v: @speed)::{ out as doubled };
";
    let result = compile_and_eval(source).unwrap();
    let (_, v_result, _) = result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == "bumper::v")
        .expect("bumper::v not found");
    assert!(v_result.is_err(), "v should violate domain constraint");
}

#[test]
fn included_dag_param_constraint_dim_mismatch() {
    let source = "
dag bumper {
    param v: Velocity(min: 1.0 kg);
    pub node out: Velocity = @v;
}
include bumper(v: 5.0 m/s)::{ out };
";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                    ..
                }),
                ..
            })
        ),
        "expected DomainDimensionMismatch, got {err:?}"
    );
}

#[test]
fn eval_inline_dag_call_const_node_in_body() {
    // `const node` inside a dag body should participate in the same
    // topological evaluation as runtime nodes.
    let source = "\
dag with_const {
    param v: Length;
    const node multiplier: Dimensionless = 3.0;
    pub node result: Length = @v * @multiplier;
}

param src: Length = 4.0 m;
node out: Length = @with_const(v: @src)::result;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 12.0).abs() < 1e-10, "expected 12.0, got {out}");
}

#[test]
fn product_and_rss_support_linear_algebra_ergonomics() {
    let source = include_str!("../../../tests/fixtures/valid/linear_algebra_ergonomics.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "box_volume") - 24.0).abs() < 1e-12);
    assert!((find_value(&result, "budget_sigma") - 5.0).abs() < 1e-12);
    assert!((find_value(&result, "chained_product") - 720.0).abs() < 1e-12);
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, result, _)| matches!(result, graphcal_eval::eval::types::AssertResult::Pass))
    );
}

#[test]
fn dag_local_required_types_enable_reusable_linear_algebra() {
    let source = include_str!("../../../tests/fixtures/valid/linear_algebra_reusable_dag.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "displacement_magnitude::result") - 13.0).abs() < 1e-12);
    assert!((find_value(&result, "duration_magnitude::result") - 13.0).abs() < 1e-12);
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, result, _)| matches!(result, graphcal_eval::eval::types::AssertResult::Pass))
    );
}

#[test]
fn eval_extern_plugin_fixture() {
    // Extern functions (#943 Phase A): dim-variable polymorphism, rational
    // result powers, and cross-variable monomial results all evaluate through
    // the demo host registry.
    let source = include_str!("../../../tests/fixtures/valid/plugin_extern.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "v_mid") - 150.0).abs() < 1e-9);
    assert!((find_value(&result, "pace") - (1.0 / 150.0)).abs() < 1e-12);
    assert!((find_value(&result, "scale") - 6.0).abs() < 1e-12);
    assert!((find_value(&result, "dv_share_total") - 1.0).abs() < 1e-12);
    // Struct results rebuild as ordinary record values: field access works.
    assert!((find_value(&result, "dv_spread") - 1500.0).abs() < 1e-9);
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, r, _)| matches!(r, graphcal_eval::eval::types::AssertResult::Pass)),
        "all fixture assertions must pass: {:?}",
        result.assertions
    );
}

#[test]
fn assumes_targets_resolve_through_semantic_instances() {
    let source = "dag producer {\n\
                      pub node x: Dimensionless = 1.0;\n\
                      pub assert okay = @x == 2.0;\n\
                      #[assumes(okay)]\n\
                      pub node y: Dimensionless = @x;\n\
                  }\n\
                  include producer()::{ okay as check, y as y_out };\n\
                  #[assumes(check)]\n\
                  node dependent: Dimensionless = 1.0;\n";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let mut assumers = result
        .assumes_map
        .get(&scoped_name("check"))
        .expect("the projected assertion keeps its assumers")
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assumers.sort();
    assert_eq!(assumers, ["dependent", "y_out"]);
}
