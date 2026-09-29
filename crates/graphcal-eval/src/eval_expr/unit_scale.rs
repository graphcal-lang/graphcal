use graphcal_compiler::hir::ResolvedUnitExpr;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitScale, UnitScaleStepError, UnitScaleTerm,
    try_fold_unit_scale,
};
use graphcal_compiler::resolved_name::ResolvedUnitName;
use graphcal_compiler::syntax::dimension::UnitRef;
use graphcal_compiler::syntax::span::Span;

use super::numeric;
use super::{EvalContext, HirLocalValueMap, RuntimeValueMap, hir_eval::eval_hir_expr};

/// Build a quantity runtime value after validating that it is finite.
pub(in crate::eval_expr) fn checked_finite_quantity(
    value: f64,
    context: &str,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let value = numeric::finite_quantity(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))?;
    RuntimeValue::quantity(value).map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn unit_scale_error(
    context: &str,
    error: PositiveFiniteScaleError,
    span: Span,
    ctx: &EvalContext<'_>,
) -> GraphcalError {
    ctx.eval_error(format!("{context} {error}"), span)
}

/// Apply a unit scale to a literal value and validate that the SI value is finite.
pub(in crate::eval_expr) fn checked_unit_scaled_value(
    value: f64,
    scale: PositiveFiniteScale,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let value = numeric::finite_quantity(value * scale.get(), "quantity literal value")
        .map_err(|err| ctx.eval_error(err.to_string(), span))?;
    RuntimeValue::quantity(value).map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn resolve_dynamic_unit_scale(
    unit: &ResolvedUnitName,
    spelling: &UnitRef,
    base_unit_scale: PositiveFiniteScale,
    span: Span,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<PositiveFiniteScale, GraphcalError> {
    let unit_dag = ctx.tir.dag_registry().get(unit.owner()).ok_or_else(|| {
        ctx.internal_error(
            format!("dynamic unit owner for `{spelling}` could not be resolved"),
            span,
        )
    })?;
    let scale_hir = unit_dag
        .semantic()
        .dynamic_unit_scales
        .get(unit)
        .ok_or_else(|| {
            ctx.internal_error(
                format!("dynamic unit scale for `{spelling}` could not be resolved"),
                span,
            )
        })?;
    let scale_ctx = ctx.for_dag(unit_dag, &scale_hir.src)?;
    if scale_hir.declared_dimension != scale_hir.base_unit_dimension {
        return Err(scale_ctx.internal_error(
            format!(
                "dynamic unit `{}` has mismatched declared and base-unit dimensions",
                scale_hir.spelling
            ),
            scale_hir.span,
        ));
    }
    let empty_locals = HirLocalValueMap::root();
    let scale_val = eval_hir_expr(&scale_hir.expr, values, &empty_locals, &scale_ctx)?;
    let RuntimeValue::Quantity(scale_f64) = scale_val else {
        return Err(scale_ctx.internal_error(
            "dynamic unit scale expression must evaluate to a quantity",
            scale_hir.expr.span,
        ));
    };
    let dynamic_scale = PositiveFiniteScale::new(scale_f64.get()).map_err(|error| {
        unit_scale_error("dynamic unit scale", error, scale_hir.expr.span, &scale_ctx)
    })?;
    dynamic_scale
        .checked_mul(base_unit_scale)
        .map_err(|error| unit_scale_error("dynamic unit scale", error, scale_hir.span, ctx))
}

/// Resolve a `UnitExpr` to its compound scale factor at runtime.
///
/// Static unit definitions come from the TIR's canonical project type store.
/// For dynamic units, the unit's strictly validated HIR scale expression in
/// the current concrete DAG instance is evaluated against the current `values`,
/// then multiplied by the base unit's static scale. Dynamic scale expressions
/// are standalone (graph/const references
/// only), so no local environment is involved.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a unit is unknown or a dynamic scale expression
/// fails to evaluate to a quantity.
pub fn resolve_unit_scale(
    unit: &ResolvedUnitExpr,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<PositiveFiniteScale, GraphcalError> {
    try_fold_unit_scale(
        &unit.terms,
        |item| {
            let resolved_unit = ctx.resolve_unit(&item.name.value);
            let info = ctx.tir.unit_info(&resolved_unit).ok_or_else(|| {
                ctx.internal_error(
                    format!("unknown checked unit `{}`", item.name.value.spelling()),
                    item.name.span,
                )
            })?;
            let scale = match &info.scale {
                UnitScale::Const(scale) | UnitScale::Runtime(scale) => *scale,
                UnitScale::Dynamic { base_unit_scale } => resolve_dynamic_unit_scale(
                    &resolved_unit,
                    item.name.value.spelling(),
                    *base_unit_scale,
                    item.name.span,
                    values,
                    ctx,
                )?,
            };
            Ok(UnitScaleTerm {
                op: item.op,
                scale,
                power: item.power,
            })
        },
        |item, error| match error {
            UnitScaleStepError::Power(error) => {
                unit_scale_error("unit scale exponentiation", error, item.name.span, ctx)
            }
            UnitScaleStepError::Compound(error) => {
                unit_scale_error("compound unit scale", error, unit.span, ctx)
            }
        },
    )
}
