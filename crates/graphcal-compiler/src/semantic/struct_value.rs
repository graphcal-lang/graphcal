//! Nominal values: a constructor applied to its complete field set.

use std::sync::Arc;

use indexmap::IndexMap;

use crate::extern_struct_result::ExternStructResult;
use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::applied_constructor::{AppliedConstructor, AppliedField};
use crate::semantic::checked_type::CheckedGenericArg;
use crate::syntax::type_name::{ConstructorName, FieldName};

/// A constructor applied to exactly its declared fields.
///
/// Built only from a checked constructor application or an extern record's
/// checked result, both of which carry the constructor's declared fields at
/// their instantiated types. Construction rejects missing, unexpected, and
/// duplicate fields and stores one value per declared field, in declaration
/// order, so a struct value always has exactly the fields of its
/// constructor, and knows each field's type.
#[derive(Debug, Clone, PartialEq)]
pub struct StructValue<V> {
    /// The nominal application, shared by every value its node builds.
    application: Arc<AppliedConstructor>,
    /// One value per field of `application`, in the same order.
    values: Vec<V>,
}

/// A field set that does not match its constructor's declared fields.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StructFieldsError {
    #[error("constructor `{constructor}` is missing field `{field}`")]
    Missing {
        constructor: ConstructorName,
        field: FieldName,
    },
    #[error("constructor `{constructor}` has no field `{field}`")]
    Unexpected {
        constructor: ConstructorName,
        field: FieldName,
    },
    #[error("constructor `{constructor}` received field `{field}` twice")]
    Duplicate {
        constructor: ConstructorName,
        field: FieldName,
    },
}

impl<V> StructValue<V> {
    /// Apply `application` to one value per declared field, in declaration
    /// order. Only a checked constructor call, which places each of its
    /// initializers at its declared field once, applies a constructor this
    /// way.
    pub(crate) const fn from_declared(
        application: Arc<AppliedConstructor>,
        values: Vec<V>,
    ) -> Self {
        Self {
            application,
            values,
        }
    }

    /// Build the record an extern function returned, whose declaration bound
    /// the record to the plugin field shape of `record`.
    ///
    /// # Errors
    ///
    /// Returns [`StructFieldsError`] when `fields` is not exactly the shape's
    /// field set.
    pub fn try_from_record(
        record: &ExternStructResult,
        fields: impl IntoIterator<Item = (FieldName, V)>,
    ) -> Result<Self, StructFieldsError> {
        Self::try_new(Arc::clone(record.applied()), fields)
    }

    fn try_new(
        application: Arc<AppliedConstructor>,
        fields: impl IntoIterator<Item = (FieldName, V)>,
    ) -> Result<Self, StructFieldsError> {
        let constructor = || application.constructor().clone();
        let mut provided = IndexMap::new();
        for (field, value) in fields {
            if provided.contains_key(&field) {
                return Err(StructFieldsError::Duplicate {
                    constructor: constructor(),
                    field,
                });
            }
            provided.insert(field, value);
        }
        let mut values = Vec::with_capacity(provided.len());
        for field in application.fields() {
            let Some(value) = provided.swap_remove(field.name()) else {
                return Err(StructFieldsError::Missing {
                    constructor: constructor(),
                    field: field.name().clone(),
                });
            };
            values.push(value);
        }
        if let Some((field, _)) = provided.into_iter().next() {
            return Err(StructFieldsError::Unexpected {
                constructor: constructor(),
                field,
            });
        }
        Ok(Self {
            application,
            values,
        })
    }

    /// A struct value of `constructor` of `type_name` whose declared fields
    /// are exactly `fields`, each at its type, for tests (including of the
    /// evaluator's defenses against values of a foreign type).
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn for_test(
        type_name: ResolvedStructTypeName,
        constructor: ConstructorName,
        fields: Vec<(FieldName, crate::semantic::checked_type::CheckedType, V)>,
    ) -> Self {
        let (declared, values): (Vec<_>, Vec<_>) = fields
            .into_iter()
            .map(|(name, field_type, value)| ((name, field_type), value))
            .unzip();
        Self {
            application: Arc::new(AppliedConstructor::for_test(
                type_name,
                constructor,
                declared,
            )),
            values,
        }
    }

    /// Canonical nominal identity.
    #[must_use]
    pub fn type_name(&self) -> &ResolvedStructTypeName {
        self.application.runtime_type()
    }

    /// The applied constructor.
    #[must_use]
    pub fn constructor(&self) -> &ConstructorName {
        self.application.constructor()
    }

    /// Concrete generic arguments of the nominal application.
    #[must_use]
    pub fn generic_args(&self) -> &[CheckedGenericArg] {
        self.application.generic_args()
    }

    fn position(&self, field: &FieldName) -> Option<usize> {
        self.application
            .fields()
            .iter()
            .position(|declared| declared.name() == field)
    }

    /// The value of `field`, when the constructor declares it.
    #[must_use]
    pub fn field(&self, field: &FieldName) -> Option<&V> {
        self.position(field)
            .and_then(|index| self.values.get(index))
    }

    /// Every field with its value, in declaration order.
    #[must_use]
    pub fn fields(&self) -> impl ExactSizeIterator<Item = (&FieldName, &V)> {
        self.typed_fields()
            .map(|(field, value)| (field.name(), value))
    }

    /// Every declared field, at its instantiated type, with its value, in
    /// declaration order.
    #[must_use]
    pub fn typed_fields(&self) -> impl ExactSizeIterator<Item = (&AppliedField, &V)> {
        self.application.fields().iter().zip(&self.values)
    }

    /// Derive a value of the same application from each owned field value.
    #[must_use]
    pub fn map<U>(self, field: impl FnMut(V) -> U) -> StructValue<U> {
        StructValue {
            application: self.application,
            values: self.values.into_iter().map(field).collect(),
        }
    }

    /// Derive a value of the same application from each owned field.
    pub fn try_map<U, E>(
        self,
        mut field: impl FnMut(&FieldName, V) -> Result<U, E>,
    ) -> Result<StructValue<U>, E> {
        let values = self
            .application
            .fields()
            .iter()
            .zip(self.values)
            .map(|(declared, value)| field(declared.name(), value))
            .collect::<Result<_, _>>()?;
        Ok(StructValue {
            application: self.application,
            values,
        })
    }

    /// Pair each field value with the value of the same field of `other`.
    ///
    /// # Errors
    ///
    /// Returns this value back when `other` applies another constructor
    /// application.
    pub fn zip<U>(self, other: &StructValue<U>) -> Result<StructValue<(V, &U)>, Self> {
        if self.application != other.application {
            return Err(self);
        }
        // One application has one field list, so the values align field by
        // field.
        Ok(StructValue {
            application: self.application,
            values: self.values.into_iter().zip(&other.values).collect(),
        })
    }

    /// The owned value of `field`.
    ///
    /// # Errors
    ///
    /// Returns this value back when its constructor does not declare `field`.
    pub fn into_field(self, field: &FieldName) -> Result<V, Self> {
        match self.position(field) {
            // One value per declared field, so a declared position has one.
            Some(index) => {
                let mut values = self.values;
                Ok(values.swap_remove(index))
            }
            None => Err(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::dag_id::DagId;
    use crate::extern_struct_result::ExternStructResult;
    use crate::function_signature::{StructFieldKind, StructShape, StructShapeField};
    use crate::resolved_name::ResolvedStructTypeName;
    use crate::semantic::applied_constructor::AppliedConstructor;
    use crate::semantic::checked_type::CheckedType;
    use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    use super::{StructFieldsError, StructValue};

    fn field(name: &str) -> FieldName {
        FieldName::expect_valid(name)
    }

    fn type_name(module: &str) -> ResolvedStructTypeName {
        ResolvedStructTypeName::for_test(
            DagId::root_in_package("struct-tests", module),
            StructTypeName::expect_valid("Pair"),
        )
    }

    fn constructor() -> ConstructorName {
        ConstructorName::expect_valid("Pair")
    }

    fn pair(module: &str, constructor: ConstructorName) -> AppliedConstructor {
        AppliedConstructor::for_test(
            type_name(module),
            constructor,
            [
                (field("left"), CheckedType::Int),
                (field("right"), CheckedType::Int),
            ],
        )
    }

    fn build(fields: Vec<(&str, i64)>) -> Result<StructValue<i64>, StructFieldsError> {
        StructValue::try_new(
            Arc::new(pair("main", constructor())),
            fields.into_iter().map(|(name, value)| (field(name), value)),
        )
    }

    #[test]
    fn fields_are_stored_in_declaration_order() {
        let value = build(vec![("right", 2), ("left", 1)]).unwrap();
        let fields = value
            .fields()
            .map(|(name, value)| (name.clone(), *value))
            .collect::<Vec<_>>();
        assert_eq!(fields, vec![(field("left"), 1), (field("right"), 2)]);
        assert_eq!(value.field(&field("right")), Some(&2));
        assert_eq!(value.field(&field("other")), None);
        assert_eq!(value.type_name(), &type_name("main"));
        assert_eq!(value.constructor(), &constructor());
        assert!(value.generic_args().is_empty());
        let typed = value
            .typed_fields()
            .map(|(field, value)| (field.field_type().clone(), *value))
            .collect::<Vec<_>>();
        assert_eq!(typed, vec![(CheckedType::Int, 1), (CheckedType::Int, 2)]);
    }

    #[test]
    fn field_sets_must_match_the_declaration() {
        assert_eq!(
            build(vec![("left", 1)]).unwrap_err(),
            StructFieldsError::Missing {
                constructor: constructor(),
                field: field("right"),
            }
        );
        assert_eq!(
            build(vec![("left", 1), ("right", 2), ("extra", 3)]).unwrap_err(),
            StructFieldsError::Unexpected {
                constructor: constructor(),
                field: field("extra"),
            }
        );
        assert_eq!(
            build(vec![("left", 1), ("left", 2), ("right", 3)]).unwrap_err(),
            StructFieldsError::Duplicate {
                constructor: constructor(),
                field: field("left"),
            }
        );
    }

    #[test]
    fn record_shapes_supply_the_declared_fields() {
        let shape = StructShape::try_new(vec![
            StructShapeField {
                name: field("left"),
                kind: StructFieldKind::Bool,
            },
            StructShapeField {
                name: field("right"),
                kind: StructFieldKind::Bool,
            },
        ])
        .unwrap();
        let record = ExternStructResult::for_test(type_name("main"), constructor(), shape);
        let value =
            StructValue::try_from_record(&record, [(field("right"), false), (field("left"), true)])
                .unwrap();
        assert_eq!(value.field(&field("left")), Some(&true));
        let typed = value
            .typed_fields()
            .map(|(field, value)| (field.name().clone(), field.field_type().clone(), *value))
            .collect::<Vec<_>>();
        assert_eq!(
            typed,
            vec![
                (field("left"), CheckedType::Bool, true),
                (field("right"), CheckedType::Bool, false),
            ]
        );
        assert!(StructValue::try_from_record(&record, [(field("left"), true)]).is_err());
    }

    #[test]
    fn owned_maps_and_selection_keep_the_application() {
        let value = build(vec![("left", 1), ("right", 2)]).unwrap();
        let mut seen = Vec::new();
        let mapped = value
            .clone()
            .try_map(|name, value| {
                seen.push(name.clone());
                Ok::<_, ()>(-value)
            })
            .unwrap();
        assert_eq!(seen, vec![field("left"), field("right")]);
        assert_eq!(mapped.constructor(), value.constructor());
        assert_eq!(mapped.type_name(), value.type_name());
        assert_eq!(mapped.field(&field("left")), Some(&-1));
        assert_eq!(
            value.clone().map(|value| value * 3).field(&field("right")),
            Some(&6)
        );
        assert_eq!(value.clone().into_field(&field("right")), Ok(2));
        assert_eq!(
            value.clone().into_field(&field("other")),
            Err(value.clone())
        );
        let failed = value.try_map(|name, value| {
            if *name == field("right") {
                Err(value)
            } else {
                Ok(value)
            }
        });
        assert_eq!(failed.unwrap_err(), 2);
    }

    #[test]
    fn zips_pair_fields_of_one_application_only() {
        let value = build(vec![("left", 1), ("right", 2)]).unwrap();
        let labels = build(vec![("right", 20), ("left", 10)]).unwrap();
        let zipped = value.clone().zip(&labels).unwrap();
        assert_eq!(zipped.field(&field("left")), Some(&(1, &10)));
        assert_eq!(zipped.field(&field("right")), Some(&(2, &20)));
        let other = StructValue::try_new(
            Arc::new(pair("other", constructor())),
            [(field("left"), 1), (field("right"), 2)],
        )
        .unwrap();
        assert_eq!(value.clone().zip(&other).unwrap_err(), value);
    }

    #[test]
    fn equality_compares_application_and_fields() {
        let value = build(vec![("left", 1), ("right", 2)]).unwrap();
        assert_eq!(value, build(vec![("right", 2), ("left", 1)]).unwrap());
        assert_ne!(value, build(vec![("left", 3), ("right", 2)]).unwrap());
        let fields = || {
            vec![
                (field("left"), CheckedType::Int, 1),
                (field("right"), CheckedType::Int, 2),
            ]
        };
        assert_eq!(
            value,
            StructValue::<i64>::for_test(type_name("main"), constructor(), fields())
        );
        let other_owner = StructValue::<i64>::for_test(type_name("other"), constructor(), fields());
        assert_ne!(value, other_owner);
        let other_constructor = StructValue::<i64>::for_test(
            type_name("main"),
            ConstructorName::expect_valid("Other"),
            fields(),
        );
        assert_ne!(value, other_constructor);
    }
}
