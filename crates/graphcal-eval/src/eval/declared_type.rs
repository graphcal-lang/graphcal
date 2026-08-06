//! Runtime display-type resolution for fields of generic value constructors.

use std::collections::HashMap;

use graphcal_compiler::dimension::Dimension;
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::types::{Registry, StructField};
use graphcal_compiler::syntax::ast::{AmbiguousGenericArg, GenericArg};
use graphcal_compiler::syntax::names::NameAtom;

/// Resolve a struct field's declared type after generic substitution.
pub(super) fn resolve_field_declared_type(
    field: &StructField,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<DeclaredType> {
    if let graphcal_compiler::desugar::desugared_ast::TypeExprKind::DimExpr(dim_expr) =
        &field.type_ann().kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
        && let Some(name) = dim_expr.terms[0]
            .term
            .name
            .value
            .as_bare()
            .map(NameAtom::as_str)
        && let Some(concrete) = generic_sub.get(name)
    {
        return Some(concrete.clone());
    }
    if let graphcal_compiler::desugar::desugared_ast::TypeExprKind::ComplexApplication {
        generic_args,
    } = &field.type_ann().kind
        && let [dimension_arg] = generic_args.as_slice()
        && let Some(dimension) = resolve_field_dimension_arg(dimension_arg, generic_sub, registry)
    {
        return Some(DeclaredType::Complex(dimension));
    }

    registry
        .dimensions
        .resolve_type_expr(field.type_ann())
        .ok()
        .flatten()
        .map(DeclaredType::Quantity)
}

fn resolve_field_dimension_arg(
    arg: &GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<Dimension> {
    match arg {
        GenericArg::Type(type_expr) => resolve_field_dimension(type_expr, generic_sub, registry),
        GenericArg::Ambiguous(ambiguous) => {
            resolve_ambiguous_field_dimension(ambiguous, generic_sub, registry)
        }
        GenericArg::Index(_) | GenericArg::Nat(_) => None,
    }
}

fn resolve_ambiguous_field_dimension(
    arg: &AmbiguousGenericArg,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<Dimension> {
    match arg {
        AmbiguousGenericArg::Name(name) => {
            field_dimension_name(name.name.as_str(), generic_sub, registry)
        }
        AmbiguousGenericArg::Mul(operands, _) => {
            let mut operands = operands.iter();
            let first = resolve_ambiguous_field_dimension(operands.next()?, generic_sub, registry)?;
            operands.try_fold(first, |product, operand| {
                product
                    .checked_mul(&resolve_ambiguous_field_dimension(
                        operand,
                        generic_sub,
                        registry,
                    )?)
                    .ok()
            })
        }
    }
}

fn resolve_field_dimension(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<Dimension> {
    use graphcal_compiler::desugar::desugared_ast::{MulDivOp, TypeExprKind};

    match &type_expr.kind {
        TypeExprKind::Dimensionless => Some(Dimension::dimensionless()),
        TypeExprKind::DimExpr(dim_expr) => {
            dim_expr
                .terms
                .iter()
                .try_fold(Dimension::dimensionless(), |acc, item| {
                    let name = item.term.name.value.leaf().as_str();
                    let dimension = field_dimension_name(name, generic_sub, registry)?;
                    let powered = dimension
                        .pow(
                            item.term
                                .power
                                .unwrap_or(graphcal_compiler::dimension::Rational::ONE),
                        )
                        .ok()?;
                    match item.op {
                        MulDivOp::Mul => acc.checked_mul(&powered).ok(),
                        MulDivOp::Div => acc.checked_div(&powered).ok(),
                    }
                })
        }
        TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::Indexed { .. }
        | TypeExprKind::TypeApplication { .. } => None,
    }
}

fn field_dimension_name(
    name: &str,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<Dimension> {
    generic_sub
        .get(name)
        .and_then(|declared| match declared {
            DeclaredType::Quantity(dimension) => Some(dimension.clone()),
            _ => None,
        })
        .or_else(|| registry.dimensions.get_dimension(name).cloned())
}
