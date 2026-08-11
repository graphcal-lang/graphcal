use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational};
use crate::hir;
use crate::hir::{NominalGenericParam, NominalTypeDef};
use crate::nat::NatPolyForm;
use crate::registry::error::GraphcalError;
use crate::registry::types::TypeGenericConstraint;
use crate::syntax::dimension::ResolvedDimName;
use crate::syntax::index_name::{IndexName, ResolvedIndexName};
use crate::syntax::module_resolve::ModuleResolveError;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;
use crate::syntax::type_name::{GenericParamName, ResolvedStructTypeName};

use super::{
    ModuleTypeContext, ProjectTypeStore, ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg,
    ResolvedIndex, ResolvedTypeExpr, nat_overflow_error,
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

/// Resolve an already-lowered HIR type expression into the TIR type
/// representation.
///
/// This is the new semantic entry point for module-aware TIR type resolution:
/// source paths should be lowered to HIR first, then TIR consumes canonical
/// `ResolvedName<Ns>` and lexical generic IDs from HIR instead of performing
/// source-path lookup itself.
pub fn resolve_hir_type_expr(
    type_ann: &hir::TypeExpr,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let ctx = HirTypeResolutionContext {
        src,
        project_types: module_ctx.types,
    };
    resolve_hir_type_expr_inner(type_ann, ctx)
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
        hir::TypeExprKind::Key(index) => Ok(ResolvedTypeExpr::Key {
            index: resolve_hir_index_ref(index, ctx)?,
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
        hir::NatExpr::Add(operands, _) => operands
            .iter()
            .map(format_hir_nat_expr)
            .collect::<Vec<_>>()
            .join(" + "),
        hir::NatExpr::Mul(operands, _) => operands
            .iter()
            .map(format_hir_nat_expr)
            .collect::<Vec<_>>()
            .join(" * "),
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
    let power = item.term.power;
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
        hir::NatExpr::Add(operands, _) => operands
            .iter()
            .try_fold(NatPolyForm::from_constant(0), |sum, operand| {
                sum.add(&normalize_hir_nat_expr(operand)?)
            }),
        hir::NatExpr::Mul(operands, _) => operands
            .iter()
            .try_fold(NatPolyForm::from_constant(1), |product, operand| {
                product.mul(&normalize_hir_nat_expr(operand)?)
            }),
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
    fn from_resolved_prefix(
        type_def: &NominalTypeDef,
        resolved_args: &[ResolvedGenericArg],
    ) -> Self {
        type_def.generic_params().iter().zip(resolved_args).fold(
            Self::default(),
            |mut substitutions, (param, arg)| {
                match arg {
                    ResolvedGenericArg::Dim(dim) => {
                        substitutions.dims.insert(param.name().clone(), dim.clone());
                    }
                    ResolvedGenericArg::Index(index) => {
                        substitutions
                            .indexes
                            .insert(param.name().clone(), index.clone());
                    }
                    ResolvedGenericArg::Nat(form, _) => {
                        substitutions
                            .nats
                            .insert(param.name().clone(), form.clone());
                    }
                    ResolvedGenericArg::Type(type_expr) => {
                        substitutions
                            .types
                            .insert(param.name().clone(), type_expr.clone());
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
    type_def: &NominalTypeDef,
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
        ResolvedTypeExpr::IndexArg(index) | ResolvedTypeExpr::Key { index, .. } => {
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
        let mut resolved = resolve_hir_generic_arg_for_param(param, default, ctx)?;
        instantiate_params_in_default(
            &mut resolved,
            type_def,
            &resolved_args,
            ctx.src,
            default.span(),
        )?;
        resolved_args.push(resolved);
    }

    Ok(ResolvedTypeExpr::GenericStruct {
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
