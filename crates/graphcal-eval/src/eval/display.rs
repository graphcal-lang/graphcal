//! Pure projection of evaluated presentation evidence. No interpreter, owner
//! lookup, host capability, or invocation environment is available here.

use super::types::{DisplayUnit, Value, validate_display_projection};
use crate::presentation_evidence::{
    LeafPresentationDiagnostic, Presentation, PresentationFailure, PresentationPathPart,
    ResolvedLeaf, ResolvedPresentation,
};
use graphcal_compiler::registry::format::format_number;
use graphcal_compiler::registry::index::CoordinateIndexData;

/// Display `value` as `presentation` says, reporting each quantity leaf whose
/// display failed; such a leaf keeps its SI value.
///
/// The presentation is read in the value's shape: a leaf presentation applies
/// to the leaves of its kind (a unit to quantity leaves, a time zone to
/// datetime leaves), and a field or an entry it does not describe stays plain.
pub(super) fn attach_presentation(
    value: &mut Value,
    presentation: Option<&ResolvedPresentation>,
) -> Vec<LeafPresentationDiagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(presentation) = presentation {
        attach(value, presentation, &[], &mut diagnostics);
    }
    diagnostics
}

fn attach(
    value: &mut Value,
    presentation: &ResolvedPresentation,
    path: &[PresentationPathPart],
    diagnostics: &mut Vec<LeafPresentationDiagnostic>,
) {
    match (presentation, value) {
        (Presentation::Plain, _) => {}
        (_, Value::Struct { fields, .. }) => {
            for (name, field) in fields {
                if let Some(presentation) = presentation.field_ref(name) {
                    let path = [path, &[PresentationPathPart::Field(name.clone())]].concat();
                    attach(field, presentation, &path, diagnostics);
                }
            }
        }
        (_, Value::Indexed { entries, .. }) => {
            for (key, entry) in entries {
                if let Some(presentation) = presentation.entry_ref(key) {
                    let path = [path, &[PresentationPathPart::Index(key.clone())]].concat();
                    attach(entry, presentation, &path, diagnostics);
                }
            }
        }
        (Presentation::Uniform(leaf), value) => attach_leaf(value, leaf, path, diagnostics),
        // A container's presentation presents no scalar leaf.
        (Presentation::Struct(_) | Presentation::Indexed(_), _) => {}
    }
}

fn attach_leaf(
    value: &mut Value,
    leaf: &ResolvedLeaf,
    path: &[PresentationPathPart],
    diagnostics: &mut Vec<LeafPresentationDiagnostic>,
) {
    match (leaf, value) {
        (ResolvedLeaf::Failed(failure), Value::Quantity { .. } | Value::Complex { .. }) => {
            diagnostics.push(LeafPresentationDiagnostic {
                path: path.to_vec(),
                failure: failure.clone(),
            });
        }
        (
            ResolvedLeaf::Unit { label, scale },
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
        }
        (ResolvedLeaf::Timezone(timezone), Value::Datetime { display_tz, .. }) => {
            *display_tz = Some(timezone.clone());
        }
        // A unit presents quantity leaves and a time zone datetime leaves.
        (ResolvedLeaf::Failed(_) | ResolvedLeaf::Unit { .. } | ResolvedLeaf::Timezone(_), _) => {}
    }
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
