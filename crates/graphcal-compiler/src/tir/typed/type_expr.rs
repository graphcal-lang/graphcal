use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{MulDivOp, TypeExpr, TypeExprKind};
use crate::dimension::{Dimension, Rational};
use crate::hir;
use crate::hir::diagnostics::hir_lower_error_to_graphcal;
use crate::nat::NatPolyForm;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{Registry, TypeDef, TypeGenericConstraint};
use crate::syntax::dimension::{DimName, ResolvedDimName};
use crate::syntax::index_name::{IndexName, ResolvedIndexName};
use crate::syntax::module_resolve::{ModuleResolveError, ModuleResolver};
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;
use crate::syntax::type_name::{GenericParamName, ResolvedStructTypeName, StructTypeName};

use super::{
    ModuleTypeContext, ModuleTypeRegistry, ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg,
    ResolvedIndex, ResolvedTypeExpr, nat_overflow_error, normalize_nat_expr,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Type resolution (single TypeExpr)
// ---------------------------------------------------------------------------

fn require_local_type_level_path<'a>(
    path: &'a NamePath,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<&'a str, GraphcalError> {
    path.as_bare()
        .map(NameAtom::as_str)
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!(
                "qualified type-level reference `{path}` needs module-aware resolution"
            ),
            src: src.clone(),
            span: span.into(),
        })
}

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

const fn module_lookup_is_absent(err: &ModuleResolveError) -> bool {
    matches!(err, ModuleResolveError::UnknownName { .. })
}

fn type_lower_error_to_graphcal(
    err: &hir::HirLowerError,
    type_ann: &TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    if let hir::HirLowerError::UnknownTypePath { path, span } = err {
        if type_expr_has_index_name_at_span(type_ann, *span)
            && let Ok(name) = IndexName::try_new(path.clone())
        {
            return GraphcalError::UnknownIndex {
                name,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        if type_expr_has_dim_term_at_span(type_ann, *span)
            && let Ok(name) = DimName::try_new(path.clone())
        {
            return GraphcalError::UnknownDimension {
                name,
                src: src.clone(),
                span: (*span).into(),
            };
        }
    }
    hir_lower_error_to_graphcal(err, src)
}

fn type_expr_has_index_name_at_span(type_ann: &TypeExpr, span: Span) -> bool {
    match &type_ann.kind {
        TypeExprKind::Indexed { base, indexes } => {
            type_expr_has_index_name_at_span(base, span)
                || indexes.iter().any(|index| match index {
                    crate::desugar::desugared_ast::IndexExpr::Name(name) => name.span == span,
                    crate::desugar::desugared_ast::IndexExpr::Finite { .. }
                    | crate::desugar::desugared_ast::IndexExpr::BareNat(_) => false,
                })
        }
        TypeExprKind::TypeApplication { generic_args, .. }
        | TypeExprKind::ComplexApplication { generic_args } => generic_args.iter().any(|arg| {
            matches!(
                arg,
                crate::desugar::desugared_ast::GenericArg::Type(type_expr)
                    if type_expr_has_index_name_at_span(type_expr, span)
            )
        }),
        TypeExprKind::DatetimeApplication { type_args } => type_args
            .iter()
            .any(|arg| type_expr_has_index_name_at_span(arg, span)),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => false,
    }
}

fn type_expr_has_dim_term_at_span(type_ann: &TypeExpr, span: Span) -> bool {
    match &type_ann.kind {
        TypeExprKind::DimExpr(dim_expr) => dim_expr
            .terms
            .iter()
            .any(|item| item.term.name.span == span),
        TypeExprKind::Indexed { base, .. } => type_expr_has_dim_term_at_span(base, span),
        TypeExprKind::TypeApplication { generic_args, .. }
        | TypeExprKind::ComplexApplication { generic_args } => generic_args.iter().any(|arg| {
            matches!(
                arg,
                crate::desugar::desugared_ast::GenericArg::Type(type_expr)
                    if type_expr_has_dim_term_at_span(type_expr, span)
            ) || matches!(
                arg,
                crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous)
                    if ambiguous.span() == span
            )
        }),
        TypeExprKind::DatetimeApplication { type_args } => type_args
            .iter()
            .any(|arg| type_expr_has_dim_term_at_span(arg, span)),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => false,
    }
}

#[derive(Clone, Copy)]
struct HirTypeResolutionContext<'a> {
    src: &'a NamedSource<Arc<String>>,
    resolver: &'a ModuleResolver,
    module_types: &'a ModuleTypeRegistry,
    registry: Option<&'a Registry>,
    prelude: &'a hir::PreludeTypeScope,
}

/// Resolve an already-lowered HIR type expression into the TIR type
/// representation.
///
/// This is the new semantic entry point for module-aware TIR type resolution:
/// source paths should be lowered to HIR first, then TIR consumes canonical
/// `ResolvedName<Ns>` and lexical generic IDs from HIR instead of performing
/// source-path lookup itself.
pub fn resolve_hir_type_expr(
    type_ann: &hir::TypeExpr,
    _registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let prelude = hir::PreludeTypeScope::graphcal();
    let ctx = HirTypeResolutionContext {
        src,
        resolver: module_ctx.resolver,
        module_types: module_ctx.types,
        registry: None,
        prelude: &prelude,
    };
    resolve_hir_type_expr_inner(type_ann, ctx)
}

fn resolve_ast_type_expr_via_hir(
    type_ann: &TypeExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let generic_scope = hir::GenericScope::new();
    let prelude = hir::PreludeTypeScope::graphcal();
    let lower_ctx =
        hir::TypeLoweringContext::new(module_ctx.owner, module_ctx.resolver, &generic_scope)
            .with_prelude(&prelude);
    let hir_type = hir::lower_type_expr(type_ann, lower_ctx)
        .map_err(|err| type_lower_error_to_graphcal(&err, type_ann, src))?;
    let resolve_ctx = HirTypeResolutionContext {
        src,
        resolver: module_ctx.resolver,
        module_types: module_ctx.types,
        registry: Some(registry),
        prelude: &prelude,
    };
    resolve_hir_type_expr_inner(&hir_type, resolve_ctx)
}

fn resolve_hir_type_expr_inner(
    type_ann: &hir::TypeExpr,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    match &type_ann.kind {
        hir::TypeExprKind::Builtin(builtin) => Ok(resolve_hir_builtin_type(*builtin)),
        hir::TypeExprKind::DimExpr(dim_expr) => resolve_hir_dim_expr(dim_expr, ctx),
        hir::TypeExprKind::Complex(dimension) => Ok(ResolvedTypeExpr::Complex {
            dimension: resolve_hir_dim_arg(dimension, ctx)?,
            span: type_ann.span,
        }),
        hir::TypeExprKind::Index(index) => Err(GraphcalError::EvalError {
            message: format!(
                "index `{}` cannot be used as a type",
                format_hir_index_ref(index)
            ),
            src: ctx.src.clone(),
            span: hir_index_ref_span(index).into(),
        }),
        hir::TypeExprKind::Struct(name) => {
            hir_struct_type_def(&name.value, name.span, ctx)?;
            Ok(ResolvedTypeExpr::Struct(name.value.clone(), name.span))
        }
        hir::TypeExprKind::GenericTypeParam(param) => Ok(ResolvedTypeExpr::GenericTypeParam(
            param.value.name.clone(),
            param.span,
        )),
        hir::TypeExprKind::TypeApplication { name, generic_args } => {
            resolve_hir_type_application(type_ann, name, generic_args, ctx)
        }
        hir::TypeExprKind::Indexed { base, indexes } => {
            let resolved_base = resolve_hir_type_expr_inner(base, ctx)?;
            let resolved_indexes = indexes
                .iter()
                .map(|index| resolve_hir_index_ref(index, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::Indexed {
                base: Box::new(resolved_base),
                indexes: resolved_indexes,
            })
        }
    }
}

const fn hir_index_ref_span(index: &hir::IndexRef) -> Span {
    match index {
        hir::IndexRef::Concrete(name) => name.span,
        hir::IndexRef::GenericParam(param) => param.span,
        hir::IndexRef::Finite(nat_expr) => nat_expr.span(),
    }
}

fn format_hir_index_ref(index: &hir::IndexRef) -> String {
    match index {
        hir::IndexRef::Concrete(name) => name.value.as_str().to_string(),
        hir::IndexRef::GenericParam(param) => param.value.name.to_string(),
        hir::IndexRef::Finite(nat_expr) => format!("Fin({})", format_hir_nat_expr(nat_expr)),
    }
}

fn format_hir_nat_expr(nat_expr: &hir::NatExpr) -> String {
    match nat_expr {
        hir::NatExpr::Literal(n, _) => n.to_string(),
        hir::NatExpr::Param(param) => param.value.name.to_string(),
        hir::NatExpr::Add(lhs, rhs, _) => {
            format!(
                "{} + {}",
                format_hir_nat_expr(lhs),
                format_hir_nat_expr(rhs)
            )
        }
        hir::NatExpr::Mul(lhs, rhs, _) => {
            format!(
                "{} * {}",
                format_hir_nat_expr(lhs),
                format_hir_nat_expr(rhs)
            )
        }
    }
}

const fn resolve_hir_builtin_type(builtin: hir::BuiltinType) -> ResolvedTypeExpr {
    match builtin {
        hir::BuiltinType::Dimensionless => ResolvedTypeExpr::Dimensionless,
        hir::BuiltinType::Bool => ResolvedTypeExpr::Bool,
        hir::BuiltinType::Int => ResolvedTypeExpr::Int,
        hir::BuiltinType::Datetime(scale) => ResolvedTypeExpr::Datetime(scale),
    }
}

fn hir_dimension(
    name: &ResolvedDimName,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<Dimension, GraphcalError> {
    ctx.module_types
        .get_dimension(name)
        .cloned()
        .or_else(|| {
            ctx.registry.and_then(|registry| {
                registry
                    .dimensions
                    .get_dimension(name.to_unowned_def_name().as_str())
                    .cloned()
            })
        })
        .ok_or_else(|| GraphcalError::UnknownDimension {
            name: name.to_unowned_def_name(),
            src: ctx.src.clone(),
            span: span.into(),
        })
}

fn hir_index_name(
    name: &ResolvedIndexName,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<IndexName, GraphcalError> {
    if ctx.module_types.get_index(name).is_some() {
        Ok(name.to_unowned_def_name())
    } else {
        Err(GraphcalError::UnknownIndex {
            name: name.to_unowned_def_name(),
            src: ctx.src.clone(),
            span: span.into(),
        })
    }
}

fn hir_struct_type_def<'a>(
    name: &ResolvedStructTypeName,
    span: Span,
    ctx: HirTypeResolutionContext<'a>,
) -> Result<&'a TypeDef, GraphcalError> {
    ctx.module_types
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
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let terms = dim_expr
        .terms
        .iter()
        .map(|item| resolve_hir_dim_expr_item(item, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    if let [
        ResolvedDimTerm::GenericParam {
            name,
            power,
            op: MulDivOp::Mul,
            span,
        },
    ] = terms.as_slice()
        && *power == Rational::ONE
    {
        return Ok(ResolvedTypeExpr::GenericDimParam(name.clone(), *span));
    }

    let has_generic = terms
        .iter()
        .any(|term| matches!(term, ResolvedDimTerm::GenericParam { .. }));
    if has_generic {
        return Ok(ResolvedTypeExpr::GenericDimExpr {
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
    Ok(ResolvedTypeExpr::Quantity(result))
}

fn resolve_hir_dim_expr_item(
    item: &hir::DimExprItem,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDimTerm, GraphcalError> {
    let power = item.term.power.unwrap_or(Rational::ONE);
    match &item.term.target {
        hir::DimTermTarget::Dimension(name) => Ok(ResolvedDimTerm::Concrete {
            dim: hir_dimension(&name.value, name.span, ctx)?,
            power,
            op: item.op,
        }),
        hir::DimTermTarget::GenericParam(param) => Ok(ResolvedDimTerm::GenericParam {
            name: param.value.name.clone(),
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
        hir::IndexRef::GenericParam(param) => Ok(ResolvedIndex::GenericParam(
            param.value.name.clone(),
            param.span,
        )),
        hir::IndexRef::Finite(nat_expr) => Ok(ResolvedIndex::Finite(
            normalize_hir_nat_expr(nat_expr)
                .map_err(|err| nat_overflow_error(err, ctx.src, nat_expr.span()))?,
            nat_expr.span(),
        )),
    }
}

fn normalize_hir_nat_expr(
    expr: &hir::NatExpr,
) -> Result<NatPolyForm, crate::nat::NatOverflowError> {
    match expr {
        hir::NatExpr::Literal(value, _) => Ok(NatPolyForm::from_constant(*value)),
        hir::NatExpr::Param(param) => Ok(NatPolyForm::from_var(param.value.name.clone())),
        hir::NatExpr::Add(lhs, rhs, _) => {
            normalize_hir_nat_expr(lhs)?.add(&normalize_hir_nat_expr(rhs)?)
        }
        hir::NatExpr::Mul(lhs, rhs, _) => {
            normalize_hir_nat_expr(lhs)?.mul(&normalize_hir_nat_expr(rhs)?)
        }
    }
}

#[derive(Default)]
struct ResolvedGenericSubstitutions {
    dims: HashMap<GenericParamName, ResolvedDimArg>,
    indexes: HashMap<GenericParamName, ResolvedIndex>,
    nats: HashMap<GenericParamName, NatPolyForm>,
    types: HashMap<GenericParamName, ResolvedTypeExpr>,
}

impl ResolvedGenericSubstitutions {
    fn from_resolved_prefix(type_def: &TypeDef, resolved_args: &[ResolvedGenericArg]) -> Self {
        type_def.generic_params.iter().zip(resolved_args).fold(
            Self::default(),
            |mut substitutions, (param, arg)| {
                match arg {
                    ResolvedGenericArg::Dim(dim) => {
                        substitutions.dims.insert(param.name.clone(), dim.clone());
                    }
                    ResolvedGenericArg::Index(index) => {
                        substitutions
                            .indexes
                            .insert(param.name.clone(), index.clone());
                    }
                    ResolvedGenericArg::Nat(form, _) => {
                        substitutions.nats.insert(param.name.clone(), form.clone());
                    }
                    ResolvedGenericArg::Type(type_expr) => {
                        substitutions
                            .types
                            .insert(param.name.clone(), type_expr.clone());
                    }
                }
                substitutions
            },
        )
    }
}

enum GenericDefaultSubstitutionError {
    Nat(crate::nat::NatOverflowError),
    Dimension(crate::dimension::RationalError),
    UnexpectedGenericDimensionTerm,
}

impl From<crate::nat::NatOverflowError> for GenericDefaultSubstitutionError {
    fn from(error: crate::nat::NatOverflowError) -> Self {
        Self::Nat(error)
    }
}

impl From<crate::dimension::RationalError> for GenericDefaultSubstitutionError {
    fn from(error: crate::dimension::RationalError) -> Self {
        Self::Dimension(error)
    }
}

fn instantiate_params_in_default(
    default: &mut ResolvedGenericArg,
    type_def: &TypeDef,
    resolved_args: &[ResolvedGenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    let substitutions = ResolvedGenericSubstitutions::from_resolved_prefix(type_def, resolved_args);
    substitute_params_in_generic_arg(default, &substitutions).map_err(|error| match error {
        GenericDefaultSubstitutionError::Nat(error) => nat_overflow_error(error, src, span),
        GenericDefaultSubstitutionError::Dimension(_error) => GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: span.into(),
        },
        GenericDefaultSubstitutionError::UnexpectedGenericDimensionTerm => internal_error(
            "generic dimension term remained after concrete-default substitution".to_string(),
            src,
            span,
        ),
    })
}

fn substitute_params_in_generic_arg(
    arg: &mut ResolvedGenericArg,
    substitutions: &ResolvedGenericSubstitutions,
) -> Result<(), GenericDefaultSubstitutionError> {
    match arg {
        ResolvedGenericArg::Dim(dim) => substitute_params_in_dim_arg(dim, substitutions),
        ResolvedGenericArg::Index(index) => {
            substitute_params_in_resolved_index(index, substitutions)
        }
        ResolvedGenericArg::Nat(form, _) => {
            *form = form.substitute_forms(&substitutions.nats)?;
            Ok(())
        }
        ResolvedGenericArg::Type(type_expr) => {
            substitute_params_in_resolved_type(type_expr, substitutions)
        }
    }
}

fn substitute_params_in_dim_arg(
    arg: &mut ResolvedDimArg,
    substitutions: &ResolvedGenericSubstitutions,
) -> Result<(), GenericDefaultSubstitutionError> {
    match arg {
        ResolvedDimArg::GenericParam(name, _) => {
            if let Some(replacement) = substitutions.dims.get(name) {
                *arg = replacement.clone();
            }
            Ok(())
        }
        ResolvedDimArg::Expr { terms, span } => {
            let expression_span = *span;
            let substituted =
                std::mem::take(terms)
                    .into_iter()
                    .try_fold(Vec::new(), |mut output, term| {
                        match term {
                            ResolvedDimTerm::GenericParam {
                                name,
                                power,
                                op,
                                span,
                            } => match substitutions.dims.get(&name) {
                                Some(replacement) => {
                                    output.extend(expand_dim_arg(replacement, power, op)?);
                                }
                                None => output.push(ResolvedDimTerm::GenericParam {
                                    name,
                                    power,
                                    op,
                                    span,
                                }),
                            },
                            concrete @ ResolvedDimTerm::Concrete { .. } => output.push(concrete),
                        }
                        Ok::<_, GenericDefaultSubstitutionError>(output)
                    })?;
            *arg = collapse_dim_terms(substituted, expression_span)?;
            Ok(())
        }
        ResolvedDimArg::Dimensionless | ResolvedDimArg::Concrete(_) => Ok(()),
    }
}

fn expand_dim_arg(
    arg: &ResolvedDimArg,
    outer_power: Rational,
    outer_op: MulDivOp,
) -> Result<Vec<ResolvedDimTerm>, GenericDefaultSubstitutionError> {
    match arg {
        ResolvedDimArg::Dimensionless => Ok(Vec::new()),
        ResolvedDimArg::Concrete(dim) => Ok(vec![ResolvedDimTerm::Concrete {
            dim: dim.clone(),
            power: outer_power,
            op: outer_op,
        }]),
        ResolvedDimArg::GenericParam(name, span) => Ok(vec![ResolvedDimTerm::GenericParam {
            name: name.clone(),
            power: outer_power,
            op: outer_op,
            span: *span,
        }]),
        ResolvedDimArg::Expr { terms, .. } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => Ok(ResolvedDimTerm::Concrete {
                    dim: dim.clone(),
                    power: (*power * outer_power)?,
                    op: combine_dim_ops(outer_op, *op),
                }),
                ResolvedDimTerm::GenericParam {
                    name,
                    power,
                    op,
                    span,
                } => Ok(ResolvedDimTerm::GenericParam {
                    name: name.clone(),
                    power: (*power * outer_power)?,
                    op: combine_dim_ops(outer_op, *op),
                    span: *span,
                }),
            })
            .collect(),
    }
}

const fn combine_dim_ops(outer: MulDivOp, inner: MulDivOp) -> MulDivOp {
    if matches!(
        (outer, inner),
        (MulDivOp::Mul, MulDivOp::Mul) | (MulDivOp::Div, MulDivOp::Div)
    ) {
        MulDivOp::Mul
    } else {
        MulDivOp::Div
    }
}

fn collapse_dim_terms(
    terms: Vec<ResolvedDimTerm>,
    span: Span,
) -> Result<ResolvedDimArg, GenericDefaultSubstitutionError> {
    if terms.is_empty() {
        return Ok(ResolvedDimArg::Dimensionless);
    }
    if terms
        .iter()
        .any(|term| matches!(term, ResolvedDimTerm::GenericParam { .. }))
    {
        return Ok(ResolvedDimArg::Expr { terms, span });
    }
    let dimension =
        terms
            .iter()
            .try_fold(Dimension::dimensionless(), |dimension, term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => {
                    let powered = dim.pow(*power)?;
                    match op {
                        MulDivOp::Mul => dimension.checked_mul(&powered).map_err(Into::into),
                        MulDivOp::Div => dimension.checked_div(&powered).map_err(Into::into),
                    }
                }
                ResolvedDimTerm::GenericParam { .. } => {
                    Err(GenericDefaultSubstitutionError::UnexpectedGenericDimensionTerm)
                }
            })?;
    if dimension.is_dimensionless() {
        Ok(ResolvedDimArg::Dimensionless)
    } else {
        Ok(ResolvedDimArg::Concrete(dimension))
    }
}

fn substitute_params_in_resolved_index(
    index: &mut ResolvedIndex,
    substitutions: &ResolvedGenericSubstitutions,
) -> Result<(), GenericDefaultSubstitutionError> {
    match index {
        ResolvedIndex::GenericParam(name, _) => {
            if let Some(replacement) = substitutions.indexes.get(name) {
                *index = replacement.clone();
            }
        }
        ResolvedIndex::Finite(form, _) => {
            *form = form.substitute_forms(&substitutions.nats)?;
        }
        ResolvedIndex::Concrete(_, _) => {}
    }
    Ok(())
}

fn dim_arg_as_resolved_type(arg: ResolvedDimArg) -> ResolvedTypeExpr {
    match arg {
        ResolvedDimArg::Dimensionless => ResolvedTypeExpr::Dimensionless,
        ResolvedDimArg::Concrete(dim) => ResolvedTypeExpr::Quantity(dim),
        ResolvedDimArg::GenericParam(name, span) => ResolvedTypeExpr::GenericDimParam(name, span),
        ResolvedDimArg::Expr { terms, span } => ResolvedTypeExpr::GenericDimExpr { terms, span },
    }
}

fn substitute_params_in_resolved_type(
    type_expr: &mut ResolvedTypeExpr,
    substitutions: &ResolvedGenericSubstitutions,
) -> Result<(), GenericDefaultSubstitutionError> {
    match type_expr {
        ResolvedTypeExpr::IndexArg(index) => {
            substitute_params_in_resolved_index(index, substitutions)
        }
        ResolvedTypeExpr::GenericDimParam(name, _) => {
            if let Some(replacement) = substitutions.dims.get(name) {
                *type_expr = dim_arg_as_resolved_type(replacement.clone());
            }
            Ok(())
        }
        ResolvedTypeExpr::GenericTypeParam(name, _) => {
            if let Some(replacement) = substitutions.types.get(name) {
                *type_expr = replacement.clone();
            }
            Ok(())
        }
        ResolvedTypeExpr::GenericDimExpr { terms, span } => {
            let mut dim_arg = ResolvedDimArg::Expr {
                terms: std::mem::take(terms),
                span: *span,
            };
            substitute_params_in_dim_arg(&mut dim_arg, substitutions)?;
            *type_expr = dim_arg_as_resolved_type(dim_arg);
            Ok(())
        }
        ResolvedTypeExpr::Complex { dimension, .. } => {
            substitute_params_in_dim_arg(dimension, substitutions)
        }
        ResolvedTypeExpr::GenericStruct { generic_args, .. } => generic_args
            .iter_mut()
            .try_for_each(|arg| substitute_params_in_generic_arg(arg, substitutions)),
        ResolvedTypeExpr::Indexed { base, indexes } => {
            substitute_params_in_resolved_type(base, substitutions)?;
            indexes
                .iter_mut()
                .try_for_each(|index| substitute_params_in_resolved_index(index, substitutions))
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::Quantity(_)
        | ResolvedTypeExpr::Struct(_, _) => Ok(()),
    }
}

/// Validate the generic-argument count for a type application: enough to
/// reach the last non-defaulted parameter, and at most the total count.
/// Shared by the HIR and syntax type-application resolvers.
fn check_type_application_arity(
    type_name: &str,
    type_def: &TypeDef,
    arg_count: usize,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let total_params = type_def.generic_params.len();
    let required_count = type_def
        .generic_params
        .iter()
        .rposition(|param| param.default.is_none())
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
    type_ann: &hir::TypeExpr,
    name: &crate::syntax::span::Spanned<ResolvedStructTypeName>,
    generic_args: &[hir::GenericArg],
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let type_def = hir_struct_type_def(&name.value, name.span, ctx)?;
    check_type_application_arity(
        name.value.as_str(),
        type_def,
        generic_args.len(),
        type_ann.span,
        ctx.src,
    )?;

    let mut resolved_args = Vec::with_capacity(type_def.generic_params.len());
    for (param, arg) in type_def.generic_params.iter().zip(generic_args) {
        resolved_args.push(resolve_hir_generic_arg_for_param(param, arg, ctx)?);
    }

    for param in type_def.generic_params.iter().skip(generic_args.len()) {
        let default = param
            .default
            .as_ref()
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: generic parameter `{}` has no default",
                    param.name
                ),
                src: ctx.src.clone(),
                span: type_ann.span.into(),
            })?;
        let default_hir = lower_generic_default(default, param, &name.value, type_def, ctx)?;
        let mut resolved = resolve_hir_generic_arg_for_param(param, &default_hir, ctx)?;
        instantiate_params_in_default(
            &mut resolved,
            type_def,
            &resolved_args,
            ctx.src,
            default_hir.span(),
        )?;
        resolved_args.push(resolved);
    }

    Ok(ResolvedTypeExpr::GenericStruct {
        name: name.value.clone(),
        generic_args: resolved_args,
        span: type_ann.span,
    })
}

fn resolve_hir_generic_arg_for_param(
    param: &crate::registry::types::TypeGenericParam,
    arg: &hir::GenericArg,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedGenericArg, GraphcalError> {
    match (param.constraint, arg) {
        (TypeGenericConstraint::Dim, hir::GenericArg::Dim(dim)) => {
            resolve_hir_dim_arg(dim, ctx).map(ResolvedGenericArg::Dim)
        }
        (TypeGenericConstraint::Index, hir::GenericArg::Index(index)) => {
            resolve_hir_index_ref(index, ctx).map(ResolvedGenericArg::Index)
        }
        (TypeGenericConstraint::Nat, hir::GenericArg::Nat(nat)) => Ok(ResolvedGenericArg::Nat(
            normalize_hir_nat_expr(nat)
                .map_err(|err| nat_overflow_error(err, ctx.src, nat.span()))?,
            nat.span(),
        )),
        (TypeGenericConstraint::Type, hir::GenericArg::Type(type_expr)) => {
            resolve_hir_type_expr_inner(type_expr, ctx).map(ResolvedGenericArg::Type)
        }
        _ => Err(internal_error(
            format!(
                "HIR generic argument for `{}` does not match its registered sort",
                param.name
            ),
            ctx.src,
            arg.span(),
        )),
    }
}

fn resolve_hir_dim_arg(
    arg: &hir::DimArg,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDimArg, GraphcalError> {
    match arg {
        hir::DimArg::Dimensionless(_) => Ok(ResolvedDimArg::Dimensionless),
        hir::DimArg::Expr(dim_expr) => match resolve_hir_dim_expr(dim_expr, ctx)? {
            ResolvedTypeExpr::Dimensionless => Ok(ResolvedDimArg::Dimensionless),
            ResolvedTypeExpr::Quantity(dim) => Ok(ResolvedDimArg::Concrete(dim)),
            ResolvedTypeExpr::GenericDimParam(name, span) => {
                Ok(ResolvedDimArg::GenericParam(name, span))
            }
            ResolvedTypeExpr::GenericDimExpr { terms, span } => {
                Ok(ResolvedDimArg::Expr { terms, span })
            }
            other => Err(internal_error(
                format!("dimension argument resolved to non-dimension type {other:?}"),
                ctx.src,
                arg.span(),
            )),
        },
    }
}

fn generic_scope_for_type(
    type_owner: &ResolvedStructTypeName,
    type_def: &TypeDef,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<hir::GenericScope, GraphcalError> {
    let mut scope = hir::GenericScope::new();
    for param in &type_def.generic_params {
        let constraint = generic_constraint(param.constraint);
        let id = hir::GenericParamId::new(
            hir::GenericParamOwner::Type(type_owner.clone()),
            param.name.clone(),
        );
        scope
            .insert_binding(hir::GenericParamBinding::new(id, constraint, span))
            .map_err(|err| hir_lower_error_to_graphcal(&err, ctx.src))?;
    }
    Ok(scope)
}

const fn generic_constraint(
    constraint: TypeGenericConstraint,
) -> crate::syntax::ast::GenericConstraint {
    match constraint {
        TypeGenericConstraint::Dim => crate::syntax::ast::GenericConstraint::Dim,
        TypeGenericConstraint::Index => crate::syntax::ast::GenericConstraint::Index,
        TypeGenericConstraint::Nat => crate::syntax::ast::GenericConstraint::Nat,
        TypeGenericConstraint::Type => crate::syntax::ast::GenericConstraint::Type,
    }
}

fn lower_type_expr_in_generic_scope(
    type_expr: &TypeExpr,
    type_owner: &ResolvedStructTypeName,
    type_def: &TypeDef,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<hir::TypeExpr, GraphcalError> {
    let scope = generic_scope_for_type(type_owner, type_def, type_expr.span, ctx)?;
    let lower_ctx = hir::TypeLoweringContext::new(type_owner.owner(), ctx.resolver, &scope)
        .with_prelude(ctx.prelude);
    hir::lower_type_expr(type_expr, lower_ctx)
        .map_err(|err| hir_lower_error_to_graphcal(&err, ctx.src))
}

fn lower_generic_default(
    default: &crate::desugar::desugared_ast::GenericArg,
    param: &crate::registry::types::TypeGenericParam,
    type_owner: &ResolvedStructTypeName,
    type_def: &TypeDef,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<hir::GenericArg, GraphcalError> {
    let mut scope = hir::GenericScope::new();
    for earlier in type_def
        .generic_params
        .iter()
        .take_while(|earlier| earlier.name != param.name)
    {
        let id = hir::GenericParamId::new(
            hir::GenericParamOwner::Type(type_owner.clone()),
            earlier.name.clone(),
        );
        scope
            .insert_binding(hir::GenericParamBinding::new(
                id,
                generic_constraint(earlier.constraint),
                default.span(),
            ))
            .map_err(|err| hir_lower_error_to_graphcal(&err, ctx.src))?;
    }
    let lower_ctx = hir::TypeLoweringContext::new(type_owner.owner(), ctx.resolver, &scope)
        .with_prelude(ctx.prelude);
    hir::lower::lower_generic_arg_for_constraint(
        default,
        generic_constraint(param.constraint),
        &param.name,
        lower_ctx,
    )
    .map_err(|err| hir_lower_error_to_graphcal(&err, ctx.src))
}

#[expect(
    clippy::too_many_arguments,
    reason = "resolves one AST index path against generic params, local registry, and module context"
)]
fn resolve_index_expr_name(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedIndex, GraphcalError> {
    if let Some(atom) = path.as_bare() {
        let text = atom.as_str();
        if nat_params.iter().any(|param| param.as_str() == text) {
            return Err(GraphcalError::ExpectedIndexFoundNat {
                expression: text.to_string(),
                src: src.clone(),
                span: span.into(),
            });
        }
        if let Some(gp) = index_params.iter().find(|p| p.as_str() == text) {
            return Ok(ResolvedIndex::GenericParam(gp.clone(), span));
        }
    }

    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_index_path(ctx.owner, path) {
            Ok(resolved) => {
                if ctx.types.get_index(&resolved).is_some() {
                    return Ok(ResolvedIndex::Concrete(resolved, span));
                }
                return Err(GraphcalError::UnknownIndex {
                    name: resolved.to_unowned_def_name(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Err(err) if path.is_bare() && module_lookup_is_absent(&err) => {}
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let text = require_local_type_level_path(path, span, src)?;
    if registry.indexes.get_index(text).is_some() {
        Ok(ResolvedIndex::Concrete(
            ResolvedIndexName::from_def(owner.clone(), IndexName::expect_valid(text)),
            span,
        ))
    } else {
        Err(GraphcalError::UnknownIndex {
            name: IndexName::expect_valid(text),
            src: src.clone(),
            span: span.into(),
        })
    }
}

fn resolve_concrete_index_path(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<Option<ResolvedIndexName>, GraphcalError> {
    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_index_path(ctx.owner, path) {
            Ok(resolved) => {
                let Some(index) = ctx.types.get_index(&resolved) else {
                    return Err(GraphcalError::UnknownIndex {
                        name: resolved.to_unowned_def_name(),
                        src: src.clone(),
                        span: span.into(),
                    });
                };
                if matches!(
                    index.kind,
                    crate::registry::types::IndexKind::Named { .. }
                        | crate::registry::types::IndexKind::RequiredNamed
                ) {
                    return Ok(Some(resolved));
                }
                return Ok(None);
            }
            Err(err) if module_lookup_is_absent(&err) => {}
            Err(_) if path.is_bare() => {
                // A bare non-local name may still be a prelude or registry-only
                // compatibility entry. Fall through to the leaf-keyed registry.
            }
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let Some(atom) = path.as_bare() else {
        return Ok(None);
    };
    let Some(index) = registry.indexes.get_index(atom.as_str()) else {
        return Ok(None);
    };
    Ok(matches!(
        index.kind,
        crate::registry::types::IndexKind::Named { .. }
            | crate::registry::types::IndexKind::RequiredNamed
    )
    .then(|| ResolvedIndexName::from_def(owner.clone(), IndexName::from_atom(atom.clone()))))
}

type ResolvedStructTypeLookup<'a> = Option<(ResolvedStructTypeName, &'a TypeDef)>;

fn resolve_struct_type_path<'a>(
    path: &NamePath,
    span: Span,
    registry: &'a Registry,
    owner: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'a>>,
) -> Result<ResolvedStructTypeLookup<'a>, GraphcalError> {
    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_struct_type_path(ctx.owner, path) {
            Ok(resolved) => {
                if let Some(type_def) = ctx.types.get_struct_type(&resolved) {
                    return Ok(Some((resolved, type_def)));
                }
                return Err(GraphcalError::UnknownStructType {
                    name: resolved.to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Err(err) if module_lookup_is_absent(&err) => {}
            Err(_err) if path.is_bare() => {}
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let Some(atom) = path.as_bare() else {
        return Ok(None);
    };
    Ok(registry.types.get_type(atom.as_str()).map(|type_def| {
        (
            ResolvedStructTypeName::from_def(
                owner.clone(),
                StructTypeName::from_atom(atom.clone()),
            ),
            type_def,
        )
    }))
}

fn resolve_dimension_path(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<Option<Dimension>, GraphcalError> {
    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_dimension_path(ctx.owner, path) {
            Ok(resolved) => {
                return ctx
                    .types
                    .get_dimension(&resolved)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| GraphcalError::UnknownDimension {
                        name: resolved.to_unowned_def_name(),
                        src: src.clone(),
                        span: span.into(),
                    });
            }
            Err(err) if path.is_bare() && module_lookup_is_absent(&err) => {}
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let text = require_local_type_level_path(path, span, src)?;
    Ok(registry.dimensions.get_dimension(text).cloned())
}

pub(super) fn resolve_generic_default_in_struct_scope(
    default: &crate::desugar::desugared_ast::GenericArg,
    param: &crate::registry::types::TypeGenericParam,
    type_owner: &ResolvedStructTypeName,
    type_def: &TypeDef,
    ctx: ModuleTypeContext<'_>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedGenericArg, GraphcalError> {
    let prelude = hir::PreludeTypeScope::graphcal();
    let resolve_ctx = HirTypeResolutionContext {
        src,
        resolver: ctx.resolver,
        module_types: ctx.types,
        registry: Some(registry),
        prelude: &prelude,
    };
    let hir_arg = lower_generic_default(default, param, type_owner, type_def, resolve_ctx)?;
    resolve_hir_generic_arg_for_param(param, &hir_arg, resolve_ctx)
}

pub(super) fn resolve_type_expr_in_struct_scope(
    type_expr: &TypeExpr,
    type_owner: &ResolvedStructTypeName,
    type_def: &TypeDef,
    ctx: ModuleTypeContext<'_>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let prelude = hir::PreludeTypeScope::graphcal();
    let resolve_ctx = HirTypeResolutionContext {
        src,
        resolver: ctx.resolver,
        module_types: ctx.types,
        registry: Some(registry),
        prelude: &prelude,
    };
    let hir_type = lower_type_expr_in_generic_scope(type_expr, type_owner, type_def, resolve_ctx)?;
    resolve_hir_type_expr_inner(&hir_type, resolve_ctx)
}

/// Resolve a `TypeExpr` into a `ResolvedTypeExpr` for tests that do not need a
/// module-aware path context.
///
/// Production callers should use [`resolve_type_expr_with_modules`] so the
/// owner comes from the loaded project/module model instead of a synthetic
/// test owner.
///
/// `dim_params` and `index_params` are the generic parameters in scope (empty
/// for top-level declarations, non-empty inside function signatures).
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a name cannot be resolved (not a known
/// dimension, struct, index, or in-scope generic parameter).
/// dimension, struct, index, or in-scope generic parameter).
#[cfg(test)]
pub(super) fn resolve_type_expr(
    type_ann: &TypeExpr,
    registry: &Registry,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let owner = crate::dag_id::DagId::root_in_package("test", "type_resolution");
    resolve_type_expr_inner(
        type_ann,
        registry,
        &owner,
        dim_params,
        index_params,
        nat_params,
        src,
        None,
    )
}

/// Resolve a `TypeExpr` with an optional module-aware path context.
pub fn resolve_type_expr_with_modules(
    type_ann: &TypeExpr,
    registry: &Registry,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    resolve_type_expr_inner(
        type_ann,
        registry,
        module_ctx.owner,
        dim_params,
        index_params,
        nat_params,
        src,
        Some(module_ctx),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "recursive resolver threads generic parameter scopes and optional module context"
)]
pub(super) fn resolve_type_expr_inner(
    type_ann: &TypeExpr,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    if let Some(ctx) = module_ctx
        && dim_params.is_empty()
        && index_params.is_empty()
        && nat_params.is_empty()
    {
        return resolve_ast_type_expr_via_hir(type_ann, registry, src, ctx);
    }

    match &type_ann.kind {
        TypeExprKind::Dimensionless => Ok(ResolvedTypeExpr::Dimensionless),
        TypeExprKind::Bool => Ok(ResolvedTypeExpr::Bool),
        TypeExprKind::Int => Ok(ResolvedTypeExpr::Int),
        TypeExprKind::Datetime => Ok(ResolvedTypeExpr::Datetime(TimeScale::UTC)),
        TypeExprKind::DatetimeApplication { type_args } => {
            resolve_datetime_application(type_ann, type_args, src)
        }
        TypeExprKind::ComplexApplication { generic_args } => resolve_complex_application(
            type_ann,
            generic_args,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        ),

        TypeExprKind::Indexed { base, indexes } => {
            let resolved_base = resolve_type_expr_inner(
                base,
                registry,
                owner,
                dim_params,
                index_params,
                nat_params,
                src,
                module_ctx,
            )?;
            let mut resolved_indexes = Vec::with_capacity(indexes.len());
            for idx in indexes {
                match idx {
                    crate::desugar::desugared_ast::IndexExpr::Finite { cardinality, span } => {
                        let form = normalize_nat_expr(cardinality, nat_params, src)?;
                        resolved_indexes.push(ResolvedIndex::Finite(form, *span));
                    }
                    crate::desugar::desugared_ast::IndexExpr::BareNat(nat_expr) => {
                        return Err(GraphcalError::ExpectedIndexFoundNat {
                            expression: nat_expr.to_string(),
                            src: src.clone(),
                            span: nat_expr.span().into(),
                        });
                    }
                    crate::desugar::desugared_ast::IndexExpr::Name(path) => {
                        resolved_indexes.push(resolve_index_expr_name(
                            &path.value,
                            path.span,
                            registry,
                            owner,
                            index_params,
                            nat_params,
                            src,
                            module_ctx,
                        )?);
                    }
                }
            }
            Ok(ResolvedTypeExpr::Indexed {
                base: Box::new(resolved_base),
                indexes: resolved_indexes,
            })
        }

        TypeExprKind::DimExpr(dim_expr) => resolve_dim_expr(
            dim_expr,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        ),

        TypeExprKind::TypeApplication { name, generic_args } => resolve_type_application(
            type_ann,
            name,
            generic_args,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        ),
    }
}

/// Resolve a dimension expression to either a [`ResolvedTypeExpr::Quantity`],
/// [`ResolvedTypeExpr::GenericDimExpr`], [`ResolvedTypeExpr::IndexArg`],
/// [`ResolvedTypeExpr::Struct`], or [`ResolvedTypeExpr::GenericDimParam`].
///
/// A single-term, no-power expression is first checked against named indexes,
/// struct types, and generic dimension parameters. Multi-term expressions with
/// generic params become `GenericDimExpr`; fully concrete expressions become `Quantity`.
#[expect(
    clippy::too_many_arguments,
    reason = "resolves ambiguous nominal or dimension syntax with all generic scopes"
)]
fn resolve_dim_expr(
    dim_expr: &crate::desugar::desugared_ast::DimExpr,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    // Single-term, no power: may be a nominal type-level reference rather than
    // a dimension expression denoting a quantity type.
    if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() {
        let term = &dim_expr.terms[0].term;
        if let Some(index) = resolve_concrete_index_path(
            &term.name.value,
            term.name.span,
            registry,
            owner,
            src,
            module_ctx,
        )? {
            return Ok(ResolvedTypeExpr::IndexArg(ResolvedIndex::Concrete(
                index, term.span,
            )));
        }
        if let Some(atom) = term.name.value.as_bare()
            && let Some(gp) = index_params.iter().find(|p| p.as_str() == atom.as_str())
        {
            return Ok(ResolvedTypeExpr::IndexArg(ResolvedIndex::GenericParam(
                gp.clone(),
                term.span,
            )));
        }
        if let Some((type_name, type_def)) = resolve_struct_type_path(
            &term.name.value,
            term.name.span,
            registry,
            owner,
            src,
            module_ctx,
        )? {
            return if type_def.generic_params.is_empty() {
                Ok(ResolvedTypeExpr::Struct(type_name, term.span))
            } else {
                resolve_known_type_application(
                    term.span,
                    type_name,
                    type_def,
                    &[],
                    registry,
                    owner,
                    dim_params,
                    index_params,
                    nat_params,
                    src,
                    module_ctx,
                )
            };
        }
        if let Some(atom) = term.name.value.as_bare()
            && let Some(gp) = dim_params.iter().find(|p| p.as_str() == atom.as_str())
        {
            return Ok(ResolvedTypeExpr::GenericDimParam(gp.clone(), term.span));
        }
    }

    let has_generic = dim_expr.terms.iter().any(|item| {
        item.term
            .name
            .value
            .as_bare()
            .is_some_and(|atom| dim_params.iter().any(|p| p.as_str() == atom.as_str()))
    });

    if has_generic {
        let terms = dim_expr
            .terms
            .iter()
            .map(|item| {
                resolve_dim_term_in_generic_expr(item, registry, dim_params, src, module_ctx)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolvedTypeExpr::GenericDimExpr {
            terms,
            span: dim_expr.span,
        })
    } else {
        let result = dim_expr.terms.iter().try_fold(
            Dimension::dimensionless(),
            |acc, item| -> Result<Dimension, GraphcalError> {
                let base = concrete_dimension_for_term(item, registry, src, module_ctx)?;
                let exp = item.term.power.unwrap_or(Rational::ONE);
                let overflow_err = || GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: item.term.span.into(),
                };
                let powered = base.pow(exp).map_err(|_| overflow_err())?;
                match item.op {
                    MulDivOp::Mul => (acc * powered).map_err(|_| overflow_err()),
                    MulDivOp::Div => (acc / powered).map_err(|_| overflow_err()),
                }
            },
        )?;
        Ok(ResolvedTypeExpr::Quantity(result))
    }
}

fn resolve_dim_term_in_generic_expr(
    item: &crate::desugar::desugared_ast::DimExprItem,
    registry: &Registry,
    dim_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedDimTerm, GraphcalError> {
    let power = item.term.power.unwrap_or(Rational::ONE);
    let op = item.op;
    if let Some(atom) = item.term.name.value.as_bare()
        && let Some(gp) = dim_params.iter().find(|p| p.as_str() == atom.as_str())
    {
        return Ok(ResolvedDimTerm::GenericParam {
            name: gp.clone(),
            power,
            op,
            span: item.term.span,
        });
    }
    concrete_dimension_for_term(item, registry, src, module_ctx)
        .map(|dim| ResolvedDimTerm::Concrete { dim, power, op })
}

fn concrete_dimension_for_term(
    item: &crate::desugar::desugared_ast::DimExprItem,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<Dimension, GraphcalError> {
    resolve_dimension_path(
        &item.term.name.value,
        item.term.name.span,
        registry,
        src,
        module_ctx,
    )?
    .ok_or_else(|| {
        let name = item
            .term
            .name
            .value
            .as_bare()
            .map_or_else(|| item.term.name.value.display_path(), ToString::to_string);
        GraphcalError::UnknownDimension {
            name: DimName::expect_valid(name),
            src: src.clone(),
            span: item.term.span.into(),
        }
    })
}

/// Resolve a `Datetime<TimeScale>` application to a [`ResolvedTypeExpr::Datetime`].
///
/// The argument list is expected to hold exactly one type argument that
/// parses as a [`TimeScale`] identifier (`UTC`, `TAI`, `TT`, …). Surfaced as
/// a dedicated helper rather than living inside [`resolve_type_application`]
/// so the dispatch in [`resolve_type_expr`] is on the AST variant rather than
/// a string compare of the built-in name.
fn resolve_datetime_application(
    type_ann: &TypeExpr,
    type_args: &[TypeExpr],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    if type_args.len() != 1 {
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `Datetime` expects 0 or 1 type argument(s), got {}",
                type_args.len()
            ),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    let arg = &type_args[0];
    match &arg.kind {
        TypeExprKind::DimExpr(dim_expr)
            if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() =>
        {
            let term = &dim_expr.terms[0].term;
            let name = require_local_type_level_path(&term.name.value, term.name.span, src)?;
            name.parse::<TimeScale>().map_or_else(
                |_| {
                    Err(GraphcalError::EvalError {
                        message: format!(
                            "unknown time scale `{name}`; \
                         expected one of: UTC, TAI, TT, TDB, ET, GPST, GST, BDT"
                        ),
                        src: src.clone(),
                        span: arg.span.into(),
                    })
                },
                |scale| Ok(ResolvedTypeExpr::Datetime(scale)),
            )
        }
        _ => Err(GraphcalError::EvalError {
            message: "expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)".to_string(),
            src: src.clone(),
            span: arg.span.into(),
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "passes full type resolution context from resolve_type_expr"
)]
fn resolve_complex_application(
    type_ann: &TypeExpr,
    generic_args: &[crate::desugar::desugared_ast::GenericArg],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let [arg] = generic_args else {
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `Complex` expects 1 generic argument, got {}",
                generic_args.len()
            ),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    };
    let param = crate::registry::types::TypeGenericParam {
        name: GenericParamName::expect_valid("D"),
        constraint: TypeGenericConstraint::Dim,
        default: None,
    };
    let resolved = resolve_type_arg_for_param(
        &param,
        arg,
        registry,
        owner,
        dim_params,
        index_params,
        nat_params,
        src,
        module_ctx,
    )?;
    let ResolvedGenericArg::Dim(dimension) = resolved else {
        return Err(internal_error(
            "Complex dimension argument resolved to a non-dimension sort".to_string(),
            src,
            arg.span(),
        ));
    };
    Ok(ResolvedTypeExpr::Complex {
        dimension,
        span: type_ann.span,
    })
}

/// Resolve a user-defined type application like `Vec3<Length, ECI>` to a
/// [`ResolvedTypeExpr`] by looking the name up in the type registry and
/// substituting defaults for any trailing optional parameters.
///
/// Built-in parameterized types (`Datetime<...>`) reach [`resolve_type_expr`]
/// through their own AST variant and never enter this function.
#[expect(
    clippy::too_many_arguments,
    reason = "passes full type resolution context from resolve_type_expr"
)]
fn resolve_type_application(
    type_ann: &TypeExpr,
    name: &crate::syntax::span::Spanned<NamePath>,
    generic_args: &[crate::desugar::desugared_ast::GenericArg],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let (type_name, type_def) =
        resolve_struct_type_path(&name.value, name.span, registry, owner, src, module_ctx)?
            .ok_or_else(|| GraphcalError::UnknownStructType {
                name: name.value.display_path(),
                src: src.clone(),
                span: name.span.into(),
            })?;
    resolve_known_type_application(
        type_ann.span,
        type_name,
        type_def,
        generic_args,
        registry,
        owner,
        dim_params,
        index_params,
        nat_params,
        src,
        module_ctx,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "passes a resolved type declaration plus the full type resolution context"
)]
fn resolve_known_type_application(
    span: Span,
    type_name: ResolvedStructTypeName,
    type_def: &TypeDef,
    generic_args: &[crate::desugar::desugared_ast::GenericArg],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    check_type_application_arity(type_name.as_str(), type_def, generic_args.len(), span, src)?;
    let mut resolved_args = Vec::with_capacity(type_def.generic_params.len());
    for (param, arg) in type_def.generic_params.iter().zip(generic_args) {
        let resolved = resolve_type_arg_for_param(
            param,
            arg,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        )?;
        resolved_args.push(resolved);
    }
    for param in type_def.generic_params.iter().skip(generic_args.len()) {
        let default_expr = param
            .default
            .as_ref()
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: generic parameter `{}` has no default",
                    param.name
                ),
                src: src.clone(),
                span: span.into(),
            })?;
        let default_ctx = module_ctx
            .map(|ctx| ModuleTypeContext::new(type_name.owner(), ctx.resolver, ctx.types));
        let mut resolved = resolve_type_arg_for_param(
            param,
            default_expr,
            registry,
            type_name.owner(),
            dim_params,
            index_params,
            nat_params,
            src,
            default_ctx,
        )?;
        instantiate_params_in_default(
            &mut resolved,
            type_def,
            &resolved_args,
            src,
            default_expr.span(),
        )?;
        resolved_args.push(resolved);
    }
    Ok(ResolvedTypeExpr::GenericStruct {
        name: type_name,
        generic_args: resolved_args,
        span,
    })
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "keeps sorted generic-argument dispatch and its shared resolution context together"
)]
fn resolve_type_arg_for_param(
    param: &crate::registry::types::TypeGenericParam,
    arg: &crate::desugar::desugared_ast::GenericArg,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedGenericArg, GraphcalError> {
    let resolve_as_type = || {
        let ambiguous_as_type;
        let type_expr = match arg {
            crate::desugar::desugared_ast::GenericArg::Type(type_expr) => type_expr,
            crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
                ambiguous_as_type = hir::lower::ambiguous_generic_arg_as_type(ambiguous);
                &ambiguous_as_type
            }
            crate::desugar::desugared_ast::GenericArg::Index(_) => {
                return Err(generic_sort_error(param, "Index", arg.span(), src));
            }
            crate::desugar::desugared_ast::GenericArg::Nat(_) => {
                return Err(generic_sort_error(param, "Nat", arg.span(), src));
            }
        };
        resolve_type_expr_inner(
            type_expr,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        )
    };

    match param.constraint {
        TypeGenericConstraint::Nat => {
            let ambiguous_as_nat;
            let nat = match arg {
                crate::desugar::desugared_ast::GenericArg::Nat(nat) => nat,
                crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
                    ambiguous_as_nat = hir::lower::ambiguous_generic_arg_as_nat(ambiguous);
                    &ambiguous_as_nat
                }
                crate::desugar::desugared_ast::GenericArg::Index(_) => {
                    return Err(generic_sort_error(param, "Index", arg.span(), src));
                }
                crate::desugar::desugared_ast::GenericArg::Type(_) => {
                    return Err(generic_sort_error(param, "type", arg.span(), src));
                }
            };
            normalize_nat_expr(nat, nat_params, src)
                .map(|form| ResolvedGenericArg::Nat(form, nat.span()))
        }
        TypeGenericConstraint::Dim => resolved_type_as_dim_arg(resolve_as_type()?)
            .map(ResolvedGenericArg::Dim)
            .ok_or_else(|| generic_sort_error(param, "non-dimension type", arg.span(), src)),
        TypeGenericConstraint::Index => {
            if let crate::desugar::desugared_ast::GenericArg::Index(index) = arg {
                let resolved = match index {
                    crate::desugar::desugared_ast::IndexExpr::Finite { cardinality, span } => {
                        ResolvedIndex::Finite(
                            normalize_nat_expr(cardinality, nat_params, src)?,
                            *span,
                        )
                    }
                    crate::desugar::desugared_ast::IndexExpr::Name(path) => {
                        resolve_index_expr_name(
                            &path.value,
                            path.span,
                            registry,
                            owner,
                            index_params,
                            nat_params,
                            src,
                            module_ctx,
                        )?
                    }
                    crate::desugar::desugared_ast::IndexExpr::BareNat(nat) => {
                        return Err(GraphcalError::ExpectedIndexFoundNat {
                            expression: nat.to_string(),
                            src: src.clone(),
                            span: nat.span().into(),
                        });
                    }
                };
                return Ok(ResolvedGenericArg::Index(resolved));
            }
            if let crate::desugar::desugared_ast::GenericArg::Nat(nat) = arg {
                return Err(GraphcalError::ExpectedIndexFoundNat {
                    expression: nat.to_string(),
                    src: src.clone(),
                    span: nat.span().into(),
                });
            }
            match resolve_as_type()? {
                ResolvedTypeExpr::IndexArg(index) => Ok(ResolvedGenericArg::Index(index)),
                _ => Err(generic_sort_error(param, "non-index type", arg.span(), src)),
            }
        }
        TypeGenericConstraint::Type => match resolve_as_type()? {
            ResolvedTypeExpr::IndexArg(_) => {
                Err(generic_sort_error(param, "Index", arg.span(), src))
            }
            ResolvedTypeExpr::Indexed { .. } => Err(generic_sort_error(
                param,
                "indexed declaration type",
                arg.span(),
                src,
            )),
            resolved => Ok(ResolvedGenericArg::Type(resolved)),
        },
    }
}

fn resolved_type_as_dim_arg(resolved: ResolvedTypeExpr) -> Option<ResolvedDimArg> {
    match resolved {
        ResolvedTypeExpr::Dimensionless => Some(ResolvedDimArg::Dimensionless),
        ResolvedTypeExpr::Quantity(dim) => Some(ResolvedDimArg::Concrete(dim)),
        ResolvedTypeExpr::GenericDimParam(name, span) => {
            Some(ResolvedDimArg::GenericParam(name, span))
        }
        ResolvedTypeExpr::GenericDimExpr { terms, span } => {
            Some(ResolvedDimArg::Expr { terms, span })
        }
        _ => None,
    }
}

fn generic_sort_error(
    param: &crate::registry::types::TypeGenericParam,
    actual: &str,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    let expected = match param.constraint {
        TypeGenericConstraint::Dim => "Dim",
        TypeGenericConstraint::Index => "Index",
        TypeGenericConstraint::Nat => "Nat",
        TypeGenericConstraint::Type => "Type",
    };
    GraphcalError::EvalError {
        message: format!(
            "generic parameter `{}` expects an argument of sort `{expected}`, got {actual}",
            param.name
        ),
        src: src.clone(),
        span: span.into(),
    }
}
