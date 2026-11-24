//! Constant evaluation in the checker's constant schedule.

use graphcal_compiler::semantic_error::graph::GraphError;
use graphcal_compiler::source_registry::SourceRegistry;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::tir::typed::CheckedTir;

use crate::cells::{Cell, Collected};
use crate::checked_program::{EvaluatedTir, ExecutionFacts};
use crate::constant_pools::{ConstOutcome, ConstPoolBuildError, UnfinishedConstant};
use crate::deferred_field_checks::DeferredFieldChecks;
use crate::eval_expr::{EvalSession, eval_root_cells};
use crate::runtime_presentation::PendingPresentedMap;
use std::rc::Rc;

/// Evaluate the constants of `tir` with the interpreter, together with the
/// compile-time presentation of every constant, inherited ones included.
pub(super) fn eval_const_pool(
    tir: CheckedTir,
    inherited: &ExecutionFacts,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
    field_checks: &Rc<DeferredFieldChecks>,
) -> Result<(EvaluatedTir, PendingPresentedMap), Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let mut presentations = inherited
        .const_presentations()
        .map(|(key, evidence)| (key.clone(), evidence.clone()))
        .collect::<PendingPresentedMap>();
    let evaluated = EvaluatedTir::evaluate(tir, inherited, |step| {
        cancellation.checkpoint()?;
        let session = EvalSession::provisional_constants_with_checks(
            step.tir,
            src,
            sources,
            cancellation.clone(),
            Rc::clone(field_checks),
        )
        .for_decl(step.key)
        .with_unavailable(step.unfinished)
        .with_partial(step.partial);
        reject_constant_call(step.expression.get(), src)?;
        let cells = match eval_root_cells(
            &session.executable(step.expression)?,
            step.visible,
            &presentations,
            &session,
        ) {
            Ok(cells) => cells,
            Err(Outcome::Failed(error)) => Cell::of_error(error),
            Err(Outcome::Cancelled) => return Err(Outcome::Cancelled),
        };
        // Reaching a hole leaves the constant unfinished, with its cells when
        // some have values (#2405); any failure is a compile-time error.
        let presented = match cells.collect() {
            Collected::Complete(presented) => presented,
            Collected::Failed(error) => return Err(Outcome::Failed(error)),
            Collected::Unavailable(reason) => {
                return unfinished(step.key, &reason, src).map(ConstOutcome::Unfinished);
            }
            Collected::Partial(cells) => {
                if let Some(error) = cells.first_failure() {
                    return Err(Outcome::Failed(error.clone()));
                }
                return unfinished(step.key, &cells.reason(step.key), src)
                    .map(|unfinished| ConstOutcome::Partial { unfinished, cells });
            }
        };
        let value = presented.value().into_owned();
        if !presented.is_plain() {
            presentations.insert(step.key.clone(), presented);
        }
        Ok::<_, Outcome<SemanticError>>(ConstOutcome::Value(value))
    })
    .map_err(|error| match error {
        ConstPoolBuildError::Evaluation(error) => error,
        ConstPoolBuildError::Invalid(error) => {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
                .into()
        }
    })?;
    Ok((evaluated, presentations))
}

/// The constant `key` as unfinished for `reason`, which names no failure:
/// a failed read of a constant is a compile-time error before it.
fn unfinished(
    key: &graphcal_compiler::resolved_name::ResolvedDeclName,
    reason: &graphcal_compiler::node_unavailable::RuntimeUnavailable,
    src: SourceId,
) -> Result<UnfinishedConstant, Outcome<SemanticError>> {
    UnfinishedConstant::of(key, &reason.clone().of_declaration(key)).ok_or_else(|| {
        SemanticError::internal_error(
            format!("constant `{key}` has no value for a failure without an error"),
            src,
            DiagnosticAnchor::WholeFile,
        )
        .into()
    })
}

fn reject_constant_call(
    expr: &graphcal_compiler::hir::expr::Expr,
    src: SourceId,
) -> Result<(), SemanticError> {
    match graphcal_compiler::hir::expr::find_dag_call(expr) {
        Some((target, span)) => Err(SemanticError::located(
            src,
            span,
            GraphError::DagCallInCompileTime { name: target },
        )),
        None => Ok(()),
    }
}
