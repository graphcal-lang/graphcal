//! Compile-time execution checking: seals a fully checked TIR into a
//! [`CheckedProgram`].

use graphcal_compiler::source_registry::SourceRegistry;
use std::collections::HashMap;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::tir::typed::CheckedTir;
#[cfg(any(test, feature = "test-internals"))]
use graphcal_compiler::tir::typed::StructFieldConstraintKey;

use crate::checked_program::{CheckedProgram, ExecutionFacts, ScheduledChecks};
use crate::constant_pools::RuntimeValueMap;
#[cfg(any(test, feature = "test-internals"))]
use crate::domain_constraint::ResolvedDomainConstraint;

mod const_eval;
mod domain_resolve;

use domain_resolve::{
    ConstScopes, DagConstScope, check_dag_const_struct_field_constraints_at_compile_time,
    resolve_domain_constraints_for_dag, resolve_struct_field_constraints_for_dags,
};

/// Test-only resolution of the struct-field constraints of `tir`.
#[cfg(any(test, feature = "test-internals"))]
pub fn resolve_struct_field_constraints(
    tir: &CheckedTir,
    const_values: &RuntimeValueMap,
    src: SourceId,
    sources: &SourceRegistry,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, SemanticError> {
    domain_resolve::resolve_struct_field_constraints(tir, const_values, src, sources)
}

/// Test-only sealing of a single checked TIR without checked modules.
#[cfg(any(test, feature = "test-internals"))]
pub fn seal_checked_program_with_cancellation(
    tir: CheckedTir,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProgram, Outcome<SemanticError>> {
    seal_checked_program(tir, &ExecutionFacts::default(), src, sources, cancellation)
}

/// Evaluate the constants of every DAG `tir` schedules, derive their domain
/// and struct-field constraints, and seal `tir` with them. The DAGs of
/// checked modules keep the facts in `inherited`.
pub fn seal_checked_program(
    tir: CheckedTir,
    inherited: &ExecutionFacts,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProgram, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let internal =
        |message: String| SemanticError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    // Newly evaluated constants and constraints are provisional until every
    // mandatory check has succeeded; sealing is the last step.
    let (evaluated, const_presentations) =
        const_eval::eval_const_pool(tir, inherited, src, sources, cancellation)?;
    let (tir, consts) = (evaluated.tir(), evaluated.consts());
    let all_const_values = consts
        .dags()
        .filter_map(|dag_id| consts.for_dag(dag_id))
        .flat_map(|values| values.iter())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<RuntimeValueMap>();
    let scheduled = tir.const_schedule().dags();
    let domain_constraints = scheduled
        .iter()
        .map(|&position| {
            cancellation.checkpoint()?;
            let dag = tir.dag_registry().at(position);
            resolve_domain_constraints_for_dag(
                tir,
                dag,
                evaluated.pool_at(position),
                &all_const_values,
                src,
                sources,
                cancellation,
            )
            .map(|constraints| (dag.dag_id().clone(), constraints))
        })
        .collect::<Result<HashMap<_, _>, Outcome<SemanticError>>>()?;

    // Field-bound evaluation only needs provisional constant scopes, not fake
    // executable artifacts with missing constraints.
    let const_scopes = ConstScopes::new(tir, |position, dag| DagConstScope {
        values: evaluated.pool_at(position),
        source: evaluated.inherited().source(dag.dag_id()).unwrap_or(src),
    });
    cancellation.checkpoint()?;
    let struct_field_constraints = resolve_struct_field_constraints_for_dags(
        tir,
        &const_scopes,
        &all_const_values,
        src,
        sources,
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
    for &position in scheduled {
        check_dag_const_struct_field_constraints_at_compile_time(
            tir.dag_registry().at(position),
            evaluated.pool_at(position),
            &all_field_constraints,
            src,
        )?;
    }

    evaluated
        .seal(ScheduledChecks {
            source: src,
            const_presentations,
            domain_constraints,
            struct_field_constraints,
        })
        .map_err(|error| internal(error.to_string()))
        .map_err(Outcome::Failed)
}
