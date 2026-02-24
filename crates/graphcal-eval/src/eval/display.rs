//! Display unit resolution: attaching human-readable unit labels to computed values,
//! formatting range steps, and converting unit expressions to strings.

use graphcal_syntax::ast::ExprKind;

use crate::registry::Registry;

use super::types::{DisplayUnit, Value};

pub(super) fn attach_display_units(
    value: &mut Value,
    expr: &graphcal_syntax::ast::Expr,
    registry: &Registry,
) {
    match (&mut *value, &expr.kind) {
        (Value::Scalar { display_unit, .. }, ExprKind::UnitLiteral { unit, .. }) => {
            *display_unit = resolve_unit_to_display(unit, registry);
        }
        (Value::Scalar { display_unit, .. }, ExprKind::Convert { target, .. }) => {
            *display_unit = resolve_unit_to_display(target, registry);
        }
        // Struct construction: recurse into each field initializer
        (Value::Struct { fields, .. }, ExprKind::StructConstruction { fields: inits, .. }) => {
            for init in inits {
                if let Some(field_val) = fields.get_mut(&init.name.value)
                    && let Some(init_expr) = &init.value
                {
                    attach_display_units(field_val, init_expr, registry);
                }
            }
        }
        // Map/table literal: recurse into each entry
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
                if let Some(entry_val) = entries.get_mut(&map_entry.keys[0].variant.value) {
                    attach_display_units(entry_val, &map_entry.value, registry);
                }
            }
        }
        // For comprehension: extract a single display unit from body, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::ForComp { body, .. }) => {
            if let Some(du) = extract_flat_display_unit(body, registry) {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Scan: extract a single display unit from init, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::Scan { init, .. })
        | (Value::Indexed { entries, .. }, ExprKind::Unfold { init, .. }) => {
            if let Some(du) = extract_flat_display_unit(init, registry) {
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
pub(super) fn resolve_unit_to_display(
    unit: &graphcal_syntax::ast::UnitExpr,
    registry: &Registry,
) -> Option<DisplayUnit> {
    let (_dim, scale) = registry.units.resolve_unit_expr(unit)?;
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
    expr: &graphcal_syntax::ast::Expr,
    registry: &Registry,
) -> Option<DisplayUnit> {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => resolve_unit_to_display(unit, registry),
        ExprKind::Convert { target, .. } => resolve_unit_to_display(target, registry),
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => entries
            .first()
            .and_then(|e| extract_flat_display_unit(&e.value, registry)),
        ExprKind::ForComp { body, .. } => extract_flat_display_unit(body, registry),
        ExprKind::Scan { init, .. } | ExprKind::Unfold { init, .. } => {
            extract_flat_display_unit(init, registry)
        }
        _ => None,
    }
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

/// Format a numeric value for display: integers without decimal point, floats with
/// reasonable precision (up to 6 decimal places, trailing zeros stripped).
#[must_use]
pub fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "value.abs() < 1e15 guarantees it fits in i64"
        )]
        let int_val = value as i64;
        format!("{int_val}")
    } else {
        let s = format!("{value:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

/// Set display unit on a scalar value. No-op for non-scalar values.
pub(super) fn set_scalar_display_unit(value: &mut Value, du: &DisplayUnit) {
    if let Value::Scalar { display_unit, .. } = value {
        *display_unit = Some(du.clone());
    }
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/hour`, `kg * m / s^2`
pub fn format_unit_expr(expr: &graphcal_syntax::ast::UnitExpr) -> String {
    use graphcal_syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for item in &expr.terms {
        let mut part = item.name.value.to_string();
        if let Some(pow) = item.power
            && pow != 1
        {
            part = format!("{part}^{pow}");
        }
        match item.op {
            MulDivOp::Mul => numerator.push(part),
            MulDivOp::Div => denominator.push(part),
        }
    }

    if denominator.is_empty() {
        numerator.join(" * ")
    } else if numerator.len() == 1 && denominator.len() == 1 {
        format!("{}/{}", numerator[0], denominator[0])
    } else {
        let num = numerator.join(" * ");
        let den = denominator.join(" * ");
        format!("{num}/{den}")
    }
}
