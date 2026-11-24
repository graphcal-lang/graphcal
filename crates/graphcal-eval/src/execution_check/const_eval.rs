//! Constant evaluation in the checker's constant schedule.

use graphcal_compiler::semantic_error::graph::GraphError;
use graphcal_compiler::source_registry::SourceRegistry;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::tir::typed::CheckedTir;

use crate::checked_program::{EvaluatedTir, ExecutionFacts};
use crate::constant_pools::{ConstPoolBuildError, UnfinishedConstant};
use crate::deferred_field_checks::DeferredFieldChecks;
use crate::eval_expr::{EvalSession, eval_root_with_presentation};
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
        .with_unavailable(step.unfinished);
        reject_constant_call(step.expression.get(), src)?;
        let presented = match eval_root_with_presentation(
            &session.executable(step.expression)?,
            step.visible,
            &presentations,
            &session,
        ) {
            Ok(presented) => presented,
            // Reaching a hole leaves the constant unfinished; any other
            // failure is a compile-time error.
            Err(Outcome::Failed(error)) => {
                let reason = crate::execution_frame::declaration_unavailable(step.key, &error);
                return match UnfinishedConstant::of(step.key, &reason) {
                    Some(unfinished) if is_unavailable(&error) => Ok(Err(unfinished)),
                    _ => Err(Outcome::Failed(error)),
                };
            }
            Err(Outcome::Cancelled) => return Err(Outcome::Cancelled),
        };
        let value = presented.value().into_owned();
        if !presented.is_plain() {
            presentations.insert(step.key.clone(), presented);
        }
        Ok::<_, Outcome<SemanticError>>(Ok(value))
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

/// Whether `error` only says a value is unavailable.
const fn is_unavailable(error: &SemanticError) -> bool {
    matches!(
        error,
        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
            kind: graphcal_compiler::semantic_error::SemanticErrorKind::Evaluation(
                graphcal_compiler::semantic_error::evaluation::EvaluationError::Unavailable { .. }
            ),
            ..
        })
    )
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
