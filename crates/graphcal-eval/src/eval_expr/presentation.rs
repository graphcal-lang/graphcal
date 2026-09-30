//! Presentation computations executed while the selected invocation is alive.
//!
//! This is not a selector evaluator: branches, locals and keys already selected
//! a value-shaped subtree in the ordinary expression kernel.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::display::unit_label::format_unit_terms_canonical;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::hir::expr::ResolvedUnitExpr;
use graphcal_compiler::semantic::unit_scale::PositiveFiniteScale;
use graphcal_compiler::tir::typed::scoped_node::ScopedUnitExpr;

use super::context::EvalSession;
use super::unit_scale::{EvaluateExecutable, resolved_unit_scale};
use crate::constant_pools::RuntimeValueMap;
use crate::presentation_evidence::{
    PendingDisplayUnit, PendingQuantityDisplay, PresentationFailure, QuantityDisplay,
};
use crate::runtime_presentation::{EvaluatedRuntimeValue, ResolvedValue};

/// A display request for `unit`, whose terms are resolved now, in the scope
/// of the tree naming it, which `owner` runs, and whose scale is computed
/// once the owning frame is complete.
pub(super) fn pending(
    unit: ScopedUnitExpr<'_>,
    owner: &DagId,
    ctx: &EvalSession<'_>,
) -> PendingQuantityDisplay {
    PendingQuantityDisplay::Requested(Box::new(PendingDisplayUnit {
        owner: owner.clone(),
        source: ctx.src.clone(),
        unit: unit.resolved(),
    }))
}

pub(super) fn scaled<R: std::fmt::Display>(
    unit: &ResolvedUnitExpr<R>,
    scale: PositiveFiniteScale,
    ctx: &EvalSession<'_>,
) -> QuantityDisplay {
    // Label algebra is display-only, including during immutable
    // constant-evidence capture.
    match format_unit_terms_canonical(
        unit.terms
            .iter()
            .map(|item| (item.op, item.name.value.to_string(), item.power)),
    ) {
        Ok(label) => QuantityDisplay::Unit { label, scale },
        Err(error) => QuantityDisplay::Failed(PresentationFailure::Formatting {
            source_name: ctx.src.name().to_owned(),
            error,
        }),
    }
}

/// Compute every pending display unit of `presented` against `values`,
/// the complete values of the root frame.
///
/// Future operation-budget accounting belongs here, shared with unit-scale work.
/// Resolving a subtree never stores the frame or any transient computational value.
pub(super) fn resolve(
    presented: EvaluatedRuntimeValue,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    evaluate: EvaluateExecutable,
) -> Result<ResolvedValue, GraphcalError> {
    presented.try_resolve(|display| match display {
        PendingQuantityDisplay::Ready(display) => Ok(display),
        PendingQuantityDisplay::Requested(request) => {
            resolve_request(&request, values, ctx, evaluate)
        }
    })
}

/// Caller-owned requests pass through a child call unchanged. They are resolved
/// when that caller's frame is complete, not against a child's partial values.
pub(super) fn resolve_frame(
    presented: EvaluatedRuntimeValue,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    callable: &crate::execution_plan::CallablePlan<'_>,
    evaluate: EvaluateExecutable,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    presented.try_map_quantity_displays(|display| match display {
        PendingQuantityDisplay::Requested(request) if callable.executes(&request.owner) => {
            resolve_request(&request, values, ctx, evaluate).map(PendingQuantityDisplay::Ready)
        }
        display @ (PendingQuantityDisplay::Ready(_) | PendingQuantityDisplay::Requested(_)) => {
            Ok(display)
        }
    })
}

/// Compute the scale of one requested display unit. An ordinary failure is
/// the leaf's display failure; invariants and cancellation abort.
fn resolve_request(
    request: &PendingDisplayUnit,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    evaluate: EvaluateExecutable,
) -> Result<QuantityDisplay, GraphcalError> {
    ctx.cancellation.checkpoint()?;
    let context = ctx.with_src(&request.source);
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PresentationEvaluation);
    match resolved_unit_scale(&request.unit, values, &context, evaluate)
        .map(|scale| scaled(&request.unit, scale, &context))
    {
        Ok(leaf) => Ok(leaf),
        Err(error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_))) => {
            Err(error)
        }
        Err(error) => Ok(QuantityDisplay::Failed(PresentationFailure::Scale {
            source_name: match &error {
                GraphcalError::EvalError { src, .. } => src.name().to_owned(),
                _ => context.src.name().to_owned(),
            },
            message: error.to_string(),
        })),
    }
}
