//! Evaluated values shaped for the view: numbers in the unit `graphcal eval`
//! shows them in, labels, and nested entries per axis.

use graphcal_eval::eval::{
    KeyRendering, RenderContext, UnitLabel, Value, format_number, quantity_display_value,
};

use super::model::{ViewEntry, ViewValue};

/// Project one evaluated value for the view, by the rule `graphcal eval`
/// follows: a quantity in its display unit, else in the default label of its
/// dimension (`m^2*kg/s^2` for a torque).
///
/// A leaf that cannot be shown in its display unit is shown in SI instead,
/// as `graphcal eval` does, and the failure is added to `problems`, prefixed
/// with the entry keys of the leaf (`[#2]: …`).
pub(super) fn view_value(
    value: &Value,
    render: &RenderContext,
    problems: &mut Vec<String>,
) -> ViewValue {
    leaf_or_entries(value, render, "", problems)
}

fn leaf_or_entries(
    value: &Value,
    render: &RenderContext,
    path: &str,
    problems: &mut Vec<String>,
) -> ViewValue {
    match value {
        Value::Quantity {
            si_value,
            display_unit,
            ..
        } => {
            let (magnitude, unit) = display_unit.as_ref().map_or_else(
                || (si_value.get(), value.display_label(render)),
                |display_unit| match quantity_display_value(*si_value, Some(display_unit)) {
                    Ok(magnitude) => (magnitude, Some(display_unit.label.clone())),
                    // A failed display unit falls back to SI, labelled by dimension.
                    Err(error) => {
                        problems.push(problem(path, &error));
                        (si_value.get(), si_label(value, render))
                    }
                },
            );
            let magnitude = format_number(magnitude);
            let display = unit
                .as_ref()
                .map_or_else(|| magnitude.clone(), |unit| format!("{magnitude} {unit}"));
            ViewValue::Quantity {
                display,
                magnitude,
                si_value: si_value.get(),
                unit,
            }
        }
        Value::Int(int) => ViewValue::Int {
            display: int.to_string(),
        },
        Value::Bool(bool) => ViewValue::Bool {
            display: bool.to_string(),
            value: *bool,
        },
        Value::Key(key) => match KeyRendering::of(key) {
            KeyRendering::Label {
                index_name,
                variant,
            } => ViewValue::Label {
                display: variant.to_string(),
                index: index_name.to_string(),
                variant: variant.to_string(),
            },
            KeyRendering::Position(position) => ViewValue::Text {
                display: position.to_string(),
            },
            KeyRendering::Coordinate(quantity) => {
                leaf_or_entries(&quantity, render, path, problems)
            }
        },
        Value::Indexed {
            index_name,
            entries,
            ..
        } => ViewValue::Indexed {
            display: format!("[{index_name}]"),
            index: index_name.to_string(),
            entries: entries
                .iter()
                .map(|(key, entry)| ViewEntry {
                    key: key.to_string(),
                    value: leaf_or_entries(entry, render, &format!("{path}[{key}]"), problems),
                })
                .collect(),
        },
        Value::Complex { .. } | Value::Struct { .. } | Value::Datetime { .. } => ViewValue::Text {
            display: value
                .format_display(render, UnitLabel::Inline)
                .unwrap_or_else(|error| {
                    problems.push(problem(path, &error));
                    "(not displayable)".to_string()
                }),
        },
    }
}

/// The SI label of a quantity's dimension, ignoring its display unit.
fn si_label(value: &Value, render: &RenderContext) -> Option<String> {
    match value {
        Value::Quantity {
            si_value,
            dimension,
            ..
        } => Value::Quantity {
            si_value: *si_value,
            dimension: dimension.clone(),
            display_unit: None,
        }
        .display_label(render),
        _ => None,
    }
}

fn problem(path: &str, error: &impl std::fmt::Display) -> String {
    if path.is_empty() {
        format!("{error} (SI value retained)")
    } else {
        format!("{path}: {error} (SI value retained)")
    }
}

/// The unit every quantity leaf of `value` is shown in, when they all agree.
pub(super) fn common_unit(value: &ViewValue) -> Option<String> {
    let mut leaves = Vec::new();
    collect_quantity_units(value, &mut leaves);
    let first = leaves.first()?.clone();
    leaves.iter().all(|unit| *unit == first).then_some(first)?
}

fn collect_quantity_units(value: &ViewValue, out: &mut Vec<Option<String>>) {
    match value {
        ViewValue::Quantity { unit, .. } => out.push(unit.clone()),
        ViewValue::Indexed { entries, .. } => {
            for entry in entries {
                collect_quantity_units(&entry.value, out);
            }
        }
        ViewValue::Int { .. }
        | ViewValue::Bool { .. }
        | ViewValue::Label { .. }
        | ViewValue::Text { .. } => {}
    }
}
