//! The nominal record type bound to a struct-returning extern function.

use std::sync::Arc;

use crate::function_signature::{StructFieldKind, StructResult, StructShape};
use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::applied_constructor::{AppliedConstructor, AppliedField};
use crate::semantic::checked_type::CheckedType;
use crate::syntax::type_name::ConstructorName;

/// The record type a struct-returning extern function was declared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternStructResult {
    /// The record constructor of the type named at the declaration site,
    /// applied with the field types of `shape`; every returned record is a
    /// value of this application.
    applied: Arc<AppliedConstructor>,
    /// The record's flattened field shape, as the plugin manifest sees it.
    shape: StructShape,
}

impl ExternStructResult {
    /// The record `constructor` of `record_type`, whose fields cross the
    /// plugin boundary as `shape`.
    #[must_use]
    pub(crate) fn new(
        record_type: ResolvedStructTypeName,
        constructor: ConstructorName,
        shape: StructShape,
    ) -> Self {
        let fields = shape
            .fields()
            .iter()
            .map(|field| {
                AppliedField::new(
                    field.name.clone(),
                    match &field.kind {
                        StructFieldKind::Quantity(dimension) => {
                            CheckedType::Quantity(dimension.clone())
                        }
                        StructFieldKind::Bool => CheckedType::Bool,
                        StructFieldKind::Int => CheckedType::Int,
                    },
                )
            })
            .collect();
        Self {
            applied: Arc::new(AppliedConstructor::new(
                record_type,
                constructor,
                Vec::new(),
                fields,
            )),
            shape,
        }
    }

    /// The record `constructor` of `record_type` named directly, for tests.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn for_test(
        record_type: ResolvedStructTypeName,
        constructor: ConstructorName,
        shape: StructShape,
    ) -> Self {
        Self::new(record_type, constructor, shape)
    }

    /// Canonical identity of the record type named at the declaration site.
    #[must_use]
    pub fn record_type(&self) -> &ResolvedStructTypeName {
        self.applied.runtime_type()
    }

    /// The record constructor applied with each field's type.
    #[must_use]
    pub const fn applied(&self) -> &Arc<AppliedConstructor> {
        &self.applied
    }

    /// Whether two struct results name the same record type.
    pub(crate) fn same_record(&self, other: &Self) -> bool {
        self.applied.runtime_type() == other.applied.runtime_type()
            && self.applied.constructor() == other.applied.constructor()
    }
}

impl StructResult for ExternStructResult {
    fn shape(&self) -> &StructShape {
        &self.shape
    }
}

#[cfg(test)]
mod tests {
    use super::ExternStructResult;
    use crate::dag_id::DagId;
    use crate::dimension::Dimension;
    use crate::function_signature::{StructFieldKind, StructShape, StructShapeField};
    use crate::resolved_name::ResolvedStructTypeName;
    use crate::semantic::checked_type::CheckedType;
    use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    #[test]
    fn the_record_is_applied_with_the_shape_field_types() {
        let field = |name: &str, kind| StructShapeField {
            name: FieldName::expect_valid(name),
            kind,
        };
        let shape = StructShape::try_new(vec![
            field("q", StructFieldKind::Quantity(Dimension::dimensionless())),
            field("b", StructFieldKind::Bool),
            field("i", StructFieldKind::Int),
        ])
        .unwrap();
        let record_type = ResolvedStructTypeName::for_test(
            DagId::root_in_package("extern", "main"),
            StructTypeName::expect_valid("Out"),
        );
        let result = ExternStructResult::new(
            record_type.clone(),
            ConstructorName::expect_valid("Out"),
            shape,
        );
        assert_eq!(result.record_type(), &record_type);
        assert!(result.applied().generic_args().is_empty());
        let fields = result
            .applied()
            .fields()
            .iter()
            .map(|field| (field.name().as_str(), field.field_type().clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                ("q", CheckedType::Quantity(Dimension::dimensionless())),
                ("b", CheckedType::Bool),
                ("i", CheckedType::Int),
            ]
        );
    }
}
