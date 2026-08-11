//! Checked projection from semantic runtime values to public output values.

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{FiniteIndex, IndexDef};
use graphcal_compiler::syntax::index_name::IndexEntryKey;

use super::display::{format_coordinate, format_coordinate_exact};
use super::types::{DisplayUnit, Value};

/// Atomic runtime/public projection input: semantic value plus its checked type.
#[derive(Debug, Clone, Copy)]
pub(super) struct EvaluatedValue<'a> {
    runtime: &'a RuntimeValue,
    declared_type: &'a DeclaredType,
}

impl<'a> EvaluatedValue<'a> {
    #[must_use]
    pub const fn new(runtime: &'a RuntimeValue, declared_type: &'a DeclaredType) -> Self {
        Self {
            runtime,
            declared_type,
        }
    }

    /// Project this checked pair into the public value model.
    pub fn project(
        self,
        tir: &graphcal_compiler::tir::typed::TIR,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Value, GraphcalError> {
        project_runtime_value(self.runtime, self.declared_type, tir, src)
    }
}

fn projection_error(
    runtime: &RuntimeValue,
    declared_type: &DeclaredType,
    message: impl Into<String>,
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::internal_error(
        format!(
            "runtime/public projection invariant failed for {} as `{}`: {}",
            runtime.kind(),
            declared_type.format(&tir.registry().dimensions),
            message.into()
        ),
        src,
        DiagnosticAnchor::WholeFile,
    )
}

#[derive(Debug, Clone, Copy)]
enum ProjectionIndex<'a> {
    Declared(&'a IndexDef),
    Finite(FiniteIndex),
}

impl<'a> ProjectionIndex<'a> {
    fn entry_keys(self) -> Vec<IndexEntryKey> {
        match self {
            Self::Declared(definition) => definition.entry_keys(),
            Self::Finite(finite) => (0..finite.cardinality().get())
                .map(|position| IndexEntryKey::position(position as u64))
                .collect(),
        }
    }

    const fn declared_definition(self) -> Option<&'a IndexDef> {
        match self {
            Self::Declared(definition) => Some(definition),
            Self::Finite(_) => None,
        }
    }
}

fn projection_index_for_ref<'a>(
    index: &IndexTypeRef,
    tir: &'a graphcal_compiler::tir::typed::TIR,
) -> Option<ProjectionIndex<'a>> {
    match index.finite_index() {
        Some(finite) => Some(ProjectionIndex::Finite(finite)),
        None => tir
            .declared_index_def(index.declared_resolved()?)
            .map(ProjectionIndex::Declared),
    }
}

fn require_matching_index<'a>(
    runtime: &RuntimeValue,
    runtime_index: &IndexTypeRef,
    declared_type: &DeclaredType,
    declared_index: &IndexTypeRef,
    tir: &'a graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<ProjectionIndex<'a>, GraphcalError> {
    if !runtime_index.matches_ref(declared_index) {
        return Err(projection_error(
            runtime,
            declared_type,
            format!(
                "runtime index `{runtime_index}` does not match checked index `{declared_index}`"
            ),
            tir,
            src,
        ));
    }
    projection_index_for_ref(runtime_index, tir).ok_or_else(|| {
        projection_error(
            runtime,
            declared_type,
            format!("checked index `{runtime_index}` has no concrete definition"),
            tir,
            src,
        )
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive runtime/declared-type pairing keeps every public projection invariant visible"
)]
fn project_runtime_value(
    runtime: &RuntimeValue,
    declared_type: &DeclaredType,
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Value, GraphcalError> {
    match (runtime, declared_type) {
        (RuntimeValue::Quantity(si_value), DeclaredType::Quantity(dimension)) => {
            if !si_value.is_finite() {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    "runtime quantity is not finite",
                    tir,
                    src,
                ));
            }
            Ok(Value::Quantity {
                si_value: *si_value,
                dimension: dimension.clone(),
                display_unit: None,
            })
        }
        (RuntimeValue::Complex(si_value), DeclaredType::Complex(dimension)) => {
            if !si_value.re().is_finite() || !si_value.im().is_finite() {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    "runtime complex quantity is not finite",
                    tir,
                    src,
                ));
            }
            Ok(Value::Complex {
                si_value: *si_value,
                dimension: dimension.clone(),
                display_unit: None,
            })
        }
        (RuntimeValue::Bool(value), DeclaredType::Bool) => Ok(Value::Bool(*value)),
        (RuntimeValue::Int(value), DeclaredType::Int) => Ok(Value::Int(*value)),
        (RuntimeValue::Int(position), DeclaredType::Key(index)) => {
            let finite = index.finite_index().ok_or_else(|| {
                projection_error(
                    runtime,
                    declared_type,
                    "integer key carrier belongs to a non-finite index",
                    tir,
                    src,
                )
            })?;
            let public_position = *position;
            let position = usize::try_from(public_position).map_err(|_| {
                projection_error(
                    runtime,
                    declared_type,
                    "finite key position is negative or exceeds the platform index range",
                    tir,
                    src,
                )
            })?;
            if position >= finite.cardinality().get() {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    "finite key position is outside its checked index",
                    tir,
                    src,
                ));
            }
            Ok(Value::Int(public_position))
        }
        (
            RuntimeValue::Label {
                index_name,
                variant,
            },
            DeclaredType::Key(declared_index),
        ) => {
            let definition = require_matching_index(
                runtime,
                index_name,
                declared_type,
                declared_index,
                tir,
                src,
            )?;
            let key = graphcal_compiler::syntax::index_name::IndexEntryKey::named(variant.clone());
            if !definition.entry_keys().contains(&key) {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    format!("runtime label `{variant}` is absent from index `{index_name}`"),
                    tir,
                    src,
                ));
            }
            Ok(Value::Label {
                index_name: index_name.clone(),
                variant: variant.clone(),
            })
        }
        (
            RuntimeValue::Struct {
                type_name,
                generic_args: runtime_args,
                fields,
            },
            DeclaredType::Struct(declared_identity, declared_args),
        ) => {
            if type_name.resolved() != declared_identity.resolved() || runtime_args != declared_args
            {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    format!(
                        "runtime nominal identity `{type_name}` or its generic arguments do not match the checked type"
                    ),
                    tir,
                    src,
                ));
            }
            let model = graphcal_compiler::tir::dim_check::ConcreteModelType::try_new(
                tir,
                declared_identity,
                declared_args,
                src,
            )
            .map_err(|error| {
                projection_error(runtime, declared_type, error.to_string(), tir, src)
            })?;
            let constructors = model.constructors(src).map_err(|error| {
                projection_error(runtime, declared_type, error.to_string(), tir, src)
            })?;
            let constructor = constructors
                .into_iter()
                .find(|constructor| constructor.name().atom() == type_name.name().atom())
                .ok_or_else(|| {
                    projection_error(
                        runtime,
                        declared_type,
                        format!(
                            "runtime constructor `{type_name}` is absent from its checked nominal type"
                        ),
                        tir,
                        src,
                    )
                })?;
            if fields.len() != constructor.fields().len() {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    "runtime struct field count does not match its checked constructor",
                    tir,
                    src,
                ));
            }
            let projected_fields = constructor
                .fields()
                .iter()
                .map(|field| {
                    let field_runtime = fields.get(field.name()).ok_or_else(|| {
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
                type_name: type_name.clone(),
                fields: projected_fields,
            })
        }
        (
            RuntimeValue::Indexed {
                index_name,
                entries,
            },
            DeclaredType::Indexed {
                element,
                index: declared_index,
            },
        ) => {
            let projection_index = require_matching_index(
                runtime,
                index_name,
                declared_type,
                declared_index,
                tir,
                src,
            )?;
            let expected_keys = projection_index.entry_keys();
            if entries.len() != expected_keys.len()
                || entries
                    .keys()
                    .zip(&expected_keys)
                    .any(|(actual, expected)| actual != expected)
            {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    "runtime indexed keys or their order do not match the checked index",
                    tir,
                    src,
                ));
            }
            let entry_display_names = projection_index
                .declared_definition()
                .and_then(|definition| coordinate_entry_display_names(definition, entries));
            let projected_entries = entries
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
        (
            RuntimeValue::CoordinateLabel {
                index_name,
                position,
                value,
            },
            DeclaredType::Quantity(_) | DeclaredType::Key(_),
        ) => {
            let projection_index = match declared_type {
                DeclaredType::Key(declared_index) => require_matching_index(
                    runtime,
                    index_name,
                    declared_type,
                    declared_index,
                    tir,
                    src,
                )?,
                DeclaredType::Quantity(_) => {
                    projection_index_for_ref(index_name, tir).ok_or_else(|| {
                        projection_error(
                            runtime,
                            declared_type,
                            format!("coordinate index `{index_name}` has no definition"),
                            tir,
                            src,
                        )
                    })?
                }
                _ => {
                    return Err(projection_error(
                        runtime,
                        declared_type,
                        "coordinate carrier has an unsupported checked type",
                        tir,
                        src,
                    ));
                }
            };
            let definition = projection_index.declared_definition().ok_or_else(|| {
                projection_error(
                    runtime,
                    declared_type,
                    "runtime coordinate label uses a structural finite index",
                    tir,
                    src,
                )
            })?;
            let data = definition.coordinate_data().ok_or_else(|| {
                projection_error(
                    runtime,
                    declared_type,
                    format!("runtime coordinate label uses non-coordinate index `{index_name}`"),
                    tir,
                    src,
                )
            })?;
            let position_out_of_bounds = *position >= data.cardinality();
            #[expect(
                clippy::float_cmp,
                reason = "coordinate runtime values are copied exactly from the checked axis"
            )]
            let coordinate_mismatch =
                !position_out_of_bounds && data.coordinate_value(*position) != *value;
            if position_out_of_bounds || coordinate_mismatch {
                return Err(projection_error(
                    runtime,
                    declared_type,
                    "runtime coordinate position/value does not match its checked index",
                    tir,
                    src,
                ));
            }
            let display_unit = data
                .display_label
                .as_ref()
                .map(|label| DisplayUnit::try_new(label.clone(), data.display_scale))
                .transpose()
                .map_err(|error| {
                    projection_error(runtime, declared_type, error.to_string(), tir, src)
                })?;
            let dimension = match declared_type {
                DeclaredType::Quantity(dimension) if dimension == &data.dimension => {
                    dimension.clone()
                }
                DeclaredType::Quantity(_) => {
                    return Err(projection_error(
                        runtime,
                        declared_type,
                        "checked quantity dimension does not match the coordinate index",
                        tir,
                        src,
                    ));
                }
                DeclaredType::Key(_) => data.dimension.clone(),
                _ => {
                    return Err(projection_error(
                        runtime,
                        declared_type,
                        "coordinate carrier has an unsupported checked type",
                        tir,
                        src,
                    ));
                }
            };
            Ok(Value::Quantity {
                si_value: *value,
                dimension,
                display_unit,
            })
        }
        (RuntimeValue::Datetime(epoch), DeclaredType::Datetime(time_scale)) => {
            Ok(Value::Datetime {
                epoch: *epoch,
                time_scale: *time_scale,
                display_tz: None,
                time_zones: tir.registry().time_zones.clone(),
            })
        }
        _ => Err(projection_error(
            runtime,
            declared_type,
            "runtime variant does not match its checked declared type",
            tir,
            src,
        )),
    }
}

fn coordinate_entry_display_names(
    definition: &IndexDef,
    entries: &IndexMap<graphcal_compiler::syntax::index_name::IndexEntryKey, RuntimeValue>,
) -> Option<IndexMap<graphcal_compiler::syntax::index_name::IndexEntryKey, String>> {
    if !definition.is_coordinate() {
        return None;
    }
    let labels = entries
        .keys()
        .enumerate()
        .map(|(position, key)| {
            (
                key.clone(),
                position,
                format_coordinate(definition, position),
            )
        })
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let duplicates = labels
        .iter()
        .filter_map(|(_, _, label)| (!seen.insert(label.clone())).then_some(label.clone()))
        .collect::<HashSet<_>>();
    let mut used_display_names = HashSet::new();
    Some(
        labels
            .into_iter()
            .map(|(key, position, label)| {
                let candidate = if duplicates.contains(&label) {
                    format_coordinate_exact(definition, position)
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
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_checked_type_is_an_internal_projection_error() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let runtime = RuntimeValue::Quantity(1.0);
        let error = EvaluatedValue::new(&runtime, &DeclaredType::Bool)
            .project(&tir, &src)
            .unwrap_err();

        assert!(matches!(error, GraphcalError::InternalError { .. }));
    }

    #[test]
    fn checked_quantity_dimension_is_required_and_preserved() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let runtime = RuntimeValue::Quantity(1.0);
        let declared =
            DeclaredType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());
        let projected = EvaluatedValue::new(&runtime, &declared)
            .project(&tir, &src)
            .unwrap();

        assert!(matches!(
            projected,
            Value::Quantity { dimension, .. } if dimension.is_dimensionless()
        ));
    }

    #[test]
    fn projection_rejects_non_finite_runtime_quantities() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let declared =
            DeclaredType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());
        let runtime = RuntimeValue::Quantity(f64::INFINITY);

        assert!(matches!(
            EvaluatedValue::new(&runtime, &declared).project(&tir, &src),
            Err(GraphcalError::InternalError { .. })
        ));
    }

    #[test]
    fn coordinate_carrier_requires_the_axis_dimension() {
        let source = "index Step = range(0.0 s, 1.0 s, step: 1.0 s);";
        let tir = crate::eval::compile_to_tir(source, "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(source.to_string()));
        let index = IndexTypeRef::with_owner(
            tir.root_dag_id().clone(),
            graphcal_compiler::syntax::index_name::IndexName::expect_valid("Step"),
        );
        let runtime = RuntimeValue::CoordinateLabel {
            index_name: index,
            position: 0,
            value: 0.0,
        };
        let declared =
            DeclaredType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());

        assert!(matches!(
            EvaluatedValue::new(&runtime, &declared).project(&tir, &src),
            Err(GraphcalError::InternalError { .. })
        ));
    }

    #[test]
    fn structural_finite_projection_does_not_require_a_registry_entry() {
        let tir = crate::eval::compile_to_tir("", "projection.gcl").unwrap();
        let src = NamedSource::new("projection.gcl", Arc::new(String::new()));
        let index = IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(2).unwrap());
        let element =
            DeclaredType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());
        let declared = DeclaredType::Indexed {
            element: Box::new(element),
            index: index.clone(),
        };
        let runtime = RuntimeValue::Indexed {
            index_name: index.clone(),
            entries: IndexMap::from([
                (IndexEntryKey::position(0), RuntimeValue::Quantity(1.0)),
                (IndexEntryKey::position(1), RuntimeValue::Quantity(2.0)),
            ]),
        };

        let projected = EvaluatedValue::new(&runtime, &declared)
            .project(&tir, &src)
            .unwrap();
        assert!(matches!(
            projected,
            Value::Indexed { entries, .. } if entries.len() == 2
        ));

        let key_runtime = RuntimeValue::Int(1);
        let key_declared = DeclaredType::Key(index);
        assert!(matches!(
            EvaluatedValue::new(&key_runtime, &key_declared)
                .project(&tir, &src)
                .unwrap(),
            Value::Int(1)
        ));
    }
}
