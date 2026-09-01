//! Constant evaluation and runtime declaration scheduling.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{DagTIR, ResolvedDagDependencies, TIR};

use crate::decl_key::RuntimeDeclKey;
use crate::eval_expr::{EvalContext, HirLocalValueMap, eval_hir_expr};
use crate::execution_facts::RuntimeValueMap;

use super::ResolvedDeclKey;

pub(super) fn eval_const_pools_for_dags(
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
        let ctx = EvalContext {
            cancellation: cancellation.clone(),
            work_budget: crate::eval_expr::fresh_work_budget(),
            builtin_fns,
            registry: tir.registry(),
            src,
            tir,
            current_dag: dag,
            current_decl: Some(key.clone()),
            root_values: Some(&visible_values),
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
        let value = eval_hir_expr(hir_expr, &visible_values, &empty_hir_locals, &ctx)?;
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

/// Build a topologically sorted runtime DAG from params and nodes in a TIR.
pub(super) fn build_runtime_dag(
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
