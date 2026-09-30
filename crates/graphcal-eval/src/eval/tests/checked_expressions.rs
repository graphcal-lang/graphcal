use super::*;
use graphcal_compiler::resolved_name::ResolvedStructTypeName;
use graphcal_compiler::syntax::type_name::{FieldName, StructTypeName};
use graphcal_compiler::tir::dim_check::body_specialization::specialize_bound_expression;

#[test]
fn scalar_prototypes_require_discharge_and_invalid_membership_never_publishes() {
    for body in ["to_int(key(Fin(N), 1))", "to_int((for i: Fin(N) { i })[1])"] {
        let source = format!("type T<N: Nat> {{ T(x: Int(min: {body})) }}");
        let tir =
            compile_to_tir(&source, "scalar-prototype.gcl").expect("unused prototype is valid");
        let src = miette::NamedSource::new("scalar-prototype.gcl", std::sync::Arc::new(source));
        let identity = ResolvedStructTypeName::for_test(
            tir.root_dag_id().clone(),
            StructTypeName::expect_valid("T"),
        );
        let nominal = tir.nominal_type_body(&identity).unwrap();
        let parameter = nominal.definition().generic_params()[0].id().clone();
        let (key, field) = nominal.constrained_fields().next().unwrap();
        assert_eq!(key.field, FieldName::expect_valid("x"));
        let bound = field.map(|field| &field.domain_bounds()[0]);
        let context = crate::eval_expr::EvalSession::provisional_constants(
            &tir,
            &src,
            graphcal_compiler::cancellation::CancellationToken::unbounded(),
        );
        let values = crate::constant_pools::RuntimeValueMap::new();
        let result = context
            .executable(bound.map(|bound| &*bound.value))
            .and_then(|tree| crate::eval_expr::eval_root(&tree, &values, &context));
        assert!(
            matches!(result, Err(GraphcalError::InternalError { ref message, .. }) if message.contains("undischarged static obligations")),
            "prototype executed: {result:?}"
        );
        for n in [0, 1] {
            assert!(
                specialize_bound_expression(&tir, bound, &HashMap::from([(parameter.clone(), n)]))
                    .is_err(),
                "invalid N={n} product published"
            );
        }
        for n in 2..=5 {
            let tree =
                specialize_bound_expression(&tir, bound, &HashMap::from([(parameter.clone(), n)]))
                    .unwrap();
            let result = crate::eval_expr::eval_root(&tree, &values, &context).unwrap();
            assert!(
                matches!(result, crate::eval_expr::RuntimeValue::Int(1)),
                "{result:?}"
            );
        }
    }
}

#[test]
fn normalized_nat_axes_do_not_replay_source_arithmetic() {
    let literal_control = compile_and_eval("node value: Int = to_int(key(Fin(1), 0));").unwrap();
    assert!(!literal_control.has_errors());
    for expression in [
        "sum(for i: Fin(N * 18446744073709551615 * 0 + 1) { 1.0 })",
        "to_int(fin_key(Fin(N * 18446744073709551615 * 0 + 1), 0))",
        "to_int(key(Fin(N * 18446744073709551615 * 0 + 1), 0))",
    ] {
        let source = format!(
            "type NatIdentity<N: Nat> {{ NatIdentity(value: Dimensionless(min: {expression})), }} node value: NatIdentity<2> = NatIdentity<2>(value: 1.0);"
        );
        let result = compile_and_eval(&source).unwrap();
        assert!(
            !result.has_errors(),
            "normalized proof disagreed with interpretation: {result:?}"
        );
    }
    let bad_position = "type T<N: Nat> { T(x: Int(min: to_int(fin_key(Fin(N * 18446744073709551615 * 0 + 1), 1)))) } node value: T<2> = T<2>(x: 1);";
    assert!(
        compile_and_eval(bad_position)
            .unwrap_err()
            .to_string()
            .contains("out of bounds"),
        "dynamic fin_key membership must remain checked"
    );
}

#[test]
fn readiness_is_checked_before_evaluating_an_earlier_sibling() {
    let source = "type T<N: Nat> { T(x: Int(min: 1 / 0 + to_int(key(Fin(N), 1)))) }".to_string();
    let tir = compile_to_tir(&source, "readiness.gcl").unwrap();
    let src = miette::NamedSource::new("readiness.gcl", std::sync::Arc::new(source));
    let identity = ResolvedStructTypeName::for_test(
        tir.root_dag_id().clone(),
        StructTypeName::expect_valid("T"),
    );
    let (_, field) = tir
        .nominal_type_body(&identity)
        .unwrap()
        .constrained_fields()
        .next()
        .unwrap();
    let bound = field.map(|field| &*field.domain_bounds()[0].value);
    let context = crate::eval_expr::EvalSession::provisional_constants(
        &tir,
        &src,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    );
    let result = context.executable(bound).and_then(|tree| {
        crate::eval_expr::eval_root(
            &tree,
            &crate::constant_pools::RuntimeValueMap::new(),
            &context,
        )
    });
    assert!(
        matches!(result, Err(GraphcalError::InternalError { ref message, .. }) if message.contains("undischarged static obligations")),
        "earlier sibling ran before readiness check: {result:?}"
    );
}

#[test]
fn deferred_descendants_block_host_work_with_an_active_positive_control() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let source = r#"
import plugin "graphcal:readiness-counter" as probe { fn tick() -> Dimensionless; }
dag worker {
    pub(bind) index Axis;
    node pending: Dimensionless = probe::tick() + sum(for i: Axis { 1.0 });
}
node control: Dimensionless = probe::tick() + 1.0;
"#;
    let project = crate::loader::LoadedProject::from_source(source, "host-readiness.gcl").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut host = crate::host_fns::HostFunctionRegistry::new();
    host.register(
        graphcal_compiler::syntax::plugin::PluginPath::new("graphcal:readiness-counter"),
        graphcal_compiler::syntax::function_name::FnName::expect_valid("tick"),
        move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(crate::host_fns::HostFnValue::F64(1.0))
        },
    );
    let checked = ProjectCompiler::new(&project)
        .host_fns(&host)
        .check()
        .unwrap();
    let tir = checked.tir();
    let src = miette::NamedSource::new("host-readiness.gcl", Arc::new(source.to_string()));
    let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
    let prepared = crate::exec_plan::compile_with_cancellation(tir, &src, &cancellation).unwrap();
    let plan = prepared.plan();
    let worker = tir
        .dag_registry()
        .values()
        .find(|dag| dag.bound_decl_identity(&scoped_name("pending")).is_some())
        .unwrap();
    let values = crate::constant_pools::RuntimeValueMap::new();
    let context = crate::eval_expr::EvalSession::checked(plan, &src, &host, cancellation);
    let runtime_expression = |dag: &graphcal_compiler::tir::typed::CheckedDag, name: &str| {
        tir.declaration_body(dag.bound_decl_identity(&scoped_name(name)).unwrap())
            .unwrap()
            .runtime_expression()
            .unwrap()
    };
    let pending = runtime_expression(worker, "pending");
    let result = context
        .executable(pending)
        .and_then(|tree| crate::eval_expr::eval_root(&tree, &values, &context));
    assert!(
        matches!(result, Err(GraphcalError::InternalError { ref message, .. }) if message.contains("undischarged static obligations")),
        "{result:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "host ran before deferred descendant was rejected"
    );
    let control = runtime_expression(tir.root(), "control");
    crate::eval_expr::eval_root(&context.executable(control).unwrap(), &values, &context).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "positive control must actually invoke the host"
    );
}

#[test]
fn replacement_bindings_complete_contextual_operands_in_their_own_product() {
    let source = r#"
dag worker {
    param time: Datetime<UTC> = datetime("2026-01-01T00:00:00Z");
    pub node out: Datetime<UTC> = @time;
}
include worker(time: datetime("2026-02-01T00:00:00Z")) as instance;
node value: Datetime<UTC> = @instance::out;
"#;
    let result = compile_and_eval(source).unwrap();
    assert!(!result.has_errors(), "{result:?}");
}

#[test]
fn nominal_bound_constructors_are_prepared_in_each_owning_scope() {
    let definitions = "type Marker { Marker(value: Dimensionless), } pub type Wrapped { Wrapped(value: Dimensionless(min: Marker(value: 1.0).value)), }";
    for value in ["1.0", "0.0"] {
        let local = format!("{definitions} node value: Wrapped = Wrapped(value: {value});");
        let result = compile_and_eval(&local).unwrap();
        assert_eq!(result.has_errors(), value == "0.0", "{result:?}");
        let inline = format!(
            "dag make {{ {definitions} pub node out: Dimensionless = Wrapped(value: {value}).value; }} node result: Dimensionless = @make()::out;"
        );
        let result = compile_and_eval(&inline).unwrap();
        assert_eq!(result.has_errors(), value == "0.0", "{result:?}");
        let entry = format!(
            "import pipeline.lib as lib; node value: lib::Wrapped = lib::Wrapped(value: {value});"
        );
        let (_directory, root) = write_pipeline_project(
            &[("lib.gcl", definitions), ("main.gcl", &entry)],
            "main.gcl",
        );
        let project = crate::loader::load_project(&root, None, &fs()).unwrap();
        let prepared = ProjectCompiler::new(&project).prepare().unwrap();
        let row = prepared.binding_builder().finish().unwrap();
        let result = prepared.evaluate(&row).unwrap();
        assert_eq!(result.has_errors(), value == "0.0", "{result:?}");
    }
}

#[test]
fn instance_trees_are_their_templates_specialized_by_the_instance_bindings() {
    let source = "dag worker { pub(bind) index Axis; pub node v: Dimensionless[Axis] = for i: Axis { 1.0 }; }\n\
                  include worker(index Axis: Fin(2)) as w;\n\
                  node total: Dimensionless = sum(@w::v);";
    let tir = compile_to_tir(source, "instance-trees.gcl").unwrap();
    let formula = |dag: &graphcal_compiler::tir::typed::CheckedDag| {
        dag.value_expr(dag.bound_decl_identity(&scoped_name("v")).unwrap())
            .unwrap()
            .id()
            .clone()
    };
    let (instances, templates): (Vec<_>, Vec<_>) = tir
        .dag_registry()
        .values()
        .filter(|dag| dag.bound_decl_identity(&scoped_name("v")).is_some())
        .partition(|dag| dag.is_semantic_instance());
    let [template] = templates.as_slice() else {
        panic!("expected one template: {templates:?}");
    };
    let [instance] = instances.as_slice() else {
        panic!("expected one instance: {instances:?}");
    };
    // The template's axis awaits its binding; the instance's is `Fin(2)`.
    assert!(matches!(
        template
            .bodies_for_test()
            .executable_value(&formula(template)),
        Err(graphcal_compiler::tir::texpr::ExecutableBodyError::Deferred(_))
    ));
    let tree = instance
        .bodies_for_test()
        .executable_value(&formula(instance))
        .unwrap();
    assert!(
        matches!(
            tree.ty(),
            graphcal_compiler::registry::checked_type::CheckedType::Indexed { index, .. }
                if index.finite_index().is_some_and(|axis| axis.cardinality().get() == 2)
        ),
        "{:?}",
        tree.ty()
    );
}
