//! Display unit resolution: attaching human-readable unit labels to computed values,
//! formatting range steps, and converting unit expressions to strings.

use std::collections::HashMap;

use graphcal_compiler::syntax::ast::{ExprKind, MapEntryKey, MulDivOp};
use graphcal_compiler::syntax::names::VariantName;
use indexmap::IndexMap;

use crate::eval_expr::{RuntimeValue, eval_expr};
use crate::registry::{Registry, UnitScale};

use super::types::{DisplayUnit, Value};
use crate::format::{format_number, format_unit_expr};

pub(super) fn attach_display_units(
    value: &mut Value,
    expr: &graphcal_compiler::syntax::ast::Expr,
    registry: &Registry,
    values: &HashMap<String, RuntimeValue>,
) {
    match (&mut *value, &expr.kind) {
        (Value::Scalar { display_unit, .. }, ExprKind::UnitLiteral { unit, .. }) => {
            *display_unit = resolve_unit_to_display(unit, registry, values);
        }
        (Value::Scalar { display_unit, .. }, ExprKind::Convert { target, .. }) => {
            *display_unit = resolve_unit_to_display(target, registry, values);
        }
        // Struct construction: recurse into each field initializer
        (Value::Struct { fields, .. }, ExprKind::StructConstruction { fields: inits, .. }) => {
            for init in inits {
                if let Some(field_val) = fields.get_mut(&init.name.value)
                    && let Some(init_expr) = &init.value
                {
                    attach_display_units(field_val, init_expr, registry, values);
                }
            }
        }
        // Map/table literal: recurse into each entry, walking through nested
        // Indexed values for multi-axis maps.
        (
            Value::Indexed { entries, .. },
            ExprKind::MapLiteral {
                entries: map_entries,
            }
            | ExprKind::TableLiteral {
                entries: map_entries,
                ..
            },
        ) => {
            for map_entry in map_entries {
                if let Some(target) = walk_indexed_keys(entries, &map_entry.keys) {
                    attach_display_units(target, &map_entry.value, registry, values);
                }
            }
        }
        // For comprehension: extract a single display unit from body, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::ForComp { body, .. }) => {
            if let Some(du) = extract_flat_display_unit(body, registry, values) {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Scan: extract a single display unit from init, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::Scan { init, .. })
        | (Value::Indexed { entries, .. }, ExprKind::Unfold { init, .. }) => {
            if let Some(du) = extract_flat_display_unit(init, registry, values) {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Timezone display: set display_tz on Datetime values
        (Value::Datetime { display_tz, .. }, ExprKind::DisplayTimezone { timezone, .. }) => {
            *display_tz = Some(timezone.clone());
        }
        // All other combinations: no display unit to attach
        _ => {}
    }
}

/// Resolve a `UnitExpr` to a `DisplayUnit`.
///
/// Handles both static and dynamic unit scales. For dynamic units, the scale
/// expression is evaluated using the provided `values` map.
pub(super) fn resolve_unit_to_display(
    unit: &graphcal_compiler::syntax::ast::UnitExpr,
    registry: &Registry,
    values: &HashMap<String, RuntimeValue>,
) -> Option<DisplayUnit> {
    let scale = resolve_display_unit_scale(unit, registry, values)?;
    Some(DisplayUnit {
        label: format_unit_expr(unit),
        scale,
    })
}

/// Extract a single display unit from a scalar-producing expression.
///
/// Used for indexed collections (for comprehensions, scan) where all entries
/// share the same display unit.
pub(super) fn extract_flat_display_unit(
    expr: &graphcal_compiler::syntax::ast::Expr,
    registry: &Registry,
    values: &HashMap<String, RuntimeValue>,
) -> Option<DisplayUnit> {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => resolve_unit_to_display(unit, registry, values),
        ExprKind::Convert { target, .. } => resolve_unit_to_display(target, registry, values),
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => entries
            .first()
            .and_then(|e| extract_flat_display_unit(&e.value, registry, values)),
        ExprKind::ForComp { body, .. } => extract_flat_display_unit(body, registry, values),
        ExprKind::Scan { init, .. } | ExprKind::Unfold { init, .. } => {
            extract_flat_display_unit(init, registry, values)
        }
        _ => None,
    }
}

/// Resolve a `UnitExpr` to its compound scale factor for display purposes.
///
/// Similar to `eval_expr::resolve_unit_scale` but returns `Option` instead of `Result`
/// and builds a minimal `EvalContext` for evaluating dynamic scale expressions.
fn resolve_display_unit_scale(
    unit: &graphcal_compiler::syntax::ast::UnitExpr,
    registry: &Registry,
    values: &HashMap<String, RuntimeValue>,
) -> Option<f64> {
    let mut compound_scale = 1.0;
    for item in &unit.terms {
        let info = registry.units.get_unit(item.name.value.as_str())?;
        let unit_scale = match &info.scale {
            UnitScale::Static(s) => *s,
            UnitScale::Dynamic {
                scale_expr,
                base_unit_scale,
            } => {
                // Build a minimal EvalContext for evaluating the scale expression.
                let builtin_consts = crate::builtins::builtin_constants();
                let builtin_fns = crate::builtins::builtin_functions();
                let empty_src =
                    miette::NamedSource::new("<display>", std::sync::Arc::new(String::new()));
                let ctx = crate::eval_expr::EvalContext {
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src: &empty_src,
                    unfold_context: None,
                };
                let empty_locals = HashMap::new();
                let scale_val = eval_expr(scale_expr, values, &empty_locals, &ctx).ok()?;
                match scale_val {
                    RuntimeValue::Scalar(s) => s * base_unit_scale,
                    _ => return None,
                }
            }
        };
        let exp = item.power.unwrap_or(1);
        let powered_scale = unit_scale.powi(exp);
        match item.op {
            MulDivOp::Mul => compound_scale *= powered_scale,
            MulDivOp::Div => compound_scale /= powered_scale,
        }
    }
    Some(compound_scale)
}

/// Format a range index step value for display, e.g. `"0 s"`, `"0.25 s"`.
pub(super) fn format_range_step(idx_def: &crate::registry::IndexDef, step_index: usize) -> String {
    let Ok(si_value) = idx_def.step_value(step_index) else {
        return format!("#{step_index}");
    };
    if let crate::registry::IndexKind::Range {
        display_label,
        display_scale,
        ..
    } = &idx_def.kind
    {
        let display_value = si_value / display_scale;
        let formatted = format_number(display_value);
        match display_label {
            Some(label) => format!("{formatted} {label}"),
            None => formatted,
        }
    } else {
        format!("#{step_index}")
    }
}

/// Set display unit on a scalar value. No-op for non-scalar values.
pub(super) fn set_scalar_display_unit(value: &mut Value, du: &DisplayUnit) {
    if let Value::Scalar { display_unit, .. } = value {
        *display_unit = Some(du.clone());
    }
}

/// Walk through nested `Indexed` entries using successive map entry keys.
///
/// For a single-axis map (`keys.len() == 1`), returns the entry matching `keys[0]`.
/// For multi-axis maps, drills into nested `Value::Indexed` using each key in turn.
fn walk_indexed_keys<'a>(
    entries: &'a mut IndexMap<VariantName, Value>,
    keys: &[MapEntryKey],
) -> Option<&'a mut Value> {
    let (first, rest) = keys.split_first()?;
    let value = entries.get_mut(&first.variant.value)?;
    if rest.is_empty() {
        Some(value)
    } else if let Value::Indexed { entries: inner, .. } = value {
        walk_indexed_keys(inner, rest)
    } else {
        None
    }
}
