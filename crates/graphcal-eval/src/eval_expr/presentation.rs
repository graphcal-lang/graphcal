//! Presentation computations executed while the selected invocation is alive.
//! This is not a selector evaluator: branches, locals and keys already selected
//! a value-shaped subtree in the ordinary expression kernel.

use graphcal_compiler::hir::expr::{ResolvedUnitExpr, ResolvedUnitExprItem, ResolvedUnitRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::format::format_unit_terms_canonical;
use graphcal_compiler::registry::unit::PositiveFiniteScale;
use graphcal_compiler::syntax::span::Spanned;

use super::context::{EvalContext, EvalSession};
use crate::constant_pools::RuntimeValueMap;
use crate::presentation_evidence::{PendingDisplayUnit, PresentationFailure, PresentationInstance};

/// A display request for `unit`, whose terms are resolved now, in the scope
/// of the tree `ctx` evaluates, and whose scale is computed once the owning
/// frame is complete.
pub(super) fn pending(unit: &ResolvedUnitExpr, ctx: &EvalContext<'_>) -> PresentationInstance {
    PresentationInstance::Pending(Box::new(PendingDisplayUnit {
        owner: ctx.dag_id().clone(),
        source: ctx.src.clone(),
        unit: ResolvedUnitExpr {
            terms: unit
                .terms
                .iter()
                .map(|term| ResolvedUnitExprItem {
                    op: term.op,
                    name: Spanned::new(
                        ResolvedUnitRef::new(
                            term.name.value.spelling().clone(),
                            ctx.resolve_unit(&term.name.value),
                        ),
                        term.name.span,
                    ),
                    power: term.power,
                })
                .collect(),
            span: unit.span,
        },
    }))
}

pub(super) fn scaled<R: std::fmt::Display>(
    unit: &ResolvedUnitExpr<R>,
    scale: PositiveFiniteScale,
    ctx: &EvalSession<'_>,
) -> PresentationInstance {
    // Label algebra is display-only, including during immutable
    // constant-evidence capture.
    match format_unit_terms_canonical(
        unit.terms
            .iter()
            .map(|item| (item.op, item.name.value.to_string(), item.power)),
    ) {
        Ok(label) => PresentationInstance::Unit { label, scale },
        Err(error) => PresentationInstance::Failed(PresentationFailure::Formatting {
            source_name: ctx.src.name().to_owned(),
            error,
        }),
    }
}

/// Future operation-budget accounting belongs here, shared with unit-scale work.
/// Resolving a subtree never stores the frame or any transient computational value.
pub fn resolve(
    evidence: PresentationInstance,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<PresentationInstance, GraphcalError> {
    resolve_selected(evidence, values, ctx, &|_| true)
}

/// Caller-owned requests pass through a child call unchanged. They are resolved
/// when that caller's frame is complete, not against a child's partial values.
pub fn resolve_frame(
    evidence: PresentationInstance,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    callable: &crate::execution_plan::CallablePlan<'_>,
) -> Result<PresentationInstance, GraphcalError> {
    resolve_selected(evidence, values, ctx, &|owner| callable.executes(owner))
}

fn resolve_selected(
    evidence: PresentationInstance,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    owns: &dyn Fn(&graphcal_compiler::dag_id::DagId) -> bool,
) -> Result<PresentationInstance, GraphcalError> {
    match evidence {
        PresentationInstance::Pending(request) if !owns(&request.owner) => {
            Ok(PresentationInstance::Pending(request))
        }
        PresentationInstance::Pending(request) => {
            ctx.cancellation.checkpoint()?;
            let context = ctx.with_src(&request.source);
            crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PresentationEvaluation);
            match super::resolved_unit_scale(&request.unit, values, &context)
                .map(|scale| scaled(&request.unit, scale, &context))
            {
                Ok(evidence) => Ok(evidence),
                Err(
                    error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_)),
                ) => Err(error),
                Err(error) => Ok(PresentationInstance::Failed(PresentationFailure::Scale {
                    source_name: match &error {
                        GraphcalError::EvalError { src, .. } => src.name().to_owned(),
                        _ => context.src.name().to_owned(),
                    },
                    message: error.to_string(),
                })),
            }
        }
        PresentationInstance::Struct { fields } => fields
            .into_iter()
            .map(|(key, evidence)| {
                resolve_selected(evidence, values, ctx, owns).map(|evidence| (key, evidence))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(PresentationInstance::fields),
        PresentationInstance::Indexed { entries } => entries
            .into_iter()
            .map(|(key, evidence)| {
                resolve_selected(evidence, values, ctx, owns).map(|evidence| (key, evidence))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(PresentationInstance::entries),
        resolved => Ok(resolved),
    }
}
