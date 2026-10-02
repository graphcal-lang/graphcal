//! Projection of presented runtime values to public output values.
//!
//! A value and its presentation are one tree ([`Presented`]), so the public
//! value and its display units are produced in one walk, and a leaf whose
//! display fails is reported beside the value, keeping its SI value.
//!
//! Struct values carry their constructor's instantiated field types, and
//! keys and indexed values their axes, so no definition is looked up here.
//! Runtime quantities, complex values, and datetimes carry no dimension or
//! time scale, however, so the checked type is walked alongside the value to
//! supply them (and an indexed value's element type, down to its leaves).
//! That pairing is the only check left: a runtime variant that differs from
//! its checked type is a violated [`Invariant`] of the evaluator.
//!
//! [`Presented`]: crate::runtime_presentation::Presented

use std::collections::HashSet;

use indexmap::IndexMap;

use graphcal_compiler::display::number::format_number;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::semantic::index_def::CoordinateIndexData;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::type_name::FieldName;

use super::types::{DisplayUnit, Value, validate_display_projection};
use crate::invariant::Invariant;
use crate::presentation_evidence::{
    LeafPresentationDiagnostic, PresentationFailure, PresentationPathPart, QuantityDisplay,
    ResolvedLeaf,
};
use crate::runtime_presentation::{PresentedRef, PresentedView};
use crate::runtime_value::{IndexedValue, RuntimeValue, StructValue};

/// Project `value`, of `declared_type`, to the public value model, displayed
/// as presented, with the display failure of each leaf that keeps its SI
/// value.
///
/// # Errors
///
/// Returns an [`Invariant`] when a runtime variant differs from its checked
/// type.
/// The value of a declaration checked as `Bool`.
///
/// # Errors
///
/// Returns an [`Invariant`] when the value is not a `Bool`.
pub fn project_bool(runtime: &RuntimeValue) -> Result<bool, Invariant> {
    match runtime {
        RuntimeValue::Bool(value) => Ok(*value),
        other => Err(mismatch(other.describe(), &CheckedType::Bool)),
    }
}

/// Project `value`, checked as `declared_type`, to its public value.
///
/// # Errors
///
/// Returns an [`Invariant`] when the value contradicts its checked type.
pub fn project(
    value: PresentedRef<'_, ResolvedLeaf>,
    declared_type: &CheckedType,
) -> Result<(Value, Vec<LeafPresentationDiagnostic>), Invariant> {
    let mut projection = Projection::default();
    let projected = projection.presented(value, declared_type, &Path::Root)?;
    Ok((projected, projection.diagnostics))
}

/// Where a leaf sits in the projected value; materialized only for a
/// diagnostic.
#[derive(Clone, Copy)]
enum Path<'a> {
    Root,
    Field(&'a Self, &'a FieldName),
    Index(&'a Self, &'a IndexEntryKey),
}

impl Path<'_> {
    fn parts(&self) -> Vec<PresentationPathPart> {
        let mut parts = Vec::new();
        let mut path = self;
        loop {
            match path {
                Path::Root => break,
                Path::Field(parent, field) => {
                    parts.push(PresentationPathPart::Field((*field).clone()));
                    path = parent;
                }
                Path::Index(parent, key) => {
                    parts.push(PresentationPathPart::Index((*key).clone()));
                    path = parent;
                }
            }
        }
        parts.reverse();
        parts
    }
}

#[derive(Default)]
struct Projection {
    diagnostics: Vec<LeafPresentationDiagnostic>,
}

impl Projection {
    fn presented(
        &mut self,
        value: PresentedRef<'_, ResolvedLeaf>,
        declared_type: &CheckedType,
        path: &Path<'_>,
    ) -> Result<Value, Invariant> {
        match value.view() {
            PresentedView::Whole { value, leaf } => self.whole(value, leaf, declared_type, path),
            PresentedView::Struct(fields) => {
                project_struct(fields, declared_type, path, |field, field_type, path| {
                    self.presented(field.as_ref(), field_type, path)
                })
            }
            PresentedView::Indexed(entries) => {
                project_indexed(entries, declared_type, path, |entry, element, path| {
                    self.presented(entry.as_ref(), element, path)
                })
            }
        }
    }

    /// Project `value`, every leaf of which `leaf` presents, or none.
    /// [`Presented`](crate::runtime_presentation::Presented) admits a leaf
    /// only over leaves of its kind, so a quantity display reaches only
    /// quantity and complex leaves, and a time zone only datetimes.
    fn whole(
        &mut self,
        value: &RuntimeValue,
        leaf: Option<&ResolvedLeaf>,
        declared_type: &CheckedType,
        path: &Path<'_>,
    ) -> Result<Value, Invariant> {
        match (value, declared_type) {
            (RuntimeValue::Quantity(si_value), CheckedType::Quantity(dimension)) => Ok(self
                .quantity(leaf, path, |display_unit| Value::Quantity {
                    si_value: *si_value,
                    dimension: dimension.clone(),
                    display_unit,
                })),
            (RuntimeValue::Complex(si_value), CheckedType::Complex(dimension)) => Ok(self
                .quantity(leaf, path, |display_unit| Value::Complex {
                    si_value: *si_value,
                    dimension: dimension.clone(),
                    display_unit,
                })),
            (RuntimeValue::Datetime(epoch), CheckedType::Datetime(time_scale)) => {
                Ok(Value::Datetime {
                    epoch: *epoch,
                    time_scale: *time_scale,
                    display_tz: match leaf {
                        Some(ResolvedLeaf::Datetime(time_zone)) => Some(time_zone.clone()),
                        Some(ResolvedLeaf::Quantity(_)) | None => None,
                    },
                })
            }
            (RuntimeValue::Bool(value), CheckedType::Bool) => Ok(Value::Bool(*value)),
            (RuntimeValue::Int(value), CheckedType::Int) => Ok(Value::Int(*value)),
            (RuntimeValue::Key(key), CheckedType::Key(_)) => Ok(Value::Key(key.clone())),
            (RuntimeValue::Struct(fields), _) => {
                project_struct(fields, declared_type, path, |field, field_type, path| {
                    self.whole(field, leaf, field_type, path)
                })
            }
            (RuntimeValue::Indexed(entries), _) => {
                project_indexed(entries, declared_type, path, |entry, element, path| {
                    self.whole(entry, leaf, element, path)
                })
            }
            (
                RuntimeValue::Quantity(_)
                | RuntimeValue::Complex(_)
                | RuntimeValue::Datetime(_)
                | RuntimeValue::Bool(_)
                | RuntimeValue::Int(_)
                | RuntimeValue::Key(_),
                _,
            ) => Err(mismatch(value.describe(), declared_type)),
        }
    }

    /// The quantity leaf `build` makes, displayed in `leaf`'s unit; a leaf
    /// whose display fails keeps its SI value and is reported.
    fn quantity(
        &mut self,
        leaf: Option<&ResolvedLeaf>,
        path: &Path<'_>,
        build: impl Fn(Option<DisplayUnit>) -> Value,
    ) -> Value {
        let failure = match leaf {
            Some(ResolvedLeaf::Quantity(QuantityDisplay::Unit { label, scale })) => {
                let displayed = build(Some(DisplayUnit::new(label.clone(), *scale)));
                match validate_display_projection(&displayed) {
                    Ok(()) => return displayed,
                    Err(error) => PresentationFailure::Projection {
                        message: error.to_string(),
                    },
                }
            }
            Some(ResolvedLeaf::Quantity(QuantityDisplay::Failed(failure))) => failure.clone(),
            Some(ResolvedLeaf::Datetime(_)) | None => return build(None),
        };
        self.diagnostics.push(LeafPresentationDiagnostic {
            path: path.parts(),
            failure,
        });
        build(None)
    }
}

/// Project a struct value field by field, each at its instantiated type.
fn project_struct<V>(
    fields: &StructValue<V>,
    declared_type: &CheckedType,
    path: &Path<'_>,
    mut project_field: impl FnMut(&V, &CheckedType, &Path<'_>) -> Result<Value, Invariant>,
) -> Result<Value, Invariant> {
    // The value's nominal application must be the checked one.
    let (declared, declared_args) = match declared_type {
        CheckedType::Struct(declared, declared_args)
            if declared.resolved() == fields.type_name()
                && declared_args.as_slice() == fields.generic_args() =>
        {
            (declared, declared_args)
        }
        _ => {
            return Err(mismatch(
                format_args!(
                    "struct `{}` of `{:?}`",
                    fields.constructor(),
                    fields.type_name()
                ),
                declared_type,
            ));
        }
    };
    let projected = fields
        .typed_fields()
        .map(|(field, value)| {
            project_field(value, field.field_type(), &Path::Field(path, field.name()))
                .map(|projected| (field.name().clone(), projected))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;
    Ok(Value::Struct {
        type_name: declared.clone(),
        constructor: fields.constructor().clone(),
        generic_args: declared_args.clone(),
        fields: projected,
    })
}

/// Project an indexed value entry by entry, each at the element type.
fn project_indexed<V>(
    entries: &IndexedValue<V>,
    declared_type: &CheckedType,
    path: &Path<'_>,
    mut project_entry: impl FnMut(&V, &CheckedType, &Path<'_>) -> Result<Value, Invariant>,
) -> Result<Value, Invariant> {
    let CheckedType::Indexed { element, .. } = declared_type else {
        return Err(mismatch(
            format_args!("indexed value `{}[...]`", entries.index()),
            declared_type,
        ));
    };
    // The entry keys are the axis's own keys by construction.
    let axis = entries.axis();
    let entry_display_names = axis
        .coordinate_data()
        .map(|data| coordinate_entry_display_names(data, axis.keys()));
    let projected = entries
        .iter()
        .map(|(key, entry)| {
            project_entry(entry, element, &Path::Index(path, key))
                .map(|projected| (key.clone(), projected))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;
    Ok(Value::Indexed {
        index_name: entries.index().clone(),
        entries: projected,
        entry_display_names,
    })
}

/// A runtime value whose variant differs from its checked type.
fn mismatch(runtime: impl std::fmt::Display, declared_type: &CheckedType) -> Invariant {
    let declared = match declared_type {
        CheckedType::Quantity(_) => "a quantity",
        CheckedType::Complex(_) => "a complex value",
        CheckedType::Bool => "a Bool",
        CheckedType::Int => "an Int",
        CheckedType::Datetime(_) => "a datetime",
        CheckedType::Key(_) => "a key",
        CheckedType::Struct(..) => "a struct value",
        CheckedType::Indexed { .. } => "an indexed value",
    };
    Invariant::violated(format_args!("runtime {runtime} was checked as {declared}"))
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

fn format_coordinate(data: &CoordinateIndexData, position: usize) -> String {
    format_coordinate_impl(data, position, false)
}

fn format_coordinate_exact(data: &CoordinateIndexData, position: usize) -> String {
    format_coordinate_impl(data, position, true)
}

#[cfg(test)]
mod tests {
    use super::project_bool;
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::dimension::{BaseDimId, Dimension, PreludeBaseDimension};
    use graphcal_compiler::resolved_name::ResolvedStructTypeName;
    use graphcal_compiler::semantic::checked_type::{CheckedType, IndexTypeRef, StructTypeRef};
    use graphcal_compiler::semantic::index_def::FiniteIndex;
    use graphcal_compiler::semantic::unit_scale::PositiveFiniteScale;
    use graphcal_compiler::syntax::index_name::IndexEntryKey;
    use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    use super::project;
    use crate::eval::types::Value;
    use crate::presentation_evidence::{
        LeafPresentationDiagnostic, PresentationFailure, PresentationPathPart, QuantityDisplay,
        ResolvedLeaf,
    };
    use crate::runtime_presentation::{Presented, PresentedRef, ResolvedValue};
    use crate::runtime_value::{IndexAxis, IndexedValue, KeyValue, RuntimeValue, StructValue};

    fn quantity(value: f64) -> RuntimeValue {
        RuntimeValue::quantity(value).unwrap()
    }

    fn length() -> Dimension {
        Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length))
    }

    fn field(name: &str) -> FieldName {
        FieldName::expect_valid(name)
    }

    fn pair_type() -> ResolvedStructTypeName {
        ResolvedStructTypeName::for_test(
            DagId::root_in_package("projection", "main"),
            StructTypeName::expect_valid("Pair"),
        )
    }

    fn pair_checked_type() -> CheckedType {
        CheckedType::Struct(StructTypeRef::from_resolved(pair_type()), Vec::new())
    }

    /// A `Pair` whose `left` field is a length and `right` field a Bool.
    fn pair<V>(left: V, right: V) -> StructValue<V> {
        StructValue::for_test(
            pair_type(),
            ConstructorName::expect_valid("Pair"),
            vec![
                (field("left"), CheckedType::Quantity(length()), left),
                (field("right"), CheckedType::Bool, right),
            ],
        )
    }

    fn km() -> ResolvedLeaf {
        ResolvedLeaf::Quantity(QuantityDisplay::Unit {
            label: "km".to_owned(),
            scale: PositiveFiniteScale::new(1000.0).unwrap(),
        })
    }

    fn plain(value: &RuntimeValue, declared: &CheckedType) -> Result<Value, String> {
        project(PresentedRef::plain(value), declared)
            .map(|(value, diagnostics)| {
                assert!(diagnostics.is_empty());
                value
            })
            .map_err(|invariant| invariant.to_string())
    }

    fn presented(
        value: &ResolvedValue,
        declared: &CheckedType,
    ) -> (Value, Vec<LeafPresentationDiagnostic>) {
        project(value.as_ref(), declared).unwrap()
    }

    fn display_label(value: &Value) -> Option<&str> {
        match value {
            Value::Quantity { display_unit, .. } | Value::Complex { display_unit, .. } => {
                display_unit.as_ref().map(|unit| unit.label.as_str())
            }
            _ => panic!("expected a quantity, got {value:?}"),
        }
    }

    #[test]
    fn bool_outputs_project_only_bools() {
        assert_eq!(project_bool(&RuntimeValue::Bool(true)), Ok(true));
        let error = project_bool(&quantity(1.0)).unwrap_err().to_string();
        assert!(error.contains("was checked as a Bool"), "{error}");
    }

    #[test]
    fn a_runtime_variant_must_match_its_checked_type() {
        let error = plain(&quantity(1.0), &CheckedType::Bool).unwrap_err();
        assert!(error.contains("was checked as a Bool"), "{error}");
        let error = plain(&RuntimeValue::Bool(true), &CheckedType::Int).unwrap_err();
        assert!(error.contains("was checked as an Int"), "{error}");
        let indexed = RuntimeValue::Indexed(IndexedValue::finite_for_test(vec![quantity(1.0)]));
        let error = plain(&indexed, &CheckedType::Quantity(length())).unwrap_err();
        assert!(error.contains("indexed value"), "{error}");
        let fields = RuntimeValue::Struct(pair(quantity(1.0), RuntimeValue::Bool(true)));
        let error = plain(&fields, &CheckedType::Int).unwrap_err();
        assert!(error.contains("struct `Pair`"), "{error}");
        let other = CheckedType::Struct(
            StructTypeRef::with_owner(
                DagId::root_in_package("projection", "other"),
                StructTypeName::expect_valid("Pair"),
            ),
            Vec::new(),
        );
        let error = plain(&fields, &other).unwrap_err();
        assert!(error.contains("was checked as a struct value"), "{error}");
    }

    #[test]
    fn scalars_take_their_dimension_and_time_scale_from_the_checked_type() {
        assert!(matches!(
            plain(&quantity(2.0), &CheckedType::Quantity(length())).unwrap(),
            Value::Quantity { dimension, display_unit: None, .. } if dimension == length()
        ));
        assert!(matches!(
            plain(
                &RuntimeValue::complex(1.0, 2.0).unwrap(),
                &CheckedType::Complex(length())
            )
            .unwrap(),
            Value::Complex { dimension, display_unit: None, .. } if dimension == length()
        ));
        let epoch = hifitime::Epoch::from_gregorian_utc_at_midnight(2026, 1, 1);
        let scale = graphcal_compiler::semantic::time_scale::TimeScale::TAI;
        assert_eq!(
            plain(
                &RuntimeValue::Datetime(epoch),
                &CheckedType::Datetime(scale)
            )
            .unwrap(),
            Value::Datetime {
                epoch,
                time_scale: scale,
                display_tz: None,
            }
        );
        assert_eq!(
            plain(&RuntimeValue::Bool(true), &CheckedType::Bool).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            plain(&RuntimeValue::Int(3), &CheckedType::Int).unwrap(),
            Value::Int(3)
        );
    }

    #[test]
    fn keys_and_indexed_values_carry_their_axes() {
        let finite = FiniteIndex::try_from_u64(2).unwrap();
        let index = IndexTypeRef::from_finite_index(finite);
        let declared = CheckedType::Indexed {
            element: Box::new(CheckedType::Quantity(length())),
            index: index.clone(),
        };
        let runtime = RuntimeValue::Indexed(IndexedValue::finite_for_test(vec![
            quantity(1.0),
            quantity(2.0),
        ]));
        assert!(matches!(
            plain(&runtime, &declared).unwrap(),
            Value::Indexed { entries, entry_display_names: None, .. } if entries.len() == 2
        ));
        let key = KeyValue::at(IndexAxis::finite(finite).unwrap(), 1).unwrap();
        assert_eq!(
            plain(&RuntimeValue::Key(key.clone()), &CheckedType::Key(index)).unwrap(),
            Value::Key(key)
        );
    }

    #[test]
    fn struct_fields_are_projected_at_their_applied_types() {
        let runtime = RuntimeValue::Struct(pair(quantity(1.0), RuntimeValue::Bool(true)));
        let Value::Struct {
            type_name,
            constructor,
            generic_args,
            fields,
        } = plain(&runtime, &pair_checked_type()).unwrap()
        else {
            panic!("expected a struct value");
        };
        assert_eq!(type_name, StructTypeRef::from_resolved(pair_type()));
        assert_eq!(constructor.as_str(), "Pair");
        assert!(generic_args.is_empty());
        assert!(matches!(
            &fields[&field("left")],
            Value::Quantity { dimension, .. } if *dimension == length()
        ));
        assert_eq!(fields[&field("right")], Value::Bool(true));
        // A field whose value does not match its applied type.
        let wrong = RuntimeValue::Struct(pair(RuntimeValue::Int(1), RuntimeValue::Bool(true)));
        assert!(plain(&wrong, &pair_checked_type()).is_err());
    }

    #[test]
    fn presentation_is_attached_in_the_same_walk() {
        let value = Presented::from_struct(pair(
            Presented::with_leaf(quantity(1500.0), km()).unwrap(),
            Presented::plain(RuntimeValue::Bool(false)),
        ));
        let (projected, diagnostics) = presented(&value, &pair_checked_type());
        assert!(diagnostics.is_empty());
        let Value::Struct { fields, .. } = projected else {
            panic!("expected a struct value");
        };
        assert_eq!(display_label(&fields[&field("left")]), Some("km"));

        let zone = graphcal_compiler::semantic::time_zone::TimeZoneRegistry::bundled()
            .parse_iana_id("Asia/Tokyo")
            .unwrap();
        let epoch = hifitime::Epoch::from_gregorian_utc_at_midnight(2026, 1, 1);
        let scale = graphcal_compiler::semantic::time_scale::TimeScale::UTC;
        let zoned = Presented::with_leaf(
            RuntimeValue::Datetime(epoch),
            ResolvedLeaf::Datetime(zone.clone()),
        )
        .unwrap();
        assert!(matches!(
            presented(&zoned, &CheckedType::Datetime(scale)).0,
            Value::Datetime { display_tz: Some(found), .. } if found == zone
        ));
    }

    #[test]
    fn a_leaf_whose_display_fails_keeps_its_si_value_and_is_reported_at_its_path() {
        let failed =
            ResolvedLeaf::Quantity(QuantityDisplay::Failed(PresentationFailure::Projection {
                message: "no scale".to_owned(),
            }));
        let tiny = ResolvedLeaf::Quantity(QuantityDisplay::Unit {
            label: "tiny".to_owned(),
            scale: PositiveFiniteScale::new(1e-300).unwrap(),
        });
        let entries = Presented::from_indexed(IndexedValue::finite_for_test(vec![
            Presented::with_leaf(quantity(1.0), km()).unwrap(),
            Presented::with_leaf(quantity(1.0), failed).unwrap(),
            Presented::with_leaf(quantity(1e300), tiny).unwrap(),
        ]));
        let declared = CheckedType::Indexed {
            element: Box::new(CheckedType::Quantity(length())),
            index: IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(3).unwrap()),
        };
        let (projected, diagnostics) = presented(&entries, &declared);
        let Value::Indexed { entries, .. } = projected else {
            panic!("expected an indexed value");
        };
        let labels = entries.values().map(display_label).collect::<Vec<_>>();
        assert_eq!(labels, vec![Some("km"), None, None]);
        let paths = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                vec![PresentationPathPart::Index(IndexEntryKey::position(1))],
                vec![PresentationPathPart::Index(IndexEntryKey::position(2))],
            ]
        );
        assert!(matches!(
            &diagnostics[1].failure,
            PresentationFailure::Projection { .. }
        ));

        // A struct's presented field is reported beneath the field.
        let axis = || IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(1).unwrap());
        let nested_pair = StructValue::for_test(
            pair_type(),
            ConstructorName::expect_valid("Pair"),
            vec![
                (
                    field("left"),
                    CheckedType::Indexed {
                        element: Box::new(CheckedType::Quantity(length())),
                        index: axis(),
                    },
                    Presented::with_leaf(
                        RuntimeValue::Indexed(IndexedValue::finite_for_test(vec![quantity(1.0)])),
                        ResolvedLeaf::Quantity(QuantityDisplay::Failed(
                            PresentationFailure::Projection {
                                message: "no scale".to_owned(),
                            },
                        )),
                    )
                    .unwrap(),
                ),
                (
                    field("right"),
                    CheckedType::Bool,
                    Presented::plain(RuntimeValue::Bool(true)),
                ),
            ],
        );
        let declared = CheckedType::Struct(StructTypeRef::from_resolved(pair_type()), Vec::new());
        let (_, diagnostics) = presented(&Presented::from_struct(nested_pair), &declared);
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.path.clone())
                .collect::<Vec<_>>(),
            vec![vec![
                PresentationPathPart::Field(field("left")),
                PresentationPathPart::Index(IndexEntryKey::position(0)),
            ]]
        );
    }
}
