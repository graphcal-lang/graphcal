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
use crate::registry::checked_type::{IndexTypeRef, StructTypeRef, Symbolic};
use crate::registry::error::GraphcalError;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;
use crate::syntax::type_name::{FieldName, GenericParamName};

use crate::registry::checked_type::{CheckedGenericArg, CheckedType};
use crate::tir::typed::Substitution;

use super::context::InferEnv;
use super::nat_forms::finite_index_error;

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
pub(super) fn instantiate_concrete_type(
    resolved: &crate::tir::typed::ResolvedDeclType,
    substitution: &Substitution,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedType, GraphcalError> {
    let instantiated = substitution
        .apply(resolved)
        .map_err(|error| error.into_graphcal(src))?;
    instantiated.to_checked_type(src)
}

fn instantiate_concrete_generic_arg(
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

impl InferEnv<'_> {
    fn infer_hir_generic_type_arg(
        &self,
        value_type: &ValueType,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        match &value_type.kind {
            ValueTypeKind::Builtin(BuiltinType::Dimensionless) => {
                Ok(CheckedType::Quantity(Dimension::dimensionless()))
            }
            ValueTypeKind::Builtin(BuiltinType::Bool) => Ok(CheckedType::Bool),
            ValueTypeKind::Builtin(BuiltinType::Int) => Ok(CheckedType::Int),
            ValueTypeKind::Builtin(BuiltinType::Datetime(scale)) => {
                Ok(CheckedType::Datetime(*scale))
            }
            ValueTypeKind::DimExpr(dim_expr) => {
                infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(CheckedType::Quantity)
            }
            ValueTypeKind::Complex(dimension) => match dimension {
                DimArg::Dimensionless(_) => Ok(CheckedType::Complex(Dimension::dimensionless())),
                DimArg::Expr(dim_expr) => {
                    infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(CheckedType::Complex)
                }
            },
            ValueTypeKind::Key(index) => Ok(CheckedType::Key(inferred_index_from_type_arg(
                index, self.src,
            )?)),
            ValueTypeKind::Struct(name) => Ok(CheckedType::Struct(
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
                Ok(CheckedType::Struct(
                    StructTypeRef::from_resolved(name.value.clone()),
                    self.resolve_applied_generic_args(type_def, generic_args, name.span)?,
                ))
            }
        }
    }

    fn infer_hir_sorted_generic_arg(
        &self,
        arg: &GenericArg,
    ) -> Result<CheckedGenericArg<Symbolic>, GraphcalError> {
        match arg {
            GenericArg::Dim(DimArg::Dimensionless(_)) => {
                Ok(CheckedGenericArg::Dim(Dimension::dimensionless()))
            }
            GenericArg::Dim(DimArg::Expr(dim_expr)) => {
                infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(CheckedGenericArg::Dim)
            }
            GenericArg::Index(index) => {
                inferred_index_from_type_arg(index, self.src).map(CheckedGenericArg::Index)
            }
            GenericArg::Nat(nat) => Ok(CheckedGenericArg::Nat(nat.value.clone())),
            GenericArg::Type(value_type) => self
                .infer_hir_generic_type_arg(value_type)
                .map(CheckedGenericArg::Type),
        }
    }
}

fn inferred_index_from_type_arg(
    index: &IndexRef,
    src: &NamedSource<Arc<String>>,
) -> Result<IndexTypeRef<Symbolic>, GraphcalError> {
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
        IndexRef::Finite(nat_expr) => IndexTypeRef::from_finite_index_form(nat_expr.value.clone())
            .map_err(|err| finite_index_error(err, src, nat_expr.span)),
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
        type_def: &NominalTypeDef,
        applied_generic_args: &[GenericArg],
        span: Span,
    ) -> Result<Vec<CheckedGenericArg<Symbolic>>, GraphcalError> {
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
                (GenericConstraint::Dim, CheckedGenericArg::Dim(_))
                    | (GenericConstraint::Index, CheckedGenericArg::Index(_))
                    | (GenericConstraint::Nat, CheckedGenericArg::Nat(_))
                    | (GenericConstraint::Type, CheckedGenericArg::Type(_))
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
                .get(param.id())
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
            args.push(
                instantiate_concrete_generic_arg(resolved_default, &subs, self.src)?.to_symbolic(),
            );
        }
        Ok(args)
    }
}
