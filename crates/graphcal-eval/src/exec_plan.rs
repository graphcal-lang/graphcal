//! Runtime execution-plan selection from a sealed checked program.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::{CheckedDag, CheckedTir};

use crate::checked_program::{CheckedProgram, SealedDag};
use crate::constant_pools::ConstantPools;
use crate::declaration_locations::DeclarationLocations;
use crate::execution_plan::{CallablePlan, ExecPlan, PreparedConstantImport, PreparedImports};
use graphcal_compiler::resolved_name::ResolvedDeclName;

/// Check a TIR and select its root execution plan.
///
/// This test convenience mirrors the production check-then-prepare pipeline
/// on a copy of `tir`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when execution checking or plan selection fails.
#[cfg(test)]
pub fn compile(
    tir: &CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<ExecPlan, GraphcalError> {
    compile_with_cancellation(
        tir,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Seal a copy of a TIR and select its root execution plan with cooperative
/// cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid plan or cancellation.
#[cfg(test)]
pub fn compile_with_cancellation(
    tir: &CheckedTir,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    let program = crate::project_compiler::seal_checked_program_with_cancellation(
        tir.clone(),
        src,
        cancellation,
    )?;
    compile_checked_with_cancellation(program, src, cancellation)
}

/// Prepare the callable plans of a sealed program.
pub fn compile_checked_with_cancellation(
    program: CheckedProgram,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    cancellation.checkpoint()?;
    let tir = program.tir();
    let declaration_locations = prepare_declaration_locations(tir, src)?;
    let root = prepare_callable_plan(
        &program,
        tir.root(),
        &declaration_locations,
        src,
        cancellation,
    )?;
    let callables = tir
        .dag_registry()
        .values()
        .filter(|dag| dag.dag_id() != tir.root_dag_id())
        .map(|dag| {
            prepare_callable_plan(&program, dag, &declaration_locations, src, cancellation)
                .map(|plan| (plan.owner.clone(), plan))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let has_unfinished_definitions = tir
        .dag_registry()
        .values()
        .any(|dag| dag.nodes().any(|node| node.definition.todo().is_some()));
    Ok(ExecPlan {
        has_unfinished_definitions,
        declaration_locations,
        root,
        callables,
        program,
    })
}

fn prepare_callable_plan(
    program: &CheckedProgram,
    body: &CheckedDag,
    declaration_locations: &DeclarationLocations,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CallablePlan, GraphcalError> {
    cancellation.checkpoint()?;
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PlanConstruction);
    let root = sealed_dag(program, body.dag_id(), src)?;
    let src = root.source();
    let invalid =
        |message: String| GraphcalError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    let schedule = body.runtime_schedule();
    let semantic_dags = schedule
        .execution_dags()
        .iter()
        .map(|owner| {
            program.dag(owner).ok_or_else(|| {
                invalid(format!(
                    "semantic runtime instance `{owner}` has no compiled DAG"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let has_instances = semantic_dags.len() > 1;
    let const_values = ConstantPools::try_new(
        semantic_dags
            .iter()
            .map(|dag| Arc::clone(dag.const_values())),
    )
    .map_err(|error| invalid(error.to_string()))?;
    let domain_constraints = if has_instances {
        Arc::new(
            semantic_dags
                .iter()
                .flat_map(|dag| dag.domain_constraints().iter())
                .map(|(key, constraint)| (key.clone(), constraint.clone()))
                .collect(),
        )
    } else {
        Arc::clone(root.domain_constraints())
    };
    validate_schedule_locations(
        schedule.order().as_slice(),
        declaration_locations,
        &semantic_dags.iter().map(|dag| dag.dag().dag_id()).collect(),
        src,
    )?;
    for (_, reads) in schedule.steps() {
        for dependency in reads {
            declaration_locations
                .body_for(dependency)
                .map_err(|error| invalid(error.to_string()))?;
        }
    }

    Ok(CallablePlan {
        owner: body.dag_id().clone(),
        execution_dags: semantic_dags
            .iter()
            .map(|dag| dag.dag().dag_id().clone())
            .collect(),
        const_values,
        imports: prepare_imports(&semantic_dags, declaration_locations, src)?,
        schedule: schedule.clone(),
        assumes_map: merge_assumes_maps(semantic_dags.iter().map(|dag| dag.dag().assumes_map())),
        expected_fail: semantic_dags
            .iter()
            .flat_map(|dag| dag.dag().expected_fail_entries())
            .map(|(assertion, expected)| (assertion.clone(), expected.clone()))
            .collect(),
        domain_constraints,
    })
}

/// Merge per-DAG `#[assumes]` tables keyed by runtime identity.
///
/// One assertion can be assumed both inside its semantic instance and by the
/// importer through a projection, so tables of different DAGs share keys.
fn merge_assumes_maps<'a>(
    maps: impl IntoIterator<Item = &'a HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>>,
) -> HashMap<ResolvedDeclName, Vec<ResolvedDeclName>> {
    let mut merged = HashMap::<ResolvedDeclName, Vec<ResolvedDeclName>>::new();
    for (assertion, assumers) in maps.into_iter().flatten() {
        let entry = merged.entry(assertion.clone()).or_default();
        for assumer in assumers {
            if !entry.contains(assumer) {
                entry.push(assumer.clone());
            }
        }
    }
    merged
}

/// Select the imports of a callable's execution DAGs: the constants the
/// program resolved when it was sealed, and the explicit runtime imports.
fn prepare_imports(
    dags: &[SealedDag<'_>],
    locations: &DeclarationLocations,
    source: &NamedSource<Arc<String>>,
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
            locations.body_for(target).map_err(|error| {
                GraphcalError::internal_error(
                    error.to_string(),
                    source,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            result.runtime.push(target.clone());
        }
    }
    Ok(result)
}

fn prepare_declaration_locations(
    tir: &CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclarationLocations, GraphcalError> {
    DeclarationLocations::try_new(tir.dag_registry().values().flat_map(|dag| {
        dag.value_declaration_identities()
            .map(|identity| (identity.clone(), dag.dag_id().clone()))
    }))
    .map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })
}

fn validate_schedule_locations(
    order: &[ResolvedDeclName],
    locations: &DeclarationLocations,
    allowed_bodies: &HashSet<&DagId>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    order.iter().try_for_each(|declaration| {
        let body = locations.body_for(declaration).map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
        if !allowed_bodies.contains(body) {
            return Err(GraphcalError::internal_error(
                format!("scheduled declaration `{declaration}` is physically in `{body}`, outside its callable closure"),
                src,
                DiagnosticAnchor::WholeFile,
            ));
        }
        Ok(())
    })
}

fn sealed_dag<'a>(
    program: &'a CheckedProgram,
    owner: &DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<SealedDag<'a>, GraphcalError> {
    program.dag(owner).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("DAG `{owner}` has no compiled body"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::registry::runtime_value::RuntimeValue;
    use graphcal_compiler::resolve::ModuleResolver;
    use graphcal_compiler::resolved_name::ResolvedDeclName;
    use graphcal_compiler::syntax::decl_name::DeclName;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::ProjectTypeStore;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let (tir, src) = checked_tir_from_source(source)?;
        compile(&tir, &src)
    }

    fn tir_from_source(
        source: &str,
    ) -> (
        graphcal_compiler::tir::typed::CheckedTir,
        NamedSource<Arc<String>>,
    ) {
        checked_tir_from_source(source).unwrap()
    }

    fn checked_tir_from_source(
        source: &str,
    ) -> Result<
        (
            graphcal_compiler::tir::typed::CheckedTir,
            NamedSource<Arc<String>>,
        ),
        GraphcalError,
    > {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_file);
        let file = desugared;
        let src = make_src(source);
        let ir = lower(&file, &src).unwrap();
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
                &src,
                &resolver,
                &project_types,
                &cancellation,
            )
            .unwrap();
        graphcal_compiler::tir::typed::TirDraft::resolve_root(
            signed,
            std::collections::HashMap::<_, _, std::hash::RandomState>::new(),
            &src,
            &resolver,
            Arc::new(project_types),
            &cancellation,
        )
        .unwrap()
        .instantiate(
            &graphcal_compiler::tir::typed::CheckedOverrideDependencies::default(),
            &src,
        )
        .unwrap()
        .check(&src, &cancellation)
        .map(|tir| (tir, src.clone()))
    }

    #[test]
    fn prepared_locations_include_parameters_without_defaults() {
        let (tir, src) = tir_from_source(
            "param input: Dimensionless; node doubled: Dimensionless = 2.0 * @input;",
        );
        let plan = compile(&tir, &src).unwrap();
        let input = resolved_key("input");
        assert!(tir.root().runtime_expr(&input).is_none());
        assert_eq!(
            plan.declaration_locations.body_for(&input).unwrap(),
            tir.root_dag_id()
        );
    }

    #[test]
    fn schedules_reject_missing_and_out_of_closure_locations() {
        let src = make_src("");
        let owner = test_dag_id();
        let other = DagId::from_virtual_relative_path(std::path::Path::new("other.gcl")).unwrap();
        let key = resolved_key("x");
        for locations in [
            DeclarationLocations::try_new([]).unwrap(),
            DeclarationLocations::try_new([(key.clone(), other)]).unwrap(),
        ] {
            assert!(
                validate_schedule_locations(
                    std::slice::from_ref(&key),
                    &locations,
                    &HashSet::from([&owner]),
                    &src,
                )
                .is_err()
            );
        }
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
    fn constant_pool_views_reject_duplicates() {
        let key = resolved_key("constant");
        let pool = Arc::new(HashMap::from([(
            key.clone(),
            RuntimeValue::quantity(2.0).unwrap(),
        )]));
        assert!(matches!(
            ConstantPools::try_new([Arc::clone(&pool), Arc::clone(&pool)]),
            Err(crate::constant_pools::ConstantPoolError::Duplicate(_))
        ));
        let pools = ConstantPools::try_new([Arc::clone(&pool)]).unwrap();
        assert!(std::ptr::eq(
            pools.get(&key).unwrap(),
            pool.get(&key).unwrap()
        ));
        assert!(pools.get(&resolved_key("absent")).is_none());
    }

    #[test]
    fn compile_simple_const() {
        let plan = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        assert!(
            (quantity(plan.root.const_values.get(&resolved_key("g0")).unwrap()) - 9.80665).abs()
                < f64::EPSILON
        );
        assert!(plan.root.schedule.order().is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let plan = compile_source(
            "const node g0: Dimensionless = 9.80665;\nconst node two_g0: Dimensionless = 2.0 * @g0;",
        )
        .unwrap();
        assert!(
            (quantity(plan.root.const_values.get(&resolved_key("two_g0")).unwrap()) - 19.6133)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn sealed_fact_stores_are_reused_by_runtime_planning() {
        let (tir, src) = tir_from_source(
            "const node lower: Dimensionless = 1.0;\n\
             param x: Dimensionless(min: @lower, max: 3.0) = 2.0;",
        );
        let plan = compile(&tir, &src).unwrap();
        let root = plan.program().dag(tir.root_dag_id()).unwrap();

        let key = resolved_key("lower");
        assert!(std::ptr::eq(
            root.const_values().get(&key).unwrap(),
            plan.root.const_values.get(&key).unwrap()
        ));
        assert!(Arc::ptr_eq(
            root.domain_constraints(),
            &plan.root.domain_constraints
        ));
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
        let src = make_src(source);
        let plan = compile(tir, &src).unwrap();
        let schedule = tir.root().runtime_schedule();
        assert_eq!(plan.root.schedule, *schedule);
        assert_eq!(plan.root.execution_dags, schedule.execution_dags());
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
    }

    #[test]
    fn constructor_application_constraints_match_resolved_field_contracts() {
        let (tir, src) = tir_from_source(
            "type Bounded { Bounded(value: Dimensionless(min: 1.0)), } node item: Bounded = Bounded(value: 2.0);",
        );
        let plan = compile(&tir, &src).unwrap();
        let field_constraints = plan.program().facts().struct_field_constraints();
        assert_eq!(field_constraints.len(), 1);
        let applications = tir
            .root()
            .expression_facts()
            .records()
            .filter_map(|(_, record)| {
                record
                    .fact
                    .concrete_value()
                    .and_then(|value| value.constructor.as_ref())
            })
            .collect::<Vec<_>>();
        assert_eq!(applications.len(), 1);
        let application = applications[0];
        let keys = application
            .constructor
            .constrained_fields()
            .map(|field| {
                graphcal_compiler::tir::typed::model::StructFieldConstraintKey::for_application(
                    graphcal_compiler::registry::checked_type::StructTypeRef::from_resolved(
                        application.definition().clone(),
                    ),
                    application.generic_args.clone(),
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
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan
            .root
            .schedule
            .order()
            .iter()
            .position(|n| n.as_str() == "x")
            .unwrap();
        let y_pos = plan
            .root
            .schedule
            .order()
            .iter()
            .position(|n| n.as_str() == "y")
            .unwrap();
        let z_pos = plan
            .root
            .schedule
            .order()
            .iter()
            .position(|n| n.as_str() == "z")
            .unwrap();
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
        let (tir, src) = tir_from_source(
            "const node a: Dimensionless = 1.0;\n\
             const node b: Dimensionless = @a + 1.0;",
        );
        let plan = compile(&tir, &src).unwrap();
        assert!(
            (quantity(
                plan.root
                    .const_values
                    .get(&ResolvedDeclName::for_test(
                        tir.root_dag_id().clone(),
                        DeclName::expect_valid("b")
                    ))
                    .unwrap()
            ) - 2.0)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn compile_uses_collected_semantic_runtime_deps() {
        let (tir, src) = tir_from_source(
            "node a: Dimensionless = 1.0;\n\
             node b: Dimensionless = @a + 1.0;",
        );
        let plan = compile(&tir, &src).unwrap();
        let a_pos = plan
            .root
            .schedule
            .order()
            .iter()
            .position(|name| {
                name == &ResolvedDeclName::for_test(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("a"),
                )
            })
            .unwrap();
        let b_pos = plan
            .root
            .schedule
            .order()
            .iter()
            .position(|name| {
                name == &ResolvedDeclName::for_test(
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
