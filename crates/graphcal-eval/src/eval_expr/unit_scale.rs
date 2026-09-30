use crate::runtime_value::RuntimeValue;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::hir::expr::ResolvedUnitExpr;
use graphcal_compiler::hir::expr::{ResolvedUnitExprItem, ResolvedUnitRef};
use graphcal_compiler::resolved_name::ResolvedUnitName;
use graphcal_compiler::semantic::unit_scale::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitScale, UnitScaleStepError, UnitScaleTerm,
    try_fold_unit_scale,
};
use graphcal_compiler::syntax::dimension::UnitRef;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::TExpr;
use graphcal_compiler::tir::typed::evaluation_unit::ScopedTree;
use graphcal_compiler::tir::typed::scoped_node::ScopedUnitExpr;

use super::context::EvalSession;
use super::numeric;
use crate::constant_pools::RuntimeValueMap;

/// The expression kernel, evaluating one executable tree with no locals.
///
/// A dynamic unit's scale is an ordinary expression, so resolving a unit
/// scale re-enters the kernel. Callers hand the kernel in rather than this
/// module naming it, so the kernel stays the only module that recurses.
pub(super) type EvaluateExecutable = for<'a, 't, 's> fn(
    &'a ScopedTree<'t, &'t TExpr>,
    &'a RuntimeValueMap,
    &'a EvalSession<'s>,
) -> Result<RuntimeValue, GraphcalError>;

fn unit_scale_error(
    context: &str,
    error: PositiveFiniteScaleError,
    span: Span,
    session: &EvalSession<'_>,
) -> GraphcalError {
    session.eval_error(format!("{context} {error}"), span)
}

/// Apply a unit scale to a literal value and validate that the SI value is finite.
pub(super) fn checked_unit_scaled_value(
    value: f64,
    scale: PositiveFiniteScale,
    span: Span,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    numeric::finite_quantity(value * scale.get(), "quantity literal value")
        .map(RuntimeValue::Quantity)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// Evaluate the scale expression of a dynamic unit in the scope of the DAG
/// that defines the unit.
fn resolve_dynamic_unit_scale(
    unit: &ResolvedUnitName,
    spelling: &UnitRef,
    base_unit_scale: PositiveFiniteScale,
    span: Span,
    values: &RuntimeValueMap,
    session: &EvalSession<'_>,
    evaluate: EvaluateExecutable,
) -> Result<PositiveFiniteScale, GraphcalError> {
    let scale = session.tir.unit_scale_body(unit).ok_or_else(|| {
        session.internal_error(
            format!("dynamic unit scale for `{spelling}` could not be resolved"),
            span,
        )
    })?;
    let scale_session = session.with_src(scale.source());
    if scale.declared_dimension() != scale.base_unit_dimension() {
        return Err(scale_session.internal_error(
            format!(
                "dynamic unit `{}` has mismatched declared and base-unit dimensions",
                scale.spelling()
            ),
            scale.span(),
        ));
    }
    let expression = scale.expression();
    let scale_val = evaluate(
        &scale_session.executable(expression)?,
        values,
        &scale_session,
    )?;
    let RuntimeValue::Quantity(scale_f64) = scale_val else {
        return Err(scale_session.internal_error(
            "dynamic unit scale expression must evaluate to a quantity",
            expression.get().span,
        ));
    };
    let dynamic_scale = PositiveFiniteScale::new(scale_f64.get()).map_err(|error| {
        unit_scale_error(
            "dynamic unit scale",
            error,
            expression.get().span,
            &scale_session,
        )
    })?;
    dynamic_scale
        .checked_mul(base_unit_scale)
        .map_err(|error| unit_scale_error("dynamic unit scale", error, scale.span(), session))
}

/// Fold the scale of the unit expression spanning `span` whose terms name
/// their units by `R`, each paired with the unit whose scale applies.
fn fold_unit_scale<'u, R: 'u>(
    span: Span,
    terms: impl IntoIterator<Item = (&'u ResolvedUnitExprItem<R>, ResolvedUnitName)>,
    values: &RuntimeValueMap,
    session: &EvalSession<'_>,
    spelling: fn(&R) -> &UnitRef,
    evaluate: EvaluateExecutable,
) -> Result<PositiveFiniteScale, GraphcalError> {
    try_fold_unit_scale(
        terms,
        |(item, resolved_unit)| {
            let info = session.tir.unit_info(resolved_unit).ok_or_else(|| {
                session.internal_error(
                    format!("unknown checked unit `{}`", spelling(&item.name.value)),
                    item.name.span,
                )
            })?;
            let scale = match &info.scale {
                UnitScale::Const(scale) | UnitScale::Runtime(scale) => *scale,
                UnitScale::Dynamic { base_unit_scale } => resolve_dynamic_unit_scale(
                    resolved_unit,
                    spelling(&item.name.value),
                    *base_unit_scale,
                    item.name.span,
                    values,
                    session,
                    evaluate,
                )?,
            };
            Ok(UnitScaleTerm {
                op: item.op,
                scale,
                power: item.power,
            })
        },
        |(item, _), error| match error {
            UnitScaleStepError::Power(error) => {
                unit_scale_error("unit scale exponentiation", error, item.name.span, session)
            }
            UnitScaleStepError::Compound(error) => {
                unit_scale_error("compound unit scale", error, span, session)
            }
        },
    )
}

/// Resolve a `UnitExpr` of an evaluated tree, whose terms resolve in the
/// tree's scope, to its compound scale factor at runtime.
///
/// Static unit definitions come from the TIR's canonical project type store.
/// For dynamic units, the unit's strictly validated HIR scale expression, in
/// the scope of the DAG instance defining the unit, is evaluated against the
/// current `values`, then multiplied by the base unit's static scale. Dynamic
/// scale expressions are standalone (graph/const references only), so no
/// local environment is involved.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a unit is unknown or a dynamic scale expression
/// fails to evaluate to a quantity.
pub(super) fn resolve_unit_scale(
    unit: ScopedUnitExpr<'_>,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
    evaluate: EvaluateExecutable,
) -> Result<PositiveFiniteScale, GraphcalError> {
    fold_unit_scale(
        unit.get().span,
        unit.terms(),
        values,
        ctx,
        graphcal_compiler::hir::expr::LocalUnit::spelling,
        evaluate,
    )
}

/// Resolve a unit expression whose units were already resolved in the scope
/// of the tree naming them, as [`resolve_unit_scale`] does.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a unit is unknown or a dynamic scale expression
/// fails to evaluate to a quantity.
pub(super) fn resolved_unit_scale(
    unit: &ResolvedUnitExpr<ResolvedUnitRef>,
    values: &RuntimeValueMap,
    session: &EvalSession<'_>,
    evaluate: EvaluateExecutable,
) -> Result<PositiveFiniteScale, GraphcalError> {
    fold_unit_scale(
        unit.span,
        unit.terms
            .iter()
            .map(|term| (term, term.name.value.resolved().clone())),
        values,
        session,
        ResolvedUnitRef::spelling,
        evaluate,
    )
}
