//! Pure projection of evaluated presentation evidence. No interpreter, owner
//! lookup, host capability, or invocation environment is available here.

use super::types::{DisplayUnit, Value, validate_display_projection};
use crate::presentation_evidence::{
    LeafPresentationDiagnostic, PresentationFailure, PresentationInstance, PresentationPathPart,
};
use graphcal_compiler::registry::format::format_number;
use thiserror::Error;

#[derive(Debug, Error)]
pub(super) enum PresentationProjectionInvariant {
    #[error("unresolved presentation computation reached final projection")]
    Pending,
    #[error("presentation evidence does not match its checked value shape")]
    Shape,
    #[error("presentation field or index is absent from its checked value")]
    Missing,
    #[error("validated presentation scale was rejected")]
    Scale,
}

pub(super) fn attach_presentation(
    value: &mut Value,
    evidence: Option<&PresentationInstance>,
) -> Result<Vec<LeafPresentationDiagnostic>, PresentationProjectionInvariant> {
    let mut diagnostics = Vec::new();
    if let Some(evidence) = evidence {
        attach(value, evidence, &[], &mut diagnostics)?;
    }
    Ok(diagnostics)
}

fn attach(
    value: &mut Value,
    evidence: &PresentationInstance,
    path: &[PresentationPathPart],
    diagnostics: &mut Vec<LeafPresentationDiagnostic>,
) -> Result<(), PresentationProjectionInvariant> {
    match (evidence, value) {
        (PresentationInstance::None, _) => Ok(()),
        (PresentationInstance::Pending(_), _) => Err(PresentationProjectionInvariant::Pending),
        (PresentationInstance::Struct { fields: evidence }, Value::Struct { fields, .. }) => {
            evidence.iter().try_for_each(|(key, evidence)| {
                let value = fields
                    .get_mut(key)
                    .ok_or(PresentationProjectionInvariant::Missing)?;
                let path = [path, &[PresentationPathPart::Field(key.clone())]].concat();
                attach(value, evidence, &path, diagnostics)
            })
        }
        (PresentationInstance::Indexed { entries: evidence }, Value::Indexed { entries, .. }) => {
            evidence.iter().try_for_each(|(key, evidence)| {
                let value = entries
                    .get_mut(key)
                    .ok_or(PresentationProjectionInvariant::Missing)?;
                let path = [path, &[PresentationPathPart::Index(key.clone())]].concat();
                attach(value, evidence, &path, diagnostics)
            })
        }
        (
            PresentationInstance::Unit { .. }
            | PresentationInstance::Timezone(_)
            | PresentationInstance::Failed(_),
            Value::Indexed { entries, .. },
        ) => entries.iter_mut().try_for_each(|(key, value)| {
            let path = [path, &[PresentationPathPart::Index(key.clone())]].concat();
            attach(value, evidence, &path, diagnostics)
        }),
        (PresentationInstance::Failed(failure), Value::Quantity { .. } | Value::Complex { .. }) => {
            diagnostics.push(LeafPresentationDiagnostic {
                path: path.to_vec(),
                failure: failure.clone(),
            });
            Ok(())
        }
        (
            PresentationInstance::Unit { label, scale },
            value @ (Value::Quantity { .. } | Value::Complex { .. }),
        ) => {
            let unit = DisplayUnit::try_new(label.clone(), scale.get())
                .map_err(|_| PresentationProjectionInvariant::Scale)?;
            set_display_unit(value, Some(unit))?;
            if let Err(error) = validate_display_projection(value) {
                set_display_unit(value, None)?;
                diagnostics.push(LeafPresentationDiagnostic {
                    path: path.to_vec(),
                    failure: PresentationFailure::Projection {
                        message: error.to_string(),
                    },
                });
            }
            Ok(())
        }
        (PresentationInstance::Timezone(timezone), Value::Datetime { display_tz, .. }) => {
            *display_tz = Some(timezone.clone());
            Ok(())
        }
        _ => Err(PresentationProjectionInvariant::Shape),
    }
}

fn set_display_unit(
    value: &mut Value,
    unit: Option<DisplayUnit>,
) -> Result<(), PresentationProjectionInvariant> {
    match value {
        Value::Quantity { display_unit, .. } | Value::Complex { display_unit, .. } => {
            *display_unit = unit;
            Ok(())
        }
        _ => Err(PresentationProjectionInvariant::Shape),
    }
}

fn format_coordinate_impl(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    position: usize,
    exact: bool,
) -> String {
    idx_def.coordinate_data().map_or_else(
        || format!("#{position}"),
        |data| {
            let display_value = data.coordinate_value(position) / data.display_scale;
            let formatted = if exact {
                display_value.to_string()
            } else {
                format_number(display_value)
            };
            match &data.display_label {
                Some(label) => format!("{formatted} {label}"),
                None => formatted,
            }
        },
    )
}

pub(super) fn format_coordinate(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    position: usize,
) -> String {
    format_coordinate_impl(idx_def, position, false)
}
pub(super) fn format_coordinate_exact(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    position: usize,
) -> String {
    format_coordinate_impl(idx_def, position, true)
}
