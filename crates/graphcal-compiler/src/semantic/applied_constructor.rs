//! A constructor applied at a nominal type: its identity together with the
//! instantiated type of each of its fields.
//!
//! Checking records one for each constructor application, and an extern
//! record declaration one for the record its function returns, so a value
//! built from either knows every field's type without looking the
//! constructor up again.

use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::checked_type::{CheckedGenericArg, CheckedType, Concrete, Concreteness};
use crate::syntax::type_name::{ConstructorName, FieldName};

/// A constructor of `runtime_type` applied to `generic_args`, with its
/// declared fields in declaration order, each at its type under those
/// arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedConstructor<V: Concreteness = Concrete> {
    runtime_type: ResolvedStructTypeName,
    constructor: ConstructorName,
    generic_args: Vec<CheckedGenericArg<V>>,
    fields: Vec<AppliedField<V>>,
}

/// One declared field of an [`AppliedConstructor`], at its instantiated type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedField<V: Concreteness = Concrete> {
    name: FieldName,
    field_type: CheckedType<V>,
}

impl<V: Concreteness> AppliedField<V> {
    #[must_use]
    pub(crate) const fn new(name: FieldName, field_type: CheckedType<V>) -> Self {
        Self { name, field_type }
    }

    /// The field's declared name.
    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    /// The field's type under the application's generic arguments.
    #[must_use]
    pub const fn field_type(&self) -> &CheckedType<V> {
        &self.field_type
    }

    /// This field with its type rewritten by `map`.
    pub(crate) fn try_map_type<W: Concreteness, E>(
        &self,
        map: impl FnOnce(&CheckedType<V>) -> Result<CheckedType<W>, E>,
    ) -> Result<AppliedField<W>, E> {
        Ok(AppliedField {
            name: self.name.clone(),
            field_type: map(&self.field_type)?,
        })
    }
}

impl<V: Concreteness> AppliedConstructor<V> {
    /// The application of `constructor` of `runtime_type` to
    /// `generic_args`, whose declared fields are `fields`, in declaration
    /// order and at their instantiated types.
    #[must_use]
    pub(crate) const fn new(
        runtime_type: ResolvedStructTypeName,
        constructor: ConstructorName,
        generic_args: Vec<CheckedGenericArg<V>>,
        fields: Vec<AppliedField<V>>,
    ) -> Self {
        Self {
            runtime_type,
            constructor,
            generic_args,
            fields,
        }
    }

    /// Canonical identity of the nominal type the value has at runtime.
    #[must_use]
    pub const fn runtime_type(&self) -> &ResolvedStructTypeName {
        &self.runtime_type
    }

    /// The applied constructor.
    #[must_use]
    pub const fn constructor(&self) -> &ConstructorName {
        &self.constructor
    }

    /// The generic arguments of the nominal application.
    #[must_use]
    pub fn generic_args(&self) -> &[CheckedGenericArg<V>] {
        &self.generic_args
    }

    /// Every declared field at its instantiated type, in declaration order.
    #[must_use]
    pub fn fields(&self) -> &[AppliedField<V>] {
        &self.fields
    }

    /// This application with its generic arguments and field types
    /// rewritten, keeping its identity and field names.
    pub(crate) fn try_map_types<W: Concreteness, E>(
        &self,
        runtime_type: ResolvedStructTypeName,
        generic_args: Vec<CheckedGenericArg<W>>,
        mut field_type: impl FnMut(&CheckedType<V>) -> Result<CheckedType<W>, E>,
    ) -> Result<AppliedConstructor<W>, E> {
        Ok(AppliedConstructor {
            runtime_type,
            constructor: self.constructor.clone(),
            generic_args,
            fields: self
                .fields
                .iter()
                .map(|field| field.try_map_type(&mut field_type))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl AppliedConstructor {
    /// An application named directly, for tests.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn for_test(
        runtime_type: ResolvedStructTypeName,
        constructor: ConstructorName,
        fields: impl IntoIterator<Item = (FieldName, CheckedType)>,
    ) -> Self {
        Self::new(
            runtime_type,
            constructor,
            Vec::new(),
            fields
                .into_iter()
                .map(|(name, field_type)| AppliedField::new(name, field_type))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AppliedConstructor, AppliedField};
    use crate::dag_id::DagId;
    use crate::dimension::Dimension;
    use crate::resolved_name::ResolvedStructTypeName;
    use crate::semantic::checked_type::{CheckedType, Concrete, Symbolic};
    use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    fn pair() -> AppliedConstructor<Symbolic> {
        AppliedConstructor::new(
            ResolvedStructTypeName::for_test(
                DagId::root_in_package("applied", "main"),
                StructTypeName::expect_valid("Pair"),
            ),
            ConstructorName::expect_valid("Pair"),
            Vec::new(),
            vec![
                AppliedField::new(
                    FieldName::expect_valid("left"),
                    CheckedType::Quantity(Dimension::dimensionless()),
                ),
                AppliedField::new(FieldName::expect_valid("right"), CheckedType::Bool),
            ],
        )
    }

    #[test]
    fn mapping_types_keeps_the_constructor_and_field_order() {
        let symbolic = pair();
        let concrete = symbolic
            .try_map_types::<Concrete, ()>(symbolic.runtime_type().clone(), Vec::new(), |ty| {
                ty.to_concrete().ok_or(())
            })
            .unwrap();
        assert_eq!(concrete.constructor(), symbolic.constructor());
        assert_eq!(concrete.runtime_type(), symbolic.runtime_type());
        assert_eq!(concrete.generic_args(), []);
        let fields = concrete
            .fields()
            .iter()
            .map(|field| (field.name().as_str(), field.field_type().clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                ("left", CheckedType::Quantity(Dimension::dimensionless())),
                ("right", CheckedType::Bool),
            ]
        );
        assert_eq!(
            symbolic.try_map_types::<Concrete, &str>(
                symbolic.runtime_type().clone(),
                Vec::new(),
                |_| Err("not concrete"),
            ),
            Err("not concrete")
        );
    }
}
