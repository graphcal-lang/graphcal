//! Runtime execution-plan selection from retained checked facts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::{DagTIR, TIR};

use crate::constant_pools::{ConstantPools, ConstantReference};
use crate::decl_key::RuntimeDeclKey;
use crate::declaration_locations::DeclarationLocations;
use crate::execution_facts::CheckedExecutionFacts;
use crate::execution_plan::{CallablePlan, ExecPlan, PreparedConstantImport, PreparedImports};
use crate::execution_scope::CheckedExecutionScope;

/// Check a TIR and select its root execution plan.
///
/// This test convenience mirrors the production check-then-prepare pipeline.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when static execution-fact checking or plan
/// selection fails.
#[cfg(test)]
pub fn compile(tir: &TIR, src: &NamedSource<Arc<String>>) -> Result<ExecPlan, GraphcalError> {
    compile_with_cancellation(
        tir,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile a TIR into an execution plan with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid plan or cancellation.
#[cfg(test)]
pub fn compile_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    let facts =
        crate::project_compiler::check_execution_facts_with_cancellation(tir, src, cancellation)?;
    compile_checked_with_cancellation(tir, &facts, src, cancellation)
}

pub fn semantic_runtime_dags_from<'a>(
    tir: &'a TIR,
    root: &'a graphcal_compiler::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a graphcal_compiler::tir::typed::DagTIR>, GraphcalError> {
    let mut owners = vec![root.dag_id().clone()];
    let mut visited = HashSet::from([root.dag_id().clone()]);
    let mut cursor = 0;
    while let Some(owner) = owners.get(cursor).cloned() {
        cursor = cursor.saturating_add(1);
        let dag = tir.dag_registry().get(&owner).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("semantic runtime instance `{owner}` has no compiled DAG"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        for edge in dag.semantic_instances() {
            let child = edge.instance.id.owner().clone();
            if visited.insert(child.clone()) {
                owners.push(child);
            }
        }
    }
    let mut instances = owners
        .into_iter()
        .skip(1)
        .map(|owner| &tir.dag_registry()[&owner])
        .collect::<Vec<_>>();
    instances.sort_by(|left, right| left.dag_id().cmp(right.dag_id()));
    Ok(std::iter::once(root).chain(instances).collect())
}

pub fn combined_runtime_order_for(
    tir: &TIR,
    root: &graphcal_compiler::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<RuntimeDeclKey>, GraphcalError> {
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::ScheduleConstruction);
    let dags = semantic_runtime_dags_from(tir, root, src)?;
    let candidates = dags
        .iter()
        .flat_map(|dag| {
            dag.source_order()
                .iter()
                .filter(|(_, category)| {
                    matches!(
                        category,
                        graphcal_compiler::declaration_category::DeclCategory::Param
                            | graphcal_compiler::declaration_category::DeclCategory::Node
                    )
                })
                .map(|(name, _)| {
                    dag.require_bound_decl_identity(name, src, DiagnosticAnchor::WholeFile)
                        .map(RuntimeDeclKey::resolved)
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let candidate_set = candidates.iter().cloned().collect::<HashSet<_>>();
    let mut indegree = candidates
        .iter()
        .cloned()
        .map(|candidate| (candidate, 0_usize))
        .collect::<HashMap<_, _>>();
    let mut dependents: HashMap<RuntimeDeclKey, Vec<RuntimeDeclKey>> = HashMap::new();
    for dag in dags {
        for (declaration, dependencies) in &dag.semantic().dependencies.runtime_deps {
            let declaration = RuntimeDeclKey::resolved(dag.runtime_decl_identity(declaration));
            if !candidate_set.contains(&declaration) {
                continue;
            }
            for dependency in dependencies {
                let dependency = RuntimeDeclKey::resolved(dag.runtime_decl_identity(dependency));
                if candidate_set.contains(&dependency) {
                    let count = indegree.entry(declaration.clone()).or_default();
                    *count = count.checked_add(1).ok_or_else(|| {
                        GraphcalError::internal_error(
                            "combined runtime dependency count overflowed",
                            src,
                            DiagnosticAnchor::WholeFile,
                        )
                    })?;
                    dependents
                        .entry(dependency)
                        .or_default()
                        .push(declaration.clone());
                }
            }
        }
    }
    let mut ready = candidates
        .iter()
        .filter(|candidate| indegree.get(*candidate) == Some(&0))
        .cloned()
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(candidates.len());
    while let Some(declaration) = ready.pop_front() {
        order.push(declaration.clone());
        for dependent in dependents.get(&declaration).into_iter().flatten() {
            let count = indegree.get_mut(dependent).ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("combined runtime dependency `{dependent}` has no schedule entry"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            *count = count.saturating_sub(1);
            if *count == 0 {
                ready.push_back(dependent.clone());
            }
        }
    }
    if order.len() != candidates.len() {
        return Err(GraphcalError::internal_error(
            "semantic instance runtime dependencies are cyclic",
            src,
            DiagnosticAnchor::WholeFile,
        ));
    }
    Ok(order)
}

/// Build a runtime schedule from facts retained by the checked project.
pub fn compile_checked_with_cancellation(
    tir: &TIR,
    facts: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    cancellation.checkpoint()?;
    validate_execution_facts(tir, facts, src, cancellation)?;
    let declaration_locations = prepare_declaration_locations(tir, src)?;
    let root = prepare_callable_plan(
        tir,
        facts,
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
            prepare_callable_plan(tir, facts, dag, &declaration_locations, src, cancellation)
                .map(|plan| (plan.owner.clone(), plan))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(ExecPlan {
        has_unfinished_definitions: tir.dag_registry().values().any(|dag| {
            dag.nodes()
                .iter()
                .any(|node| node.definition.todo().is_some())
        }),
        declaration_locations,
        root,
        callables,
        checked_execution_facts: facts.clone(),
    })
}

fn prepare_callable_plan(
    tir: &TIR,
    facts: &CheckedExecutionFacts,
    body: &DagTIR,
    declaration_locations: &DeclarationLocations,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CallablePlan, GraphcalError> {
    cancellation.checkpoint()?;
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PlanConstruction);
    let root_scope = checked_scope(tir, facts, body.dag_id(), src)?;
    let root_facts = root_scope.facts();
    let src = root_facts.source();
    let semantic_dags = semantic_runtime_dags_from(tir, root_scope.dag(), src)?;
    let semantic_facts = semantic_dags
        .iter()
        .map(|dag| checked_scope(tir, facts, dag.dag_id(), src).map(CheckedExecutionScope::facts))
        .collect::<Result<Vec<_>, _>>()?;
    let has_instances = semantic_dags.len() > 1;
    let const_values = ConstantPools::try_new(
        semantic_facts
            .iter()
            .map(|facts| Arc::clone(&facts.const_values)),
    )
    .map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })?;
    let domain_constraints = if has_instances {
        Arc::new(
            semantic_facts
                .iter()
                .flat_map(|facts| facts.domain_constraints.iter())
                .map(|(key, constraint)| (key.clone(), constraint.clone()))
                .collect(),
        )
    } else {
        Arc::clone(&root_facts.domain_constraints)
    };
    let topo_order = if has_instances {
        combined_runtime_order_for(tir, body, src)?
    } else {
        root_facts.topo_order.as_ref().clone()
    };

    validate_schedule_locations(
        &topo_order,
        declaration_locations,
        &semantic_dags.iter().map(|dag| dag.dag_id()).collect(),
        src,
    )?;

    Ok(CallablePlan {
        owner: body.dag_id().clone(),
        execution_dags: semantic_dags
            .iter()
            .map(|dag| dag.dag_id().clone())
            .collect(),
        const_values,
        imports: prepare_imports(tir, facts, &semantic_dags, declaration_locations, src)?,
        dependencies: prepare_dependencies(tir, &topo_order, declaration_locations, src)?,
        topo_order,
        assumes_map: semantic_dags
            .iter()
            .flat_map(|dag| dag.assumes_map().iter().map(move |entry| (*dag, entry)))
            .map(|(dag, (name, assumers))| {
                let key =
                    dag.require_bound_decl_identity(name, src, DiagnosticAnchor::WholeFile)?;
                let assumers = assumers
                    .iter()
                    .map(|assumer| {
                        dag.require_bound_decl_identity(assumer, src, DiagnosticAnchor::WholeFile)
                            .map(RuntimeDeclKey::resolved)
                    })
                    .collect::<Result<Vec<_>, GraphcalError>>()?;
                Ok((RuntimeDeclKey::resolved(key), assumers))
            })
            .collect::<Result<HashMap<_, _>, GraphcalError>>()?,
        expected_fail: semantic_dags
            .iter()
            .flat_map(|dag| dag.expected_fail_entries().map(move |entry| (*dag, entry)))
            .map(|(dag, (name, expected))| {
                dag.require_bound_decl_identity(name, src, DiagnosticAnchor::WholeFile)
                    .map(|key| (RuntimeDeclKey::resolved(key), expected.clone()))
            })
            .collect::<Result<HashMap<_, _>, GraphcalError>>()?,
        domain_constraints,
    })
}

fn prepare_dependencies(
    tir: &TIR,
    order: &[RuntimeDeclKey],
    locations: &DeclarationLocations,
    source: &NamedSource<Arc<String>>,
) -> Result<HashMap<RuntimeDeclKey, Vec<RuntimeDeclKey>>, GraphcalError> {
    let invalid = |message: String| {
        GraphcalError::internal_error(message, source, DiagnosticAnchor::WholeFile)
    };
    let positions = order
        .iter()
        .enumerate()
        .map(|(position, key)| (key, position))
        .collect::<HashMap<_, _>>();
    order
        .iter()
        .map(|key| {
            let body = locations
                .body_for(key)
                .map_err(|error| invalid(error.to_string()))?;
            let dag = tir
                .dag_registry()
                .get(body)
                .ok_or_else(|| invalid(format!("prepared body `{body}` is absent")))?;
            let dependencies = dag
                .semantic()
                .dependencies
                .runtime_deps
                .get(key.as_resolved())
                .into_iter()
                .flatten()
                .map(|dependency| RuntimeDeclKey::resolved(dag.runtime_decl_identity(dependency)))
                .collect::<Vec<_>>();
            for dependency in &dependencies {
                locations
                    .body_for(dependency)
                    .map_err(|error| invalid(error.to_string()))?;
                if let Some(dependency_position) = positions.get(dependency)
                    && dependency_position >= &positions[key]
                {
                    return Err(invalid(format!(
                        "schedule evaluates `{key}` before its dependency `{dependency}`"
                    )));
                }
            }
            Ok((key.clone(), dependencies))
        })
        .collect()
}

fn prepare_imports(
    tir: &TIR,
    facts: &CheckedExecutionFacts,
    dags: &[&DagTIR],
    locations: &DeclarationLocations,
    source: &NamedSource<Arc<String>>,
) -> Result<PreparedImports, GraphcalError> {
    use graphcal_compiler::ir::imported_binding::ImportedValueKind;
    let invalid = |message: String| {
        GraphcalError::internal_error(message, source, DiagnosticAnchor::WholeFile)
    };
    let mut result = PreparedImports::default();
    for dag in dags {
        let own_names = dag
            .consts()
            .iter()
            .map(|entry| entry.name.member())
            .chain(dag.params().iter().map(|entry| entry.name.member()))
            .chain(dag.nodes().iter().map(|entry| entry.name.member()))
            .collect::<HashSet<_>>();
        for (scoped, binding) in dag.imported_bindings() {
            // Lexical shadows are decided once, never rediscovered during a call.
            if !dag.semantic().decl_bindings.contains_key(scoped)
                && own_names.contains(scoped.member())
            {
                continue;
            }
            let source_key = RuntimeDeclKey::resolved(binding.target().clone());
            let owner = locations
                .body_for(&source_key)
                .map_err(|error| invalid(error.to_string()))?;
            let scope = checked_scope(tir, facts, owner, source)?;
            match binding.kind() {
                ImportedValueKind::Constant => result.constants.push(PreparedConstantImport {
                    destination: RuntimeDeclKey::resolved(
                        dag.runtime_decl_identity(binding.target()),
                    ),
                    value: ConstantReference::try_new(
                        Arc::clone(&scope.facts().const_values),
                        source_key,
                    )
                    .map_err(|error| invalid(error.to_string()))?,
                }),
                ImportedValueKind::Runtime => result.runtime.push(source_key),
            }
        }
    }
    Ok(result)
}

fn prepare_declaration_locations(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclarationLocations, GraphcalError> {
    DeclarationLocations::try_new(tir.dag_registry().values().flat_map(|dag| {
        dag.value_declaration_identities().map(|identity| {
            (
                RuntimeDeclKey::resolved(identity.clone()),
                dag.dag_id().clone(),
            )
        })
    }))
    .map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })
}

fn validate_schedule_locations(
    order: &[RuntimeDeclKey],
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

fn checked_scope<'a>(
    tir: &'a TIR,
    facts: &'a CheckedExecutionFacts,
    owner: &graphcal_compiler::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedExecutionScope<'a>, GraphcalError> {
    CheckedExecutionScope::new(tir, facts, owner).map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })
}

/// Validate coverage once at preparation, including DAGs reached only by calls.
fn validate_execution_facts(
    tir: &TIR,
    all_facts: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), GraphcalError> {
    use graphcal_compiler::declaration_category::DeclCategory;

    for dag in tir.dag_registry().values() {
        cancellation.checkpoint()?;
        let scope = checked_scope(tir, all_facts, dag.dag_id(), src)?;
        let facts = scope.facts();
        let invalid = |message: String| {
            GraphcalError::internal_error(message, facts.source(), DiagnosticAnchor::WholeFile)
        };
        let expressions = dag
            .expression_facts()
            .map_err(|error| invalid(error.to_string()))?;
        for (_, record) in expressions.records() {
            if let graphcal_compiler::tir::expression_facts::ExpressionFact::Value {
                constructor: Some(application),
                ..
            } = &record.fact
            {
                for field in &application.required_constraints {
                    let key = graphcal_compiler::tir::typed::model::StructFieldConstraintKey::for_application(
                        graphcal_compiler::registry::declared_type::StructTypeRef::from_resolved(application.definition.clone()),
                        application.generic_args.clone(), application.constructor.clone(), field.clone(),
                    );
                    if !all_facts.struct_field_constraints.contains_key(&key) {
                        return Err(invalid(format!(
                            "constructor application has no required field constraint: {key:?}"
                        )));
                    }
                }
            }
        }
        dag.imported_bindings().values().try_for_each(|binding| {
            crate::execution_scope::checked_imported_constant(tir, all_facts, binding)
                .map(|_| ())
                .map_err(|error| invalid(error.to_string()))
        })?;
        let expected = dag
            .source_order()
            .iter()
            .filter(|(_, category)| matches!(category, DeclCategory::Param | DeclCategory::Node))
            .map(|(name, _)| {
                dag.require_bound_decl_identity(name, facts.source(), DiagnosticAnchor::WholeFile)
                    .map(RuntimeDeclKey::resolved)
            })
            .collect::<Result<HashSet<_>, _>>()?;
        let scheduled = facts.topo_order.iter().cloned().collect::<HashSet<_>>();
        if scheduled != expected || scheduled.len() != facts.topo_order.len() {
            return Err(invalid(format!(
                "DAG `{}` has incomplete or duplicate runtime schedule entries",
                dag.dag_id()
            )));
        }
        for entry in dag.consts() {
            let key = RuntimeDeclKey::resolved(dag.require_bound_decl_identity(
                &entry.name,
                facts.source(),
                DiagnosticAnchor::Source(entry.span),
            )?);
            if !facts.const_values.contains_key(&key) {
                return Err(invalid(format!(
                    "checked constant `{key}` has no evaluated value"
                )));
            }
        }
        for declaration in dag.semantic().domain_bounds.keys() {
            let key = RuntimeDeclKey::resolved(declaration.clone());
            if !facts.domain_constraints.contains_key(&key) {
                return Err(invalid(format!(
                    "declaration `{key}` has no resolved domain constraint"
                )));
            }
        }
        // Also reject dangling include edges in non-root callable DAGs.
        for edge in dag.semantic_instances() {
            checked_scope(tir, all_facts, edge.instance.id.owner(), facts.source())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::registry::runtime_value::RuntimeValue;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
    use graphcal_compiler::syntax::module_resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::{ProjectTypeStore, type_resolve_with_modules};

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let (mut tir, src) = resolved_tir_from_source(source);
        graphcal_compiler::tir::dim_check::check_dimensions_tir(&mut tir, &src)?;
        compile(&tir, &src)
    }

    fn tir_from_source(
        source: &str,
    ) -> (graphcal_compiler::tir::typed::TIR, NamedSource<Arc<String>>) {
        let (mut tir, src) = resolved_tir_from_source(source);
        graphcal_compiler::tir::dim_check::check_dimensions_tir(&mut tir, &src).unwrap();
        (tir, src)
    }

    fn resolved_tir_from_source(
        source: &str,
    ) -> (graphcal_compiler::tir::typed::TIR, NamedSource<Arc<String>>) {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = desugared;
        let src = make_src(source);
        let ir = lower(&file, &src).unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(ir.dag_id().clone(), &file.declarations)
            .unwrap();
        let mut project_types = ProjectTypeStore::default();
        project_types.insert_graphcal_prelude().unwrap();
        project_types.insert_local_hir(&ir).unwrap();
        let tir = type_resolve_with_modules(ir, &src, &resolver, Arc::new(project_types)).unwrap();
        (tir, src)
    }

    #[test]
    fn prepared_locations_include_parameters_without_defaults() {
        let (tir, src) = tir_from_source(
            "param input: Dimensionless; node doubled: Dimensionless = 2.0 * @input;",
        );
        let plan = compile(&tir, &src).unwrap();
        let input = resolved_key("input");
        assert!(tir.root().runtime_expr(input.as_resolved()).is_none());
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

    fn resolved_key(name: &str) -> RuntimeDeclKey {
        RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
            test_dag_id(),
            DeclName::expect_valid(name),
        ))
    }

    #[test]
    fn preparation_rejects_dependency_order_corruption() {
        let (tir, src) = tir_from_source(
            "node antecedent: Dimensionless = 1.0; node subsequent: Dimensionless = @antecedent;",
        );
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let mut facts = crate::project_compiler::check_execution_facts_with_cancellation(
            &tir,
            &src,
            &cancellation,
        )
        .unwrap();
        let root = Arc::make_mut(
            Arc::make_mut(&mut facts.by_dag)
                .get_mut(tir.root_dag_id())
                .unwrap(),
        );
        Arc::make_mut(&mut root.topo_order).reverse();
        assert!(
            matches!(compile_checked_with_cancellation(&tir, &facts, &src, &cancellation), Err(GraphcalError::InternalError { message, .. }) if message.contains("before its dependency"))
        );
    }

    #[test]
    fn constant_pool_views_reject_duplicates_and_missing_imports() {
        let key = resolved_key("constant");
        let pool = Arc::new(HashMap::from([(
            key.clone(),
            RuntimeValue::quantity(2.0).unwrap(),
        )]));
        assert!(matches!(
            ConstantPools::try_new([Arc::clone(&pool), Arc::clone(&pool)]),
            Err(crate::constant_pools::ConstantPoolError::Duplicate(_))
        ));
        assert!(matches!(
            ConstantReference::try_new(Arc::clone(&pool), resolved_key("absent")),
            Err(crate::constant_pools::ConstantPoolError::Missing(_))
        ));
        let imported = ConstantReference::try_new(Arc::clone(&pool), key.clone()).unwrap();
        assert!(std::ptr::eq(
            imported.value().unwrap(),
            pool.get(&key).unwrap()
        ));
    }

    #[test]
    fn compile_simple_const() {
        let plan = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        assert!(
            (quantity(plan.root.const_values.get(&resolved_key("g0")).unwrap()) - 9.80665).abs()
                < f64::EPSILON
        );
        assert!(plan.root.topo_order.is_empty());
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
    fn checked_fact_stores_are_reused_by_runtime_planning() {
        let (tir, src) = tir_from_source(
            "const node lower: Dimensionless = 1.0;\n\
             param x: Dimensionless(min: @lower, max: 3.0) = 2.0;",
        );
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let facts = crate::project_compiler::check_execution_facts_with_cancellation(
            &tir,
            &src,
            &cancellation,
        )
        .unwrap();
        let plan = compile_checked_with_cancellation(&tir, &facts, &src, &cancellation).unwrap();
        let root_facts = facts.for_dag(tir.root_dag_id()).unwrap();

        let key = resolved_key("lower");
        assert!(std::ptr::eq(
            root_facts.const_values.get(&key).unwrap(),
            plan.root.const_values.get(&key).unwrap()
        ));
        assert!(Arc::ptr_eq(
            &root_facts.domain_constraints,
            &plan.root.domain_constraints
        ));
        assert!(Arc::ptr_eq(
            &facts.struct_field_constraints,
            &plan.checked_execution_facts.struct_field_constraints
        ));
    }

    #[test]
    fn preparation_rejects_incomplete_or_mismatched_facts() {
        #[derive(Debug, Clone, Copy)]
        enum Damage {
            MissingDag,
            WrongOwner,
            MissingConstant,
            MissingScheduleEntry,
            DuplicateScheduleEntry,
            MissingConstraint,
        }

        let (tir, src) = tir_from_source(
            "const node lower_bound: Dimensionless = 0.0;\n\
             param x: Dimensionless(min: @lower_bound);\n\
             node y: Dimensionless = @x + 1.0;",
        );
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let checked = crate::project_compiler::check_execution_facts_with_cancellation(
            &tir,
            &src,
            &cancellation,
        )
        .unwrap();
        for damage in [
            Damage::MissingDag,
            Damage::WrongOwner,
            Damage::MissingConstant,
            Damage::MissingScheduleEntry,
            Damage::DuplicateScheduleEntry,
            Damage::MissingConstraint,
        ] {
            let mut corrupted = checked.clone();
            let dags = Arc::make_mut(&mut corrupted.by_dag);
            let facts = Arc::make_mut(dags.get_mut(tir.root_dag_id()).unwrap());
            match damage {
                Damage::MissingDag => {
                    dags.remove(tir.root_dag_id()).unwrap();
                }
                Damage::WrongOwner => {
                    facts.dag_id = graphcal_compiler::dag_id::DagId::from_virtual_relative_path(
                        std::path::Path::new("other.gcl"),
                    )
                    .unwrap();
                }
                Damage::MissingConstant => Arc::make_mut(&mut facts.const_values).clear(),
                Damage::MissingScheduleEntry => {
                    Arc::make_mut(&mut facts.topo_order).pop().unwrap();
                }
                Damage::DuplicateScheduleEntry => {
                    let key = facts.topo_order[0].clone();
                    Arc::make_mut(&mut facts.topo_order).push(key);
                }
                Damage::MissingConstraint => {
                    Arc::make_mut(&mut facts.domain_constraints).clear();
                }
            }
            let error = compile_checked_with_cancellation(&tir, &corrupted, &src, &cancellation)
                .expect_err("corrupt checked facts must never produce an executable plan");
            assert!(
                matches!(error, GraphcalError::InternalError { .. }),
                "{damage:?}: {error:?}"
            );
        }
        // Mutation of a clone must not damage already published artifacts.
        compile_checked_with_cancellation(&tir, &checked, &src, &cancellation).unwrap();
    }

    #[test]
    fn imported_constants_require_defining_facts_and_runtime_absence_is_explicit() {
        use crate::execution_scope::{ExecutionScopeError, checked_imported_constant};
        use graphcal_compiler::ir::imported_binding::{ImportedBinding, ImportedValueKind};

        let (tir, src) =
            tir_from_source("const node C: Dimensionless = 2.0; param x: Dimensionless;");
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let facts = crate::project_compiler::check_execution_facts_with_cancellation(
            &tir,
            &src,
            &cancellation,
        )
        .unwrap();
        let binding = |name, kind| {
            let target = resolved_key(name).as_resolved().clone();
            ImportedBinding::new(
                target.clone(),
                tir.runtime_declared_type(&target, &src).unwrap(),
                kind,
            )
        };
        let constant = binding("C", ImportedValueKind::Constant);
        assert!(
            (quantity(
                checked_imported_constant(&tir, &facts, &constant)
                    .unwrap()
                    .unwrap()
            ) - 2.0)
                .abs()
                < f64::EPSILON
        );
        assert!(
            checked_imported_constant(&tir, &facts, &binding("x", ImportedValueKind::Runtime))
                .unwrap()
                .is_none()
        );
        for wrong in [
            binding("C", ImportedValueKind::Runtime),
            binding("x", ImportedValueKind::Constant),
        ] {
            assert!(matches!(
                checked_imported_constant(&tir, &facts, &wrong),
                Err(ExecutionScopeError::WrongImportedKind { .. })
            ));
        }
        let mut missing_value = facts.clone();
        let dag_facts = Arc::make_mut(
            Arc::make_mut(&mut missing_value.by_dag)
                .get_mut(tir.root_dag_id())
                .unwrap(),
        );
        Arc::make_mut(&mut dag_facts.const_values).clear();
        assert!(matches!(
            checked_imported_constant(&tir, &missing_value, &constant),
            Err(ExecutionScopeError::MissingConstant(_))
        ));
        let mut missing_owner = facts.clone();
        Arc::make_mut(&mut missing_owner.by_dag).clear();
        assert!(matches!(
            checked_imported_constant(&tir, &missing_owner, &constant),
            Err(ExecutionScopeError::MissingFacts(_))
        ));
        // Corrupting isolated test copies must not damage the published facts.
        assert!(
            checked_imported_constant(&tir, &facts, &constant)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn preparation_rejects_missing_included_instance_facts() {
        let source = "dag lib { pub node out: Dimensionless = 1.0; }\n\
                      include lib() as inst;\n\
                      node result: Dimensionless = @inst::out;";
        let loaded = crate::loader::LoadedProject::from_source(source, "test.gcl").unwrap();
        let checked = crate::project_compiler::ProjectCompiler::new(&loaded)
            .check()
            .unwrap();
        let tir = checked.tir();
        let src = make_src(source);
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let mut facts = crate::project_compiler::check_execution_facts_with_cancellation(
            tir,
            &src,
            &cancellation,
        )
        .unwrap();
        let instance = tir
            .root()
            .semantic_instances()
            .first()
            .unwrap()
            .instance
            .id
            .owner();
        Arc::make_mut(&mut facts.by_dag).remove(instance).unwrap();
        let error =
            compile_checked_with_cancellation(tir, &facts, &src, &cancellation).unwrap_err();
        assert!(matches!(error, GraphcalError::InternalError { .. }));
    }

    #[test]
    fn preparation_rejects_missing_required_constructor_field_contract() {
        let (tir, src) = tir_from_source(
            "type Bounded { Bounded(value: Dimensionless(min: 1.0)), } node item: Bounded = Bounded(value: 2.0);",
        );
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let mut facts = crate::project_compiler::check_execution_facts_with_cancellation(
            &tir,
            &src,
            &cancellation,
        )
        .unwrap();
        assert_eq!(facts.struct_field_constraints.len(), 1);
        let applications = tir
            .root()
            .expression_facts()
            .unwrap()
            .records()
            .filter_map(|(_, record)| match &record.fact {
                graphcal_compiler::tir::expression_facts::ExpressionFact::Value {
                    constructor: Some(application),
                    ..
                } => Some(application),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].required_constraints.len(), 1);
        facts.struct_field_constraints = Arc::new(HashMap::new());
        assert!(matches!(
            compile_checked_with_cancellation(&tir, &facts, &src, &cancellation),
            Err(GraphcalError::InternalError { .. })
        ));
    }

    #[test]
    fn compile_runtime_dag() {
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan
            .root
            .topo_order
            .iter()
            .position(|n| n.member() == "x")
            .unwrap();
        let y_pos = plan
            .root
            .topo_order
            .iter()
            .position(|n| n.member() == "y")
            .unwrap();
        let z_pos = plan
            .root
            .topo_order
            .iter()
            .position(|n| n.member() == "z")
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
                    .get(&RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                        tir.root_dag_id().clone(),
                        DeclName::expect_valid("b")
                    )))
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
            .topo_order
            .iter()
            .position(|name| {
                name == &RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("a"),
                ))
            })
            .unwrap();
        let b_pos = plan
            .root
            .topo_order
            .iter()
            .position(|name| {
                name == &RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("b"),
                ))
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
