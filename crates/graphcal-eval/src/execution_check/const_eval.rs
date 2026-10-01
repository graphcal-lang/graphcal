//! Constant evaluation in the checker's constant schedule.

use graphcal_compiler::semantic_error::graph::GraphError;
use graphcal_compiler::source_registry::SourceRegistry;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::tir::typed::CheckedTir;

use crate::checked_program::{EvaluatedTir, ExecutionFacts};
use crate::constant_pools::ConstPoolBuildError;
use crate::eval_expr::{EvalSession, eval_root_with_presentation};
use crate::runtime_presentation::PendingPresentedMap;

/// Evaluate the constants of `tir` with the interpreter, together with the
/// compile-time presentation of every constant, inherited ones included.
pub(super) fn eval_const_pool(
    tir: CheckedTir,
    inherited: &ExecutionFacts,
    src: SourceId,
    sources: &SourceRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(EvaluatedTir, PendingPresentedMap), Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let mut presentations = inherited
        .const_presentations()
        .map(|(key, evidence)| (key.clone(), evidence.clone()))
        .collect::<PendingPresentedMap>();
    let evaluated = EvaluatedTir::evaluate(tir, inherited, |step| {
        cancellation.checkpoint()?;
        let session =
            EvalSession::provisional_constants(step.tir, src, sources, cancellation.clone())
                .for_decl(step.key);
        reject_constant_call(step.expression.get(), src)?;
        let presented = eval_root_with_presentation(
            &session.executable(step.expression)?,
            step.visible,
            &presentations,
            &session,
        )?;
        let value = presented.value().into_owned();
        if !presented.is_plain() {
            presentations.insert(step.key.clone(), presented);
        }
        Ok::<_, Outcome<SemanticError>>(value)
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

fn reject_constant_call(
    expr: &graphcal_compiler::hir::expr::Expr,
    src: SourceId,
) -> Result<(), SemanticError> {
    match graphcal_compiler::hir::expr::find_dag_call(expr) {
        Some((target, span)) => Err(SemanticError::located(
            src,
            span,
            GraphError::DagCallInCompileTime {
                name: target.to_string(),
            },
        )),
        None => Ok(()),
    }
}
