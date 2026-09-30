//! Compile-time execution checking: seals a fully checked TIR into a
//! [`CheckedProgram`].

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::tir::typed::CheckedTir;
#[cfg(test)]
use graphcal_compiler::tir::typed::StructFieldConstraintKey;

use crate::checked_program::{CheckedProgram, ExecutionFacts, ScheduledChecks};
use crate::constant_pools::RuntimeValueMap;
#[cfg(test)]
use crate::domain_constraint::ResolvedDomainConstraint;

mod const_eval;
mod domain_resolve;

use domain_resolve::{
    DagConstScope, check_dag_const_struct_field_constraints_at_compile_time,
    resolve_domain_constraints_for_dag, resolve_struct_field_constraints_for_dags,
};

#[cfg(test)]
pub(super) fn resolve_struct_field_constraints(
    tir: &CheckedTir,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    domain_resolve::resolve_struct_field_constraints(tir, const_values, src)
}

/// Evaluate the constants of every DAG `tir` schedules, derive their domain
/// and struct-field constraints, and seal `tir` with them. The DAGs of
/// checked modules keep the facts in `inherited`.
pub(super) fn seal_checked_program(
    tir: CheckedTir,
    inherited: &ExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProgram, GraphcalError> {
    cancellation.checkpoint()?;
    let internal =
        |message: String| GraphcalError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    // Newly evaluated constants and constraints are provisional until every
    // mandatory check has succeeded; sealing is the last step.
    let (evaluated, const_presentations) =
        const_eval::eval_const_pool(tir, inherited, src, cancellation)?;
    let (tir, consts) = (evaluated.tir(), evaluated.consts());
    let pool = |dag_id| {
        consts.for_dag(dag_id).ok_or_else(|| {
            internal(format!(
                "checked DAG `{dag_id}` has no initialized const pool"
            ))
        })
    };
    let all_const_values = consts
        .dags()
        .filter_map(|dag_id| consts.for_dag(dag_id))
        .flat_map(|values| values.iter())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<RuntimeValueMap>();
    let scheduled = tir.const_schedule().dags();
    let domain_constraints = scheduled
        .iter()
        .map(|dag_id| {
            cancellation.checkpoint()?;
            resolve_domain_constraints_for_dag(
                tir,
                &tir.dag_registry()[dag_id],
                pool(dag_id)?,
                &all_const_values,
                src,
                cancellation,
            )
            .map(|constraints| (dag_id.clone(), constraints))
        })
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;

    // Field-bound evaluation only needs provisional constant scopes, not fake
    // executable artifacts with missing constraints.
    let const_scopes = consts
        .dags()
        .map(|dag_id| {
            Ok((
                dag_id.clone(),
                DagConstScope {
                    values: pool(dag_id)?,
                    source: evaluated.inherited().source(dag_id).unwrap_or(src),
                },
            ))
        })
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;
    cancellation.checkpoint()?;
    let struct_field_constraints = resolve_struct_field_constraints_for_dags(
        tir,
        &const_scopes,
        &all_const_values,
        src,
        cancellation,
    )?
    .into_values()
    .flatten()
    .collect::<HashMap<_, _>>();
    let mut all_field_constraints = evaluated.inherited().struct_field_constraints().clone();
    all_field_constraints.extend(
        struct_field_constraints
            .iter()
            .map(|(key, constraint)| (key.clone(), constraint.clone())),
    );
    for dag_id in scheduled {
        check_dag_const_struct_field_constraints_at_compile_time(
            &tir.dag_registry()[dag_id],
            pool(dag_id)?,
            &all_field_constraints,
            src,
        )?;
    }

    evaluated
        .seal(ScheduledChecks {
            source: src.clone(),
            const_presentations,
            domain_constraints,
            struct_field_constraints,
        })
        .map_err(|error| internal(error.to_string()))
}
