//! Concrete nominal-type expansion for transport-independent model schemas.

use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use super::InferredGenericArg;
use crate::registry::declared_type::{
    DeclaredGenericArg, DeclaredType, IndexTypeRef, StructTypeRef,
};
use crate::registry::error::GraphcalError;
use crate::registry::type_def::{TypeDef, TypeDefKind, TypeGenericConstraint, UnionMemberDef};
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName};

/// Failure to construct a checked concrete type for model-schema expansion.
///
/// The public schema expander accepts only [`ConcreteModelType`], so malformed
/// arity, sort, or concreteness cannot be passed to field substitution.
#[derive(Debug, Clone, Error)]
pub enum ConcreteModelTypeError {
    #[error("model schema cannot find type definition for `{identity}`")]
    UnknownType { identity: StructTypeRef },
    #[error("required type `{identity}` was not concretely bound")]
    RequiredType { identity: StructTypeRef },
    #[error(
        "concrete model type `{identity}` expects {expected} generic argument(s), got {actual}"
    )]
    GenericArityMismatch {
        identity: StructTypeRef,
        expected: usize,
        actual: usize,
    },
    #[error(
        "generic parameter `{parameter}` on `{identity}` expects sort {expected}, got {actual}"
    )]
    GenericSortMismatch {
        identity: StructTypeRef,
        parameter: GenericParamName,
        expected: TypeGenericConstraint,
        actual: TypeGenericConstraint,
    },
    #[error("generic argument for `{parameter}` on `{identity}` is not concrete")]
    NonConcreteGenericArgument {
        identity: StructTypeRef,
        parameter: GenericParamName,
    },
    #[error("generic Type parameter `{parameter}` on `{identity}` cannot accept an Index argument")]
    IndexTypeArgument {
        identity: StructTypeRef,
        parameter: GenericParamName,
    },
    #[error(
        "generic Type parameter `{parameter}` on `{identity}` cannot accept an indexed declaration type"
    )]
    IndexedTypeArgument {
        identity: StructTypeRef,
        parameter: GenericParamName,
    },
    #[error("index argument `{index}` is not concrete")]
    NonConcreteIndex { index: IndexTypeRef },
    #[error("model schema cannot find index definition for `{index}`")]
    UnknownIndex { index: IndexTypeRef },
    #[error("required index `{index}` was not concretely bound")]
    RequiredIndex { index: IndexTypeRef },
    #[error(transparent)]
    Compiler(#[from] GraphcalError),
}

impl ConcreteModelTypeError {
    /// Convert schema-construction failure into the compiler diagnostic used by
    /// evaluation shells. Source-language failures retain their original
    /// diagnostic; malformed safe-API inputs are internal invariant failures.
    #[must_use]
    pub fn into_graphcal_error(self, src: &NamedSource<Arc<String>>) -> GraphcalError {
        match self {
            Self::Compiler(error) => error,
            invariant => GraphcalError::InternalError {
                message: invariant.to_string(),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            },
        }
    }
}

#[derive(Debug)]
struct ConcreteModelDefinition<'tir> {
    type_def: &'tir TypeDef,
    constructors: &'tir [UnionMemberDef],
}

/// A concrete algebraic model type whose generic application has been checked.
///
/// Fields are private and the value borrows the validating TIR, so safe callers
/// cannot fabricate an application with the wrong generic arity or sort, leave
/// a generic argument symbolic, or expand it against another TIR.
#[derive(Debug)]
pub struct ConcreteModelType<'tir> {
    tir: &'tir crate::tir::typed::TIR,
    identity: StructTypeRef,
    generic_args: Vec<DeclaredGenericArg>,
    definition: ConcreteModelDefinition<'tir>,
}

impl<'tir> ConcreteModelType<'tir> {
    /// Validate one nominal application before model-schema expansion.
    ///
    /// # Errors
    ///
    /// Returns a focused schema error for malformed API inputs, or preserves a
    /// compiler diagnostic when concrete generic field obligations fail.
    pub fn try_new(
        tir: &'tir crate::tir::typed::TIR,
        identity: &StructTypeRef,
        generic_args: &[DeclaredGenericArg],
        src: &NamedSource<Arc<String>>,
    ) -> Result<Self, ConcreteModelTypeError> {
        let definition = validate_model_type_definition(tir, identity, generic_args)?;
        let inferred_args = generic_args
            .iter()
            .map(InferredGenericArg::from)
            .collect::<Vec<_>>();
        let inferred_application = super::InferredType::Struct(
            super::InferredStructType::from_ref(identity.clone()),
            inferred_args,
        );
        let declared_types = tir.build_declared_types(src)?;
        super::infer::hir::validate_concrete_type_obligations(
            &inferred_application,
            &declared_types,
            tir.root(),
            tir,
            &tir.registry,
            crate::registry::builtins::builtin_functions(),
            src,
            Span::new(0, 0),
            &crate::cancellation::CancellationToken::unbounded(),
        )?;
        Ok(Self {
            tir,
            identity: identity.clone(),
            generic_args: generic_args.to_vec(),
            definition,
        })
    }

    #[must_use]
    pub const fn identity(&self) -> &StructTypeRef {
        &self.identity
    }

    #[must_use]
    pub fn generic_args(&self) -> &[DeclaredGenericArg] {
        &self.generic_args
    }

    /// Expand this checked type into constructors and substituted fields.
    ///
    /// # Errors
    ///
    /// Returns a compiler diagnostic if checked TIR field metadata is missing.
    pub fn constructors(
        &self,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Vec<ConcreteModelConstructor>, GraphcalError> {
        let inferred_args = self
            .generic_args
            .iter()
            .map(InferredGenericArg::from)
            .collect::<Vec<_>>();
        self.definition
            .constructors
            .iter()
            .map(|constructor| {
                let fields = constructor
                    .fields()
                    .iter()
                    .map(|field| {
                        super::infer::hir::resolved_field_type(
                            self.identity.resolved(),
                            constructor,
                            field.name(),
                            self.definition.type_def,
                            &inferred_args,
                            self.tir.root(),
                            &self.tir.registry,
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
}

fn validate_model_type_definition<'tir>(
    tir: &'tir crate::tir::typed::TIR,
    identity: &StructTypeRef,
    generic_args: &[DeclaredGenericArg],
) -> Result<ConcreteModelDefinition<'tir>, ConcreteModelTypeError> {
    let type_def = tir
        .root()
        .semantic
        .type_defs
        .struct_types
        .get(identity.resolved())
        .ok_or_else(|| ConcreteModelTypeError::UnknownType {
            identity: identity.clone(),
        })?;
    let TypeDefKind::Union { members } = type_def.kind() else {
        return Err(ConcreteModelTypeError::RequiredType {
            identity: identity.clone(),
        });
    };
    if generic_args.len() != type_def.generic_params().len() {
        return Err(ConcreteModelTypeError::GenericArityMismatch {
            identity: identity.clone(),
            expected: type_def.generic_params().len(),
            actual: generic_args.len(),
        });
    }
    for (parameter, argument) in type_def.generic_params().iter().zip(generic_args) {
        let actual = generic_argument_sort(argument);
        if parameter.constraint != actual {
            return Err(ConcreteModelTypeError::GenericSortMismatch {
                identity: identity.clone(),
                parameter: parameter.name.clone(),
                expected: parameter.constraint,
                actual,
            });
        }
        validate_concrete_generic_argument(tir, identity, &parameter.name, argument)?;
    }
    Ok(ConcreteModelDefinition {
        type_def,
        constructors: members,
    })
}

const fn generic_argument_sort(argument: &DeclaredGenericArg) -> TypeGenericConstraint {
    match argument {
        DeclaredGenericArg::Dim(_) => TypeGenericConstraint::Dim,
        DeclaredGenericArg::Index(_) => TypeGenericConstraint::Index,
        DeclaredGenericArg::Nat(_) => TypeGenericConstraint::Nat,
        DeclaredGenericArg::Type(_) => TypeGenericConstraint::Type,
    }
}

fn validate_concrete_generic_argument(
    tir: &crate::tir::typed::TIR,
    identity: &StructTypeRef,
    parameter: &GenericParamName,
    argument: &DeclaredGenericArg,
) -> Result<(), ConcreteModelTypeError> {
    match argument {
        DeclaredGenericArg::Dim(_) => Ok(()),
        DeclaredGenericArg::Index(index) => validate_concrete_index(tir, index),
        DeclaredGenericArg::Nat(form) if form.constant_value().is_some() => Ok(()),
        DeclaredGenericArg::Nat(_) => Err(ConcreteModelTypeError::NonConcreteGenericArgument {
            identity: identity.clone(),
            parameter: parameter.clone(),
        }),
        DeclaredGenericArg::Type(declared_type) => {
            validate_concrete_type_argument(tir, identity, parameter, declared_type)
        }
    }
}

fn validate_concrete_type_argument(
    tir: &crate::tir::typed::TIR,
    identity: &StructTypeRef,
    parameter: &GenericParamName,
    declared_type: &DeclaredType,
) -> Result<(), ConcreteModelTypeError> {
    match declared_type {
        DeclaredType::Struct(nested_identity, nested_args) => {
            validate_model_type_definition(tir, nested_identity, nested_args).map(|_| ())
        }
        DeclaredType::Key(index) => validate_concrete_index(tir, index),
        DeclaredType::IndexArg(_) => Err(ConcreteModelTypeError::IndexTypeArgument {
            identity: identity.clone(),
            parameter: parameter.clone(),
        }),
        DeclaredType::Indexed { .. } => Err(ConcreteModelTypeError::IndexedTypeArgument {
            identity: identity.clone(),
            parameter: parameter.clone(),
        }),
        DeclaredType::Quantity(_)
        | DeclaredType::Complex(_)
        | DeclaredType::Bool
        | DeclaredType::Int
        | DeclaredType::Datetime(_) => Ok(()),
    }
}

fn validate_concrete_index(
    tir: &crate::tir::typed::TIR,
    index: &IndexTypeRef,
) -> Result<(), ConcreteModelTypeError> {
    if index.finite_index().is_some() {
        return Ok(());
    }
    let Some(resolved) = index.declared_resolved() else {
        return Err(ConcreteModelTypeError::NonConcreteIndex {
            index: index.clone(),
        });
    };
    let definition = tir
        .root()
        .semantic
        .collection_refs
        .index_defs
        .get(resolved)
        .or_else(|| tir.declared_index_def(resolved))
        .ok_or_else(|| ConcreteModelTypeError::UnknownIndex {
            index: index.clone(),
        })?;
    if definition.is_required() {
        return Err(ConcreteModelTypeError::RequiredIndex {
            index: index.clone(),
        });
    }
    Ok(())
}

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
