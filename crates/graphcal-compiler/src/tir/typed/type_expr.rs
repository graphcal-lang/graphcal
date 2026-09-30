use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::Dimension;
use crate::graphcal_error::GraphcalError;
use crate::hir;
use crate::hir::{NominalGenericParam, NominalTypeDef};
use crate::resolve::error::ModuleResolveError;
use crate::resolved_name::{ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName};
use crate::syntax::ast::GenericConstraint;
use crate::syntax::index_name::IndexName;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;

use super::{
    ModuleTypeContext, ProjectTypeStore, ResolvedDeclType, ResolvedDim, ResolvedDimTerm,
    ResolvedGenericArg, ResolvedIndex, ResolvedValueType, Substitution,
};

// ---------------------------------------------------------------------------
// Type resolution
// ---------------------------------------------------------------------------

pub(super) fn module_resolve_error(
    err: &ModuleResolveError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

pub(super) fn internal_error(
    message: String,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::InternalError {
        message,
        src: src.clone(),
        span: span.into(),
    }
}

#[derive(Clone, Copy)]
struct HirTypeResolutionContext<'a> {
    src: &'a NamedSource<Arc<String>>,
    project_types: &'a ProjectTypeStore,
}

/// Resolve an already-lowered HIR declaration type into the TIR type
/// representation.
///
/// This is the new semantic entry point for module-aware TIR type resolution:
/// source paths should be lowered to HIR first, then TIR consumes canonical
/// `ResolvedName<Ns>` and lexical generic IDs from HIR instead of performing
/// source-path lookup itself.
pub fn resolve_hir_decl_type(
    decl_type: &hir::DeclType,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedDeclType, GraphcalError> {
    resolve_hir_decl_type_with_project_types(decl_type, src, module_ctx.types)
}

pub(super) fn resolve_hir_decl_type_with_project_types(
    decl_type: &hir::DeclType,
    src: &NamedSource<Arc<String>>,
    project_types: &ProjectTypeStore,
) -> Result<ResolvedDeclType, GraphcalError> {
    let ctx = HirTypeResolutionContext { src, project_types };
    match decl_type {
        hir::DeclType::Value(value_type) => {
            resolve_hir_value_type(value_type, ctx).map(ResolvedDeclType::Value)
        }
        hir::DeclType::Indexed {
            element, indexes, ..
        } => Ok(ResolvedDeclType::Indexed {
            element: resolve_hir_value_type(element, ctx)?,
            indexes: indexes.try_map_ref(|index| resolve_hir_index_ref(index, ctx))?,
        }),
    }
}

fn resolve_hir_value_type(
    value_type: &hir::ValueType,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedValueType, GraphcalError> {
    match &value_type.kind {
        hir::ValueTypeKind::Builtin(builtin) => Ok(resolve_hir_builtin_type(*builtin)),
        hir::ValueTypeKind::DimExpr(dim_expr) => {
            resolve_hir_dim_expr(dim_expr, ctx).map(ResolvedValueType::Quantity)
        }
        hir::ValueTypeKind::Complex(dimension) => Ok(ResolvedValueType::Complex {
            dimension: resolve_hir_dim_arg(dimension, ctx)?,
            span: value_type.span,
        }),
        hir::ValueTypeKind::Key(index) => Ok(ResolvedValueType::Key {
            index: resolve_hir_index_ref(index, ctx)?,
            span: value_type.span,
        }),
        hir::ValueTypeKind::Struct(name) => {
            hir_struct_type_def(&name.value, name.span, ctx)?;
            Ok(ResolvedValueType::Struct {
                name: name.value.clone(),
                generic_args: Vec::new(),
                span: name.span,
            })
        }
        hir::ValueTypeKind::GenericTypeParam(param) => Ok(ResolvedValueType::GenericTypeParam(
            param.value.clone(),
            param.span,
        )),
        hir::ValueTypeKind::TypeApplication { name, generic_args } => {
            resolve_hir_type_application(value_type, name, generic_args, ctx)
        }
    }
}

const fn resolve_hir_builtin_type(builtin: hir::BuiltinType) -> ResolvedValueType {
    match builtin {
        hir::BuiltinType::Dimensionless => {
            ResolvedValueType::Quantity(ResolvedDim::dimensionless())
        }
        hir::BuiltinType::Bool => ResolvedValueType::Bool,
        hir::BuiltinType::Int => ResolvedValueType::Int,
        hir::BuiltinType::Datetime(scale) => ResolvedValueType::Datetime(scale),
    }
}

fn hir_dimension(
    name: &ResolvedDimName,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<Dimension, GraphcalError> {
    ctx.project_types
        .get_dimension(name)
        .cloned()
        .ok_or_else(|| GraphcalError::UnknownDimension {
            name: NamePath::from(name.atom().clone()),
            src: ctx.src.clone(),
            span: span.into(),
        })
}

fn hir_index_name(
    name: &ResolvedIndexName,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<IndexName, GraphcalError> {
    if ctx.project_types.get_index(name).is_some() {
        Ok(name.to_unowned_def_name())
    } else {
        Err(GraphcalError::UnknownIndex {
            name: name.to_unowned_def_name().into(),
            src: ctx.src.clone(),
            span: span.into(),
        })
    }
}

fn hir_struct_type_def<'a>(
    name: &ResolvedStructTypeName,
    span: Span,
    ctx: HirTypeResolutionContext<'a>,
) -> Result<&'a NominalTypeDef, GraphcalError> {
    ctx.project_types
        .get_struct_type(name)
        .ok_or_else(|| GraphcalError::UnknownStructType {
            name: name.to_string(),
            src: ctx.src.clone(),
            span: span.into(),
        })
}

fn resolve_hir_dim_expr(
    dim_expr: &hir::DimExpr,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDim, GraphcalError> {
    let terms = dim_expr
        .terms
        .iter()
        .map(|item| resolve_hir_dim_expr_item(item, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    let has_generic = terms
        .iter()
        .any(|term| matches!(term, ResolvedDimTerm::GenericParam { .. }));
    if has_generic {
        return Ok(ResolvedDim::Symbolic {
            terms,
            span: dim_expr.span,
        });
    }

    let result = terms.iter().try_fold(
        Dimension::dimensionless(),
        |acc, term| -> Result<Dimension, GraphcalError> {
            let ResolvedDimTerm::Concrete { dim, power, op } = term else {
                return Err(GraphcalError::InternalError {
                    message: "generic dimension term reached concrete dimension folding"
                        .to_string(),
                    src: ctx.src.clone(),
                    span: dim_expr.span.into(),
                });
            };
            let overflow_err = || GraphcalError::DimensionOverflow {
                src: ctx.src.clone(),
                span: dim_expr.span.into(),
            };
            let powered = dim.pow(*power).map_err(|_| overflow_err())?;
            match op {
                MulDivOp::Mul => (acc * powered).map_err(|_| overflow_err()),
                MulDivOp::Div => (acc / powered).map_err(|_| overflow_err()),
            }
        },
    )?;
    Ok(ResolvedDim::Concrete(result))
}

fn resolve_hir_dim_expr_item(
    item: &hir::DimExprItem,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDimTerm, GraphcalError> {
    let power = item.term.power;
    match &item.term.target {
        hir::DimTermTarget::Dimension(name) => Ok(ResolvedDimTerm::Concrete {
            dim: hir_dimension(&name.value, name.span, ctx)?,
            power,
            op: item.op,
        }),
        hir::DimTermTarget::GenericParam(param) => Ok(ResolvedDimTerm::GenericParam {
            name: param.value.clone(),
            power,
            op: item.op,
            span: item.term.span,
        }),
    }
}

fn resolve_hir_index_ref(
    index: &hir::IndexRef,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedIndex, GraphcalError> {
    match index {
        hir::IndexRef::Concrete(name) => {
            hir_index_name(&name.value, name.span, ctx)?;
            Ok(ResolvedIndex::Concrete(name.value.clone(), name.span))
        }
        hir::IndexRef::GenericParam(param) => {
            Ok(ResolvedIndex::GenericParam(param.value.clone(), param.span))
        }
        hir::IndexRef::Finite(cardinality) => Ok(ResolvedIndex::Finite(
            cardinality.value.clone(),
            cardinality.span,
        )),
    }
}

/// Validate the generic-argument count for a type application: enough to
/// reach the last non-defaulted parameter, and at most the total count.
/// Shared by the HIR and syntax type-application resolvers.
fn check_type_application_arity(
    type_name: &str,
    type_def: &NominalTypeDef,
    arg_count: usize,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let total_params = type_def.generic_params().len();
    let required_count = type_def
        .generic_params()
        .iter()
        .rposition(|param| param.default().is_none())
        .map_or(0, |index| index.saturating_add(1));
    if arg_count < required_count || arg_count > total_params {
        let hint = if required_count == total_params {
            format!("{total_params}")
        } else {
            format!("{required_count}..{total_params}")
        };
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{type_name}` expects {hint} generic argument(s), got {arg_count}"
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    Ok(())
}

fn resolve_hir_type_application(
    type_ann: &hir::ValueType,
    name: &crate::syntax::span::Spanned<ResolvedStructTypeName>,
    generic_args: &[hir::GenericArg],
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedValueType, GraphcalError> {
    let type_def = hir_struct_type_def(&name.value, name.span, ctx)?;
    check_type_application_arity(
        name.value.as_str(),
        type_def,
        generic_args.len(),
        type_ann.span,
        ctx.src,
    )?;

    let mut resolved_args = Vec::with_capacity(type_def.generic_params().len());
    for (param, arg) in type_def.generic_params().iter().zip(generic_args) {
        resolved_args.push(resolve_hir_generic_arg_for_param(param, arg, ctx)?);
    }

    for param in type_def.generic_params().iter().skip(generic_args.len()) {
        let default = param.default().ok_or_else(|| GraphcalError::EvalError {
            message: format!(
                "internal: generic parameter `{}` has no default",
                param.name()
            ),
            src: ctx.src.clone(),
            span: type_ann.span.into(),
        })?;
        // A default may name earlier parameters; instantiate it with the
        // arguments resolved so far.
        let resolved = resolve_hir_generic_arg_for_param(param, default, ctx)?;
        let instantiated = Substitution::for_params(type_def.generic_params(), &resolved_args)
            .apply_generic_arg(&resolved)
            .map_err(|error| error.into_graphcal(ctx.src))?;
        resolved_args.push(instantiated);
    }

    Ok(ResolvedValueType::Struct {
        name: name.value.clone(),
        generic_args: resolved_args,
        span: type_ann.span,
    })
}

pub(super) fn resolve_hir_generic_arg(
    param: &NominalGenericParam,
    arg: &hir::GenericArg,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedGenericArg, GraphcalError> {
    resolve_hir_generic_arg_for_param(
        param,
        arg,
        HirTypeResolutionContext {
            src,
            project_types: module_ctx.types,
        },
    )
}

fn resolve_hir_generic_arg_for_param(
    param: &NominalGenericParam,
    arg: &hir::GenericArg,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedGenericArg, GraphcalError> {
    match (param.constraint(), arg) {
        (GenericConstraint::Dim, hir::GenericArg::Dim(dim)) => {
            resolve_hir_dim_arg(dim, ctx).map(ResolvedGenericArg::Dim)
        }
        (GenericConstraint::Index, hir::GenericArg::Index(index)) => {
            resolve_hir_index_ref(index, ctx).map(ResolvedGenericArg::Index)
        }
        (GenericConstraint::Nat, hir::GenericArg::Nat(nat)) => {
            Ok(ResolvedGenericArg::Nat(nat.value.clone(), nat.span))
        }
        (GenericConstraint::Type, hir::GenericArg::Type(value_type)) => {
            resolve_hir_value_type(value_type, ctx).map(ResolvedGenericArg::Type)
        }
        _ => Err(internal_error(
            format!(
                "HIR generic argument for `{}` does not match its registered sort",
                param.name()
            ),
            ctx.src,
            arg.span(),
        )),
    }
}

fn resolve_hir_dim_arg(
    arg: &hir::DimArg,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDim, GraphcalError> {
    match arg {
        hir::DimArg::Dimensionless(_) => Ok(ResolvedDim::dimensionless()),
        hir::DimArg::Expr(dim_expr) => resolve_hir_dim_expr(dim_expr, ctx),
    }
}
