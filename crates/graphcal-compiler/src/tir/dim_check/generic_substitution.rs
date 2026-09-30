//! Concrete generic substitutions of nominal-type applications and the
//! field types they instantiate.

use crate::hir::nominal::{NominalConstructor, NominalGenericParam, NominalTypeDef};
use crate::hir::types::GenericParamId;
use crate::resolved_name::ResolvedStructTypeName;
use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::Symbolic;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::span::Span;
use crate::syntax::type_name::{FieldName, GenericParamName};

use crate::semantic::checked_type::{CheckedGenericArg, CheckedType};
use crate::tir::typed::Substitution;

pub(in crate::tir::dim_check) fn resolved_type_field_key(
    owning_type: &ResolvedStructTypeName,
    constructor: &NominalConstructor,
    field: &FieldName,
) -> crate::tir::typed::ResolvedStructFieldTypeKey {
    crate::tir::typed::ResolvedStructFieldTypeKey {
        owning_type: owning_type.clone(),
        constructor: constructor.name(),
        field: field.clone(),
    }
}

pub(in crate::tir::dim_check) fn generic_substitution_prefix(
    type_def: &NominalTypeDef,
    type_args: &[CheckedGenericArg<Symbolic>],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Substitution, GraphcalError> {
    if type_args.len() > type_def.generic_params().len() {
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` expects at most {} generic arguments, got {}",
                type_def.name(),
                type_def.generic_params().len(),
                type_args.len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }

    // A validated concrete argument, embedded into the symbolic form. An
    // indexed type is not a value type and so cannot bind a `Type` parameter.
    let bound = |param: &NominalGenericParam, arg: &CheckedGenericArg| {
        crate::tir::typed::declared_to_resolved_generic_arg(arg, span)
            .ok_or_else(|| generic_arg_internal_sort_error(param, src, span))
    };
    let mut subs = Substitution::default();
    for (param, arg) in type_def.generic_params().iter().zip(type_args) {
        match param.constraint() {
            GenericConstraint::Dim => match arg {
                CheckedGenericArg::Dim(dim) => subs.bind(
                    param.id().clone(),
                    bound(param, &CheckedGenericArg::Dim(dim.clone()))?,
                ),
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            GenericConstraint::Index => match arg {
                CheckedGenericArg::Index(index) => match index.to_concrete() {
                    Some(concrete) => subs.bind(
                        param.id().clone(),
                        bound(param, &CheckedGenericArg::Index(concrete))?,
                    ),
                    None => {
                        return Err(non_concrete_generic_argument(
                            param.name(),
                            &index.to_string(),
                            src,
                            span,
                        ));
                    }
                },
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            GenericConstraint::Nat => match arg {
                CheckedGenericArg::Nat(form) => {
                    let Some(value) = form.constant_value() else {
                        return Err(non_concrete_generic_argument(
                            param.name(),
                            &form.format(),
                            src,
                            span,
                        ));
                    };
                    subs.bind(
                        param.id().clone(),
                        bound(param, &CheckedGenericArg::Nat(value))?,
                    );
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            GenericConstraint::Type => match arg {
                CheckedGenericArg::Type(type_expr) => match type_expr.to_concrete() {
                    Some(concrete) => subs.bind(
                        param.id().clone(),
                        bound(param, &CheckedGenericArg::Type(concrete))?,
                    ),
                    None => {
                        return Err(non_concrete_generic_argument(
                            param.name(),
                            &format!("{type_expr:?}"),
                            src,
                            span,
                        ));
                    }
                },
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
        }
    }
    Ok(subs)
}

pub(in crate::tir::dim_check) fn concrete_generic_substitutions(
    type_def: &NominalTypeDef,
    type_args: &[CheckedGenericArg<Symbolic>],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ConcreteGenericSubstitutions, GraphcalError> {
    if type_args.len() != type_def.generic_params().len() {
        return Err(GraphcalError::EvalError {
            message: format!(
                "concrete type `{}` requires exactly {} generic arguments, got {}",
                type_def.name(),
                type_def.generic_params().len(),
                type_args.len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    let substitution = generic_substitution_prefix(type_def, type_args, src, span)?;
    let nats = type_def
        .generic_params()
        .iter()
        .zip(type_args)
        .filter_map(|(parameter, arg)| match arg {
            CheckedGenericArg::Nat(form) => form
                .constant_value()
                .map(|value| (parameter.id().clone(), value)),
            CheckedGenericArg::Dim(_)
            | CheckedGenericArg::Index(_)
            | CheckedGenericArg::Type(_) => None,
        })
        .collect();
    Ok(ConcreteGenericSubstitutions { substitution, nats })
}

pub(in crate::tir::dim_check) fn non_concrete_generic_argument(
    parameter: &GenericParamName,
    argument: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: format!("generic argument `{argument}` for `{parameter}` is not concrete"),
        src: src.clone(),
        span: span.into(),
    }
}

pub(in crate::tir::dim_check) fn generic_arg_internal_sort_error(
    param: &NominalGenericParam,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::InternalError {
        message: format!(
            "generic argument for `{}` does not match its registered sort",
            param.name()
        ),
        src: src.clone(),
        span: span.into(),
    }
}

/// Complete, sort-checked, concrete bindings for one nominal application.
///
/// This wrapper can only be constructed after exact arity, sort, and
/// concreteness validation.
#[derive(Clone)]
pub(in crate::tir::dim_check) struct ConcreteGenericSubstitutions {
    substitution: Substitution,
    nats: HashMap<GenericParamId, u64>,
}

impl ConcreteGenericSubstitutions {
    pub(in crate::tir::dim_check) const fn nats(&self) -> &HashMap<GenericParamId, u64> {
        &self.nats
    }

    pub(in crate::tir::dim_check) fn field_type(
        &self,
        resolved: &crate::tir::typed::ResolvedDeclType,
        src: &NamedSource<Arc<String>>,
    ) -> Result<CheckedType, GraphcalError> {
        instantiate_concrete_type(resolved, &self.substitution, src)
    }
}

/// Instantiate a symbolic type whose every generic parameter `substitution`
/// binds to a concrete argument.
pub(in crate::tir::dim_check) fn instantiate_concrete_type(
    resolved: &crate::tir::typed::ResolvedDeclType,
    substitution: &Substitution,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedType, GraphcalError> {
    let instantiated = substitution
        .apply(resolved)
        .map_err(|error| error.into_graphcal(src))?;
    instantiated.to_checked_type(src)
}

pub(in crate::tir::dim_check) fn instantiate_concrete_generic_arg(
    resolved: &crate::tir::typed::ResolvedGenericArg,
    substitution: &Substitution,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedGenericArg, GraphcalError> {
    let instantiated = substitution
        .apply_generic_arg(resolved)
        .map_err(|error| error.into_graphcal(src))?;
    crate::tir::typed::resolved_generic_arg_to_declared(&instantiated, src)
}

pub(in crate::tir::dim_check) fn resolved_field_type(
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
    type_def: &NominalTypeDef,
    type_args: &[CheckedGenericArg<Symbolic>],
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<CheckedType, GraphcalError> {
    let resolved =
        dag.semantic
            .type_defs
            .field_type(key)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "semantic type metadata missing field type for `{}.{}`",
                    key.constructor, key.field
                ),
                src: src.clone(),
                span: span.into(),
            })?;
    concrete_generic_substitutions(type_def, type_args, src, span)?.field_type(resolved, src)
}
