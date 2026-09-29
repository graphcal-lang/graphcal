//! Constant evaluation in the checker's constant schedule.

use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::CheckedTir;

use crate::checked_program::{EvaluatedTir, ExecutionFacts};
use crate::constant_pools::ConstPoolBuildError;
use crate::eval_expr::{EvalSession, eval_root_with_presentation};
use crate::presentation_evidence::PresentationInstanceMap;

/// Evaluate the constants of `tir` with the interpreter, together with the
/// compile-time presentation of every constant, inherited ones included.
pub(super) fn eval_const_pool(
    tir: CheckedTir,
    inherited: &ExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(EvaluatedTir, PresentationInstanceMap), GraphcalError> {
    cancellation.checkpoint()?;
    let mut presentations = inherited
        .const_presentations()
        .map(|(key, evidence)| (key.clone(), evidence.clone()))
        .collect::<PresentationInstanceMap>();
    let evaluated = EvaluatedTir::evaluate(tir, inherited, |step| {
        cancellation.checkpoint()?;
        let session = EvalSession::provisional_constants(step.tir, src, cancellation.clone())
            .with_roots(step.visible, None)
            .for_decl(step.key);
        reject_constant_call(step.expression.get(), src)?;
        let (value, presentation) = eval_root_with_presentation(
            &session.executable(step.expression)?,
            step.visible,
            &presentations,
            &session,
        )?
        .into_parts();
        if !presentation.is_none() {
            presentations.insert(step.key.clone(), presentation);
        }
        Ok(value)
    })
    .map_err(|error| match error {
        ConstPoolBuildError::Evaluation(error) => error,
        ConstPoolBuildError::Invalid(error) => {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        }
    })?;
    Ok((evaluated, presentations))
}

fn reject_constant_call(
    expr: &graphcal_compiler::hir::expr::Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match graphcal_compiler::hir::find_dag_call(expr) {
        Some((target, span)) => Err(GraphcalError::DagCallInCompileTime {
            name: target.to_string(),
            src: src.clone(),
            span: span.into(),
        }),
        None => Ok(()),
    }
}
