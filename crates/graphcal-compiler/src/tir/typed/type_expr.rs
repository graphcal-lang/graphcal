use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::Dimension;
use crate::generic_param::GenericArgArity;
use crate::hir::nominal::{NominalGenericParam, NominalTypeDef};
use crate::resolve::error::ModuleResolveError;
use crate::resolved_name::{ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::module::ModuleError;
use crate::semantic_error::structure::StructError;
use crate::source_id::SourceId;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::index_name::IndexName;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;
use crate::syntax::type_name::StructTypeName;

use super::{
    ModuleTypeContext, ProjectTypeStore, ResolvedDeclType, ResolvedDim, ResolvedDimTerm,
    ResolvedGenericArg, ResolvedIndex, ResolvedValueType, Substitution,
};

// ---------------------------------------------------------------------------
// Type resolution
// ---------------------------------------------------------------------------

pub(super) fn module_resolve_error(
    err: &ModuleResolveError,
    src: SourceId,
    span: Span,
) -> SemanticError {
    SemanticError::located(src, span, ModuleError::resolution(err.clone()))
}

pub(super) fn internal_error(message: String, src: SourceId, span: Span) -> SemanticError {
    SemanticError::internal_error(
        message,
        src,
        crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
    )
}

#[derive(Clone, Copy)]
struct HirTypeResolutionContext<'a> {
    src: SourceId,
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
    decl_type: &crate::hir::types::DeclType,
    src: SourceId,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedDeclType, SemanticError> {
    resolve_hir_decl_type_with_project_types(decl_type, src, module_ctx.types)
}

pub(super) fn resolve_hir_decl_type_with_project_types(
    decl_type: &crate::hir::types::DeclType,
    src: SourceId,
    project_types: &ProjectTypeStore,
) -> Result<ResolvedDeclType, SemanticError> {
    let ctx = HirTypeResolutionContext { src, project_types };
    match decl_type {
        crate::hir::types::DeclType::Value(value_type) => {
            resolve_hir_value_type(value_type, ctx).map(ResolvedDeclType::Value)
        }
        crate::hir::types::DeclType::Indexed {
            element, indexes, ..
        } => Ok(ResolvedDeclType::Indexed {
            element: resolve_hir_value_type(element, ctx)?,
            indexes: indexes.try_map_ref(|index| resolve_hir_index_ref(index, ctx))?,
        }),
    }
}

fn resolve_hir_value_type(
    value_type: &crate::hir::types::ValueType,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedValueType, SemanticError> {
    match &value_type.kind {
        crate::hir::types::ValueTypeKind::Builtin(builtin) => {
            Ok(resolve_hir_builtin_type(*builtin))
        }
        crate::hir::types::ValueTypeKind::DimExpr(dim_expr) => {
            resolve_hir_dim_expr(dim_expr, ctx).map(ResolvedValueType::Quantity)
        }
        crate::hir::types::ValueTypeKind::Complex(dimension) => Ok(ResolvedValueType::Complex {
            dimension: resolve_hir_dim_arg(dimension, ctx)?,
            span: value_type.span,
        }),
        crate::hir::types::ValueTypeKind::Key(index) => Ok(ResolvedValueType::Key {
            index: resolve_hir_index_ref(index, ctx)?,
            span: value_type.span,
        }),
        crate::hir::types::ValueTypeKind::Struct(name) => {
            hir_struct_type_def(&name.value, name.span, ctx)?;
            Ok(ResolvedValueType::Struct {
                name: name.value.clone(),
                generic_args: Vec::new(),
                span: name.span,
            })
        }
        crate::hir::types::ValueTypeKind::GenericTypeParam(param) => Ok(
            ResolvedValueType::GenericTypeParam(param.value.clone(), param.span),
        ),
        crate::hir::types::ValueTypeKind::TypeApplication { name, generic_args } => {
            resolve_hir_type_application(value_type, name, generic_args, ctx)
        }
    }
}

const fn resolve_hir_builtin_type(builtin: crate::hir::types::BuiltinType) -> ResolvedValueType {
    match builtin {
        crate::hir::types::BuiltinType::Dimensionless => {
            ResolvedValueType::Quantity(ResolvedDim::dimensionless())
        }
        crate::hir::types::BuiltinType::Bool => ResolvedValueType::Bool,
        crate::hir::types::BuiltinType::Int => ResolvedValueType::Int,
        crate::hir::types::BuiltinType::Datetime(scale) => ResolvedValueType::Datetime(scale),
    }
}

fn hir_dimension(
    name: &ResolvedDimName,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<Dimension, SemanticError> {
    ctx.project_types
        .get_dimension(name)
        .cloned()
        .ok_or_else(|| {
            SemanticError::located(
                ctx.src,
                span,
                DimensionError::UnknownDimension {
                    name: NamePath::from(name.atom().clone()),
                },
            )
        })
}

fn hir_index_name(
    name: &ResolvedIndexName,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<IndexName, SemanticError> {
    if ctx.project_types.get_index(name).is_some() {
        Ok(name.to_unowned_def_name())
    } else {
        Err(SemanticError::located(
            ctx.src,
            span,
            IndexError::UnknownIndex {
                name: name.to_unowned_def_name().into(),
            },
        ))
    }
}

fn hir_struct_type_def<'a>(
    name: &ResolvedStructTypeName,
    span: Span,
    ctx: HirTypeResolutionContext<'a>,
) -> Result<&'a NominalTypeDef, SemanticError> {
    ctx.project_types.get_struct_type(name).ok_or_else(|| {
        SemanticError::located(
            ctx.src,
            span,
            StructError::UnknownStructType {
                name: name.to_string(),
            },
        )
    })
}

fn resolve_hir_dim_expr(
    dim_expr: &crate::hir::types::DimExpr,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDim, SemanticError> {
    let terms = dim_expr
        .terms
        .iter()
        .map(|item| resolve_hir_dim_expr_item(item, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    // The terms fold to a concrete dimension exactly when none is generic.
    let concrete = terms
        .iter()
        .map(|term| match term {
            ResolvedDimTerm::Concrete { dim, power, op } => Some((dim.clone(), *power, *op)),
            ResolvedDimTerm::GenericParam { .. } => None,
        })
        .collect::<Option<Vec<_>>>();
    let Some(concrete) = concrete else {
        return Ok(ResolvedDim::Symbolic {
            terms,
            span: dim_expr.span,
        });
    };

    let result = concrete.into_iter().try_fold(
        Dimension::dimensionless(),
        |acc, (dim, power, op)| -> Result<Dimension, SemanticError> {
            let overflow_err = || {
                SemanticError::located(ctx.src, dim_expr.span, DimensionError::DimensionOverflow)
            };
            let powered = dim.pow(power).map_err(|_| overflow_err())?;
            match op {
                MulDivOp::Mul => (acc * powered).map_err(|_| overflow_err()),
                MulDivOp::Div => (acc / powered).map_err(|_| overflow_err()),
            }
        },
    )?;
    Ok(ResolvedDim::Concrete(result))
}

fn resolve_hir_dim_expr_item(
    item: &crate::hir::types::DimExprItem,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDimTerm, SemanticError> {
    let power = item.term.power;
    match &item.term.target {
        crate::hir::types::DimTermTarget::Dimension(name) => Ok(ResolvedDimTerm::Concrete {
            dim: hir_dimension(&name.value, name.span, ctx)?,
            power,
            op: item.op,
        }),
        crate::hir::types::DimTermTarget::GenericParam(param) => {
            Ok(ResolvedDimTerm::GenericParam {
                name: param.value.clone(),
                power,
                op: item.op,
                span: item.term.span,
            })
        }
    }
}

fn resolve_hir_index_ref(
    index: &crate::hir::types::IndexRef,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedIndex, SemanticError> {
    match index {
        crate::hir::types::IndexRef::Concrete(name) => {
            hir_index_name(&name.value, name.span, ctx)?;
            Ok(ResolvedIndex::Concrete(name.value.clone(), name.span))
        }
        crate::hir::types::IndexRef::GenericParam(param) => {
            Ok(ResolvedIndex::GenericParam(param.value.clone(), param.span))
        }
        crate::hir::types::IndexRef::Finite(cardinality) => Ok(ResolvedIndex::Finite(
            cardinality.value.clone(),
            cardinality.span,
        )),
    }
}

/// Validate the generic-argument count for a type application: enough to
/// reach the last non-defaulted parameter, and at most the total count.
/// Shared by the HIR and syntax type-application resolvers.
fn check_type_application_arity(
    type_name: StructTypeName,
    type_def: &NominalTypeDef,
    arg_count: usize,
    span: Span,
    src: SourceId,
) -> Result<(), SemanticError> {
    let arity = GenericArgArity::of_defaults(
        type_def
            .generic_params()
            .iter()
            .map(|param| param.default().is_some()),
    );
    if !arity.accepts(arg_count) {
        return Err(SemanticError::located(
            src,
            span,
            StructError::GenericArgCount {
                type_name,
                expected: arity,
                got: arg_count,
            },
        ));
    }
    Ok(())
}

fn resolve_hir_type_application(
    type_ann: &crate::hir::types::ValueType,
    name: &crate::syntax::span::Spanned<ResolvedStructTypeName>,
    generic_args: &[crate::hir::types::GenericArg],
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedValueType, SemanticError> {
    let type_def = hir_struct_type_def(&name.value, name.span, ctx)?;
    check_type_application_arity(
        name.value.to_unowned_def_name(),
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
        let default = param.default().ok_or_else(|| {
            SemanticError::located(
                ctx.src,
                type_ann.span,
                StructError::MissingGenericDefault {
                    param: param.name().clone(),
                },
            )
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
    arg: &crate::hir::types::GenericArg,
    src: SourceId,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedGenericArg, SemanticError> {
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
    arg: &crate::hir::types::GenericArg,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedGenericArg, SemanticError> {
    match (param.constraint(), arg) {
        (GenericConstraint::Dim, crate::hir::types::GenericArg::Dim(dim)) => {
            resolve_hir_dim_arg(dim, ctx).map(ResolvedGenericArg::Dim)
        }
        (GenericConstraint::Index, crate::hir::types::GenericArg::Index(index)) => {
            resolve_hir_index_ref(index, ctx).map(ResolvedGenericArg::Index)
        }
        (GenericConstraint::Nat, crate::hir::types::GenericArg::Nat(nat)) => {
            Ok(ResolvedGenericArg::Nat(nat.value.clone(), nat.span))
        }
        (GenericConstraint::Type, crate::hir::types::GenericArg::Type(value_type)) => {
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
    arg: &crate::hir::types::DimArg,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDim, SemanticError> {
    match arg {
        crate::hir::types::DimArg::Dimensionless(_) => Ok(ResolvedDim::dimensionless()),
        crate::hir::types::DimArg::Expr(dim_expr) => resolve_hir_dim_expr(dim_expr, ctx),
    }
}
