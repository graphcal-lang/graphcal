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
    DagConstScope, check_dag_const_struct_field_constraints_at_compile_time,
    resolve_domain_constraints_for_dag, resolve_struct_field_constraints_for_dags,
};

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
pub(super) fn resolve_struct_field_constraints(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    domain_resolve::resolve_struct_field_constraints(tir, const_values, src)
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

fn initialized_const_pool<'a>(
    const_pools: &'a HashMap<graphcal_compiler::dag_id::DagId, RuntimeValueMap>,
    dag_id: &graphcal_compiler::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<&'a RuntimeValueMap, GraphcalError> {
    const_pools.get(dag_id).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("checked DAG `{dag_id}` has no initialized const pool"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

fn provisional_const_scopes<'a>(
    inherited: &'a CheckedExecutionFacts,
    const_pools: &'a HashMap<graphcal_compiler::dag_id::DagId, RuntimeValueMap>,
    src: &'a NamedSource<Arc<String>>,
) -> HashMap<graphcal_compiler::dag_id::DagId, DagConstScope<'a>> {
    inherited
        .by_dag
        .iter()
        .map(|(id, facts)| {
            (
                id.clone(),
                DagConstScope {
                    values: &facts.const_values,
                    source: facts.source(),
                },
            )
        })
        .chain(const_pools.iter().map(|(id, values)| {
            (
                id.clone(),
                DagConstScope {
                    values,
                    source: src,
                },
            )
        }))
        .collect()
}

fn check_dag_execution_facts(
    tir: &TIR,
    inherited: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedExecutionFacts, GraphcalError> {
    cancellation.checkpoint()?;
    // Inherited artifacts remain shared and immutable. Newly evaluated constants
    // and constraints are provisional until every mandatory check has succeeded.
    let mut dag_facts = inherited.by_dag.as_ref().clone();
    let dag_ids = tir
        .dag_registry()
        .keys()
        .filter(|dag_id| !dag_facts.contains_key(*dag_id))
        .cloned()
        .collect::<HashSet<_>>();
    let initial_values = known_const_values(tir, &dag_facts);
    let const_pools = eval_const_pools_for_dags(tir, &dag_ids, initial_values, src, cancellation)?;

    let mut all_const_values = known_const_values(tir, &dag_facts);
    all_const_values.extend(const_pools.values().flat_map(|values| {
        values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
    }));
    let mut schedules = dag_ids
        .iter()
        .map(|dag_id| {
            build_runtime_dag(&tir.dag_registry()[dag_id], src, cancellation)
                .map(|order| (dag_id.clone(), order))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut constraints = dag_ids
        .iter()
        .map(|dag_id| {
            cancellation.checkpoint()?;
            resolve_domain_constraints_for_dag(
                tir,
                &tir.dag_registry()[dag_id],
                initialized_const_pool(&const_pools, dag_id, src)?,
                &all_const_values,
                src,
                cancellation,
            )
            .map(|constraints| (dag_id.clone(), constraints))
        })
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;

    // Field-bound evaluation only needs provisional constant scopes, not fake
    // executable artifacts with missing constraints or schedules.
    let const_scopes = provisional_const_scopes(inherited, &const_pools, src);
    cancellation.checkpoint()?;
    let field_constraints = resolve_struct_field_constraints_for_dags(
        tir,
        &const_scopes,
        &all_const_values,
        src,
        cancellation,
    )?;
    let mut all_field_constraints = inherited.struct_field_constraints.as_ref().clone();
    all_field_constraints.extend(field_constraints.into_values().flatten());
    for dag_id in &dag_ids {
        check_dag_const_struct_field_constraints_at_compile_time(
            &tir.dag_registry()[dag_id],
            initialized_const_pool(&const_pools, dag_id, src)?,
            &all_field_constraints,
            src,
        )?;
    }

    // Publication is the last step. No checked artifact is subsequently filled
    // in with Arc::get_mut or exposed before constant field validation.
    let completed = const_pools
        .into_iter()
        .map(|(dag_id, const_values)| {
            let missing = || {
                GraphcalError::internal_error(
                    format!("DAG `{dag_id}` has incomplete execution checks"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            };
            let facts = CheckedDagExecutionFacts {
                dag_id: dag_id.clone(),
                source: src.clone(),
                const_values: Arc::new(const_values),
                topo_order: Arc::new(schedules.remove(&dag_id).ok_or_else(missing)?),
                domain_constraints: Arc::new(constraints.remove(&dag_id).ok_or_else(missing)?),
            };
            Ok((dag_id, Arc::new(facts)))
        })
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;
    dag_facts.extend(completed);
    Ok(freeze_checked_execution_facts(
        dag_facts,
        all_field_constraints,
    ))
}
