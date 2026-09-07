//! Presentation computations executed while the selected invocation is alive.
//! This is not a selector evaluator: branches, locals and keys already selected
//! a value-shaped subtree in the ordinary expression kernel.

use graphcal_compiler::hir::expr::ResolvedUnitExpr;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::format::format_unit_terms_canonical;
use graphcal_compiler::registry::unit::PositiveFiniteScale;

use super::context::EvalContext;
use crate::execution_facts::RuntimeValueMap;
use crate::presentation_evidence::{PendingDisplayUnit, PresentationFailure, PresentationInstance};

pub(super) fn pending(unit: &ResolvedUnitExpr, ctx: &EvalContext<'_>) -> PresentationInstance {
    PresentationInstance::Pending(Box::new(PendingDisplayUnit {
        owner: ctx.current_dag.dag_id().clone(),
        source: ctx.src.clone(),
        unit: unit.clone(),
    }))
}

pub(super) fn scaled(
    unit: &ResolvedUnitExpr,
    scale: f64,
    ctx: &EvalContext<'_>,
) -> Result<PresentationInstance, GraphcalError> {
    let scale = PositiveFiniteScale::new(scale)
        .map_err(|error| ctx.internal_error(error.to_string(), unit.span))?;
    // The computational scale has already been validated. Label algebra is
    // display-only, including during immutable constant-evidence capture.
    Ok(
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
        },
    )
}

/// Future operation-budget accounting belongs here, shared with unit-scale work.
/// Resolving a subtree never stores the frame or any transient computational value.
pub fn resolve(
    evidence: PresentationInstance,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<PresentationInstance, GraphcalError> {
    resolve_selected(evidence, values, ctx, &|_| true)
}

/// Caller-owned requests pass through a child call unchanged. They are resolved
/// when that caller's frame is complete, not against a child's partial values.
pub fn resolve_frame(
    evidence: PresentationInstance,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    owners: &[graphcal_compiler::dag_id::DagId],
) -> Result<PresentationInstance, GraphcalError> {
    resolve_selected(evidence, values, ctx, &|owner| owners.contains(owner))
}

fn resolve_selected(
    evidence: PresentationInstance,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    owns: &dyn Fn(&graphcal_compiler::dag_id::DagId) -> bool,
) -> Result<PresentationInstance, GraphcalError> {
    match evidence {
        PresentationInstance::Pending(request) if !owns(&request.owner) => {
            Ok(PresentationInstance::Pending(request))
        }
        PresentationInstance::Pending(request) => {
            ctx.cancellation.checkpoint()?;
            let owner = ctx.tir.dag_registry().get(&request.owner).ok_or_else(|| {
                ctx.internal_error(
                    "presentation request has no checked owner",
                    request.unit.span,
                )
            })?;
            let context = ctx.for_dag(owner, &request.source)?;
            crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PresentationEvaluation);
            match super::unit_scale::resolve_unit_scale(&request.unit, values, &context)
                .and_then(|scale| scaled(&request.unit, scale, &context))
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
