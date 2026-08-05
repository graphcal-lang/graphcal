//! Concrete nominal-type expansion for transport-independent model schemas.

use std::sync::Arc;

use miette::NamedSource;

use super::InferredGenericArg;
use crate::registry::declared_type::{DeclaredGenericArg, DeclaredType, StructTypeRef};
use crate::registry::error::GraphcalError;
use crate::registry::type_def::TypeDefKind;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName};

/// One concrete field after applying all nominal generic arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcreteModelField {
    name: FieldName,
    declared_type: DeclaredType,
}

impl ConcreteModelField {
    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    #[must_use]
    pub const fn declared_type(&self) -> &DeclaredType {
        &self.declared_type
    }
}

/// One constructor in a concrete algebraic type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcreteModelConstructor {
    name: ConstructorName,
    fields: Vec<ConcreteModelField>,
}

impl ConcreteModelConstructor {
    #[must_use]
    pub const fn name(&self) -> &ConstructorName {
        &self.name
    }

    #[must_use]
    pub fn fields(&self) -> &[ConcreteModelField] {
        &self.fields
    }
}

/// Expand a concrete nominal type into constructors and substituted fields.
///
/// # Errors
///
/// Returns a compiler diagnostic if the TIR lacks the resolved type metadata or
/// if a supposedly concrete generic field cannot be resolved.
pub fn concrete_model_constructors(
    tir: &crate::tir::typed::TIR,
    identity: &StructTypeRef,
    generic_args: &[DeclaredGenericArg],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<ConcreteModelConstructor>, GraphcalError> {
    let dag = tir.root();
    let type_def = dag
        .semantic
        .type_defs
        .struct_types
        .get(identity.resolved())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("model schema cannot find concrete type definition for `{identity}`"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    let TypeDefKind::Union { members } = type_def.kind() else {
        return Err(GraphcalError::InternalError {
            message: format!("model schema cannot expose unresolved required type `{identity}`"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        });
    };
    let inferred_args = generic_args
        .iter()
        .map(InferredGenericArg::from)
        .collect::<Vec<_>>();
    let inferred_application = super::InferredType::Struct(
        super::InferredStructType::from_ref(identity.clone()),
        inferred_args.clone(),
    );
    let declared_types = tir.build_declared_types(src)?;
    super::infer::hir::validate_concrete_type_obligations(
        &inferred_application,
        &declared_types,
        dag,
        tir,
        &tir.registry,
        crate::registry::builtins::builtin_functions(),
        src,
        Span::new(0, 0),
        &crate::cancellation::CancellationToken::unbounded(),
    )?;
    members
        .iter()
        .map(|constructor| {
            let fields = constructor
                .fields()
                .iter()
                .map(|field| {
                    super::infer::hir::resolved_field_type(
                        identity.resolved(),
                        constructor,
                        field.name(),
                        type_def,
                        &inferred_args,
                        dag,
                        &tir.registry,
                        src,
                        Span::new(0, 0),
                    )
                    .map(|inferred| ConcreteModelField {
                        name: field.name().clone(),
                        declared_type: DeclaredType::from(&inferred),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ConcreteModelConstructor {
                name: constructor.name().clone(),
                fields,
            })
        })
        .collect()
}
