//! Pure projection of evaluated presentation evidence. No interpreter, owner
//! lookup, host capability, or invocation environment is available here.

use super::types::{DisplayUnit, Value, validate_display_projection};
use crate::invariant::Invariant;
use crate::presentation_evidence::{
    LeafPresentationDiagnostic, PresentationFailure, PresentationLeaf, PresentationPathPart,
    QuantityDisplay, ResolvedLeaf,
};
use crate::runtime_presentation::{PresentedView, ResolvedValue};
use graphcal_compiler::registry::format::format_number;
use graphcal_compiler::registry::index::CoordinateIndexData;

/// Display `value` as `presented` says, reporting each quantity leaf whose
/// display failed; such a leaf keeps its SI value.
///
/// `value` must be the public projection of `presented`'s runtime value.
///
/// # Errors
///
/// Returns an [`Invariant`] when `value` does not have the shape and leaf
/// kinds of `presented`'s runtime value. The public [`Value`] is a second
/// value model, produced from the runtime value by a separate projection
/// walk, so this correspondence is checked here rather than carried by a type.
pub(super) fn attach_presentation(
    value: &mut Value,
    presented: &ResolvedValue,
) -> Result<Vec<LeafPresentationDiagnostic>, Invariant> {
    let mut diagnostics = Vec::new();
    attach(value, presented.view(), &[], &mut diagnostics)?;
    Ok(diagnostics)
}

fn attach(
    value: &mut Value,
    presented: PresentedView<'_, ResolvedLeaf>,
    path: &[PresentationPathPart],
    diagnostics: &mut Vec<LeafPresentationDiagnostic>,
) -> Result<(), Invariant> {
    match presented {
        PresentedView::Whole { leaf: None, .. } => Ok(()),
        PresentedView::Whole {
            leaf: Some(leaf), ..
        } => attach_leaf(value, leaf, path, diagnostics),
        PresentedView::Struct(fields) => {
            let Value::Struct {
                fields: projected, ..
            } = value
            else {
                return Err(projection_mismatch("a struct value", value));
            };
            fields.fields().try_for_each(|(name, field)| {
                let projected = projected.get_mut(name).ok_or_else(|| {
                    Invariant::violated(format_args!(
                        "the public projection of a struct value lost field `{name}`"
                    ))
                })?;
                let path = [path, &[PresentationPathPart::Field(name.clone())]].concat();
                attach(projected, field.view(), &path, diagnostics)
            })
        }
        PresentedView::Indexed(entries) => {
            let Value::Indexed {
                entries: projected, ..
            } = value
            else {
                return Err(projection_mismatch("an indexed value", value));
            };
            entries.iter().try_for_each(|(key, entry)| {
                let projected = projected.get_mut(key).ok_or_else(|| {
                    Invariant::violated(format_args!(
                        "the public projection of an indexed value lost entry `{key}`"
                    ))
                })?;
                let path = [path, &[PresentationPathPart::Index(key.clone())]].concat();
                attach(projected, entry.view(), &path, diagnostics)
            })
        }
    }
}

/// Present every leaf of `value` by `leaf`. The runtime value holds only
/// leaves of the leaf's kind (checked when the presentation was attached to
/// it), so its projection does too.
fn attach_leaf(
    value: &mut Value,
    leaf: &ResolvedLeaf,
    path: &[PresentationPathPart],
    diagnostics: &mut Vec<LeafPresentationDiagnostic>,
) -> Result<(), Invariant> {
    match (leaf, value) {
        (_, Value::Indexed { entries, .. }) => entries.iter_mut().try_for_each(|(key, entry)| {
            let path = [path, &[PresentationPathPart::Index(key.clone())]].concat();
            attach_leaf(entry, leaf, &path, diagnostics)
        }),
        (
            ResolvedLeaf::Quantity(QuantityDisplay::Failed(failure)),
            Value::Quantity { .. } | Value::Complex { .. },
        ) => {
            diagnostics.push(LeafPresentationDiagnostic {
                path: path.to_vec(),
                failure: failure.clone(),
            });
            Ok(())
        }
        (
            ResolvedLeaf::Quantity(QuantityDisplay::Unit { label, scale }),
            value @ (Value::Quantity { .. } | Value::Complex { .. }),
        ) => {
            set_display_unit(value, Some(DisplayUnit::new(label.clone(), *scale)));
            if let Err(error) = validate_display_projection(value) {
                set_display_unit(value, None);
                diagnostics.push(LeafPresentationDiagnostic {
                    path: path.to_vec(),
                    failure: PresentationFailure::Projection {
                        message: error.to_string(),
                    },
                });
            }
            Ok(())
        }
        (ResolvedLeaf::Datetime(timezone), Value::Datetime { display_tz, .. }) => {
            *display_tz = Some(timezone.clone());
            Ok(())
        }
        (ResolvedLeaf::Quantity(_) | ResolvedLeaf::Datetime(_), value) => Err(projection_mismatch(
            format_args!("a {:?} leaf", leaf.kind()),
            value,
        )),
    }
}

/// A public projection that does not have the shape of its runtime value.
fn projection_mismatch(expected: impl std::fmt::Display, value: &Value) -> Invariant {
    let actual = match value {
        Value::Quantity { .. } => "a quantity",
        Value::Complex { .. } => "a complex value",
        Value::Bool(_) => "a Bool",
        Value::Int(_) => "an Int",
        Value::Key(_) => "a key",
        Value::Struct { .. } => "a struct value",
        Value::Indexed { .. } => "an indexed value",
        Value::Datetime { .. } => "a datetime",
    };
    Invariant::violated(format_args!(
        "the public projection of {expected} is {actual}"
    ))
}

fn set_display_unit(value: &mut Value, unit: Option<DisplayUnit>) {
    if let Value::Quantity { display_unit, .. } | Value::Complex { display_unit, .. } = value {
        *display_unit = unit;
    }
}

fn format_coordinate_impl(data: &CoordinateIndexData, position: usize, exact: bool) -> String {
    let display_value = data.coordinate_value(position) / data.display().scale.get();
    let formatted = if exact {
        display_value.to_string()
    } else {
        format_number(display_value)
    };
    match &data.display().label {
        Some(label) => format!("{formatted} {label}"),
        None => formatted,
    }
}

pub(super) fn format_coordinate(data: &CoordinateIndexData, position: usize) -> String {
    format_coordinate_impl(data, position, false)
}
pub(super) fn format_coordinate_exact(data: &CoordinateIndexData, position: usize) -> String {
    format_coordinate_impl(data, position, true)
}
