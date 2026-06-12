//! Display unit resolution: attaching human-readable unit labels to computed values,
//! formatting range steps, and converting unit expressions to strings.

use graphcal_compiler::hir::ExprKind;
use graphcal_compiler::hir::expr::MapEntryKey;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::names::IndexVariantName;
use indexmap::IndexMap;

use crate::eval_expr::{EvalContext, RuntimeValueMap};

use super::types::{DisplayUnit, Value};
use graphcal_compiler::registry::format::{format_number, format_unit_expr_canonical};

/// Attach display units to a computed value based on its defining expression.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when a display unit's scale cannot be resolved
/// (unknown unit, non-positive or non-finite scale, dynamic scale evaluation
/// failure). A conversion the user wrote must either take effect or fail
/// loudly — silently falling back to the base unit would misreport the value.
pub(super) fn attach_display_units(
    value: &mut Value,
    expr: &graphcal_compiler::hir::Expr,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<(), GraphcalError> {
    match (&mut *value, &expr.kind) {
        (Value::Scalar { display_unit, .. }, ExprKind::UnitLiteral { unit, .. }) => {
            *display_unit = Some(resolve_unit_to_display(unit, ctx, values)?);
        }
        (Value::Scalar { display_unit, .. }, ExprKind::Convert { target, .. }) => {
            *display_unit = Some(resolve_unit_to_display(target, ctx, values)?);
        }
        // Element-wise conversion on indexed values (#648 U1): apply the
        // target uniformly to every scalar entry, through nested axes.
        (Value::Indexed { entries, .. }, ExprKind::Convert { target, .. }) => {
            let du = resolve_unit_to_display(target, ctx, values)?;
            for entry_val in entries.values_mut() {
                set_scalar_display_unit_deep(entry_val, &du);
            }
        }
        // Constructor call: recurse into each field initializer.
        (Value::Struct { fields, .. }, ExprKind::ConstructorCall { fields: inits, .. }) => {
            for init in inits {
                if let Some(field_val) = fields.get_mut(&init.name.value) {
                    attach_display_units(field_val, &init.value, ctx, values)?;
                }
            }
        }
        // Map/table literal: recurse into each entry, walking through nested
        // Indexed values for multi-axis maps.
        (
            Value::Indexed { entries, .. },
            ExprKind::MapLiteral {
                entries: map_entries,
            },
        ) => {
            for map_entry in map_entries {
                if let Some(target) = walk_indexed_keys(entries, map_entry.keys.as_slice()) {
                    attach_display_units(target, &map_entry.value, ctx, values)?;
                }
            }
        }
        // For comprehension: extract a single display unit from body, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::ForComp { body, .. }) => {
            if let Some(du) = extract_flat_display_unit(body, ctx, values)? {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Scan: extract a single display unit from init, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::Scan { init, .. })
        | (Value::Indexed { entries, .. }, ExprKind::Unfold { init, .. }) => {
            if let Some(du) = extract_flat_display_unit(init, ctx, values)? {
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
    Ok(())
}

/// Resolve a `UnitExpr` to a `DisplayUnit`.
///
/// Handles both static and dynamic unit scales. For dynamic units, the scale
/// expression is evaluated using the provided evaluation context and value map.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when the scale cannot be resolved; see
/// [`attach_display_units`].
pub(super) fn resolve_unit_to_display(
    unit: &graphcal_compiler::desugar::desugared_ast::UnitExpr,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<DisplayUnit, GraphcalError> {
    let scale = crate::eval_expr::resolve_unit_scale(unit, values, ctx)?;
    Ok(DisplayUnit {
        label: format_unit_expr_canonical(unit),
        scale,
    })
}

/// Extract a single display unit from a scalar-producing expression.
///
/// Used for indexed collections (for comprehensions, scan) where all entries
/// share the same display unit. `Ok(None)` means the expression carries no
/// display unit; `Err` means it carries one that failed to resolve.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when a present display unit's scale cannot be
/// resolved; see [`attach_display_units`].
pub(super) fn extract_flat_display_unit(
    expr: &graphcal_compiler::hir::Expr,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<Option<DisplayUnit>, GraphcalError> {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => resolve_unit_to_display(unit, ctx, values).map(Some),
        ExprKind::Convert { target, .. } => resolve_unit_to_display(target, ctx, values).map(Some),
        ExprKind::MapLiteral { entries } => entries.first().map_or(Ok(None), |e| {
            extract_flat_display_unit(&e.value, ctx, values)
        }),
        ExprKind::ForComp { body, .. } => extract_flat_display_unit(body, ctx, values),
        ExprKind::Scan { init, .. } | ExprKind::Unfold { init, .. } => {
            extract_flat_display_unit(init, ctx, values)
        }
        _ => Ok(None),
    }
}

/// Format a range index step value for display, e.g. `"0 s"`, `"0.25 s"`.
pub(super) fn format_range_step(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    step_index: usize,
) -> String {
    idx_def.range_data().map_or_else(
        || format!("#{step_index}"),
        |data| {
            let si_value = data.step_value(step_index);
            let display_value = si_value / data.display_scale;
            let formatted = format_number(display_value);
            match &data.display_label {
                Some(label) => format!("{formatted} {label}"),
                None => formatted,
            }
        },
    )
}

/// Set display unit on a scalar value. No-op for non-scalar values.
pub(super) fn set_scalar_display_unit(value: &mut Value, du: &DisplayUnit) {
    if let Value::Scalar { display_unit, .. } = value {
        *display_unit = Some(du.clone());
    }
}

/// Set display unit on every scalar leaf, descending through nested
/// `Indexed` layers (multi-axis values).
pub(super) fn set_scalar_display_unit_deep(value: &mut Value, du: &DisplayUnit) {
    match value {
        Value::Scalar { display_unit, .. } => *display_unit = Some(du.clone()),
        Value::Indexed { entries, .. } => {
            for entry in entries.values_mut() {
                set_scalar_display_unit_deep(entry, du);
            }
        }
        _ => {}
    }
}

/// Walk through nested `Indexed` entries using successive map entry keys.
///
/// For a single-axis map (`keys.len() == 1`), returns the entry matching `keys[0]`.
/// For multi-axis maps, drills into nested `Value::Indexed` using each key in turn.
fn walk_indexed_keys<'a>(
    entries: &'a mut IndexMap<IndexVariantName, Value>,
    keys: &[MapEntryKey],
) -> Option<&'a mut Value> {
    let (first, rest) = keys.split_first()?;
    let variant = match first {
        MapEntryKey::IndexVariant(resolved) => resolved.value.variant(),
        MapEntryKey::NatRangeVariant { variant, .. } => &variant.value,
    };
    let value = entries.get_mut(variant)?;
    if rest.is_empty() {
        Some(value)
    } else if let Value::Indexed { entries: inner, .. } = value {
        walk_indexed_keys(inner, rest)
    } else {
        None
    }
}
