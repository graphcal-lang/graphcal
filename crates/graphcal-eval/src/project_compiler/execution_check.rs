//! Compile-time execution-fact checking for fully resolved TIR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::{DagTIR, StructFieldConstraintKey, TIR};

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::ResolvedDomainConstraint;
use crate::execution_facts::{CheckedDagExecutionFacts, CheckedExecutionFacts, RuntimeValueMap};

mod const_schedule;
mod domain_resolve;

use const_schedule::{build_runtime_dag, eval_const_pools_for_dags};
use domain_resolve::{
    check_dag_const_struct_field_constraints_at_compile_time,
    resolve_domain_constraints_for_dag, resolve_struct_field_constraints_for_dags,
};
#[cfg(test)]
pub(crate) use domain_resolve::resolve_struct_field_constraints;

type ResolvedDeclKey = graphcal_compiler::syntax::decl_name::ResolvedDeclName;

/// Check every DAG not already present in `inherited`, preserving dependency
/// facts and their defining sources while compiling an importing file.
pub(super) fn check_execution_facts_with_inherited(
    tir: &TIR,
    inherited: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedExecutionFacts, GraphcalError> {
    check_dag_execution_facts(tir, inherited, src, cancellation)
}

#[cfg(test)]
pub(crate) fn check_execution_facts_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedExecutionFacts, GraphcalError> {
    check_execution_facts_with_inherited(
        tir,
        &CheckedExecutionFacts::empty(),
        src,
        cancellation,
    )
}

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
    facts: &HashMap<graphcal_compiler::dag_id::DagId, Arc<CheckedDagExecutionFacts>>,
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
    dag_facts: HashMap<graphcal_compiler::dag_id::DagId, Arc<CheckedDagExecutionFacts>>,
    struct_field_constraints: HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
) -> CheckedExecutionFacts {
    CheckedExecutionFacts {
        by_dag: Arc::new(dag_facts),
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
    // Preserve inherited per-DAG facts by Arc. Only DAGs owned by the file
    // currently being checked are allocated and mutated below.
    let mut dag_facts = inherited.by_dag.as_ref().clone();
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
            Arc::new(CheckedDagExecutionFacts {
                dag_id: dag_id.clone(),
                source: src.clone(),
                topo_order: Arc::new(build_runtime_dag(dag, src, cancellation)?),
                domain_constraints: Arc::new(HashMap::new()),
                const_values: Arc::new(const_values),
                struct_field_constraints: Arc::new(HashMap::new()),
            }),
        );
    }

    let all_const_values = known_const_values(tir, &dag_facts);
    for dag_id in &dag_ids {
        cancellation.checkpoint()?;
        let dag = &tir.dag_registry()[dag_id];
        let facts = dag_facts
            .get_mut(dag_id)
            .and_then(Arc::get_mut)
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("newly checked DAG facts for `{dag_id}` were lost or shared early"),
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
    let mut all_field_constraints = inherited.struct_field_constraints.as_ref().clone();
    for (owner, constraints) in field_constraints {
        all_field_constraints.extend(
            constraints
                .iter()
                .map(|(key, constraint)| (key.clone(), constraint.clone())),
        );
        if dag_ids.contains(&owner) {
            let facts = dag_facts
                .get_mut(&owner)
                .and_then(Arc::get_mut)
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!(
                            "new field-constraint owner `{owner}` has no uniquely owned checked facts"
                        ),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            facts.struct_field_constraints = Arc::new(constraints);
        }
    }
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

