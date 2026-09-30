//! The assertion stage of runtime output assembly: every assertion the root
//! DAG reports, and the `#[assumes]` table keyed by the root's source names.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{
    AssertionOperands, DeclarationBody, ResolvedProjection, Scoped,
};

use crate::assertion_eval::evaluate_assert_with_expected_fail;
use crate::eval::types::{AssertResult, NodeUnavailable};
use crate::eval_expr::{EvalSession, RuntimeValueMap, eval_root};

use super::root_names::{qualified_below, root_source_names};
use super::{declaration_body, dependency_failure_message};

/// The checked body of the assertion `owner`, in the scope of its owner.
fn assertion_body<'tir>(
    tir: &'tir graphcal_compiler::tir::typed::CheckedTir,
    owner: &ResolvedDeclName,
    src: &NamedSource<Arc<String>>,
) -> Result<
    (
        DeclarationBody<'tir>,
        Scoped<'tir, graphcal_compiler::tir::typed::TypedAssertEntry>,
    ),
    GraphcalError,
> {
    let unit = declaration_body(tir, owner, src)?;
    let entry = unit.assertion().ok_or_else(|| {
        GraphcalError::internal_error(
            format!("assertion `{owner}` has no checked body"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    Ok((unit, entry))
}

/// Evaluate every assertion reported for the root DAG: root assertions in
/// source order, then assertions projected from the semantic instances the
/// root's plan includes. Each result applies its `expected_fail` inversion.
///
/// A root assertion whose body references a failed declaration reports the
/// dependency failure (with its root cause) instead of evaluating over a
/// value map where the failed name is simply absent (#814).
pub(in crate::eval) fn evaluate_assertions(
    plan: &crate::execution_plan::ExecPlan<'_>,
    src: &NamedSource<Arc<String>>,
    ctx: &EvalSession<'_>,
    values: &RuntimeValueMap,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
) -> Result<Vec<(ScopedName, AssertResult, Span)>, GraphcalError> {
    let tir = plan.tir();
    let mut assertions: Vec<(ScopedName, AssertResult, Span)> = tir
        .root()
        .asserts()
        .map(|entry| {
            let owner = entry.identity();
            let (unit, body) = assertion_body(tir, &owner, src)?;
            let body = body.map(|entry| &*entry.body);
            let entry_ctx = ctx.for_decl(&owner);
            let assert_result =
                assert_dependency_failure(body, errors, &entry_ctx).unwrap_or_else(|| {
                    evaluate_assert_with_expected_fail(body, unit.expected_fail(), &mut |expr| {
                        eval_root(&entry_ctx.executable(expr)?, values, &entry_ctx)
                    })
                });
            Ok((
                ScopedName::local(entry.name().clone()),
                assert_result,
                entry.span,
            ))
        })
        .collect::<Result<_, GraphcalError>>()?;
    for (parent, instances) in plan.root().closure_instances() {
        let parent_dag = parent.dag();
        for planned in instances {
            let instance = planned.instance();
            let record = instance.record();
            for ResolvedProjection {
                target: owner,
                projection,
            } in instance.assertion_projections()
            {
                let (unit, entry) = assertion_body(tir, &owner, src)?;
                let assertion_ctx = ctx.with_src(src).for_decl(&owner);
                let expected = projection
                    .expected_fail
                    .as_ref()
                    .or_else(|| unit.expected_fail());
                let result = evaluate_assert_with_expected_fail(
                    entry.map(|entry| &*entry.body),
                    expected,
                    &mut |expr| eval_root(&assertion_ctx.executable(expr)?, values, &assertion_ctx),
                );
                // A parent outside the root's subtree contributes no qualifier.
                let exposed = record.instance.exposed_name(projection);
                let name = qualified_below(tir.root_dag_id(), parent_dag.dag_id(), &exposed)
                    .unwrap_or(exposed);
                assertions.push((name, result, entry.get().span));
            }
        }
    }
    Ok(assertions)
}

/// The `#[assumes]` table of the root DAG and its execution closure, keyed
/// by the root source names of the assertions and their assumers.
pub(super) fn root_assumes_map(
    plan: &crate::execution_plan::ExecPlan<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, Vec<ScopedName>>, GraphcalError> {
    let source_names_by_key = root_source_names(plan)
        .into_iter()
        .collect::<HashMap<_, _>>();
    let source_name = |key: &ResolvedDeclName, role: &str| {
        source_names_by_key.get(key).cloned().ok_or_else(|| {
            GraphcalError::internal_error(
                format!("{role} `{key}` is missing from checked source order"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })
    };
    merge_assumes_maps(
        plan.root()
            .execution_dags()
            .iter()
            .map(|scope| scope.dag().assumes_map()),
    )
    .iter()
    .map(|(assertion, assumers)| {
        let assumer_names = assumers
            .iter()
            .map(|assumer| source_name(assumer, "assertion assumer"))
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        Ok((source_name(assertion, "assertion")?, assumer_names))
    })
    .collect()
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

/// If any declaration referenced by an assertion body failed to evaluate,
/// render the dependency-failure message the assertion should report (#814).
///
/// Mirrors the node path's `DependencyFailed` contract: a reference to a
/// failed declaration is not "undefined", it is unevaluable. Direct
/// evaluation failures carry their root cause inline; transitive failures
/// list only the dependency's name (its own failure is reported on that
/// declaration).
fn assert_dependency_failure(
    body: Scoped<'_, graphcal_compiler::hir::AssertBody>,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
    ctx: &EvalSession<'_>,
) -> Option<AssertResult> {
    let body_exprs = match body.operands() {
        AssertionOperands::Condition(expr) => vec![expr],
        AssertionOperands::Tolerance {
            actual,
            expected,
            tolerance,
        } => vec![actual, expected, tolerance],
    };
    match ctx.unavailable_dependencies(body_exprs.iter().copied()) {
        Ok(Some(reason)) if reason.is_incomplete() => Some(AssertResult::Blocked { reason }),
        Err(error) => Some(AssertResult::Error {
            message: error.to_string(),
        }),
        _ => dependency_failure_message(body_exprs, errors)
            .map(|message| AssertResult::Error { message }),
    }
}
