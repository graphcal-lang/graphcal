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
use crate::presentation_evidence::{
    PendingDisplayUnit, PendingQuantityDisplay, PresentationFailure, QuantityDisplay,
};
use crate::runtime_presentation::{EvaluatedRuntimeValue, ResolvedValue};

/// A display request for `unit`, whose terms are resolved now, in the scope
/// of the tree `ctx` evaluates, and whose scale is computed once the owning
/// frame is complete.
pub(super) fn pending(unit: &ResolvedUnitExpr, ctx: &EvalContext<'_>) -> PendingQuantityDisplay {
    PendingQuantityDisplay::Requested(Box::new(PendingDisplayUnit {
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
pub fn resolve(
    presented: EvaluatedRuntimeValue,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<ResolvedValue, GraphcalError> {
    presented.try_resolve(|display| match display {
        PendingQuantityDisplay::Ready(display) => Ok(display),
        PendingQuantityDisplay::Requested(request) => resolve_request(&request, values, ctx),
    })
}

/// Caller-owned requests pass through a child call unchanged. They are resolved
/// when that caller's frame is complete, not against a child's partial values.
pub fn resolve_frame(
    presented: EvaluatedRuntimeValue,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    callable: &crate::execution_plan::CallablePlan<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    presented.try_map_quantity_displays(|display| match display {
        PendingQuantityDisplay::Requested(request) if callable.executes(&request.owner) => {
            resolve_request(&request, values, ctx).map(PendingQuantityDisplay::Ready)
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
) -> Result<QuantityDisplay, GraphcalError> {
    ctx.cancellation.checkpoint()?;
    let context = ctx.with_src(&request.source);
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::PresentationEvaluation);
    match super::resolved_unit_scale(&request.unit, values, &context)
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
