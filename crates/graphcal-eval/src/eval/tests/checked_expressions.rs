use super::*;
use graphcal_compiler::syntax::type_name::{
    ConstructorName, FieldName, ResolvedStructTypeName, StructTypeName,
};
use graphcal_compiler::tir::dim_check::expression_facts::specialize_bound_expression_facts;

#[test]
fn field_access_rejects_forged_constructor_in_the_retained_type() {
    let source = "type Token { Token(value: Int), } node stored: Token = Token(value: 1); node projected: Int = @stored.value;";
    let tir = compile_to_tir(source, "field-membership.gcl").unwrap();
    let src = miette::NamedSource::new(
        "field-membership.gcl",
        std::sync::Arc::new(source.to_owned()),
    );
    let projected = tir
        .root()
        .bound_decl_identity(&scoped_name("projected"))
        .unwrap();
    let expr = tir.root().value_expr(projected).unwrap();
    for (constructor, valid) in [("Token", true), ("NotToken", false)] {
        let values = HashMap::from([(
            crate::decl_key::RuntimeDeclKey::for_local_decl(tir.root(), &scoped_name("stored"))
                .unwrap(),
            crate::eval_expr::RuntimeValue::Struct {
                type_name: ResolvedStructTypeName::from_def(
                    tir.root_dag_id().clone(),
                    StructTypeName::expect_valid("Token"),
                ),
                constructor: ConstructorName::expect_valid(constructor),
                generic_args: Vec::new(),
                fields: indexmap::IndexMap::from([(
                    FieldName::expect_valid("value"),
                    crate::eval_expr::RuntimeValue::Int(99),
                )]),
            },
        )]);
        let context = crate::eval_expr::EvalContext::provisional_constants(
            &tir,
            tir.root_dag_id(),
            &src,
            graphcal_compiler::registry::builtins::builtin_functions(),
            graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap()
        .with_roots(&values, None);
        let result = crate::eval_expr::eval_hir_expr(
            expr,
            &values,
            &crate::eval_expr::HirLocalValueMap::root(),
            &context,
        );
        if valid {
            assert!(
                matches!(result, Ok(crate::eval_expr::RuntimeValue::Int(99))),
                "positive control: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(GraphcalError::EvalError { .. })),
                "forged constructor accepted: {result:?}"
            );
        }
    }
}

#[test]
fn scalar_prototypes_require_discharge_and_invalid_membership_never_publishes() {
    for body in ["to_int(key(Fin(N), 1))", "to_int((for i: Fin(N) { i })[1])"] {
        let source = format!("type T<N: Nat> {{ T(x: Int(min: {body})) }}");
        let tir =
            compile_to_tir(&source, "scalar-prototype.gcl").expect("unused prototype is valid");
        let src = miette::NamedSource::new("scalar-prototype.gcl", std::sync::Arc::new(source));
        let identity = ResolvedStructTypeName::from_def(
            tir.root_dag_id().clone(),
            StructTypeName::expect_valid("T"),
        );
        let parameter = tir.root().semantic().type_defs.struct_types[&identity].generic_params()[0]
            .id()
            .clone();
        let key = graphcal_compiler::tir::typed::model::ResolvedStructFieldTypeKey {
            owning_type: identity,
            constructor: ConstructorName::expect_valid("T"),
            field: FieldName::expect_valid("x"),
        };
        let bound = &tir
            .root()
            .semantic()
            .type_defs
            .field(&key)
            .unwrap()
            .domain_bounds()[0]
            .value;
        let context = crate::eval_expr::EvalContext::provisional_constants(
            &tir,
            tir.root_dag_id(),
            &src,
            graphcal_compiler::registry::builtins::builtin_functions(),
            graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap();
        let values = crate::execution_facts::RuntimeValueMap::new();
        let locals = crate::eval_expr::HirLocalValueMap::root();
        let result = crate::eval_expr::eval_hir_expr(bound, &values, &locals, &context);
        assert!(
            matches!(result, Err(GraphcalError::InternalError { ref message, .. }) if message.contains("undischarged static obligations")),
            "prototype executed: {result:?}"
        );
        for n in [0, 1] {
            assert!(
                specialize_bound_expression_facts(
                    &tir,
                    tir.root(),
                    bound,
                    &HashMap::from([(parameter.clone(), n)]),
                    &src
                )
                .is_err(),
                "invalid N={n} product published"
            );
        }
        for n in 2..=5 {
            let facts = specialize_bound_expression_facts(
                &tir,
                tir.root(),
                bound,
                &HashMap::from([(parameter.clone(), n)]),
                &src,
            )
            .unwrap();
            let result = crate::eval_expr::eval_hir_expr(
                bound,
                &values,
                &locals,
                &context.clone().with_expression_facts(&facts).unwrap(),
            )
            .unwrap();
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
    let bound = tir
        .root()
        .semantic()
        .type_defs
        .constrained_fields()
        .next()
        .unwrap()
        .1
        .domain_bounds()
        .first()
        .unwrap();
    let context = crate::eval_expr::EvalContext::provisional_constants(
        &tir,
        tir.root_dag_id(),
        &src,
        graphcal_compiler::registry::builtins::builtin_functions(),
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap();
    let result = crate::eval_expr::eval_hir_expr(
        &bound.value,
        &crate::execution_facts::RuntimeValueMap::new(),
        &crate::eval_expr::HirLocalValueMap::root(),
        &context,
    );
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
    let execution =
        crate::project_compiler::check_execution_facts_with_cancellation(tir, &src, &cancellation)
            .unwrap();
    let plan =
        crate::exec_plan::compile_checked_with_cancellation(tir, &execution, &src, &cancellation)
            .unwrap();
    let worker = tir
        .dag_registry()
        .values()
        .find(|dag| dag.bound_decl_identity(&scoped_name("pending")).is_some())
        .unwrap();
    let values = crate::execution_facts::RuntimeValueMap::new();
    let locals = crate::eval_expr::HirLocalValueMap::root();
    let context = |dag: &graphcal_compiler::tir::typed::model::DagTIR| {
        crate::eval_expr::EvalContext::checked(
            tir,
            &plan,
            dag.dag_id(),
            &src,
            graphcal_compiler::registry::builtins::builtin_functions(),
            &host,
            cancellation.clone(),
        )
        .unwrap()
    };
    let pending = worker
        .value_expr(worker.bound_decl_identity(&scoped_name("pending")).unwrap())
        .unwrap();
    let result = crate::eval_expr::eval_hir_expr(pending, &values, &locals, &context(worker));
    assert!(
        matches!(result, Err(GraphcalError::InternalError { ref message, .. }) if message.contains("undischarged static obligations")),
        "{result:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "host ran before deferred descendant was rejected"
    );
    let control = tir
        .root()
        .value_expr(
            tir.root()
                .bound_decl_identity(&scoped_name("control"))
                .unwrap(),
        )
        .unwrap();
    crate::eval_expr::eval_hir_expr(control, &values, &locals, &context(tir.root())).unwrap();
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
