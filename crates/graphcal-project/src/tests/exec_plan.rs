//! Execution-plan invariants over programs that need the project pipeline
//! (inline DAG calls and includes).

use std::collections::HashMap;

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_eval::exec_plan::{compile, prepare_callable_plan_for_test};
use graphcal_eval::execution_plan::{ExecPlan, PlannedDeclaration, PlannedInstance};

fn test_dag_id() -> graphcal_compiler::dag_id::DagId {
    graphcal_compiler::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl"))
        .unwrap()
}

fn resolved_key(name: &str) -> ResolvedDeclName {
    ResolvedDeclName::for_test(test_dag_id(), DeclName::expect_valid(name))
}

/// The runtime identities of the root callable's steps, in order.
fn root_order<'a>(plan: &'a ExecPlan<'_>) -> Vec<&'a ResolvedDeclName> {
    plan.root()
        .steps()
        .map(|step| step.declaration().key())
        .collect()
}

#[test]
fn callables_reject_missing_and_out_of_closure_locations() {
    let source = "dag helper { pub node out: Dimensionless = 1.0; }\n\
                  node x: Dimensionless = @helper()::out;";
    let loaded = crate::loader::LoadedProject::from_source(source, "test.gcl").unwrap();
    let checked = crate::project_compiler::ProjectCompiler::new(&loaded)
        .check()
        .unwrap();
    let tir = checked.tir();
    let prepared = compile(tir, loaded.root_file().source_id(), checked.sources()).unwrap();
    let plan = prepared.plan();
    let program = plan.program();
    let scopes = tir
        .dag_registry()
        .keys()
        .map(|owner| (owner, program.dag(owner).unwrap()))
        .collect::<HashMap<_, _>>();
    let root = scopes[tir.root_dag_id()];
    let helper = *scopes
        .values()
        .find(|scope| scope.dag().dag_id() != tir.root_dag_id())
        .unwrap();
    let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
    let x = plan.declaration(&resolved_key("x")).unwrap();
    let misplaced =
        PlannedDeclaration::new(x.key(), helper, x.body().clone(), x.reads(), x.domain());
    for (declarations, expected) in [
        (HashMap::new(), "has no prepared physical location"),
        (
            HashMap::from([(x.key(), misplaced)]),
            "outside its callable closure",
        ),
    ] {
        let error =
            prepare_callable_plan_for_test(tir, &scopes, root, &declarations, &cancellation)
                .unwrap_err();
        assert!(
            matches!(&error, Outcome::Failed(GraphcalError::Internal(internal)) if internal.message().contains(expected)),
            "{error:?}"
        );
    }
}

#[test]
fn callable_plans_use_the_checked_closure_schedule() {
    let source = "dag lib { param x: Dimensionless; pub node out: Dimensionless = @x + 1.0; }\n\
                  include lib(x: @seed) as inst;\n\
                  param seed: Dimensionless = 1.0;\n\
                  node result: Dimensionless = @inst::out;";
    let loaded = crate::loader::LoadedProject::from_source(source, "test.gcl").unwrap();
    let checked = crate::project_compiler::ProjectCompiler::new(&loaded)
        .check()
        .unwrap();
    let tir = checked.tir();
    let prepared = compile(tir, loaded.root_file().source_id(), checked.sources()).unwrap();
    let plan = prepared.plan();
    let schedule = tir.root().runtime_schedule();
    assert_eq!(
        root_order(plan),
        schedule.order().iter().collect::<Vec<_>>()
    );
    assert_eq!(
        plan.root()
            .execution_dags()
            .iter()
            .map(|scope| scope.dag().dag_id().clone())
            .collect::<Vec<_>>(),
        schedule.execution_dags()
    );
    assert_eq!(schedule.execution_dags().len(), 2);
    let position = |name: &str| {
        schedule
            .order()
            .iter()
            .position(|key| key.as_str() == name)
            .unwrap()
    };
    assert!(position("seed") < position("x"));
    assert!(position("x") < position("out"));
    assert!(position("out") < position("result"));

    // The root's include is planned with the sealed DAG that runs it, and
    // no other DAG can be paired with it.
    let [planned] = plan.root().semantic_instances() else {
        panic!("expected one planned instance");
    };
    let instance = planned.instance();
    assert_eq!(
        planned.scope().dag().dag_id(),
        instance.record().instance.id().owner()
    );
    assert!(PlannedInstance::try_new(instance, planned.scope()).is_ok());
    assert!(PlannedInstance::try_new(instance, plan.root().scope()).is_err());
}
