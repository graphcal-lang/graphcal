use graphcal_compiler::hir::ResolvedUnitExpr;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::UnitScale;
use graphcal_compiler::syntax::ast::MulDivOp;
use graphcal_compiler::syntax::dimension::{ResolvedUnitName, UnitRef};
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
    numeric::finite_quantity(value, context)
        .map(RuntimeValue::Quantity)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn checked_positive_finite_unit_scale(
    value: f64,
    context: &str,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    numeric::positive_finite_scale(value, context)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// Apply a unit scale to a literal value and validate that the SI value is finite.
pub(in crate::eval_expr) fn checked_unit_scaled_value(
    value: f64,
    scale: f64,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    numeric::finite_quantity(value * scale, "quantity literal value")
        .map(RuntimeValue::Quantity)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

fn resolve_dynamic_unit_scale(
    unit: &ResolvedUnitName,
    spelling: &UnitRef,
    base_unit_scale: f64,
    span: Span,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    let unit_dag = ctx.tir.dag_registry().get(unit.owner()).ok_or_else(|| {
        ctx.eval_error(
            format!("dynamic unit owner for `{spelling}` could not be resolved"),
            span,
        )
    })?;
    let scale_hir = unit_dag
        .semantic()
        .dynamic_unit_scales
        .get(unit)
        .ok_or_else(|| {
            ctx.eval_error(
                format!("dynamic unit scale for `{spelling}` could not be resolved"),
                span,
            )
        })?;
    let scale_ctx = ctx.for_dag(unit_dag, &scale_hir.src);
    if scale_hir.declared_dimension != scale_hir.base_unit_dimension {
        return Err(scale_ctx.eval_error(
            format!(
                "internal: dynamic unit `{}` has mismatched declared and base-unit dimensions",
                scale_hir.spelling
            ),
            scale_hir.span,
        ));
    }
    let empty_locals = HirLocalValueMap::root();
    let scale_val = eval_hir_expr(&scale_hir.expr, values, &empty_locals, &scale_ctx)?;
    let RuntimeValue::Quantity(scale_f64) = scale_val else {
        return Err(scale_ctx.eval_error(
            "dynamic unit scale expression must evaluate to a quantity",
            scale_hir.expr.span,
        ));
    };
    let dynamic_scale = checked_positive_finite_unit_scale(
        scale_f64,
        "dynamic unit scale",
        scale_hir.expr.span,
        &scale_ctx,
    )?;
    let base_scale =
        checked_positive_finite_unit_scale(base_unit_scale, "base unit scale", span, ctx)?;
    checked_positive_finite_unit_scale(
        dynamic_scale * base_scale,
        "dynamic unit scale",
        scale_hir.span,
        ctx,
    )
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
) -> Result<f64, GraphcalError> {
    let mut compound_scale = 1.0;
    for item in &unit.terms {
        let source_unit = item.name.value.resolved();
        let instance_unit = ctx.current_dag.runtime_unit_identity(source_unit);
        let resolved_unit = if ctx.tir.unit_info(&instance_unit).is_some() {
            instance_unit
        } else {
            source_unit.clone()
        };
        let info = ctx.tir.unit_info(&resolved_unit).ok_or_else(|| {
            ctx.eval_error(
                format!("unknown unit `{}`", item.name.value.spelling()),
                item.name.span,
            )
        })?;
        let unit_scale = match &info.scale {
            UnitScale::Static(s) => {
                checked_positive_finite_unit_scale(s.get(), "unit scale", item.name.span, ctx)?
            }
            UnitScale::Dynamic { base_unit_scale } => resolve_dynamic_unit_scale(
                &resolved_unit,
                item.name.value.spelling(),
                base_unit_scale.get(),
                item.name.span,
                values,
                ctx,
            )?,
        };
        let exp = item.power;
        let powered_scale = checked_positive_finite_unit_scale(
            graphcal_compiler::registry::types::pow_scale(unit_scale, exp),
            "unit scale exponentiation",
            item.name.span,
            ctx,
        )?;
        compound_scale = match item.op {
            MulDivOp::Mul => compound_scale * powered_scale,
            MulDivOp::Div => compound_scale / powered_scale,
        };
        compound_scale = checked_positive_finite_unit_scale(
            compound_scale,
            "compound unit scale",
            unit.span,
            ctx,
        )?;
    }
    Ok(compound_scale)
}
