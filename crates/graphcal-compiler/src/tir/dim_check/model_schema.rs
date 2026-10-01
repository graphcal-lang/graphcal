//! Concrete nominal-type expansion for transport-independent model schemas.

use thiserror::Error;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir::nominal::{NominalConstructor, NominalTypeDef, NominalTypeKind};
use crate::semantic::checked_type::{CheckedGenericArg, CheckedType, IndexTypeRef, StructTypeRef};
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName};

/// Failure to validate a nominal application for model-schema expansion.
#[derive(Debug, Clone, Error)]
pub enum ConcreteModelTypeError {
    #[error("model schema cannot find type definition for `{identity}`")]
    UnknownType { identity: StructTypeRef },
    #[error("required type `{identity}` was not concretely bound")]
    RequiredType { identity: StructTypeRef },
    #[error("model type `{identity}` expects {expected} generic argument(s), got {actual}")]
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
        expected: GenericConstraint,
        actual: GenericConstraint,
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
    Compiler(#[from] SemanticError),
}

impl ConcreteModelTypeError {
    /// Convert schema-validation failure into the compiler diagnostic used by
    /// evaluation shells. Source-language failures retain their original
    /// diagnostic; malformed safe-API inputs are internal invariant failures.
    #[must_use]
    pub fn into_semantic_error(self, src: SourceId) -> SemanticError {
        match self {
            Self::Compiler(error) => error,
            invariant => {
                SemanticError::internal_error(invariant.to_string(), src, DiagnosticAnchor::Builtin)
            }
        }
    }
}

#[derive(Debug)]
struct ModelTypeDefinition<'tir> {
    type_def: &'tir NominalTypeDef,
    constructors: &'tir [NominalConstructor],
}

/// A complete, sort-checked algebraic application tied to its validating TIR.
///
/// This phase may retain typed required type or index identities while include
/// reconciliation is still pending. [`ConcreteModelType`] is the stricter
/// model-boundary state in which every such identity has been bound.
#[derive(Debug)]
pub struct ValidatedModelType<'tir> {
    tir: &'tir crate::tir::typed::CheckedTir,
    identity: StructTypeRef,
    generic_args: Vec<CheckedGenericArg>,
    definition: ModelTypeDefinition<'tir>,
}

impl<'tir> ValidatedModelType<'tir> {
    /// Validate exact generic arity, sort, type-argument shape, and concrete
    /// field obligations before expanding an algebraic application.
    ///
    /// Required nominal identities remain valid at this compiler phase because
    /// include reconciliation may bind them later.
    ///
    /// # Errors
    ///
    /// Returns a focused schema error for malformed API inputs, or preserves a
    /// compiler diagnostic when generic field obligations fail.
    pub fn try_new(
        tir: &'tir crate::tir::typed::CheckedTir,
        identity: &StructTypeRef,
        generic_args: &[CheckedGenericArg],
        src: SourceId,
    ) -> Result<Self, ConcreteModelTypeError> {
        let definition = validate_model_type_definition(tir, identity, generic_args)?;
        validate_application_obligations(tir, identity, generic_args, &definition, src)?;
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
    pub fn generic_args(&self) -> &[CheckedGenericArg] {
        &self.generic_args
    }

    /// Expand this checked type into constructors and substituted fields.
    ///
    /// # Errors
    ///
    /// Returns a compiler diagnostic if checked TIR field metadata is missing.
    pub fn constructors(
        &self,
        _src: SourceId,
    ) -> Result<Vec<ConcreteModelConstructor>, SemanticError> {
        let tir: &dyn crate::tir::typed::TirRead = self.tir;
        let metadata_dag = tir
            .dag_with_type_metadata(self.identity.resolved())
            .unwrap_or_else(|| tir.root());
        self.definition
            .constructors
            .iter()
            .map(|constructor| {
                let fields = constructor
                    .fields()
                    .iter()
                    .map(|field| {
                        super::generic_substitution::resolved_field_type(
                            &super::generic_substitution::resolved_type_field_key(
                                self.identity.resolved(),
                                constructor,
                                field.name(),
                            ),
                            self.definition.type_def,
                            &self
                                .generic_args
                                .iter()
                                .map(CheckedGenericArg::to_symbolic)
                                .collect::<Vec<_>>(),
                            metadata_dag,
                            self.definition.type_def.source(),
                            field.type_annotation().span,
                        )
                        .map(|inferred| ConcreteModelField {
                            name: field.name().clone(),
                            declared_type: inferred,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ConcreteModelConstructor {
                    name: constructor.name(),
                    fields,
                })
            })
            .collect()
    }
}

/// A validated model type with no required or symbolic generic bindings.
///
/// Its private checked inner state makes wrong arity, wrong sort, unresolved
/// bindable identities, and expansion against a different TIR unrepresentable
/// to safe callers.
#[derive(Debug)]
pub struct ConcreteModelType<'tir> {
    validated: ValidatedModelType<'tir>,
}

impl<'tir> ConcreteModelType<'tir> {
    /// Validate one fully bound nominal application for public model schemas.
    ///
    /// # Errors
    ///
    /// Returns a focused schema error for malformed or unresolved API inputs,
    /// or preserves a compiler diagnostic when generic field obligations fail.
    pub fn try_new(
        tir: &'tir crate::tir::typed::CheckedTir,
        identity: &StructTypeRef,
        generic_args: &[CheckedGenericArg],
        src: SourceId,
    ) -> Result<Self, ConcreteModelTypeError> {
        let validated = ValidatedModelType::try_new(tir, identity, generic_args, src)?;
        validate_bound_generic_arguments(tir, identity, generic_args)?;
        Ok(Self { validated })
    }

    #[must_use]
    pub const fn identity(&self) -> &StructTypeRef {
        self.validated.identity()
    }

    #[must_use]
    pub fn generic_args(&self) -> &[CheckedGenericArg] {
        self.validated.generic_args()
    }

    /// Expand this concrete type into constructors and substituted fields.
    ///
    /// # Errors
    ///
    /// Returns a compiler diagnostic if checked TIR field metadata is missing.
    pub fn constructors(
        &self,
        src: SourceId,
    ) -> Result<Vec<ConcreteModelConstructor>, SemanticError> {
        self.validated.constructors(src)
    }
}

fn validate_application_obligations(
    tir: &dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    generic_args: &[CheckedGenericArg],
    definition: &ModelTypeDefinition<'_>,
    _src: SourceId,
) -> Result<(), ConcreteModelTypeError> {
    let application = CheckedType::Struct(identity.clone(), generic_args.to_vec());
    let metadata_dag = tir
        .dag_with_type_metadata(identity.resolved())
        .unwrap_or_else(|| tir.root());
    crate::outcome::without_cancellation(|cancellation| {
        super::concrete_obligations::validate_concrete_type_obligations(
            &application.to_symbolic(),
            metadata_dag,
            tir,
            definition.type_def.source(),
            definition.type_def.span(),
            cancellation,
        )
    })?;
    Ok(())
}

fn validate_model_type_definition<'tir>(
    tir: &'tir dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    generic_args: &[CheckedGenericArg],
) -> Result<ModelTypeDefinition<'tir>, ConcreteModelTypeError> {
    let type_def = validate_nominal_signature(tir, identity, generic_args)?;
    let NominalTypeKind::Union { members } = type_def.kind() else {
        return Err(ConcreteModelTypeError::RequiredType {
            identity: identity.clone(),
        });
    };
    Ok(ModelTypeDefinition {
        type_def,
        constructors: members,
    })
}

fn validate_nominal_signature<'tir>(
    tir: &'tir dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    generic_args: &[CheckedGenericArg],
) -> Result<&'tir NominalTypeDef, ConcreteModelTypeError> {
    let type_def = tir.struct_type_def(identity.resolved()).ok_or_else(|| {
        ConcreteModelTypeError::UnknownType {
            identity: identity.clone(),
        }
    })?;
    if generic_args.len() != type_def.generic_params().len() {
        return Err(ConcreteModelTypeError::GenericArityMismatch {
            identity: identity.clone(),
            expected: type_def.generic_params().len(),
            actual: generic_args.len(),
        });
    }
    for (parameter, argument) in type_def.generic_params().iter().zip(generic_args) {
        let actual = generic_argument_sort(argument);
        if parameter.constraint() != actual {
            return Err(ConcreteModelTypeError::GenericSortMismatch {
                identity: identity.clone(),
                parameter: parameter.name().clone(),
                expected: parameter.constraint(),
                actual,
            });
        }
        validate_generic_argument_shape(tir, identity, parameter.name(), argument)?;
    }
    Ok(type_def)
}

const fn generic_argument_sort(argument: &CheckedGenericArg) -> GenericConstraint {
    match argument {
        CheckedGenericArg::Dim(_) => GenericConstraint::Dim,
        CheckedGenericArg::Index(_) => GenericConstraint::Index,
        CheckedGenericArg::Nat(_) => GenericConstraint::Nat,
        CheckedGenericArg::Type(_) => GenericConstraint::Type,
    }
}

fn validate_generic_argument_shape(
    tir: &dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    parameter: &GenericParamName,
    argument: &CheckedGenericArg,
) -> Result<(), ConcreteModelTypeError> {
    match argument {
        CheckedGenericArg::Dim(_) | CheckedGenericArg::Nat(_) => Ok(()),
        CheckedGenericArg::Index(index) => validate_index_reference(tir, index),
        CheckedGenericArg::Type(declared_type) => {
            validate_type_argument_shape(tir, identity, parameter, declared_type)
        }
    }
}

fn validate_type_argument_shape(
    tir: &dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    parameter: &GenericParamName,
    declared_type: &CheckedType,
) -> Result<(), ConcreteModelTypeError> {
    match declared_type {
        CheckedType::Struct(nested_identity, nested_args) => {
            validate_nominal_signature(tir, nested_identity, nested_args).map(|_| ())
        }
        CheckedType::Key(index) => validate_index_reference(tir, index),
        CheckedType::Indexed { .. } => Err(ConcreteModelTypeError::IndexedTypeArgument {
            identity: identity.clone(),
            parameter: parameter.clone(),
        }),
        CheckedType::Quantity(_)
        | CheckedType::Complex(_)
        | CheckedType::Bool
        | CheckedType::Int
        | CheckedType::Datetime(_) => Ok(()),
    }
}

fn validate_bound_generic_arguments(
    tir: &dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    generic_args: &[CheckedGenericArg],
) -> Result<(), ConcreteModelTypeError> {
    let type_def = validate_nominal_signature(tir, identity, generic_args)?;
    type_def
        .generic_params()
        .iter()
        .zip(generic_args)
        .try_for_each(|(parameter, argument)| {
            validate_bound_generic_argument(tir, identity, parameter.name(), argument)
        })
}

fn validate_bound_generic_argument(
    tir: &dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    parameter: &GenericParamName,
    argument: &CheckedGenericArg,
) -> Result<(), ConcreteModelTypeError> {
    match argument {
        CheckedGenericArg::Dim(_) | CheckedGenericArg::Nat(_) => Ok(()),
        CheckedGenericArg::Index(index) => validate_bound_index(tir, index),
        CheckedGenericArg::Type(declared_type) => {
            validate_bound_type_argument(tir, identity, parameter, declared_type)
        }
    }
}

fn validate_bound_type_argument(
    tir: &dyn crate::tir::typed::TirRead,
    identity: &StructTypeRef,
    parameter: &GenericParamName,
    declared_type: &CheckedType,
) -> Result<(), ConcreteModelTypeError> {
    match declared_type {
        CheckedType::Struct(nested_identity, nested_args) => {
            let type_def = validate_nominal_signature(tir, nested_identity, nested_args)?;
            if matches!(type_def.kind(), NominalTypeKind::Required) {
                return Err(ConcreteModelTypeError::RequiredType {
                    identity: nested_identity.clone(),
                });
            }
            validate_bound_generic_arguments(tir, nested_identity, nested_args)
        }
        CheckedType::Key(index) => validate_bound_index(tir, index),
        CheckedType::Indexed { .. } => Err(ConcreteModelTypeError::IndexedTypeArgument {
            identity: identity.clone(),
            parameter: parameter.clone(),
        }),
        CheckedType::Quantity(_)
        | CheckedType::Complex(_)
        | CheckedType::Bool
        | CheckedType::Int
        | CheckedType::Datetime(_) => Ok(()),
    }
}

fn validate_index_reference(
    tir: &dyn crate::tir::typed::TirRead,
    index: &IndexTypeRef,
) -> Result<(), ConcreteModelTypeError> {
    index_definition(tir, index).map(|_| ())
}

fn validate_bound_index(
    tir: &dyn crate::tir::typed::TirRead,
    index: &IndexTypeRef,
) -> Result<(), ConcreteModelTypeError> {
    let Some(definition) = index_definition(tir, index)? else {
        return Ok(());
    };
    if definition.is_required() {
        return Err(ConcreteModelTypeError::RequiredIndex {
            index: index.clone(),
        });
    }
    Ok(())
}

fn index_definition<'tir>(
    tir: &'tir dyn crate::tir::typed::TirRead,
    index: &IndexTypeRef,
) -> Result<Option<&'tir crate::semantic::index_def::IndexDef>, ConcreteModelTypeError> {
    if index.finite_index().is_some() {
        return Ok(None);
    }
    let Some(resolved) = index.declared_resolved() else {
        return Err(ConcreteModelTypeError::NonConcreteIndex {
            index: index.clone(),
        });
    };
    tir.declared_index_def(resolved)
        .map(Some)
        .ok_or_else(|| ConcreteModelTypeError::UnknownIndex {
            index: index.clone(),
        })
}

/// One concrete field after applying all nominal generic arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcreteModelField {
    name: FieldName,
    declared_type: CheckedType,
}

impl ConcreteModelField {
    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    #[must_use]
    pub const fn declared_type(&self) -> &CheckedType {
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
