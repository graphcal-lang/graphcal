use std::collections::HashMap;

use graphcal_compiler::desugar::resolved_ast::{MulDivOp, UnitExpr};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::UnitScale;
use graphcal_compiler::syntax::span::Span;

use super::numeric;
use super::{EvalContext, RuntimeValueMap, eval_expr};

/// Build a scalar runtime value after validating that it is finite.
pub(in crate::eval_expr) fn checked_finite_scalar(
    value: f64,
    context: &str,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    numeric::finite_scalar(value, context)
        .map(RuntimeValue::Scalar)
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
    numeric::finite_scalar(value * scale, "unit literal value")
        .map(RuntimeValue::Scalar)
        .map_err(|err| ctx.eval_error(err.to_string(), span))
}

/// Resolve a `UnitExpr` to its compound scale factor at runtime.
///
/// For static units, this is equivalent to `registry.units.resolve_unit_expr()`.
/// For dynamic units, the scale expression is evaluated using the current `values`
/// and `local_values` maps, then multiplied by the base unit's static scale.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a unit is unknown or a dynamic scale expression
/// fails to evaluate to a scalar.
pub fn resolve_unit_scale(
    unit: &UnitExpr,
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    let mut compound_scale = 1.0;
    for item in &unit.terms {
        let info = ctx
            .registry
            .units
            .get_unit(item.name.value.as_str())
            .ok_or_else(|| {
                ctx.eval_error(
                    format!("unknown unit `{}`", item.name.value),
                    item.name.span,
                )
            })?;
        let unit_scale = match &info.scale {
            UnitScale::Static(s) => {
                checked_positive_finite_unit_scale(s.get(), "unit scale", item.name.span, ctx)?
            }
            UnitScale::Dynamic {
                scale_expr,
                base_unit_scale,
            } => {
                let scale_val = eval_expr(scale_expr, values, local_values, ctx)?;
                let RuntimeValue::Scalar(scale_f64) = scale_val else {
                    return Err(ctx.eval_error(
                        "dynamic unit scale expression must evaluate to a scalar",
                        scale_expr.span,
                    ));
                };
                let dynamic_scale = checked_positive_finite_unit_scale(
                    scale_f64,
                    "dynamic unit scale",
                    scale_expr.span,
                    ctx,
                )?;
                let base_scale = checked_positive_finite_unit_scale(
                    base_unit_scale.get(),
                    "base unit scale",
                    item.name.span,
                    ctx,
                )?;
                checked_positive_finite_unit_scale(
                    dynamic_scale * base_scale,
                    "dynamic unit scale",
                    scale_expr.span,
                    ctx,
                )?
            }
        };
        let exp = item.power.unwrap_or(1);
        let powered_scale = checked_positive_finite_unit_scale(
            unit_scale.powi(exp),
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
