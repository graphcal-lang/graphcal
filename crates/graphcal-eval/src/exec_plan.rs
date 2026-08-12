//! Execution plan — the result of compiling a TIR.
//!
//! Contains evaluated const values, topologically sorted runtime declarations,
//! and their expressions, ready for evaluation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::declared_type::{DeclaredGenericArg, DeclaredType, StructTypeRef};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, GenericParamName};
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::{
    ResolvedDomainBound as EvaluatedDomainBound, ResolvedDomainBounds as EvaluatedDomainBounds,
    ResolvedDomainConstraint,
};
use crate::eval_expr::{EvalContext, HirLocalValueMap, RuntimeValue, eval_hir_expr};
use crate::execution_facts::{CheckedDagExecutionFacts, CheckedExecutionFacts, RuntimeValueMap};
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::{
    DagTIR, ResolvedDagDependencies, StructFieldConstraintKey, TIR,
};

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Evaluated const values (in base SI units).
    /// Key-lookup only, order irrelevant.
    pub(crate) const_values: Arc<RuntimeValueMap>,
    /// Compile-time constants imported from dependency module artifacts.
    /// These are injected directly into the evaluation environment.
    /// Iterated once during env setup; feeds into `HashMap` (key-lookup only).
    pub(crate) imported_values: RuntimeValueMap,
    /// Topologically sorted names for runtime evaluation (params + nodes).
    pub(crate) topo_order: Vec<RuntimeDeclKey>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Key-lookup only, order irrelevant.
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Key-lookup only, order irrelevant.
    pub(crate) expected_fail: HashMap<ScopedName, graphcal_compiler::ir::resolve::ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    /// Key-lookup only, order irrelevant.
    pub(crate) domain_constraints: Arc<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>>,
    /// Resolved domain constraints for struct/union member fields, keyed by
    /// owner-qualified struct/constructor/field identity. Looked up at every
    /// `ExprKind::ConstructorCall` evaluation to validate field values.
    pub(crate) struct_field_constraints:
        Arc<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
    /// Per-DAG checked facts required by nested callable evaluation.
    pub(crate) checked_execution_facts: CheckedExecutionFacts,
}

/// Compile a TIR into an execution plan.
///
/// This performs:
/// 1. Topological sort of const declarations + compile-time evaluation
/// 2. Topological sort of runtime declarations (params + nodes) into evaluation order
///
/// # Errors
///
/// Returns a [`GraphcalError`] if there is a cyclic dependency or if
/// const evaluation fails.
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
    let facts = check_execution_facts_with_cancellation(tir, src, cancellation)?;
    compile_checked_with_cancellation(tir, &facts, src, cancellation)
}

/// Evaluate and validate compile-time facts needed by later execution planning.
#[cfg(test)]
pub fn check_execution_facts_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedExecutionFacts, GraphcalError> {
    check_execution_facts_with_inherited(tir, &CheckedExecutionFacts::empty(), src, cancellation)
}

/// Check every DAG not already present in `inherited`, preserving dependency
/// facts (including their defining source) while compiling an importing file.
pub fn check_execution_facts_with_inherited(
    tir: &TIR,
    inherited: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedExecutionFacts, GraphcalError> {
    check_dag_execution_facts(tir, inherited, src, cancellation)
}

/// Build a runtime schedule from facts retained by the checked project.
pub fn compile_checked_with_cancellation(
    tir: &TIR,
    facts: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    cancellation.checkpoint()?;
    let root_facts = facts.for_dag(tir.root_dag_id()).ok_or_else(|| {
        GraphcalError::internal_error(
            format!(
                "checked execution facts are missing root DAG `{}`",
                tir.root_dag_id()
            ),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    debug_assert_eq!(&root_facts.dag_id, tir.root_dag_id());

    Ok(ExecPlan {
        const_values: Arc::clone(&root_facts.const_values),
        imported_values: tir
            .root()
            .imported_bindings()
            .values()
            .filter_map(|binding| {
                binding.value().map(|value| {
                    (
                        RuntimeDeclKey::resolved(binding.target().clone()),
                        value.clone(),
                    )
                })
            })
            .collect(),
        topo_order: root_facts.topo_order.as_ref().clone(),
        assumes_map: tir.root().assumes_map().clone(),
        expected_fail: tir
            .root()
            .expected_fail_entries()
            .map(|(name, expected)| (name.clone(), expected.clone()))
            .collect(),
        domain_constraints: Arc::clone(&root_facts.domain_constraints),
        struct_field_constraints: Arc::clone(&facts.struct_field_constraints),
        checked_execution_facts: facts.clone(),
    })
}

type ResolvedDeclKey = graphcal_compiler::syntax::decl_name::ResolvedDeclName;

fn visible_values_with_imports(
    dag: &DagTIR,
    local_const_values: &RuntimeValueMap,
    known_const_values: &RuntimeValueMap,
) -> RuntimeValueMap {
    let mut values = known_const_values.clone();
    values.extend(dag.imported_bindings().values().filter_map(|binding| {
        binding.value().map(|value| {
            (
                RuntimeDeclKey::resolved(binding.target().clone()),
                value.clone(),
            )
        })
    }));
    values.extend(
        local_const_values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    values
}

fn known_const_values(
    tir: &TIR,
    facts: &HashMap<graphcal_compiler::dag_id::DagId, CheckedDagExecutionFacts>,
) -> RuntimeValueMap {
    let mut values = facts
        .values()
        .flat_map(|facts| facts.const_values.iter())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<RuntimeValueMap>();
    for dag in tir.dag_registry().values() {
        values.extend(dag.imported_bindings().values().filter_map(|binding| {
            binding.value().map(|value| {
                (
                    RuntimeDeclKey::resolved(binding.target().clone()),
                    value.clone(),
                )
            })
        }));
    }
    values
}

fn freeze_checked_execution_facts(
    dag_facts: HashMap<graphcal_compiler::dag_id::DagId, CheckedDagExecutionFacts>,
    struct_field_constraints: HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
) -> CheckedExecutionFacts {
    CheckedExecutionFacts {
        by_dag: Arc::new(
            dag_facts
                .into_iter()
                .map(|(dag_id, facts)| (dag_id, Arc::new(facts)))
                .collect(),
        ),
        struct_field_constraints: Arc::new(struct_field_constraints),
    }
}

fn initialized_const_pool(
    const_pools: &HashMap<graphcal_compiler::dag_id::DagId, RuntimeValueMap>,
    dag_id: &graphcal_compiler::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValueMap, GraphcalError> {
    const_pools.get(dag_id).cloned().ok_or_else(|| {
        GraphcalError::internal_error(
            format!("checked DAG `{dag_id}` has no initialized const pool"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

fn check_dag_execution_facts(
    tir: &TIR,
    inherited: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedExecutionFacts, GraphcalError> {
    cancellation.checkpoint()?;
    let mut dag_facts = inherited
        .by_dag
        .iter()
        .map(|(dag_id, facts)| (dag_id.clone(), facts.as_ref().clone()))
        .collect::<HashMap<_, _>>();
    let dag_ids = tir
        .dag_registry()
        .keys()
        .filter(|dag_id| !dag_facts.contains_key(*dag_id))
        .cloned()
        .collect::<HashSet<_>>();
    let initial_values = known_const_values(tir, &dag_facts);
    let const_pools = eval_const_pools_for_dags(tir, &dag_ids, initial_values, src, cancellation)?;

    for dag_id in &dag_ids {
        cancellation.checkpoint()?;
        let dag = tir.dag_registry().get(dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("checked DAG `{dag_id}` disappeared from the TIR registry"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let const_values = initialized_const_pool(&const_pools, dag_id, src)?;
        dag_facts.insert(
            dag_id.clone(),
            CheckedDagExecutionFacts {
                dag_id: dag_id.clone(),
                source: src.clone(),
                topo_order: Arc::new(build_runtime_dag(dag, src, cancellation)?),
                domain_constraints: Arc::new(HashMap::new()),
                const_values: Arc::new(const_values),
                struct_field_constraints: Arc::new(HashMap::new()),
            },
        );
    }

    let all_const_values = known_const_values(tir, &dag_facts);
    for dag_id in &dag_ids {
        cancellation.checkpoint()?;
        let dag = &tir.dag_registry()[dag_id];
        let facts = dag_facts.get_mut(dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("newly checked DAG facts for `{dag_id}` were lost"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        facts.domain_constraints = Arc::new(resolve_domain_constraints_for_dag(
            tir,
            dag,
            &facts.const_values,
            &all_const_values,
            facts.source(),
            cancellation,
        )?);
    }

    cancellation.checkpoint()?;
    let field_constraints = resolve_struct_field_constraints_for_dags(
        tir,
        &dag_facts,
        &all_const_values,
        src,
        cancellation,
    )?;
    for (owner, constraints) in field_constraints {
        let facts = dag_facts.get_mut(&owner).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("field-constraint owner `{owner}` has no checked DAG facts"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let mut merged = facts.struct_field_constraints.as_ref().clone();
        merged.extend(constraints);
        facts.struct_field_constraints = Arc::new(merged);
    }

    let all_field_constraints = dag_facts
        .values()
        .flat_map(|facts| facts.struct_field_constraints.iter())
        .map(|(key, constraint)| (key.clone(), constraint.clone()))
        .collect::<HashMap<_, _>>();
    for dag_id in &dag_ids {
        let dag = &tir.dag_registry()[dag_id];
        let facts = &dag_facts[dag_id];
        check_dag_const_struct_field_constraints_at_compile_time(
            dag,
            &facts.const_values,
            &all_field_constraints,
            facts.source(),
        )?;
    }

    Ok(freeze_checked_execution_facts(
        dag_facts,
        all_field_constraints,
    ))
}

#[expect(
    clippy::too_many_lines,
    reason = "dependency-ordered const evaluation keeps each canonical DAG pool and its diagnostics together"
)]
fn eval_const_pools_for_dags(
    tir: &TIR,
    dag_ids: &HashSet<graphcal_compiler::dag_id::DagId>,
    mut visible_values: RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<graphcal_compiler::dag_id::DagId, RuntimeValueMap>, GraphcalError> {
    cancellation.checkpoint()?;
    let mut graph = DiGraph::<ResolvedDeclKey, ()>::new();
    let mut index_map = HashMap::new();
    let mut declaration_by_key = HashMap::new();
    let mut sorted_dag_ids = dag_ids.iter().collect::<Vec<_>>();
    sorted_dag_ids.sort();
    for dag_id in sorted_dag_ids {
        let dag = &tir.dag_registry()[dag_id];
        let mut entries = dag.consts().iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        for entry in entries {
            cancellation.checkpoint()?;
            let key = dag.require_bound_decl_identity(
                &entry.name,
                src,
                DiagnosticAnchor::Source(entry.span),
            )?;
            index_map.insert(key.clone(), graph.add_node(key.clone()));
            declaration_by_key.insert(key, (dag_id.clone(), entry.name.clone(), entry.span));
        }
    }
    for dag_id in dag_ids {
        let dag = &tir.dag_registry()[dag_id];
        for (dependent, dependencies) in &dag.semantic().dependencies.const_deps {
            let Some(&dependent_idx) = index_map.get(dependent) else {
                continue;
            };
            for dependency in dependencies {
                if let Some(&dependency_idx) = index_map.get(dependency) {
                    graph.add_edge(dependency_idx, dependent_idx, ());
                }
            }
        }
    }
    let order = toposort(&graph, None).map_err(|cycle| {
        let key = &graph[cycle.node_id()];
        let (_, name, span) = &declaration_by_key[key];
        GraphcalError::CyclicDependency {
            name: name.to_string(),
            src: src.clone(),
            span: (*span).into(),
        }
    })?;

    let builtin_fns = builtin_functions();
    let empty_hir_locals = HirLocalValueMap::root();
    let mut const_pools = dag_ids
        .iter()
        .cloned()
        .map(|dag_id| (dag_id, RuntimeValueMap::new()))
        .collect::<HashMap<_, _>>();
    for index in order {
        cancellation.checkpoint()?;
        let key = &graph[index];
        let (dag_id, name, _) = &declaration_by_key[key];
        let dag = &tir.dag_registry()[dag_id];
        let local_values = &const_pools[dag_id];
        let values = visible_values_with_imports(dag, local_values, &visible_values);
        let ctx = EvalContext {
            cancellation: cancellation.clone(),
            work_budget: crate::eval_expr::fresh_work_budget(),
            builtin_fns,
            registry: tir.registry(),
            src,
            tir,
            current_dag: dag,
            current_decl: Some(key.clone()),
            root_values: Some(&values),
            root_presentation_instances: None,
            checked_execution_facts: None,
            presentation_calls: None,
            struct_field_constraints: None,
            generic_nat_bindings: None,
            host_fns: None,
        };
        let hir_expr = dag.const_expr(key).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("constant schedule references missing declaration `{name}`"),
                src,
                DiagnosticAnchor::Source(declaration_by_key[key].2),
            )
        })?;
        if let Some((target, call_span)) = graphcal_compiler::hir::find_dag_call(hir_expr) {
            return Err(GraphcalError::DagCallInCompileTime {
                name: target.to_string(),
                src: src.clone(),
                span: call_span.into(),
            });
        }
        let value = eval_hir_expr(hir_expr, &values, &empty_hir_locals, &ctx)?;
        let runtime_key = RuntimeDeclKey::resolved(key.clone());
        visible_values.insert(runtime_key.clone(), value.clone());
        let pool = const_pools.get_mut(dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("checked DAG `{dag_id}` has no initialized const pool"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        pool.insert(runtime_key, value);
    }
    Ok(const_pools)
}

#[expect(
    dead_code,
    reason = "legacy root-only test helper retained for focused plan tests"
)]
pub fn eval_consts_from_tir_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeValueMap, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_fns = builtin_functions();
    let dag = tir.root();

    if dag.consts().is_empty() {
        return Ok(HashMap::new());
    }

    let sorted_names = const_eval_order(dag, src, cancellation)?;

    let empty_hir_locals = HirLocalValueMap::root();
    let mut visible_values = visible_values_with_imports(dag, &HashMap::new(), &HashMap::new());
    let mut local_const_values: RuntimeValueMap = HashMap::new();

    for name in sorted_names {
        cancellation.checkpoint()?;
        let identity = dag.require_bound_decl_identity(&name, src, DiagnosticAnchor::WholeFile)?;
        let key = RuntimeDeclKey::resolved(identity);
        let ctx = EvalContext {
            cancellation: cancellation.clone(),
            work_budget: crate::eval_expr::fresh_work_budget(),
            builtin_fns,
            registry: tir.registry(),
            src,
            tir,
            current_dag: tir.root(),
            current_decl: Some(key.as_resolved().clone()),
            root_values: Some(&visible_values),
            root_presentation_instances: None,
            checked_execution_facts: None,
            presentation_calls: None,
            struct_field_constraints: None,
            generic_nat_bindings: None,
            host_fns: None,
        };
        let hir_expr = dag.const_expr(key.as_resolved()).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("constant schedule references missing declaration `{name}`"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        // Defense in depth for TIR values constructed or mutated outside the
        // compiler pipeline. Valid TIR rejects runtime DAG instantiation in a
        // const body during policy checking.
        if let Some((target, call_span)) = graphcal_compiler::hir::find_dag_call(hir_expr) {
            return Err(GraphcalError::DagCallInCompileTime {
                name: target.to_string(),
                src: src.clone(),
                span: call_span.into(),
            });
        }
        let value = eval_hir_expr(hir_expr, &visible_values, &empty_hir_locals, &ctx)?;
        visible_values.insert(key.clone(), value.clone());
        local_const_values.insert(key, value);
    }

    Ok(local_const_values)
}

#[expect(dead_code, reason = "used only by the root-only const helper")]
fn const_eval_order(
    dag: &DagTIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    const_eval_order_resolved(dag, &dag.semantic().dependencies, src, cancellation)
}

#[expect(dead_code, reason = "used only by the root-only const helper")]
fn const_eval_order_resolved(
    dag: &DagTIR,
    deps: &ResolvedDagDependencies,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    cancellation.checkpoint()?;
    let mut graph = DiGraph::<ResolvedDeclKey, ()>::new();
    let mut index_map: HashMap<ResolvedDeclKey, petgraph::graph::NodeIndex> = HashMap::new();
    let mut local_name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
    let mut span_by_key: HashMap<ResolvedDeclKey, Span> = HashMap::new();

    // Sort consts by name for canonical tie-breaking among incomparable nodes.
    let mut sorted_consts: Vec<&_> = dag.consts().iter().collect();
    sorted_consts.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in &sorted_consts {
        cancellation.checkpoint()?;
        let key = dag.require_bound_decl_identity(
            &entry.name,
            src,
            DiagnosticAnchor::Source(entry.span),
        )?;
        let idx = graph.add_node(key.clone());
        index_map.insert(key.clone(), idx);
        local_name_by_key.insert(key.clone(), entry.name.clone());
        span_by_key.insert(key, entry.span);
    }

    for (name, dep_set) in &deps.const_deps {
        cancellation.checkpoint()?;
        let Some(&dependent_idx) = index_map.get(name) else {
            continue;
        };
        for dep in dep_set {
            if let Some(&dep_idx) = index_map.get(dep) {
                graph.add_edge(dep_idx, dependent_idx, ());
            }
        }
    }

    let sorted = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        match (
            span_by_key.get(cycle_node).copied(),
            local_name_by_key.get(cycle_node),
        ) {
            (Some(span), Some(name)) => GraphcalError::CyclicDependency {
                name: name.to_string(),
                src: src.clone(),
                span: span.into(),
            },
            _ => GraphcalError::internal_error(
                format!("constant cycle node `{cycle_node}` is missing declaration metadata"),
                src,
                DiagnosticAnchor::WholeFile,
            ),
        }
    })?;

    Ok(sorted
        .into_iter()
        .filter_map(|idx| local_name_by_key.get(&graph[idx]).cloned())
        .collect())
}

/// Build a topologically sorted runtime DAG from params and nodes in a TIR.
fn build_runtime_dag(
    dag: &DagTIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<RuntimeDeclKey>, GraphcalError> {
    // Merge params and nodes, then sort by name for canonical tie-breaking
    // among incomparable nodes in the topological sort.
    enum DeclRef<'a> {
        Param(&'a graphcal_compiler::ir::lower::ParamEntry),
        Node(&'a graphcal_compiler::ir::lower::NodeEntry),
    }

    impl DeclRef<'_> {
        const fn name(&self) -> &ScopedName {
            match self {
                Self::Param(e) => &e.name,
                Self::Node(e) => &e.name,
            }
        }

        const fn span(&self) -> Span {
            match self {
                Self::Param(e) => e.span,
                Self::Node(e) => e.span,
            }
        }
    }

    cancellation.checkpoint()?;
    let mut decl_spans: Vec<(ScopedName, Span)> = Vec::new();

    let mut all_decls: Vec<DeclRef<'_>> = dag
        .params()
        .iter()
        .map(DeclRef::Param)
        .chain(dag.nodes().iter().map(DeclRef::Node))
        .collect();
    all_decls.sort_by(|a, b| a.name().cmp(b.name()));

    for decl in &all_decls {
        cancellation.checkpoint()?;
        decl_spans.push((decl.name().clone(), decl.span()));
    }

    runtime_eval_order(dag, &decl_spans, src, cancellation)?
        .into_iter()
        .map(|name| {
            dag.require_bound_decl_identity(&name, src, DiagnosticAnchor::WholeFile)
                .map(RuntimeDeclKey::resolved)
        })
        .collect()
}

fn runtime_eval_order(
    dag: &DagTIR,
    decl_spans: &[(ScopedName, Span)],
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    runtime_eval_order_resolved(
        dag,
        decl_spans,
        &dag.semantic().dependencies,
        src,
        cancellation,
    )
}

fn runtime_eval_order_resolved(
    dag: &DagTIR,
    decl_spans: &[(ScopedName, Span)],
    deps: &ResolvedDagDependencies,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    cancellation.checkpoint()?;
    let mut graph = DiGraph::<ResolvedDeclKey, ()>::new();
    let mut index_map: HashMap<ResolvedDeclKey, petgraph::graph::NodeIndex> = HashMap::new();
    let mut local_name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
    let mut span_by_key: HashMap<ResolvedDeclKey, Span> = HashMap::new();

    for (name, span) in decl_spans {
        cancellation.checkpoint()?;
        let key = dag.require_bound_decl_identity(name, src, DiagnosticAnchor::Source(*span))?;
        let idx = graph.add_node(key.clone());
        index_map.insert(key.clone(), idx);
        local_name_by_key.insert(key.clone(), name.clone());
        span_by_key.insert(key, *span);
    }

    for (name, dep_set) in &deps.runtime_deps {
        cancellation.checkpoint()?;
        let Some(&dependent_idx) = index_map.get(name) else {
            continue;
        };
        for dep in dep_set {
            if let Some(&dep_idx) = index_map.get(dep) {
                graph.add_edge(dep_idx, dependent_idx, ());
            }
        }
    }

    let topo_indices = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        match (
            span_by_key.get(cycle_node).copied(),
            local_name_by_key.get(cycle_node),
        ) {
            (Some(span), Some(name)) => GraphcalError::CyclicDependency {
                name: name.to_string(),
                src: src.clone(),
                span: span.into(),
            },
            _ => GraphcalError::internal_error(
                format!("runtime cycle node `{cycle_node}` is missing declaration metadata"),
                src,
                DiagnosticAnchor::WholeFile,
            ),
        }
    })?;

    Ok(topo_indices
        .into_iter()
        .filter_map(|idx| local_name_by_key.get(&graph[idx]).cloned())
        .collect())
}

/// Resolve domain constraints from type annotations on consts, params, and nodes.
///
/// Evaluates each compile-time bound expression using const values and builtins,
/// selects the target's quantity, integer, or datetime representation, and
/// checks `min <= max`. Bound types and target compatibility are validated
/// earlier in `dim_check::check_dimensions_tir`.
///
/// For const declarations, the resolved constraint is also checked against the
/// already-evaluated const value at compile time, raising `DomainViolation`
/// if the value is out of bounds.
#[expect(dead_code, reason = "root-only compatibility helper for focused tests")]
pub fn resolve_domain_constraints_with_cancellation(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>, GraphcalError> {
    resolve_domain_constraints_for_dag(
        tir,
        tir.root(),
        const_values,
        const_values,
        src,
        cancellation,
    )
}

fn resolve_domain_constraints_for_dag(
    tir: &TIR,
    dag: &DagTIR,
    const_values: &RuntimeValueMap,
    all_const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_fns = builtin_functions();
    let visible_const_values = visible_values_with_imports(dag, const_values, all_const_values);

    let ctx = EvalContext {
        cancellation: cancellation.clone(),
        work_budget: crate::eval_expr::fresh_work_budget(),
        builtin_fns,
        registry: tir.registry(),
        src,
        tir,
        current_dag: dag,
        current_decl: None,
        root_values: Some(&visible_const_values),
        root_presentation_instances: None,
        checked_execution_facts: None,
        presentation_calls: None,
        struct_field_constraints: None,
        generic_nat_bindings: None,
        host_fns: None,
    };
    let mut constraints = HashMap::new();
    let decl_iter = dag
        .consts()
        .iter()
        .map(|entry| (&entry.name, entry.span, true))
        .chain(
            dag.params()
                .iter()
                .map(|entry| (&entry.name, entry.span, false)),
        )
        .chain(
            dag.nodes()
                .iter()
                .map(|entry| (&entry.name, entry.span, false)),
        );

    for (name, decl_span, is_const) in decl_iter {
        cancellation.checkpoint()?;
        let resolved_key =
            dag.require_bound_decl_identity(name, src, DiagnosticAnchor::Source(decl_span))?;
        let Some(domain_bounds) = dag.semantic().domain_bounds.get(&resolved_key) else {
            continue;
        };
        let constraint_src = domain_bounds.first().map_or(src, |bound| &bound.src);
        let target = resolve_constraint_target(
            &name.to_string(),
            dag.resolved_decl_types().get(name).map(strip_indexed),
            decl_span,
            constraint_src,
        )?;
        let resolved_constraint = resolve_constraint_from_bounds(
            domain_bounds,
            &name.to_string(),
            target,
            &visible_const_values,
            &ctx.for_decl(&resolved_key).with_src(constraint_src),
            constraint_src,
        )?;
        let runtime_key = RuntimeDeclKey::resolved(resolved_key);
        if is_const
            && let Some(value) = const_values.get(&runtime_key)
            && let Err(violation) =
                crate::domain_check::check_domain_constraint(value, &resolved_constraint)
        {
            return Err(GraphcalError::DomainViolation {
                name: name.to_string(),
                value: format_runtime_value(value),
                violation: violation.message,
                src: src.clone(),
                span: decl_span.into(),
            });
        }
        constraints.insert(runtime_key, resolved_constraint);
    }
    Ok(constraints)
}

#[derive(Debug, Clone, Copy)]
enum ConstraintTarget {
    Quantity,
    Int,
    Datetime(graphcal_compiler::registry::time_scale::TimeScale),
}

/// Evaluate a declaration's or field's stored HIR domain bounds into the
/// representation selected by its constrained value family.
fn resolve_constraint_from_bounds(
    bounds: &[graphcal_compiler::tir::typed::ResolvedDomainBound],
    display_name: &str,
    target: ConstraintTarget,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDomainConstraint, GraphcalError> {
    match target {
        ConstraintTarget::Quantity => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            |value, bound| match value {
                RuntimeValue::Quantity(value) => Ok(*value),
                RuntimeValue::Int(value) => exact_domain_int_bound(*value, src, bound.value.span),
                other => Err(domain_bound_value_error(
                    display_name,
                    bound,
                    "a quantity",
                    other,
                    src,
                )),
            },
            |expr, value| format_quantity_bound_display(expr, *value),
        )
        .map(ResolvedDomainConstraint::quantity),
        ConstraintTarget::Int => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            |value, bound| match value {
                RuntimeValue::Int(value) => Ok(*value),
                other => Err(domain_bound_value_error(
                    display_name,
                    bound,
                    "Int",
                    other,
                    src,
                )),
            },
            |_expr, value| value.to_string(),
        )
        .map(ResolvedDomainConstraint::int),
        ConstraintTarget::Datetime(scale) => {
            let evaluated = evaluate_domain_bounds(
                bounds,
                display_name,
                values,
                ctx,
                src,
                |value, bound| match value {
                    RuntimeValue::Datetime(epoch) if epoch.time_scale == scale.to_hifitime() => {
                        Ok(*epoch)
                    }
                    other => Err(domain_bound_value_error(
                        display_name,
                        bound,
                        &format!("Datetime<{scale}>"),
                        other,
                        src,
                    )),
                },
                |_expr, epoch| epoch.to_string(),
            )?;
            ResolvedDomainConstraint::datetime(scale, evaluated).map_err(|error| {
                let anchor = bounds.first().map_or(DiagnosticAnchor::WholeFile, |bound| {
                    DiagnosticAnchor::Source(bound.span)
                });
                GraphcalError::internal_error(
                    format!(
                        "datetime domain bounds on `{display_name}` violated their checked scale invariant: {error}"
                    ),
                    src,
                    anchor,
                )
            })
        }
    }
}

fn evaluate_domain_bounds<T: PartialOrd>(
    bounds: &[graphcal_compiler::tir::typed::ResolvedDomainBound],
    display_name: &str,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    src: &NamedSource<Arc<String>>,
    convert: impl Fn(
        &RuntimeValue,
        &graphcal_compiler::tir::typed::ResolvedDomainBound,
    ) -> Result<T, GraphcalError>,
    format_display: impl Fn(&graphcal_compiler::hir::Expr, &T) -> String,
) -> Result<EvaluatedDomainBounds<T>, GraphcalError> {
    let Some(first) = bounds.first() else {
        return Err(GraphcalError::internal_error(
            format!("domain constraint on `{display_name}` has no bounds"),
            src,
            DiagnosticAnchor::WholeFile,
        ));
    };
    let empty_locals = HirLocalValueMap::root();
    let evaluated = bounds
        .iter()
        .map(|bound| {
            let runtime_value = eval_hir_expr(&bound.value, values, &empty_locals, ctx)?;
            let value = convert(&runtime_value, bound)?;
            let display = format_display(&bound.value, &value);
            Ok((bound.kind, EvaluatedDomainBound::new(value, display)))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let constraint_span = bounds
        .iter()
        .skip(1)
        .fold(first.span, |span, bound| span.merge(bound.span));
    let (min, max) =
        evaluated
            .into_iter()
            .fold((None, None), |(min, max), (kind, bound)| match kind {
                graphcal_compiler::syntax::ast::DomainBoundKind::Min => (Some(bound), max),
                graphcal_compiler::syntax::ast::DomainBoundKind::Max => (min, Some(bound)),
            });
    if let (Some(min), Some(max)) = (&min, &max)
        && min.value() > max.value()
    {
        return Err(GraphcalError::DomainMinExceedsMax {
            name: display_name.to_string(),
            min: min.display().to_string(),
            max: max.display().to_string(),
            src: src.clone(),
            span: constraint_span.into(),
        });
    }
    Ok(EvaluatedDomainBounds::new(min, max))
}

fn domain_bound_value_error(
    display_name: &str,
    bound: &graphcal_compiler::tir::typed::ResolvedDomainBound,
    expected: &str,
    actual: &RuntimeValue,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: format!(
            "{} domain bound on `{display_name}` must evaluate to {expected}, got {}",
            bound.kind,
            actual.kind()
        ),
        src: src.clone(),
        span: bound.value.span.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConcreteNominalApplication {
    identity: StructTypeRef,
    generic_args: Vec<DeclaredGenericArg>,
}

fn collect_concrete_nominal_applications(
    declared: &DeclaredType,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    applications: &mut HashSet<ConcreteNominalApplication>,
) -> Result<(), GraphcalError> {
    match declared {
        DeclaredType::Struct(identity, generic_args) => {
            for arg in generic_args {
                if let DeclaredGenericArg::Type(type_arg) = arg {
                    collect_concrete_nominal_applications(type_arg, tir, src, applications)?;
                }
            }
            let application = ConcreteNominalApplication {
                identity: identity.clone(),
                generic_args: generic_args.clone(),
            };
            if !applications.insert(application) {
                return Ok(());
            }
            let model_type = graphcal_compiler::tir::dim_check::ValidatedModelType::try_new(
                tir,
                identity,
                generic_args,
                src,
            )
            .map_err(|error| error.into_graphcal_error(src))?;
            for constructor in model_type.constructors(src)? {
                for field in constructor.fields() {
                    collect_concrete_nominal_applications(
                        field.declared_type(),
                        tir,
                        src,
                        applications,
                    )?;
                }
            }
            Ok(())
        }
        DeclaredType::Indexed { element, .. } => {
            collect_concrete_nominal_applications(element, tir, src, applications)
        }
        DeclaredType::Quantity(_)
        | DeclaredType::Complex(_)
        | DeclaredType::Bool
        | DeclaredType::Int
        | DeclaredType::Datetime(_)
        | DeclaredType::IndexArg(_)
        | DeclaredType::Key(_) => Ok(()),
    }
}

fn generic_nat_bindings(
    type_def: &graphcal_compiler::hir::NominalTypeDef,
    generic_args: &[DeclaredGenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<HashMap<GenericParamName, u64>, GraphcalError> {
    if type_def.generic_params().len() != generic_args.len() {
        return Err(GraphcalError::InternalError {
            message: format!(
                "concrete application of `{}` has {} generic arguments, expected {}",
                type_def.name(),
                generic_args.len(),
                type_def.generic_params().len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    type_def
        .generic_params()
        .iter()
        .zip(generic_args)
        .filter_map(|(param, arg)| match arg {
            DeclaredGenericArg::Nat(form) => Some((param, form)),
            DeclaredGenericArg::Dim(_)
            | DeclaredGenericArg::Index(_)
            | DeclaredGenericArg::Type(_) => None,
        })
        .map(|(param, form)| {
            form.constant_value()
                .map(|value| (param.name().clone(), value))
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "concrete Nat argument `{}` for `{}` remained symbolic",
                        form.format(),
                        param.name()
                    ),
                    src: src.clone(),
                    span: span.into(),
                })
        })
        .collect()
}

/// Resolve domain constraints declared on struct/union member fields.
///
/// Field bounds are stored atomically with their resolved target types in each
/// DAG's semantic type defs; this evaluates each constrained field's
/// typed `min`/`max` bounds, validates `min ≤ max`, and stores the result keyed
/// by the canonical owning struct type, constructor, and field name. Concrete
/// include instances already receive their own canonical owner during lowering;
/// no root-owned display alias is fabricated here.
///
/// Bound types and target compatibility are validated earlier by the compiler's
/// field-domain checks. This pass focuses on the runtime-relevant pieces: bound
/// evaluation, `min ≤ max`, and storage.
#[cfg(test)]
pub fn resolve_struct_field_constraints(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    resolve_struct_field_constraints_with_cancellation(
        tir,
        const_values,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

struct FieldConstraintResolutionContext<'a> {
    tir: &'a TIR,
    dag_facts: &'a HashMap<graphcal_compiler::dag_id::DagId, CheckedDagExecutionFacts>,
    all_const_values: &'a RuntimeValueMap,
    builtin_fns: &'a graphcal_compiler::registry::builtins::BuiltinFunctions,
    fallback_src: &'a NamedSource<Arc<String>>,
    cancellation: &'a graphcal_compiler::cancellation::CancellationToken,
}

type DagFieldConstraints = HashMap<
    graphcal_compiler::dag_id::DagId,
    HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
>;
type ApplicationFieldConstraints = (
    graphcal_compiler::dag_id::DagId,
    Vec<(StructFieldConstraintKey, ResolvedDomainConstraint)>,
);

fn application_field_constraint_key(
    application: &ConcreteNominalApplication,
    key: &graphcal_compiler::tir::typed::ResolvedStructFieldTypeKey,
) -> StructFieldConstraintKey {
    StructFieldConstraintKey::for_application(
        StructTypeRef::from_resolved(key.owning_type.clone()),
        application.generic_args.clone(),
        key.constructor.clone(),
        key.field.clone(),
    )
}

fn resolve_application_field_constraints(
    application: &ConcreteNominalApplication,
    ctx: &FieldConstraintResolutionContext<'_>,
) -> Result<ApplicationFieldConstraints, GraphcalError> {
    ctx.cancellation.checkpoint()?;
    let dag_id = application.identity.resolved().owner();
    let dag = ctx.tir.dag_registry().get(dag_id).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("type owner `{dag_id}` has no checked DAG"),
            ctx.fallback_src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let type_def = dag
        .semantic()
        .type_defs
        .struct_types
        .get(application.identity.resolved())
        .ok_or_else(|| {
            GraphcalError::internal_error(
                format!(
                    "semantic type metadata missing concrete application `{}`",
                    application.identity
                ),
                ctx.fallback_src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let facts = ctx.dag_facts.get(dag_id).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("type owner `{dag_id}` has no checked execution facts"),
            ctx.fallback_src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let owner_src = facts.source();
    let visible_const_values =
        visible_values_with_imports(dag, &facts.const_values, ctx.all_const_values);
    let nat_bindings = generic_nat_bindings(
        type_def,
        &application.generic_args,
        type_def.source(),
        type_def.span(),
    )?;
    let application_ctx = EvalContext {
        cancellation: ctx.cancellation.clone(),
        work_budget: crate::eval_expr::fresh_work_budget(),
        builtin_fns: ctx.builtin_fns,
        registry: ctx.tir.registry(),
        src: owner_src,
        tir: ctx.tir,
        current_dag: dag,
        current_decl: None,
        root_values: Some(&visible_const_values),
        root_presentation_instances: None,
        checked_execution_facts: None,
        presentation_calls: None,
        struct_field_constraints: None,
        generic_nat_bindings: Some(&nat_bindings),
        host_fns: None,
    };
    let mut constraints = Vec::new();
    for (key, field_semantics) in dag
        .semantic()
        .type_defs
        .constrained_fields()
        .filter(|(key, _)| key.owning_type == *application.identity.resolved())
    {
        let display_name = format!("{}.{}", key.constructor, key.field);
        let bounds = field_semantics.domain_bounds();
        let Some(first_bound) = bounds.first() else {
            return Err(GraphcalError::internal_error(
                format!("constrained field `{display_name}` has no domain bounds"),
                type_def.source(),
                DiagnosticAnchor::Source(type_def.span()),
            ));
        };
        let bound_span = first_bound.span;
        let constraint_src = &first_bound.src;
        let target = resolve_constraint_target(
            &display_name,
            Some(strip_indexed(field_semantics.resolved_type())),
            bound_span,
            constraint_src,
        )?;
        let constraint = resolve_constraint_from_bounds(
            bounds,
            &display_name,
            target,
            &visible_const_values,
            &application_ctx.with_src(constraint_src),
            constraint_src,
        )?;
        constraints.push((
            application_field_constraint_key(application, key),
            constraint,
        ));
    }
    Ok((dag_id.clone(), constraints))
}

fn collect_field_constraint_applications(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<ConcreteNominalApplication>, GraphcalError> {
    let mut applications = HashSet::new();
    for declared in tir.build_declared_types(src)?.values() {
        collect_concrete_nominal_applications(declared, tir, src, &mut applications)?;
    }
    for dag in tir.dag_registry().values() {
        for application in
            graphcal_compiler::tir::dim_check::concrete_constructor_applications(tir, dag, src)?
        {
            applications.insert(ConcreteNominalApplication {
                identity: application.identity().clone(),
                generic_args: application.generic_args().to_vec(),
            });
        }
        for (identity, type_def) in &dag.semantic().type_defs.struct_types {
            if type_def.generic_params().is_empty() {
                applications.insert(ConcreteNominalApplication {
                    identity: StructTypeRef::from_resolved(identity.clone()),
                    generic_args: Vec::new(),
                });
            }
        }
    }
    Ok(applications)
}

fn resolve_struct_field_constraints_for_dags(
    tir: &TIR,
    dag_facts: &HashMap<graphcal_compiler::dag_id::DagId, CheckedDagExecutionFacts>,
    all_const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<DagFieldConstraints, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_fns = builtin_functions();
    let context = FieldConstraintResolutionContext {
        tir,
        dag_facts,
        all_const_values,
        builtin_fns,
        fallback_src: src,
        cancellation,
    };
    let mut grouped = DagFieldConstraints::new();
    for application in collect_field_constraint_applications(tir, src)? {
        let (owner, entries) = resolve_application_field_constraints(&application, &context)?;
        grouped.entry(owner).or_default().extend(entries);
    }
    Ok(grouped)
}

#[cfg(test)]
pub fn resolve_struct_field_constraints_with_cancellation(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    let dag_facts = tir
        .dag_registry()
        .keys()
        .map(|dag_id| {
            let values = if dag_id == tir.root_dag_id() {
                const_values.clone()
            } else {
                RuntimeValueMap::new()
            };
            (
                dag_id.clone(),
                CheckedDagExecutionFacts {
                    dag_id: dag_id.clone(),
                    source: src.clone(),
                    const_values: Arc::new(values),
                    topo_order: Arc::new(Vec::new()),
                    domain_constraints: Arc::new(HashMap::new()),
                    struct_field_constraints: Arc::new(HashMap::new()),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let all_const_values = known_const_values(tir, &dag_facts);
    resolve_struct_field_constraints_for_dags(tir, &dag_facts, &all_const_values, src, cancellation)
        .map(|grouped| grouped.into_values().flatten().collect())
}

/// Walk every top-level const value and validate it against resolved
/// struct-field constraints. Used both inside [`compile`] and from the
/// check-only path in `project_compiler/lowering.rs` so that struct-field
/// violations on const nodes surface under `graphcal check`, not only
/// during full evaluation.
///
/// # Errors
///
/// Returns the first [`GraphcalError::DomainViolation`] encountered.
#[expect(dead_code, reason = "root-only compatibility helper for focused tests")]
pub fn check_const_struct_field_constraints_at_compile_time(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    check_dag_const_struct_field_constraints_at_compile_time(
        tir.root(),
        const_values,
        field_constraints,
        src,
    )
}

fn check_dag_const_struct_field_constraints_at_compile_time(
    dag: &DagTIR,
    const_values: &RuntimeValueMap,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for entry in dag.consts() {
        let key = RuntimeDeclKey::resolved(dag.require_bound_decl_identity(
            &entry.name,
            src,
            DiagnosticAnchor::Source(entry.span),
        )?);
        if let Some(value) = const_values.get(&key) {
            let owning_type = dag
                .resolved_decl_types()
                .get(&entry.name)
                .and_then(struct_type_ref_from_resolved_type);
            check_const_struct_field_constraints(
                value,
                entry.name.member().as_str(),
                entry.span,
                owning_type.as_ref(),
                field_constraints,
                src,
            )?;
        }
    }
    Ok(())
}

fn struct_type_ref_from_resolved_type(
    resolved: &graphcal_compiler::tir::typed::ResolvedTypeExpr,
) -> Option<StructTypeRef> {
    match strip_indexed(resolved) {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Struct(name, _)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericStruct { name, .. } => {
            Some(StructTypeRef::from_resolved(name.clone()))
        }
        _ => None,
    }
}

fn find_struct_field_constraint<'a>(
    field_constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    generic_args: &[graphcal_compiler::registry::declared_type::DeclaredGenericArg],
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type.and_then(|owning_type| {
        field_constraints.get(&StructFieldConstraintKey::for_application(
            owning_type.clone(),
            generic_args.to_vec(),
            constructor.clone(),
            field.clone(),
        ))
    })
}

/// Recursively validate a const value against resolved struct-field
/// constraints. For `RuntimeValue::Struct`, looks up each field's
/// owner-qualified struct/constructor/field constraint and emits
/// `DomainViolation` on the first violation. Indexed values recurse
/// element-wise; nested structs recurse field-wise. Other variants short-circuit
/// to `Ok(())`.
fn check_const_struct_field_constraints(
    value: &RuntimeValue,
    decl_name: &str,
    decl_span: Span,
    owning_type: Option<&StructTypeRef>,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match value {
        RuntimeValue::Struct {
            type_name,
            generic_args,
            fields,
        } => {
            let runtime_owning_type = StructTypeRef::from_resolved(type_name.resolved().clone());
            let effective_owning_type = owning_type.or(Some(&runtime_owning_type));
            let constructor = ConstructorName::from_atom(type_name.name().atom().clone());
            for (field_name, field_value) in fields {
                if let Some(constraint) = find_struct_field_constraint(
                    field_constraints,
                    effective_owning_type,
                    generic_args,
                    &constructor,
                    field_name,
                ) && let Err(violation) =
                    crate::domain_check::check_domain_constraint(field_value, constraint)
                {
                    return Err(GraphcalError::DomainViolation {
                        name: format!("{decl_name}.{field_name}"),
                        value: format_runtime_value(field_value),
                        violation: violation.message,
                        src: src.clone(),
                        span: decl_span.into(),
                    });
                }
                // Recurse for nested struct fields. The nested runtime value
                // carries its canonical owner when module-aware constructor
                // evaluation created it, so the recursive call can recover the
                // owner even without a field-declared type side channel.
                check_const_struct_field_constraints(
                    field_value,
                    &format!("{decl_name}.{field_name}"),
                    decl_span,
                    None,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        RuntimeValue::Indexed { entries, .. } => {
            for (variant, entry) in entries {
                check_const_struct_field_constraints(
                    entry,
                    &format!("{decl_name}.{variant}"),
                    decl_span,
                    owning_type,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Format a runtime value for inclusion in a `DomainViolation` error message.
fn format_runtime_value(rv: &RuntimeValue) -> String {
    match rv {
        RuntimeValue::Quantity(v) => graphcal_compiler::registry::format::format_number(*v),
        RuntimeValue::Int(i) => format!("{i}"),
        RuntimeValue::Datetime(epoch) => epoch.to_string(),
        RuntimeValue::Indexed { entries, .. } => {
            // Show the first violating entry's value if recoverable; otherwise summary.
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{k}: {}", format_runtime_value(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// Strip `Indexed` wrapper to get the base resolved type.
fn strip_indexed(
    resolved: &graphcal_compiler::tir::typed::ResolvedTypeExpr,
) -> &graphcal_compiler::tir::typed::ResolvedTypeExpr {
    match resolved {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Indexed { base, .. } => {
            strip_indexed(base)
        }
        other => other,
    }
}

/// Resolve the typed constraint family selected by a declaration or field type.
fn resolve_constraint_target(
    name: &str,
    base_resolved: Option<&graphcal_compiler::tir::typed::ResolvedTypeExpr>,
    decl_span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<ConstraintTarget, GraphcalError> {
    use graphcal_compiler::tir::typed::ResolvedTypeExpr;

    let Some(resolved) = base_resolved else {
        return Err(GraphcalError::InternalError {
            message: format!("domain constraint target `{name}` has no resolved type"),
            src: src.clone(),
            span: decl_span.into(),
        });
    };
    match resolved {
        ResolvedTypeExpr::Quantity(_)
        | ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => Ok(ConstraintTarget::Quantity),
        ResolvedTypeExpr::Int => Ok(ConstraintTarget::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(ConstraintTarget::Datetime(*scale)),
        ResolvedTypeExpr::Bool => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Bool".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Complex { .. } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Complex".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Key { .. } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Key".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::IndexArg(index) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("index {}", index.format_for_diagnostic()),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Struct(struct_name, _)
        | ResolvedTypeExpr::GenericStruct {
            name: struct_name, ..
        } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("struct `{}`", struct_name.as_str()),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::GenericTypeParam(param, _) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("generic Type parameter `{param}`"),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Indexed { .. } => Err(GraphcalError::InternalError {
            message: format!("domain constraint target `{name}` was not reduced to its base type"),
            src: src.clone(),
            span: decl_span.into(),
        }),
    }
}

/// Format a bound expression for display (e.g., `"100 kg"`, `"0.01 N"`).
///
/// For simple expressions (numbers, quantity literals, unary negation), the
/// original syntactic form is preserved. For complex expressions, the
/// pre-evaluated SI value is displayed as a fallback — no re-evaluation needed.
fn exact_domain_int_bound(
    value: i64,
    src: &NamedSource<Arc<String>>,
    span: graphcal_compiler::syntax::span::Span,
) -> Result<f64, GraphcalError> {
    crate::eval_expr::numeric::exact_i64_to_f64(value).map_err(|_| GraphcalError::EvalError {
        message: format!("domain bound integer {value} is too large for exact quantity comparison"),
        src: src.clone(),
        span: span.into(),
    })
}

fn format_quantity_bound_display(expr: &graphcal_compiler::hir::Expr, si_value: f64) -> String {
    use graphcal_compiler::hir::ExprKind;
    match &expr.kind {
        ExprKind::Number(n) => graphcal_compiler::registry::format::format_number(*n),
        ExprKind::Integer(n) => format!("{n}"),
        ExprKind::QuantityLiteral { value, unit } => {
            let unit_str = graphcal_compiler::registry::format::format_unit_terms_with_config(
                unit.terms
                    .iter()
                    .map(|item| (item.op, item.name.value.to_string(), item.power)),
                true,
            );
            let val_str = graphcal_compiler::registry::format::format_number(*value);
            format!("{val_str} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_quantity_bound_display(operand, -si_value))
        }
        // Fallback: display the already-evaluated SI value.
        _ => graphcal_compiler::registry::format::format_number(si_value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
    use graphcal_compiler::syntax::module_resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::{ProjectTypeStore, type_resolve_with_modules};

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let (tir, src) = tir_from_source(source);
        compile(&tir, &src)
    }

    fn tir_from_source(
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
        let tir = type_resolve_with_modules(ir, &src, &resolver, &project_types).unwrap();
        (tir, src)
    }

    fn quantity(rv: &RuntimeValue) -> f64 {
        match rv {
            RuntimeValue::Quantity(v) => *v,
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
    fn compile_simple_const() {
        let plan = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        assert!((quantity(&plan.const_values[&resolved_key("g0")]) - 9.80665).abs() < f64::EPSILON);
        assert!(plan.topo_order.is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let plan = compile_source(
            "const node g0: Dimensionless = 9.80665;\nconst node two_g0: Dimensionless = 2.0 * @g0;",
        )
        .unwrap();
        assert!((quantity(&plan.const_values[&resolved_key("two_g0")]) - 19.6133).abs() < 1e-10);
    }

    #[test]
    fn checked_fact_stores_are_reused_by_runtime_planning() {
        let (tir, src) = tir_from_source(
            "const node lower: Dimensionless = 1.0;\n\
             param x: Dimensionless(min: @lower, max: 3.0) = 2.0;",
        );
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let facts = check_execution_facts_with_cancellation(&tir, &src, &cancellation).unwrap();
        let plan = compile_checked_with_cancellation(&tir, &facts, &src, &cancellation).unwrap();
        let root_facts = facts.for_dag(tir.root_dag_id()).unwrap();

        assert!(Arc::ptr_eq(&root_facts.const_values, &plan.const_values));
        assert!(Arc::ptr_eq(
            &root_facts.domain_constraints,
            &plan.domain_constraints
        ));
        assert!(Arc::ptr_eq(
            &facts.struct_field_constraints,
            &plan.struct_field_constraints
        ));
    }

    #[test]
    fn compile_runtime_dag() {
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "x")
            .unwrap();
        let y_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "y")
            .unwrap();
        let z_pos = plan
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
                &plan.const_values[&RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("b")
                ))]
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
