//! Runtime execution-plan preparation from a sealed checked program.

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::source_id::SourceId;

use crate::checked_program::{CheckedProgram, SealedDag};
use crate::execution_plan::{
    CallablePlan, ExecPlan, PlannedBody, PlannedDeclaration, PlannedInstance,
    PreparedConstantImport, PreparedImports,
};

self_cell::self_cell!(
    /// A sealed program together with the execution plan prepared from it.
    pub struct PreparedPlan {
        owner: CheckedProgram,

        #[covariant]
        dependent: ExecPlan,
    }
);

impl PreparedPlan {
    /// The prepared plan.
    #[must_use]
    pub fn plan(&self) -> &ExecPlan<'_> {
        self.borrow_dependent()
    }
}

impl std::fmt::Debug for PreparedPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.plan().fmt(formatter)
    }
}

/// Check a TIR and select its root execution plan.
///
/// This test convenience mirrors the production check-then-prepare pipeline
/// on a copy of `tir`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when execution checking or plan selection fails.
#[cfg(any(test, feature = "test-internals"))]
pub fn compile(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: SourceId,
    sources: &graphcal_compiler::source_registry::SourceRegistry,
) -> Result<PreparedPlan, GraphcalError> {
    graphcal_compiler::outcome::without_cancellation(|cancellation| {
        compile_with_cancellation(tir, src, sources, cancellation)
    })
}

/// Seal a copy of a TIR and select its root execution plan with cooperative
/// cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid plan or cancellation.
#[cfg(any(test, feature = "test-internals"))]
pub fn compile_with_cancellation(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: SourceId,
    sources: &graphcal_compiler::source_registry::SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedPlan, Outcome<GraphcalError>> {
    let program = crate::execution_check::seal_checked_program_with_cancellation(
        tir.clone(),
        src,
        sources,
        cancellation,
    )?;
    compile_checked_with_cancellation(program, src, cancellation)
}

/// Prepare the callable plans of a sealed program.
pub fn compile_checked_with_cancellation(
    program: CheckedProgram,
    src: SourceId,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedPlan, Outcome<GraphcalError>> {
    PreparedPlan::try_new(program, |program| prepare(program, src, cancellation))
}

fn invalid(message: impl Into<String>, src: SourceId) -> GraphcalError {
    GraphcalError::internal_error(message, src, DiagnosticAnchor::WholeFile)
}

fn prepare<'p>(
    program: &'p CheckedProgram,
    src: SourceId,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan<'p>, Outcome<GraphcalError>> {
    cancellation.checkpoint()?;
    let tir = program.tir();
    let scopes = program
        .positioned()
        .map(|(_, scope)| (scope.dag().dag_id(), scope))
        .collect::<HashMap<_, _>>();
    let declarations =
        prepare_declarations(tir, program.positioned().map(|(_, scope)| scope), src)?;
    ExecPlan::new(program, declarations.clone(), |scope| {
        prepare_callable_plan(tir, &scopes, scope, &declarations, cancellation)
    })
}

/// Plan every value declaration of every DAG once: its body (in the scope of
/// its owner), the declarations it reads and its domain constraint.
fn prepare_declarations<'p>(
    tir: &'p graphcal_compiler::tir::typed::CheckedTir,
    scopes: impl IntoIterator<Item = SealedDag<'p>>,
    src: SourceId,
) -> Result<HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>, GraphcalError> {
    let mut declarations = HashMap::<&ResolvedDeclName, PlannedDeclaration<'p>>::new();
    for scope in scopes {
        let dag = scope.dag();
        for key in dag.value_declaration_identities() {
            let unit = tir.declaration_body(key).ok_or_else(|| {
                invalid(
                    format!("checked declaration `{key}` has no body in its owner"),
                    scope.source(),
                )
            })?;
            let body = match (unit.is_todo(), unit.runtime_expression()) {
                (true, _) => PlannedBody::Todo,
                (false, Some(root)) => PlannedBody::Expression {
                    root,
                    tree: root.executable(),
                },
                // Required ports have no default; constants are pooled.
                (false, None) => PlannedBody::Supplied,
            };
            let reads = match (&body, dag.runtime_schedule().dependencies_of(key)) {
                (_, Some(reads)) => reads,
                (PlannedBody::Supplied, None) => &[],
                (PlannedBody::Todo | PlannedBody::Expression { .. }, None) => {
                    return Err(invalid(
                        format!("checked declaration `{key}` has no dependencies"),
                        scope.source(),
                    ));
                }
            };
            let planned = PlannedDeclaration::new(
                key,
                scope,
                body,
                reads,
                scope.domain_constraints().get(key),
            );
            if let Some(first) = declarations.insert(key, planned) {
                return Err(invalid(
                    format!(
                        "declaration `{key}` has duplicate physical locations in `{}` and `{}`",
                        first.scope().dag().dag_id(),
                        dag.dag_id()
                    ),
                    src,
                ));
            }
        }
    }
    Ok(declarations)
}

/// Test-only access to the callable preparation of [`compile_checked_with_cancellation`].
///
/// # Errors
///
/// Returns a [`GraphcalError`] when a scheduled declaration has no prepared
/// location in the callable's closure.
#[cfg(feature = "test-internals")]
#[expect(
    clippy::implicit_hasher,
    reason = "test-only forwarder of the planner's own maps"
)]
pub fn prepare_callable_plan_for_test<'p>(
    tir: &'p graphcal_compiler::tir::typed::CheckedTir,
    scopes: &HashMap<&DagId, SealedDag<'p>>,
    body: SealedDag<'p>,
    declarations: &HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CallablePlan<'p>, Outcome<GraphcalError>> {
    prepare_callable_plan(tir, scopes, body, declarations, cancellation)
}

fn prepare_callable_plan<'p>(
    tir: &'p graphcal_compiler::tir::typed::CheckedTir,
    scopes: &HashMap<&DagId, SealedDag<'p>>,
    body: SealedDag<'p>,
    declarations: &HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CallablePlan<'p>, Outcome<GraphcalError>> {
    cancellation.checkpoint()?;
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PlanConstruction);
    let src = body.source();
    let schedule = body.dag().runtime_schedule();
    let execution_dags = schedule
        .execution_dags()
        .iter()
        .map(|owner| {
            scopes.get(owner).copied().ok_or_else(|| {
                invalid(
                    format!("semantic runtime instance `{owner}` has no compiled DAG"),
                    src,
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let scheduled = schedule
        .order()
        .iter()
        .map(|declaration| {
            let planned = located(declarations, declaration, src)?;
            let physical = planned.scope().dag().dag_id();
            if !schedule.execution_dags().contains(physical) {
                return Err(invalid(
                    format!(
                        "scheduled declaration `{declaration}` is physically in `{physical}`, outside its callable closure"
                    ),
                    src,
                ));
            }
            for dependency in planned.reads() {
                located(declarations, dependency, src)?;
            }
            Ok(planned.clone())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let plan_instances = |parent: SealedDag<'p>| {
        parent
            .dag()
            .semantic_instances()
            .iter()
            .map(|record| {
                let owner = record.instance.id().owner();
                let instance = tir.dag_registry().semantic_instance(record);
                instance
                    .zip(scopes.get(owner).copied())
                    .ok_or_else(|| {
                        invalid(
                            format!("semantic instance `{owner}` has no compiled DAG"),
                            src,
                        )
                    })
                    .and_then(|(instance, scope)| {
                        PlannedInstance::try_new(instance, scope)
                            .map_err(|error| invalid(error.to_string(), src))
                    })
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let instances = plan_instances(body)?;
    // The callable's own body and every semantic instance of its closure,
    // in `DagId` order, each with the instances it includes.
    let mut parents = execution_dags
        .iter()
        .copied()
        .filter(|scope| std::ptr::eq(scope.dag(), body.dag()) || scope.dag().is_semantic_instance())
        .collect::<Vec<_>>();
    parents.sort_by(|left, right| left.dag().dag_id().cmp(right.dag().dag_id()));
    let closure_instances = parents
        .into_iter()
        .map(|parent| Ok((parent, plan_instances(parent)?)))
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let imports = prepare_imports(&execution_dags, declarations, src)?;
    CallablePlan::new(
        body,
        execution_dags,
        instances,
        closure_instances,
        imports,
        scheduled,
    )
    .map_err(|error| invalid(error.to_string(), src))
    .map_err(Outcome::Failed)
}

/// The planned declaration `key` denotes.
fn located<'a, 'p>(
    declarations: &'a HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>,
    key: &ResolvedDeclName,
    src: SourceId,
) -> Result<&'a PlannedDeclaration<'p>, GraphcalError> {
    declarations.get(key).ok_or_else(|| {
        invalid(
            format!("declaration `{key}` has no prepared physical location"),
            src,
        )
    })
}

/// Select the imports of a callable's execution DAGs: the constants the
/// program resolved when it was sealed, and the explicit runtime imports.
fn prepare_imports(
    dags: &[SealedDag<'_>],
    declarations: &HashMap<&ResolvedDeclName, PlannedDeclaration<'_>>,
    source: SourceId,
) -> Result<PreparedImports, GraphcalError> {
    use graphcal_compiler::ir::imported_binding::ImportedValueKind;
    let mut result = PreparedImports::default();
    for dag in dags {
        result
            .constants
            .extend(
                dag.imported_constants()
                    .iter()
                    .map(|constant| PreparedConstantImport {
                        destination: dag.dag().imported_destination(constant.value().key()),
                        value: constant.value().clone(),
                    }),
            );
        for binding in dag
            .dag()
            .imported_bindings()
            .values()
            .filter(|binding| matches!(binding.kind(), ImportedValueKind::Runtime))
        {
            let target = binding.target();
            located(declarations, target, source)?;
            result.runtime.push(target.clone());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_value::RuntimeValue;
    use crate::test_tir::checked_tir_from_source;
    use graphcal_compiler::resolved_name::ResolvedDeclName;
    use graphcal_compiler::syntax::decl_name::DeclName;
    use std::collections::HashSet;

    fn compile_source(source: &str) -> Result<PreparedPlan, GraphcalError> {
        let (tir, src, sources) = checked_tir_from_source(source)?;
        compile(&tir, src, &sources)
    }

    fn tir_from_source(
        source: &str,
    ) -> (
        graphcal_compiler::tir::typed::CheckedTir,
        SourceId,
        graphcal_compiler::source_registry::SourceRegistry,
    ) {
        checked_tir_from_source(source).unwrap()
    }

    /// The runtime identities of the root callable's steps, in order.
    fn root_order<'a>(plan: &'a ExecPlan<'_>) -> Vec<&'a ResolvedDeclName> {
        plan.root()
            .steps()
            .map(|step| step.declaration().key())
            .collect()
    }

    fn root_constant<'a>(plan: &'a ExecPlan<'_>, key: &ResolvedDeclName) -> &'a RuntimeValue {
        plan.root().scope().const_values().get(key).unwrap()
    }

    #[test]
    fn prepared_declarations_include_parameters_without_defaults() {
        let (tir, src, sources) = tir_from_source(
            "param input: Dimensionless; node doubled: Dimensionless = 2.0 * @input;",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        let input = resolved_key("input");
        assert!(tir.root().body_for_test().runtime_expr(&input).is_none());
        let declaration = prepared.plan().declaration(&input).unwrap();
        assert_eq!(declaration.scope().dag().dag_id(), tir.root_dag_id());
        assert!(matches!(declaration.body(), PlannedBody::Supplied));
    }

    #[test]
    fn plan_debug_output_names_bodies_by_kind() {
        let prepared = compile_source(
            "const node BASE: Dimensionless = 1.0;\n\
             param input: Dimensionless;\n\
             node done: Dimensionless = @input + @BASE;\n\
             node pending: Dimensionless = todo { @done };",
        )
        .unwrap();
        let debug = format!("{prepared:?}");
        assert!(debug.starts_with("ExecPlan {"), "{debug}");
        for kind in ["\"executable\"", "\"supplied\"", "\"todo\""] {
            assert!(debug.contains(kind), "{kind} in {debug}");
        }
        assert!(prepared.plan().has_unfinished_definitions());
    }

    #[test]
    fn steps_depend_on_earlier_scheduled_reads_only() {
        let (tir, src, sources) = tir_from_source(
            "const node BASE: Dimensionless = 1.0;\n\
             param input: Dimensionless = @BASE;\n\
             node doubled: Dimensionless = 2.0 * @input + @BASE;\n\
             node independent: Dimensionless = 3.0;",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        let root = prepared.plan().root();
        let deps_of = |name: &str| {
            let step = root
                .steps()
                .find(|step| step.declaration().key().as_str() == name)
                .unwrap();
            step.deps()
                .iter()
                .map(|dep| root.step(*dep).declaration().key().as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(deps_of("doubled"), ["input"]);
        assert!(deps_of("input").is_empty());
        assert!(deps_of("independent").is_empty());
    }

    /// Index `scheduled` as the steps of a copy of `root`.
    fn index<'p>(
        root: &CallablePlan<'p>,
        scheduled: Vec<PlannedDeclaration<'p>>,
    ) -> Result<CallablePlan<'p>, crate::execution_plan::StepIndexError> {
        CallablePlan::new(
            root.scope(),
            root.execution_dags().to_vec(),
            root.semantic_instances().to_vec(),
            root.closure_instances().to_vec(),
            PreparedImports::default(),
            scheduled,
        )
    }

    #[test]
    fn step_indexing_rejects_duplicate_and_unordered_schedules() {
        let (tir, src, sources) = tir_from_source(
            "node a: Dimensionless = 1.0;\n\
             node b: Dimensionless = @a + 1.0;",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        let plan = prepared.plan();
        let root = plan.root();
        let a = plan.declaration(&resolved_key("a")).unwrap().clone();
        let b = plan.declaration(&resolved_key("b")).unwrap().clone();
        assert!(index(root, vec![a.clone(), b.clone()]).is_ok());
        assert!(matches!(
            index(root, vec![b, a.clone()]),
            Err(crate::execution_plan::StepIndexError::Unordered { .. })
        ));
        assert!(matches!(
            index(root, vec![a.clone(), a]),
            Err(crate::execution_plan::StepIndexError::Duplicate(_))
        ));
    }

    fn quantity(rv: &RuntimeValue) -> f64 {
        match rv {
            RuntimeValue::Quantity(v) => v.get(),
            other => panic!("expected quantity, got {other:?}"),
        }
    }

    fn test_dag_id() -> graphcal_compiler::dag_id::DagId {
        graphcal_compiler::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(
            "test.gcl",
        ))
        .unwrap()
    }

    fn resolved_key(name: &str) -> ResolvedDeclName {
        ResolvedDeclName::for_test(test_dag_id(), DeclName::expect_valid(name))
    }

    #[test]
    fn compile_simple_const() {
        let prepared = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        let plan = prepared.plan();
        assert!(
            (quantity(root_constant(plan, &resolved_key("g0"))) - 9.80665).abs() < f64::EPSILON
        );
        assert!(root_order(plan).is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let prepared = compile_source(
            "const node g0: Dimensionless = 9.80665;\nconst node two_g0: Dimensionless = 2.0 * @g0;",
        )
        .unwrap();
        assert!(
            (quantity(root_constant(prepared.plan(), &resolved_key("two_g0"))) - 19.6133).abs()
                < 1e-10
        );
    }

    #[test]
    fn sealed_fact_stores_are_reused_by_runtime_planning() {
        let (tir, src, sources) = tir_from_source(
            "const node lower: Dimensionless = 1.0;\n\
             param x: Dimensionless(min: @lower, max: 3.0) = 2.0;",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        let plan = prepared.plan();
        let root = plan.program().dag(tir.root_dag_id()).unwrap();

        let key = resolved_key("lower");
        assert!(std::ptr::eq(
            root.const_values().get(&key).unwrap(),
            root_constant(plan, &key)
        ));
        let x = resolved_key("x");
        assert!(std::ptr::eq(
            root.domain_constraints().get(&x).unwrap(),
            plan.domain_constraint(&x).unwrap()
        ));
    }

    #[test]
    fn constructor_application_constraints_match_resolved_field_contracts() {
        let (tir, src, sources) = tir_from_source(
            "type Bounded { Bounded(value: Dimensionless(min: 1.0)), } node item: Bounded = Bounded(value: 2.0);",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        let field_constraints = prepared.plan().program().facts().struct_field_constraints();
        assert_eq!(field_constraints.len(), 1);
        let mut applications = Vec::new();
        for (_, body) in tir.root().bodies_for_test().roots() {
            if let graphcal_compiler::tir::texpr::CheckedBody::Executable(body) = body {
                graphcal_compiler::tir::texpr::visit_tnodes(body.as_node(), &mut |node| {
                    if let graphcal_compiler::tir::texpr::TNodeRef::Value(expr) = node
                        && let Some(application) = expr.application()
                    {
                        applications.push(application);
                    }
                });
            }
        }
        assert_eq!(applications.len(), 1);
        let application = applications[0];
        let keys = application
            .constructor
            .constrained_fields()
            .map(|field| {
                graphcal_compiler::tir::typed::model::StructFieldConstraintKey::for_application(
                    graphcal_compiler::semantic::checked_type::StructTypeRef::from_resolved(
                        application.definition().clone(),
                    ),
                    application.generic_args().to_vec(),
                    application.constructor.name(),
                    field.clone(),
                )
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            keys,
            field_constraints.keys().cloned().collect::<HashSet<_>>()
        );
    }

    #[test]
    fn compile_runtime_dag() {
        let prepared = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let order = root_order(prepared.plan());
        let position = |name: &str| order.iter().position(|n| n.as_str() == name).unwrap();
        let (x_pos, y_pos, z_pos) = (position("x"), position("y"), position("z"));
        assert!(x_pos < y_pos);
        assert!(y_pos < z_pos);
    }

    #[test]
    fn compile_const_cycle() {
        let err = compile_source(
            "const node a: Dimensionless = @b + 1.0;\nconst node b: Dimensionless = @a + 1.0;",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_runtime_cycle() {
        let err =
            compile_source("node a: Dimensionless = @b + 1.0;\nnode b: Dimensionless = @a + 1.0;")
                .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_uses_collected_semantic_const_deps() {
        let (tir, src, sources) = tir_from_source(
            "const node a: Dimensionless = 1.0;\n\
             const node b: Dimensionless = @a + 1.0;",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        assert!(
            (quantity(root_constant(
                prepared.plan(),
                &ResolvedDeclName::for_test(tir.root_dag_id().clone(), DeclName::expect_valid("b"))
            )) - 2.0)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn compile_uses_collected_semantic_runtime_deps() {
        let (tir, src, sources) = tir_from_source(
            "node a: Dimensionless = 1.0;\n\
             node b: Dimensionless = @a + 1.0;",
        );
        let prepared = compile(&tir, src, &sources).unwrap();
        let order = root_order(prepared.plan());
        let a_pos = order
            .iter()
            .position(|name| {
                *name
                    == &ResolvedDeclName::for_test(
                        tir.root_dag_id().clone(),
                        DeclName::expect_valid("a"),
                    )
            })
            .unwrap();
        let b_pos = order
            .iter()
            .position(|name| {
                *name
                    == &ResolvedDeclName::for_test(
                        tir.root_dag_id().clone(),
                        DeclName::expect_valid("b"),
                    )
            })
            .unwrap();
        assert!(a_pos < b_pos);
    }

    // -----------------------------------------------------------------------
    // Domain constraints on const nodes (#441)
    // -----------------------------------------------------------------------

    #[test]
    fn const_domain_value_within_bounds_passes() {
        compile_source("const node MAX_M: Mass(min: 1.0 kg, max: 100.0 kg) = 50.0 kg;").unwrap();
    }

    #[test]
    fn const_domain_value_below_min_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_value_above_max_rejected() {
        let err = compile_source("const node X: Mass(max: 10.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_min_exceeds_max_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg, max: 50.0 kg) = 75.0 kg;")
            .unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainMinExceedsMax { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_invalid_target_rejected() {
        // `Bool` is not a valid constraint target; this should now fire on consts too.
        let err = compile_source("const node FLAG: Bool(min: 0.0) = true;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::InvalidDomainTarget { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_value_within_bounds() {
        compile_source("const node N: Int(min: 1, max: 100) = 5;").unwrap();
    }

    #[test]
    fn const_domain_int_value_out_of_bounds_rejected() {
        let err = compile_source("const node N: Int(min: 1, max: 10) = 100;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_preserves_full_range_bounds() {
        compile_source(
            "const node MIN_I: Int(\
             min: -9223372036854775807 - 1, \
             max: 9223372036854775807) = -9223372036854775807 - 1;",
        )
        .unwrap();
    }

    #[test]
    fn const_datetime_domain_bounds_are_inclusive() {
        compile_source(
            r#"
const node START: Datetime<TT> = epoch<TT>("2024-01-01T00:00:00");
const node EVENT: Datetime<TT>(
    min: @START,
    max: epoch<TT>("2024-12-31T23:59:59"),
) = @START;
"#,
        )
        .unwrap();
    }

    #[test]
    fn const_datetime_domain_violation_is_rejected() {
        let error = compile_source(
            r#"
const node EVENT: Datetime(
    min: datetime("2024-01-01T00:00:00Z"),
    max: datetime("2024-12-31T23:59:59Z"),
) = datetime("2025-01-01T00:00:00Z");
"#,
        )
        .unwrap_err();
        assert!(matches!(error, GraphcalError::DomainViolation { .. }));
    }

    #[test]
    fn datetime_domain_min_exceeds_max_is_rejected() {
        let error = compile_source(
            r#"
const node EVENT: Datetime<TT>(
    min: epoch<TT>("2025-01-01T00:00:00"),
    max: epoch<TT>("2024-01-01T00:00:00"),
) = epoch<TT>("2024-06-01T00:00:00");
"#,
        )
        .unwrap_err();
        assert!(matches!(error, GraphcalError::DomainMinExceedsMax { .. }));
    }
}
