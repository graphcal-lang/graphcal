//! The assertion stage of runtime output assembly: every assertion the root
//! DAG reports, the source names of the root's runtime identities, and the
//! `#[assumes]` table keyed by those names.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{
    AssertionOperands, CheckedInstance, ResolvedProjection, Scoped,
};

use crate::assertion_eval::evaluate_assert_with_expected_fail;
use crate::eval::types::{AssertResult, NodeUnavailable};
use crate::eval_expr::{EvalSession, RuntimeValueMap, eval_root};

use super::{declaration_body, dependency_failure_message};

fn root_instance_name(
    root: &graphcal_compiler::dag_id::DagId,
    parent: &graphcal_compiler::dag_id::DagId,
    exposed: &ScopedName,
) -> ScopedName {
    // A parent outside the root's subtree contributes no qualifier.
    let parent_path = parent.scopes_below(root).into_iter().flatten();
    ScopedName::from_parts(
        graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(
            parent_path
                .chain(exposed.qualifier().iter().cloned())
                .collect(),
        )
        .ok(),
        exposed.leaf().clone(),
    )
}

/// One semantic-instance record paired with the checked DAG it materialized.
fn semantic_instance<'tir>(
    tir: &'tir graphcal_compiler::tir::typed::CheckedTir,
    record: &'tir graphcal_compiler::ir::instance::HirInstanceRecord,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedInstance<'tir>, GraphcalError> {
    tir.dag_registry().semantic_instance(record).ok_or_else(|| {
        GraphcalError::internal_error(
            format!(
                "semantic instance `{}` is absent from checked TIR",
                record.instance.id().owner()
            ),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

/// Evaluate every assertion reported for the root DAG: root assertions in
/// source order, then assertions projected from semantic instances. Each
/// result applies its `expected_fail` inversion.
///
/// A root assertion whose body references a failed declaration reports the
/// dependency failure (with its root cause) instead of evaluating over a
/// value map where the failed name is simply absent (#814).
pub(in crate::eval) fn evaluate_assertions(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
    ctx: &EvalSession<'_>,
    values: &RuntimeValueMap,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
) -> Result<Vec<(ScopedName, AssertResult, Span)>, GraphcalError> {
    let mut assertions: Vec<(ScopedName, AssertResult, Span)> = tir
        .root()
        .asserts()
        .map(|entry| {
            let owner = entry.identity();
            let unit = declaration_body(tir, &owner, src)?;
            let body = unit
                .assertion()
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("assertion `{owner}` has no checked body"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?
                .map(|entry| &*entry.body);
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
    let mut semantic_parents = tir
        .local_dags()
        .map(|(_, dag)| dag)
        .filter(|dag| dag.dag_id() == tir.root_dag_id() || dag.is_semantic_instance())
        .collect::<Vec<_>>();
    semantic_parents.sort_by(|left, right| left.dag_id().cmp(right.dag_id()));
    for parent_dag in semantic_parents {
        for record in parent_dag.semantic_instances() {
            for ResolvedProjection {
                target: owner,
                projection,
            } in semantic_instance(tir, record, src)?.assertion_projections()
            {
                let unit = declaration_body(tir, &owner, src)?;
                let entry = unit.assertion().ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("projected assertion `{owner}` is absent from semantic instance"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
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
                assertions.push((
                    root_instance_name(
                        tir.root_dag_id(),
                        parent_dag.dag_id(),
                        &record.instance.exposed_name(projection),
                    ),
                    result,
                    entry.get().span,
                ));
            }
        }
    }
    Ok(assertions)
}

/// Source-level names of the runtime declarations the root DAG exposes, in
/// deterministic order: root declarations in source order, then the output
/// and assertion projections of each root semantic instance in record order.
///
/// Declarations private to a semantic instance have no root source name and
/// are absent.
pub(in crate::eval) fn root_source_names(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<(ResolvedDeclName, ScopedName)>, GraphcalError> {
    let mut names = tir
        .root()
        .decls()
        .iter()
        .map(|entry| (entry.identity(), ScopedName::local(entry.name().clone())))
        .collect::<Vec<_>>();
    for record in tir.root().semantic_instances() {
        let instance = semantic_instance(tir, record, src)?;
        names.extend(
            instance
                .output_projections()
                .map(|resolved| {
                    let name = record.instance.exposed_name(resolved.projection);
                    (resolved.target, name)
                })
                .chain(instance.assertion_projections().map(|resolved| {
                    let name = record.instance.exposed_name(resolved.projection);
                    (resolved.target, name)
                })),
        );
    }
    Ok(names)
}

/// The `#[assumes]` table of the root DAG and its execution closure, keyed
/// by the root source names of the assertions and their assumers.
pub(super) fn root_assumes_map(
    plan: &crate::execution_plan::ExecPlan<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, Vec<ScopedName>>, GraphcalError> {
    let source_names_by_key = root_source_names(plan.tir(), src)?
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
