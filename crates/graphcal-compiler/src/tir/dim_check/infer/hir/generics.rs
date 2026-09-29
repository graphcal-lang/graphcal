//! Generic-argument inference and substitution for nominal types.

use crate::hir::nominal::{NominalConstructor, NominalGenericParam, NominalTypeDef};
use crate::hir::types::{
    BuiltinType, DimArg, DimExpr, DimTermTarget, GenericArg, GenericParamId, IndexRef, ValueType,
    ValueTypeKind,
};
use crate::resolved_name::ResolvedStructTypeName;
use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::registry::error::GraphcalError;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;
use crate::syntax::type_name::{FieldName, GenericParamName};

use crate::tir::dim_check::{InferredGenericArg, InferredType};

use super::context::InferEnv;
use super::nat_forms::{finite_index_error, resolve_hir_nat_form};

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

fn generic_substitution_prefix(
    type_def: &NominalTypeDef,
    type_args: &[InferredGenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<GenericSubstitutions, GraphcalError> {
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

    let mut subs = GenericSubstitutions::default();
    for (param, arg) in type_def.generic_params().iter().zip(type_args) {
        match param.constraint() {
            GenericConstraint::Dim => match arg {
                InferredGenericArg::Dim(dim) => {
                    subs.dims.insert(param.name().clone(), dim.clone());
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            GenericConstraint::Index => match arg {
                InferredGenericArg::Index(index) if inferred_index_is_concrete(index) => {
                    subs.indexes.insert(param.name().clone(), index.clone());
                }
                InferredGenericArg::Index(index) => {
                    return Err(non_concrete_generic_argument(
                        param.name(),
                        &index.to_string(),
                        src,
                        span,
                    ));
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            GenericConstraint::Nat => match arg {
                InferredGenericArg::Nat(form) if form.is_constant() => {
                    subs.nats.insert(param.name().clone(), form.constant());
                }
                InferredGenericArg::Nat(form) => {
                    return Err(non_concrete_generic_argument(
                        param.name(),
                        &form.format(),
                        src,
                        span,
                    ));
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            GenericConstraint::Type => match arg {
                InferredGenericArg::Type(type_expr) if inferred_type_is_concrete(type_expr) => {
                    subs.types.insert(param.name().clone(), type_expr.clone());
                }
                InferredGenericArg::Type(type_expr) => {
                    return Err(non_concrete_generic_argument(
                        param.name(),
                        &format!("{type_expr:?}"),
                        src,
                        span,
                    ));
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
        }
    }
    Ok(subs)
}

pub(in crate::tir::dim_check) fn concrete_generic_substitutions(
    type_def: &NominalTypeDef,
    type_args: &[InferredGenericArg],
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
    let values = generic_substitution_prefix(type_def, type_args, src, span)?;
    let nats = type_def
        .generic_params()
        .iter()
        .zip(type_args)
        .filter_map(|(parameter, arg)| match arg {
            InferredGenericArg::Nat(form) => Some((parameter.id().clone(), form.constant())),
            _ => None,
        })
        .collect();
    Ok(ConcreteGenericSubstitutions { values, nats })
}

fn inferred_index_is_concrete(index: &IndexTypeRef) -> bool {
    index
        .finite_index_form()
        .is_none_or(|form| form.is_constant())
}

fn inferred_type_is_concrete(inferred: &InferredType) -> bool {
    match inferred {
        InferredType::Key(index) => inferred_index_is_concrete(index),
        InferredType::Struct(_, args) => args.iter().all(|arg| match arg {
            InferredGenericArg::Dim(_) => true,
            InferredGenericArg::Index(index) => inferred_index_is_concrete(index),
            InferredGenericArg::Nat(form) => form.is_constant(),
            InferredGenericArg::Type(type_expr) => inferred_type_is_concrete(type_expr),
        }),
        InferredType::Indexed { element, index } => {
            inferred_index_is_concrete(index) && inferred_type_is_concrete(element)
        }
        InferredType::Quantity(_)
        | InferredType::Complex(_)
        | InferredType::Bool
        | InferredType::Int
        | InferredType::Datetime(_) => true,
    }
}

fn non_concrete_generic_argument(
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

fn generic_arg_internal_sort_error(
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

#[derive(Clone, Default)]
pub(super) struct GenericSubstitutions {
    dims: HashMap<GenericParamName, Dimension>,
    indexes: HashMap<GenericParamName, IndexTypeRef>,
    nats: HashMap<GenericParamName, u64>,
    types: HashMap<GenericParamName, InferredType>,
}

/// Complete, sort-checked, concrete bindings for one nominal application.
///
/// This wrapper can only be constructed after exact arity, sort, and
/// concreteness validation. Nat substitutions retain their lexical owner.
#[derive(Clone)]
pub(in crate::tir::dim_check) struct ConcreteGenericSubstitutions {
    values: GenericSubstitutions,
    nats: HashMap<GenericParamId, u64>,
}

impl ConcreteGenericSubstitutions {
    const fn bindings(&self) -> &GenericSubstitutions {
        &self.values
    }

    pub(in crate::tir::dim_check) const fn nats(&self) -> &HashMap<GenericParamId, u64> {
        &self.nats
    }

    pub(in crate::tir::dim_check) fn field_type(
        &self,
        resolved: &crate::tir::typed::ResolvedTypeExpr,
        src: &NamedSource<Arc<String>>,
    ) -> Result<InferredType, GraphcalError> {
        substitute_resolved_type_with_type_params(resolved, self.bindings(), src)
    }
}

pub(super) fn substitute_resolved_type_with_type_params(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
    subs: &GenericSubstitutions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    crate::tir::typed::substitute_resolved_type_with_types(
        resolved,
        &subs.dims,
        &subs.indexes,
        &subs.nats,
        &subs.types,
        src,
    )
}

fn substitute_resolved_generic_arg_with_type_params(
    resolved: &crate::tir::typed::ResolvedGenericArg,
    subs: &GenericSubstitutions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredGenericArg, GraphcalError> {
    crate::tir::typed::substitute_resolved_generic_arg(
        resolved,
        &subs.dims,
        &subs.indexes,
        &subs.nats,
        &subs.types,
        src,
    )
}

pub(in crate::tir::dim_check) fn resolved_field_type(
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
    type_def: &NominalTypeDef,
    type_args: &[InferredGenericArg],
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<InferredType, GraphcalError> {
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
    let subs = concrete_generic_substitutions(type_def, type_args, src, span)?;
    substitute_resolved_type_with_type_params(resolved, subs.bindings(), src)
}

impl InferEnv<'_> {
    fn infer_hir_generic_type_arg(
        &self,
        value_type: &ValueType,
    ) -> Result<InferredType, GraphcalError> {
        match &value_type.kind {
            ValueTypeKind::Builtin(BuiltinType::Dimensionless) => {
                Ok(InferredType::Quantity(Dimension::dimensionless()))
            }
            ValueTypeKind::Builtin(BuiltinType::Bool) => Ok(InferredType::Bool),
            ValueTypeKind::Builtin(BuiltinType::Int) => Ok(InferredType::Int),
            ValueTypeKind::Builtin(BuiltinType::Datetime(scale)) => {
                Ok(InferredType::Datetime(*scale))
            }
            ValueTypeKind::DimExpr(dim_expr) => {
                infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(InferredType::Quantity)
            }
            ValueTypeKind::Complex(dimension) => match dimension {
                DimArg::Dimensionless(_) => Ok(InferredType::Complex(Dimension::dimensionless())),
                DimArg::Expr(dim_expr) => {
                    infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(InferredType::Complex)
                }
            },
            ValueTypeKind::Key(index) => Ok(InferredType::Key(inferred_index_from_type_arg(
                index, self.src,
            )?)),
            ValueTypeKind::Struct(name) => Ok(InferredType::Struct(
                StructTypeRef::from_resolved(name.value.clone()),
                vec![],
            )),
            ValueTypeKind::GenericTypeParam(param) => Err(GraphcalError::EvalError {
                message: format!(
                    "generic type parameter `{}` is not concretely bound",
                    param.value.name
                ),
                src: self.src.clone(),
                span: param.span.into(),
            }),
            ValueTypeKind::TypeApplication { name, generic_args } => {
                let type_def = self
                    .dag
                    .semantic
                    .type_defs
                    .struct_types
                    .get(&name.value)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic type metadata missing generic type `{}`",
                            name.value
                        ),
                        src: self.src.clone(),
                        span: name.span.into(),
                    })?;
                Ok(InferredType::Struct(
                    StructTypeRef::from_resolved(name.value.clone()),
                    self.resolve_applied_generic_args(
                        &name.value,
                        type_def,
                        generic_args,
                        name.span,
                    )?,
                ))
            }
        }
    }

    fn infer_hir_sorted_generic_arg(
        &self,
        arg: &GenericArg,
    ) -> Result<InferredGenericArg, GraphcalError> {
        match arg {
            GenericArg::Dim(DimArg::Dimensionless(_)) => {
                Ok(InferredGenericArg::Dim(Dimension::dimensionless()))
            }
            GenericArg::Dim(DimArg::Expr(dim_expr)) => {
                infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(InferredGenericArg::Dim)
            }
            GenericArg::Index(index) => {
                inferred_index_from_type_arg(index, self.src).map(InferredGenericArg::Index)
            }
            GenericArg::Nat(nat) => {
                resolve_hir_nat_form(nat, self.src).map(InferredGenericArg::Nat)
            }
            GenericArg::Type(value_type) => self
                .infer_hir_generic_type_arg(value_type)
                .map(InferredGenericArg::Type),
        }
    }
}

fn inferred_index_from_type_arg(
    index: &IndexRef,
    src: &NamedSource<Arc<String>>,
) -> Result<IndexTypeRef, GraphcalError> {
    match index {
        IndexRef::Concrete(name) => Ok(IndexTypeRef::from_resolved(name.value.clone())),
        IndexRef::GenericParam(param) => Err(GraphcalError::EvalError {
            message: format!(
                "generic index parameter `{}` is not concretely bound",
                param.value.name
            ),
            src: src.clone(),
            span: param.span.into(),
        }),
        IndexRef::Finite(nat_expr) => {
            let form = resolve_hir_nat_form(nat_expr, src)?;
            IndexTypeRef::from_finite_index_form(form)
                .map_err(|err| finite_index_error(err, src, nat_expr.span()))
        }
    }
}

fn infer_hir_dim_expr_arg(
    dim_expr: &DimExpr,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    dim_expr
        .terms
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, item| {
            let (dim, power, span) = match &item.term.target {
                DimTermTarget::Dimension(target) => {
                    let dim = tir.dimension(&target.value).cloned().ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: NamePath::from(target.value.atom().clone()),
                            src: src.clone(),
                            span: target.span.into(),
                        }
                    })?;
                    (dim, item.term.power, item.term.span)
                }
                DimTermTarget::GenericParam(param) => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic dimension parameter `{}` is not concretely bound",
                            param.value.name
                        ),
                        src: src.clone(),
                        span: param.span.into(),
                    });
                }
            };
            let powered = dim
                .pow(power)
                .map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: span.into(),
                })?;
            match item.op {
                crate::desugar::desugared_ast::MulDivOp::Mul => {
                    acc.checked_mul(&powered)
                        .map_err(|_| GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: span.into(),
                        })
                }
                crate::desugar::desugared_ast::MulDivOp::Div => {
                    acc.checked_div(&powered)
                        .map_err(|_| GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: span.into(),
                        })
                }
            }
        })
}

impl InferEnv<'_> {
    pub(super) fn resolve_applied_generic_args(
        &self,
        owning_type: &ResolvedStructTypeName,
        type_def: &NominalTypeDef,
        applied_generic_args: &[GenericArg],
        span: Span,
    ) -> Result<Vec<InferredGenericArg>, GraphcalError> {
        if applied_generic_args.is_empty() && type_def.generic_params().is_empty() {
            return Ok(Vec::new());
        }
        let total_params = type_def.generic_params().len();
        let required_count = type_def
            .generic_params()
            .iter()
            .rposition(|param| param.default().is_none())
            .map_or(0, |index| index.saturating_add(1));
        if applied_generic_args.len() < required_count || applied_generic_args.len() > total_params
        {
            let hint = if required_count == total_params {
                format!("{total_params}")
            } else {
                format!("{required_count}..{total_params}")
            };
            return Err(GraphcalError::EvalError {
                message: format!(
                    "type `{}` expects {hint} generic argument(s), got {}",
                    type_def.name(),
                    applied_generic_args.len()
                ),
                src: self.src.clone(),
                span: span.into(),
            });
        }
        let mut args = Vec::with_capacity(total_params);
        for (param, arg) in type_def.generic_params().iter().zip(applied_generic_args) {
            let inferred = self.infer_hir_sorted_generic_arg(arg)?;
            let matches_sort = matches!(
                (param.constraint(), &inferred),
                (GenericConstraint::Dim, InferredGenericArg::Dim(_))
                    | (GenericConstraint::Index, InferredGenericArg::Index(_))
                    | (GenericConstraint::Nat, InferredGenericArg::Nat(_))
                    | (GenericConstraint::Type, InferredGenericArg::Type(_))
            );
            if !matches_sort {
                return Err(generic_arg_internal_sort_error(param, self.src, arg.span()));
            }
            args.push(inferred);
        }
        for param in type_def
            .generic_params()
            .iter()
            .skip(applied_generic_args.len())
        {
            let resolved_default = &self
                .dag
                .semantic
                .type_defs
                .generic_defaults
                .get(&(owning_type.clone(), param.name().clone()))
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!(
                        "internal: generic parameter `{}` has no default",
                        param.name()
                    ),
                    src: self.src.clone(),
                    span: span.into(),
                })?
                .resolved;
            let subs = generic_substitution_prefix(type_def, &args, self.src, span)?;
            args.push(substitute_resolved_generic_arg_with_type_params(
                resolved_default,
                &subs,
                self.src,
            )?);
        }
        Ok(args)
    }
}
