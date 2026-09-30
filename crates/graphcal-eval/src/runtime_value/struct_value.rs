//! Nominal values: a constructor applied to its complete field set.

use indexmap::IndexMap;

use graphcal_compiler::function_signature::StructShape;
use graphcal_compiler::registry::checked_type::CheckedGenericArg;
use graphcal_compiler::resolved_name::ResolvedStructTypeName;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};
use graphcal_compiler::tir::texpr::ConstructorApplication;

/// A constructor applied to exactly its declared fields.
///
/// Built only from a checked constructor application or an extern record's
/// checked field shape, both of which name the constructor's declared fields.
/// Construction rejects missing, unexpected, and duplicate fields and stores
/// the fields in declaration order, so a struct value always has exactly the
/// fields of its constructor.
#[derive(Debug, Clone)]
pub struct StructValue<V> {
    /// Canonical nominal identity, independent of source aliases and display spelling.
    type_name: ResolvedStructTypeName,
    /// Constructor member identity within `type_name` (not a display leaf).
    constructor: ConstructorName,
    /// Concrete generic identity needed by field constraints and equality.
    generic_args: Vec<CheckedGenericArg>,
    /// Every declared field, in declaration order.
    fields: IndexMap<FieldName, V>,
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
    /// Apply a checked constructor application to evaluated field values.
    ///
    /// # Errors
    ///
    /// Returns [`StructFieldsError`] when `fields` is not exactly the
    /// constructor's declared field set.
    pub fn try_from_application(
        application: &ConstructorApplication,
        fields: impl IntoIterator<Item = (FieldName, V)>,
    ) -> Result<Self, StructFieldsError> {
        Self::try_new(
            application.runtime_type.clone(),
            application.constructor.name(),
            application.generic_args.clone(),
            application
                .constructor
                .variant()
                .fields()
                .iter()
                .map(graphcal_compiler::hir::nominal::NominalField::name),
            fields,
        )
    }

    /// Build the record an extern function returned, whose declaration bound
    /// the record `constructor` of `type_name` to the plugin field `shape`.
    ///
    /// # Errors
    ///
    /// Returns [`StructFieldsError`] when `fields` is not exactly the shape's
    /// field set.
    pub fn try_from_record_shape(
        type_name: ResolvedStructTypeName,
        constructor: ConstructorName,
        shape: &StructShape,
        fields: impl IntoIterator<Item = (FieldName, V)>,
    ) -> Result<Self, StructFieldsError> {
        Self::try_new(
            type_name,
            constructor,
            Vec::new(),
            shape.fields().iter().map(|field| &field.name),
            fields,
        )
    }

    fn try_new<'a>(
        type_name: ResolvedStructTypeName,
        constructor: ConstructorName,
        generic_args: Vec<CheckedGenericArg>,
        declared: impl IntoIterator<Item = &'a FieldName>,
        fields: impl IntoIterator<Item = (FieldName, V)>,
    ) -> Result<Self, StructFieldsError> {
        let mut provided = IndexMap::new();
        for (field, value) in fields {
            if provided.contains_key(&field) {
                return Err(StructFieldsError::Duplicate { constructor, field });
            }
            provided.insert(field, value);
        }
        let mut ordered = IndexMap::with_capacity(provided.len());
        for field in declared {
            let Some(value) = provided.swap_remove(field) else {
                return Err(StructFieldsError::Missing {
                    constructor,
                    field: field.clone(),
                });
            };
            ordered.insert(field.clone(), value);
        }
        if let Some((field, _)) = provided.into_iter().next() {
            return Err(StructFieldsError::Unexpected { constructor, field });
        }
        Ok(Self {
            type_name,
            constructor,
            generic_args,
            fields: ordered,
        })
    }

    /// A struct value built without checking its constructor's fields, for
    /// tests of the evaluator's defenses against foreign values.
    #[cfg(test)]
    #[must_use]
    pub const fn for_test(
        type_name: ResolvedStructTypeName,
        constructor: ConstructorName,
        fields: IndexMap<FieldName, V>,
    ) -> Self {
        Self {
            type_name,
            constructor,
            generic_args: Vec::new(),
            fields,
        }
    }

    /// Canonical nominal identity.
    #[must_use]
    pub const fn type_name(&self) -> &ResolvedStructTypeName {
        &self.type_name
    }

    /// The applied constructor.
    #[must_use]
    pub const fn constructor(&self) -> &ConstructorName {
        &self.constructor
    }

    /// Concrete generic arguments of the nominal application.
    #[must_use]
    pub fn generic_args(&self) -> &[CheckedGenericArg] {
        &self.generic_args
    }

    /// The value of `field`, when the constructor declares it.
    #[must_use]
    pub fn field(&self, field: &FieldName) -> Option<&V> {
        self.fields.get(field)
    }

    /// Every field with its value, in declaration order.
    pub fn fields(&self) -> impl ExactSizeIterator<Item = (&FieldName, &V)> {
        self.fields.iter()
    }

    /// Whether both values apply the same constructor of the same nominal
    /// application.
    #[must_use]
    pub fn same_application<W>(&self, other: &StructValue<W>) -> bool {
        self.type_name == other.type_name
            && self.constructor == other.constructor
            && self.generic_args == other.generic_args
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::function_signature::{StructFieldKind, StructShape, StructShapeField};
    use graphcal_compiler::resolved_name::ResolvedStructTypeName;
    use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    use crate::runtime_value::struct_value::{StructFieldsError, StructValue};

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

    fn build(fields: Vec<(&str, i64)>) -> Result<StructValue<i64>, StructFieldsError> {
        let declared = [field("left"), field("right")];
        StructValue::try_new(
            type_name("main"),
            constructor(),
            Vec::new(),
            declared.iter(),
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
        let value = StructValue::try_from_record_shape(
            type_name("main"),
            constructor(),
            &shape,
            [(field("right"), false), (field("left"), true)],
        )
        .unwrap();
        assert_eq!(value.field(&field("left")), Some(&true));
        assert!(
            StructValue::try_from_record_shape(
                type_name("main"),
                constructor(),
                &shape,
                [(field("left"), true)],
            )
            .is_err()
        );
    }

    #[test]
    fn applications_compare_identity_constructor_and_arguments() {
        let value = build(vec![("left", 1), ("right", 2)]).unwrap();
        let same = build(vec![("left", 3), ("right", 4)]).unwrap();
        assert!(value.same_application(&same));
        let other_owner = StructValue::<i64>::for_test(
            type_name("other"),
            constructor(),
            indexmap::IndexMap::new(),
        );
        assert!(!value.same_application(&other_owner));
        let other_constructor = StructValue::<i64>::for_test(
            type_name("main"),
            ConstructorName::expect_valid("Other"),
            indexmap::IndexMap::new(),
        );
        assert!(!value.same_application(&other_constructor));
    }
}
