//! Checked projection from semantic runtime values to public output values.

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use crate::runtime_value::RuntimeValue;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::checked_type::CheckedType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::index::CoordinateIndexData;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;

use super::display::{format_coordinate, format_coordinate_exact};
use super::types::Value;

/// Atomic runtime/public projection input: semantic value plus its checked type.
#[derive(Debug, Clone, Copy)]
pub(super) struct EvaluatedValue<'a> {
    runtime: &'a RuntimeValue,
    declared_type: &'a CheckedType,
}

impl<'a> EvaluatedValue<'a> {
    #[must_use]
    pub const fn new(runtime: &'a RuntimeValue, declared_type: &'a CheckedType) -> Self {
        Self {
            runtime,
            declared_type,
        }
    }

    /// Project this checked pair into the public value model.
    pub fn project(
        self,
        tir: &graphcal_compiler::tir::typed::CheckedTir,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Value, GraphcalError> {
        project_runtime_value(self.runtime, self.declared_type, tir, src)
    }
}

fn projection_error(
    runtime: &RuntimeValue,
    declared_type: &CheckedType,
    message: impl Into<String>,
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::internal_error(
        format!(
            "runtime/public projection invariant failed for {} as `{}`: {}",
            runtime.describe(),
            declared_type.format(&tir.registry().dimensions),
            message.into()
        ),
        src,
        DiagnosticAnchor::WholeFile,
    )
}

/// Pair a runtime value with its checked type and project it.
///
/// Keys and indexed values carry their own axes, so only two checks remain:
/// the runtime variant must match the checked type (runtime values do not
/// carry dimensions, time scales, or field types, so the checked type is
/// walked alongside), and a struct's constructor must expand to its checked
/// field types through the TIR.
fn project_runtime_value(
    runtime: &RuntimeValue,
    declared_type: &CheckedType,
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<Value, GraphcalError> {
    match (runtime, declared_type) {
        (RuntimeValue::Quantity(si_value), CheckedType::Quantity(dimension)) => {
            Ok(Value::Quantity {
                si_value: si_value.get(),
                dimension: dimension.clone(),
                display_unit: None,
            })
        }
        (RuntimeValue::Complex(si_value), CheckedType::Complex(dimension)) => Ok(Value::Complex {
            si_value: *si_value,
            dimension: dimension.clone(),
            display_unit: None,
        }),
        (RuntimeValue::Bool(value), CheckedType::Bool) => Ok(Value::Bool(*value)),
        (RuntimeValue::Int(value), CheckedType::Int) => Ok(Value::Int(*value)),
        (RuntimeValue::Key(key), CheckedType::Key(_)) => Ok(Value::Key(key.clone())),
        (RuntimeValue::Struct(value), CheckedType::Struct(declared_identity, declared_args)) => {
            project_struct(
                runtime,
                value,
                declared_type,
                declared_identity,
                declared_args,
                tir,
                src,
            )
        }
        (RuntimeValue::Indexed(indexed), CheckedType::Indexed { element, .. }) => {
            // The entry keys are the axis's own keys by construction.
            let index_name = indexed.index();
            let entry_display_names = indexed
                .axis()
                .coordinate_data()
                .map(|data| coordinate_entry_display_names(data, indexed.axis().keys()));
            let projected_entries = indexed
                .iter()
                .map(|(key, entry)| {
                    EvaluatedValue::new(entry, element)
                        .project(tir, src)
                        .map(|value| (key.clone(), value))
                })
                .collect::<Result<IndexMap<_, _>, _>>()?;
            Ok(Value::Indexed {
                index_name: index_name.clone(),
                entries: projected_entries,
                entry_display_names,
            })
        }
        (RuntimeValue::Datetime(epoch), CheckedType::Datetime(time_scale)) => Ok(Value::Datetime {
            epoch: *epoch,
            time_scale: *time_scale,
            display_tz: None,
        }),
        _ => Err(projection_error(
            runtime,
            declared_type,
            "runtime variant does not match its checked declared type",
            tir,
            src,
        )),
    }
}

/// Project a struct value field by field through its constructor's checked
/// field types.
fn project_struct(
    runtime: &RuntimeValue,
    value: &crate::runtime_value::StructValue<RuntimeValue>,
    declared_type: &CheckedType,
    declared_identity: &graphcal_compiler::registry::checked_type::StructTypeRef,
    declared_args: &[graphcal_compiler::registry::checked_type::CheckedGenericArg],
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<Value, GraphcalError> {
    let type_name = value.type_name();
    let runtime_constructor = value.constructor();
    let runtime_args = value.generic_args();
    if type_name != declared_identity.resolved() || runtime_args != declared_args {
        return Err(projection_error(
            runtime,
            declared_type,
            format!(
                "runtime nominal identity `{:?}` or its generic arguments do not match checked identity `{:?}`",
                type_name,
                declared_identity.resolved()
            ),
            tir,
            src,
        ));
    }
    // Constructor lookup is a correctness boundary: losing it would
    // discard each field's checked type and could render a quantity in
    // the wrong unit. Any inconsistency must fail the projection.
    let model = graphcal_compiler::tir::dim_check::ConcreteModelType::try_new(
        tir,
        declared_identity,
        declared_args,
        src,
    )
    .map_err(|error| projection_error(runtime, declared_type, error.to_string(), tir, src))?;
    let constructors = model
        .constructors(src)
        .map_err(|error| projection_error(runtime, declared_type, error.to_string(), tir, src))?;
    let constructor = constructors
        .into_iter()
        .find(|constructor| constructor.name() == runtime_constructor)
        .ok_or_else(|| {
            projection_error(
                runtime,
                declared_type,
                format!(
                    "runtime constructor `{runtime_constructor}` is absent from its checked nominal type"
                ),
                tir,
                src,
            )
        })?;
    // A struct value holds exactly its constructor's declared fields,
    // so each checked field has its runtime value.
    let projected_fields = constructor
        .fields()
        .iter()
        .map(|field| {
            let field_runtime = value.field(field.name()).ok_or_else(|| {
                projection_error(
                    runtime,
                    declared_type,
                    format!("runtime struct is missing field `{}`", field.name()),
                    tir,
                    src,
                )
            })?;
            EvaluatedValue::new(field_runtime, field.declared_type())
                .project(tir, src)
                .map(|value| (field.name().clone(), value))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;
    Ok(Value::Struct {
        type_name: declared_identity.clone(),
        constructor: runtime_constructor.clone(),
        generic_args: runtime_args.to_vec(),
        fields: projected_fields,
    })
}

fn coordinate_entry_display_names(
    data: &CoordinateIndexData,
    keys: &NonEmpty<IndexEntryKey>,
) -> IndexMap<IndexEntryKey, String> {
    let labels = keys
        .iter()
        .enumerate()
        .map(|(position, key)| (key.clone(), position, format_coordinate(data, position)))
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let duplicates = labels
        .iter()
        .filter_map(|(_, _, label)| (!seen.insert(label.clone())).then_some(label.clone()))
        .collect::<HashSet<_>>();
    let mut used_display_names = HashSet::new();
    labels
        .into_iter()
        .map(|(key, position, label)| {
            let candidate = if duplicates.contains(&label) {
                format_coordinate_exact(data, position)
            } else {
                label
            };
            let display = if used_display_names.insert(candidate.clone()) {
                candidate
            } else {
                format!("{candidate} [#{position}]")
            };
            (key, display)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_checked_type_is_an_internal_projection_error() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let runtime = RuntimeValue::quantity(1.0).unwrap();
        let error = EvaluatedValue::new(&runtime, &CheckedType::Bool)
            .project(&tir, &src)
            .unwrap_err();

        assert!(matches!(error, GraphcalError::InternalError { .. }));
    }

    #[test]
    fn checked_quantity_dimension_is_required_and_preserved() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let runtime = RuntimeValue::quantity(1.0).unwrap();
        let declared =
            CheckedType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());
        let projected = EvaluatedValue::new(&runtime, &declared)
            .project(&tir, &src)
            .unwrap();

        assert!(matches!(
            projected,
            Value::Quantity { dimension, .. } if dimension.is_dimensionless()
        ));
    }

    #[test]
    fn runtime_ingress_rejects_non_finite_quantities_before_projection() {
        assert!(RuntimeValue::quantity(f64::INFINITY).is_err());
    }

    #[test]
    fn structural_finite_projection_does_not_require_a_registry_entry() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let finite = graphcal_compiler::registry::types::FiniteIndex::try_from_u64(2).unwrap();
        let index =
            graphcal_compiler::registry::checked_type::IndexTypeRef::from_finite_index(finite);
        let element =
            CheckedType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());
        let declared = CheckedType::Indexed {
            element: Box::new(element),
            index: index.clone(),
        };
        let runtime =
            RuntimeValue::Indexed(crate::runtime_value::IndexedValue::finite_for_test(vec![
                RuntimeValue::quantity(1.0).unwrap(),
                RuntimeValue::quantity(2.0).unwrap(),
            ]));

        let projected = EvaluatedValue::new(&runtime, &declared)
            .project(&tir, &src)
            .unwrap();
        assert!(matches!(
            projected,
            Value::Indexed { entries, .. } if entries.len() == 2
        ));

        let key = crate::runtime_value::KeyValue::at(
            crate::runtime_value::IndexAxis::finite(finite).unwrap(),
            1,
        )
        .unwrap();
        let key_runtime = RuntimeValue::Key(key.clone());
        let key_declared = CheckedType::Key(index);
        assert_eq!(
            EvaluatedValue::new(&key_runtime, &key_declared)
                .project(&tir, &src)
                .unwrap(),
            Value::Key(key)
        );
    }
}
