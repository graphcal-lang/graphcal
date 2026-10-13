//! Evaluated values shaped for the view: numbers in their display unit,
//! labels, and nested entries per axis.

use std::collections::HashMap;

use graphcal_compiler::dimension::Dimension;
use graphcal_compiler::ir::prelude_definitions::{PreludeDefinitionError, prelude_definitions};
use graphcal_compiler::semantic::unit_scale::PositiveFiniteScale;
use graphcal_eval::eval::{
    DisplayProjectionError, KeyRendering, RenderContext, UnitLabel, Value, format_number,
    quantity_display_value,
};

use super::{ViewEntry, ViewValue};

/// The unit a quantity without an explicit display unit is shown in: the
/// prelude's one coherent (scale 1) unit of its dimension, such as `W` for
/// `Power`, when exactly one exists. The SI value is unchanged either way.
#[derive(Debug, Clone)]
pub(super) struct CoherentUnits {
    by_dimension: HashMap<Dimension, Option<String>>,
}

impl CoherentUnits {
    /// Index the prelude's static units by dimension.
    ///
    /// # Errors
    ///
    /// Returns the prelude construction failure, a compiler bug.
    pub(super) fn from_prelude() -> Result<Self, PreludeDefinitionError> {
        let prelude = prelude_definitions()?;
        let mut by_dimension = HashMap::<Dimension, Option<String>>::new();
        for (name, info) in prelude.units() {
            let coherent = info.scale.static_scale() == Some(PositiveFiniteScale::ONE);
            if !coherent || info.dimension.is_dimensionless() {
                continue;
            }
            by_dimension
                .entry(info.dimension.clone())
                .and_modify(|unit| *unit = None)
                .or_insert_with(|| Some(name.as_str().to_string()));
        }
        Ok(Self { by_dimension })
    }

    /// The coherent prelude unit of `dimension`, if it is unambiguous.
    pub(super) fn of(&self, dimension: &Dimension) -> Option<&str> {
        self.by_dimension.get(dimension)?.as_deref()
    }
}

/// Project one evaluated value for the view.
///
/// # Errors
///
/// Returns a display-unit projection failure (non-finite or underflowing
/// conversion) for the first leaf that cannot be shown.
pub(super) fn view_value(
    value: &Value,
    render: &RenderContext,
    units: &CoherentUnits,
) -> Result<ViewValue, DisplayProjectionError> {
    match value {
        Value::Quantity {
            si_value,
            dimension,
            display_unit,
        } => {
            let (magnitude, unit) = match display_unit {
                Some(display_unit) => (
                    quantity_display_value(*si_value, Some(display_unit))?,
                    Some(display_unit.label.clone()),
                ),
                None => (
                    si_value.get(),
                    units
                        .of(dimension)
                        .map(str::to_string)
                        .or_else(|| value.display_label(render)),
                ),
            };
            let magnitude = format_number(magnitude);
            let display = unit
                .as_ref()
                .map_or_else(|| magnitude.clone(), |unit| format!("{magnitude} {unit}"));
            Ok(ViewValue::Quantity {
                display,
                magnitude,
                si_value: si_value.get(),
                unit,
            })
        }
        Value::Int(int) => Ok(ViewValue::Int {
            display: int.to_string(),
        }),
        Value::Bool(bool) => Ok(ViewValue::Bool {
            display: bool.to_string(),
            value: *bool,
        }),
        Value::Key(key) => match KeyRendering::of(key) {
            KeyRendering::Label {
                index_name,
                variant,
            } => Ok(ViewValue::Label {
                display: variant.to_string(),
                index: index_name.to_string(),
                variant: variant.to_string(),
            }),
            KeyRendering::Position(position) => Ok(ViewValue::Text {
                display: position.to_string(),
            }),
            KeyRendering::Coordinate(quantity) => view_value(&quantity, render, units),
        },
        Value::Indexed {
            index_name,
            entries,
            ..
        } => Ok(ViewValue::Indexed {
            display: format!("[{index_name}]"),
            index: index_name.to_string(),
            entries: entries
                .iter()
                .map(|(key, entry)| {
                    view_value(entry, render, units).map(|projected| ViewEntry {
                        key: value.indexed_entry_display_name(key),
                        value: projected,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        }),
        Value::Complex { .. } | Value::Struct { .. } | Value::Datetime { .. } => {
            Ok(ViewValue::Text {
                display: value.format_display(render, UnitLabel::Inline)?,
            })
        }
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
