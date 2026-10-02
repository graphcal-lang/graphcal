//! The assertion stage of runtime output assembly: every assertion the root
//! DAG reports, and the `#[assumes]` table keyed by the root's source names.

use std::collections::HashMap;

use graphcal_compiler::cancellation::Cancelled;
use graphcal_compiler::ir::instance::InstanceAssertionProjection;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::evaluation_unit::DeclarationBody;
use graphcal_compiler::tir::typed::model::TypedAssertEntry;
use graphcal_compiler::tir::typed::{AssertionOperands, BodyKind, Scoped};

use crate::assertion_eval::evaluate_assert_with_expected_fail;
use crate::eval::types::{AssertResult, RuntimeUnavailable};
use crate::eval_expr::{EvalSession, RuntimeValueMap, eval_root};

use super::dependency_failures::dependency_failure_message;
use super::root_names::{RootNames, root_source_names};
use crate::execution_plan::{ClosureDag, PlannedAssertion, PlannedInstance};

/// Evaluate every assertion reported for the root DAG: root assertions in
/// source order, then assertions projected from the semantic instances the
/// root's plan includes. Each result applies its `expected_fail` inversion.
///
/// A root assertion whose body references a failed declaration reports the
/// dependency failure (with its root cause) instead of evaluating over a
/// value map where the failed name is simply absent (#814).
pub(super) fn evaluate_assertions(
    plan: &crate::execution_plan::ExecPlan<'_>,
    src: SourceId,
    ctx: &EvalSession<'_>,
    values: &RuntimeValueMap,
    errors: &HashMap<ResolvedDeclName, RuntimeUnavailable>,
    names: &RootNames<'_>,
) -> Result<Vec<(ScopedName, AssertResult, Span)>, Outcome<SemanticError>> {
    let tir = plan.tir();
    let mut assertions: Vec<(ScopedName, AssertResult, Span)> = tir
        .declaration_bodies(plan.root().scope().position())
        .filter_map(|unit| match unit.kind() {
            BodyKind::Assert(entry) => Some((unit, entry)),
            _ => None,
        })
        .map(|(unit, entry)| {
            let owner = unit.identity().clone();
            let span = entry.get().span;
            let body = entry.map(|entry| &*entry.body);
            let entry_ctx = ctx.for_decl(&owner);
            let assert_result = match assert_dependency_failure(body, errors, names, &entry_ctx)? {
                Some(result) => result,
                None => {
                    evaluate_assert_with_expected_fail(body, unit.expected_fail(), &mut |expr| {
                        eval_root(&entry_ctx.executable(expr)?, values, &entry_ctx)
                    })?
                }
            };
            Ok((
                ScopedName::local(owner.leaf().clone()),
                assert_result.map_names(|declaration| names.name(declaration)),
                span,
            ))
        })
        .collect::<Result<_, Outcome<SemanticError>>>()?;
    for (parent, planned, projection, unit, entry) in projected_assertions(plan) {
        let owner = unit.identity();
        let assertion_ctx = ctx.with_src(src).for_decl(owner);
        let expected = projection
            .expected_fail
            .as_ref()
            .or_else(|| unit.expected_fail());
        let body = entry.map(|entry| &*entry.body);
        // A dependency failure names its declarations as the root does,
        // exactly as for the root's own assertions.
        let result = match assert_dependency_failure(body, errors, names, &assertion_ctx)? {
            Some(result) => result,
            None => evaluate_assert_with_expected_fail(body, expected, &mut |expr| {
                eval_root(&assertion_ctx.executable(expr)?, values, &assertion_ctx)
            })?,
        };
        assertions.push((
            projected_assertion_name(parent, planned, projection),
            result.map_names(|declaration| names.name(declaration)),
            entry.get().span,
        ));
    }
    Ok(assertions)
}

/// Every assertion an include site of the root's closure exposes, with the
/// closure DAG that includes the instance, in report order.
fn projected_assertions<'a, 'p>(
    plan: &'a crate::execution_plan::ExecPlan<'p>,
) -> impl Iterator<
    Item = (
        &'a ClosureDag<'p>,
        &'a PlannedInstance<'p>,
        &'p InstanceAssertionProjection,
        DeclarationBody<'p>,
        Scoped<'p, TypedAssertEntry>,
    ),
> {
    plan.root()
        .closure_instances()
        .iter()
        .flat_map(|(parent, instances)| {
            instances.iter().flat_map(move |planned| {
                planned.assertions().iter().map(
                    move |&PlannedAssertion {
                              projection,
                              body,
                              entry,
                          }| (parent, planned, projection, body, entry),
                )
            })
        })
}

/// The name the root reports an assertion `projection` of `planned`
/// exposes under: its exposed name, qualified by the scopes of `parent`, the
/// DAG that includes the instance, below the root.
fn projected_assertion_name(
    parent: &ClosureDag<'_>,
    planned: &PlannedInstance<'_>,
    projection: &InstanceAssertionProjection,
) -> ScopedName {
    parent.qualify(
        &planned
            .instance()
            .record()
            .instance
            .exposed_name(projection),
    )
}

/// The `#[assumes]` table of the root DAG and its execution closure, keyed
/// by the names the root reports the assertions under, each with the names
/// of its assumers.
///
/// An assumer is a declaration of the closure DAG whose table names it, so
/// it has the name the root exposes it under, else its name qualified by the
/// scopes of that DAG. An assertion the root does not report (one private to
/// a semantic instance) has no entry.
pub(super) fn root_assumes_map(
    plan: &crate::execution_plan::ExecPlan<'_>,
) -> HashMap<ScopedName, Vec<ScopedName>> {
    let tir = plan.tir();
    let reported = tir
        .declaration_bodies(plan.root().scope().position())
        .filter(|unit| matches!(unit.kind(), BodyKind::Assert(_)))
        .map(|unit| {
            (
                unit.identity().clone(),
                ScopedName::local(unit.identity().leaf().clone()),
            )
        })
        .chain(
            projected_assertions(plan).map(|(parent, planned, projection, unit, _)| {
                (
                    unit.identity().clone(),
                    projected_assertion_name(parent, planned, projection),
                )
            }),
        )
        .collect::<HashMap<_, _>>();
    let exposed = root_source_names(plan)
        .into_iter()
        .collect::<HashMap<_, _>>();
    let mut merged = HashMap::<ResolvedDeclName, Vec<ScopedName>>::new();
    for closure in plan.root().execution_dags() {
        for (assertion, assumers) in closure.scope().dag().assumes_map() {
            let names = merged.entry(assertion.clone()).or_default();
            for assumer in assumers {
                let name = exposed
                    .get(assumer)
                    .cloned()
                    .unwrap_or_else(|| closure.member(assumer.leaf()));
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    merged
        .into_iter()
        .filter_map(|(assertion, assumers)| {
            reported
                .get(&assertion)
                .map(|name| (name.clone(), assumers))
        })
        .collect()
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
    body: Scoped<'_, graphcal_compiler::hir::expr::AssertBody>,
    errors: &HashMap<ResolvedDeclName, RuntimeUnavailable>,
    names: &RootNames<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Option<AssertResult<ResolvedDeclName>>, Cancelled> {
    let body_exprs = match body.operands() {
        AssertionOperands::Condition(expr) => vec![expr],
        AssertionOperands::Tolerance {
            actual,
            expected,
            tolerance,
        } => vec![actual, expected, tolerance],
    };
    Ok(
        match ctx.unavailable_dependencies(body_exprs.iter().copied()) {
            Ok(Some(reason)) if reason.is_incomplete() => Some(AssertResult::Blocked { reason }),
            Err(Outcome::Cancelled) => return Err(Cancelled),
            Err(Outcome::Failed(error)) => Some(AssertResult::Error {
                message: error.to_string(),
            }),
            Ok(_) => dependency_failure_message(body_exprs, errors, names)
                .map(|message| AssertResult::Error { message }),
        },
    )
}
