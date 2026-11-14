//! Prepared plans, callables, frames, and canonical body sharing.

use super::*;

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
                        graphcal_compiler::tir::texpr::CheckedBody::Deferred(body)
                        | graphcal_compiler::tir::texpr::CheckedBody::Unfinished(body) => {
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
    register_test_host_fn(
        &mut host,
        "graphcal:selector-counter",
        "toggle",
        &[],
        graphcal_compiler::function_signature::NamedParamKind::bool(),
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
    register_test_host_fn(
        &mut host,
        "graphcal:pure-plan-test",
        "twice",
        &["x"],
        graphcal_compiler::function_signature::NamedParamKind::dimensionless(),
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
                .map(|closure| closure.scope().dag().dag_id())
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
        for closure in callable.execution_dags() {
            let scope = closure.scope();
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
            let value = import.value.value().unwrap();
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
