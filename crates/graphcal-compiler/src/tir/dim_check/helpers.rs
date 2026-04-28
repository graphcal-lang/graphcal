use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::Expr;
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{GenericParamName, IndexName, VariantName};

use super::{DeclaredType, InferredType};

pub(super) fn is_bool_type(ty: &InferredType) -> bool {
    match ty {
        InferredType::Bool => true,
        InferredType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Check if a declared type matches an inferred type.
///
/// Supports union subtyping: if the declared type is a union and the inferred
/// type is a member of that union, this returns true (implicit widening).
pub(super) fn types_match(
    declared: &DeclaredType,
    inferred: &InferredType,
    registry: &Registry,
) -> bool {
    match (declared, inferred) {
        (DeclaredType::Scalar(d), InferredType::Scalar(i)) => d == i,
        (DeclaredType::Bool, InferredType::Bool) => true,
        (DeclaredType::Int, inferred) if inferred.is_int_like() => true,
        (DeclaredType::Datetime(d), InferredType::Datetime(i)) => d == i,
        (DeclaredType::Label(d), InferredType::Label(i)) => d == i,
        (DeclaredType::Struct(d, d_args), InferredType::Struct(i, i_args)) => {
            if d == i
                && d_args.len() == i_args.len()
                && d_args
                    .iter()
                    .zip(i_args)
                    .all(|(da, ia)| types_match(da, ia, registry))
            {
                return true;
            }
            // Union subtyping: if declared is a union type and inferred is a member
            registry.types.is_member_of_union(i.as_str(), d.as_str())
        }
        (
            DeclaredType::Indexed {
                element: d_elem,
                index: d_idx,
            },
            InferredType::Indexed {
                element: i_elem,
                index: i_idx,
            },
        ) => d_idx == i_idx && types_match(d_elem, i_elem, registry),
        _ => false,
    }
}

/// Format a declared type for display in diagnostics.
pub(super) fn format_declared_type(dt: &DeclaredType, registry: &Registry) -> String {
    dt.format(&registry.dimensions)
}

/// Format an inferred type for display in diagnostics.
#[must_use]
pub fn format_inferred_type(it: &InferredType, registry: &Registry) -> String {
    if let InferredType::Fin(bound) = it {
        return format!("Fin({})", bound.format());
    }
    DeclaredType::from(it).format(&registry.dimensions)
}

impl From<&InferredType> for DeclaredType {
    fn from(it: &InferredType) -> Self {
        match it {
            InferredType::Scalar(d) => Self::Scalar(d.clone()),
            InferredType::Bool => Self::Bool,
            InferredType::Int | InferredType::Fin(_) => Self::Int,
            InferredType::Datetime(scale) => Self::Datetime(*scale),
            InferredType::Label(index) => Self::Label(index.clone()),
            InferredType::Struct(n, args) => {
                Self::Struct(n.clone(), args.iter().map(Self::from).collect())
            }
            InferredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: index.clone(),
            },
        }
    }
}

impl From<&DeclaredType> for InferredType {
    fn from(dt: &DeclaredType) -> Self {
        match dt {
            DeclaredType::Scalar(d) => Self::Scalar(d.clone()),
            DeclaredType::Bool => Self::Bool,
            DeclaredType::Int => Self::Int,
            DeclaredType::Datetime(scale) => Self::Datetime(*scale),
            DeclaredType::Label(index) => Self::Label(index.clone()),
            DeclaredType::Struct(n, args) => {
                Self::Struct(n.clone(), args.iter().map(Self::from).collect())
            }
            DeclaredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: index.clone(),
            },
        }
    }
}

/// Resolve a field's `TypeExpr` to an `InferredType`, substituting generic type params.
///
/// For non-generic types (empty `type_args`), the field's `type_ann` is resolved directly
/// using the registry. For generic types, generic params in the field type are substituted
/// with the corresponding concrete type args.
pub(super) fn resolve_field_type(
    field_type_ann: &crate::desugar::desugared_ast::TypeExpr,
    type_def: &crate::registry::types::TypeDef,
    type_args: &[InferredType],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::registry::types::TypeGenericConstraint;

    if type_def.generic_params.is_empty() {
        // Non-generic type: resolve the field type_ann using the registry
        let dim = registry
            .dimensions
            .resolve_type_expr(field_type_ann)
            .ok_or_else(|| GraphcalError::EvalError {
                message: "cannot resolve field type expression".to_string(),
                src: src.clone(),
                span: field_type_ann.span.into(),
            })?;
        return Ok(InferredType::Scalar(dim));
    }

    // Generic type: build substitution maps from generic params + type args
    let mut dim_sub: HashMap<GenericParamName, Dimension> = HashMap::new();
    let mut index_sub: HashMap<GenericParamName, IndexName> = HashMap::new();
    // Track unconstrained (Type) params separately — they map to InferredType
    let mut type_sub: HashMap<GenericParamName, InferredType> = HashMap::new();

    for (param, arg) in type_def.generic_params.iter().zip(type_args.iter()) {
        match param.constraint {
            TypeGenericConstraint::Dim => match arg {
                InferredType::Scalar(dim) => {
                    dim_sub.insert(param.name.clone(), dim.clone());
                }
                other => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` expects a scalar dimension, but got {}",
                            param.name.as_str(),
                            format_inferred_type(other, registry),
                        ),
                        src: src.clone(),
                        span: field_type_ann.span.into(),
                    });
                }
            },
            TypeGenericConstraint::Index => match arg {
                InferredType::Struct(name, _) => {
                    index_sub.insert(param.name.clone(), IndexName::new(name.as_str()));
                }
                other => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` expects an index type, but got {}",
                            param.name.as_str(),
                            format_inferred_type(other, registry),
                        ),
                        src: src.clone(),
                        span: field_type_ann.span.into(),
                    });
                }
            },
            TypeGenericConstraint::Nat => {
                // Nat generics on type definitions are not yet used
            }
            TypeGenericConstraint::Unconstrained => {
                type_sub.insert(param.name.clone(), arg.clone());
            }
        }
    }

    // Collect dim param and index param name lists for resolve_type_expr
    let dim_params: Vec<GenericParamName> = type_def
        .generic_params
        .iter()
        .filter(|p| p.constraint == TypeGenericConstraint::Dim)
        .map(|p| p.name.clone())
        .collect();
    let index_params: Vec<GenericParamName> = type_def
        .generic_params
        .iter()
        .filter(|p| p.constraint == TypeGenericConstraint::Index)
        .map(|p| p.name.clone())
        .collect();

    // Check if the field type references an unconstrained (Type) param directly
    if let crate::desugar::desugared_ast::TypeExprKind::DimExpr(dim_expr) = &field_type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
    {
        let name = &dim_expr.terms[0].term.name.name;
        if let Some(inferred) = type_sub.get(name.as_str()) {
            return Ok(inferred.clone());
        }
    }

    // Resolve using TIR type resolution with generic params in scope, then substitute
    let resolved = crate::tir::typed::resolve_type_expr(
        field_type_ann,
        registry,
        &dim_params,
        &index_params,
        &[],
        src,
    )?;
    let no_nat_sub = std::collections::HashMap::new();
    crate::tir::typed::substitute_resolved_type(&resolved, &dim_sub, &index_sub, &no_nat_sub, src)
}

/// Check that all match arm types are identical, returning the common type.
pub(super) fn check_arm_types_match(
    arm_types: &[InferredType],
    arms: &[crate::desugar::desugared_ast::MatchArm],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    expr: &Expr,
) -> Result<InferredType, GraphcalError> {
    if let Some(first) = arm_types.first() {
        for (i, arm_type) in arm_types.iter().enumerate().skip(1) {
            if arm_type != first {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(first, registry),
                    found: format_inferred_type(arm_type, registry),
                    help: "all match arms must return the same type".to_string(),
                    src: src.clone(),
                    span: arms[i].body.span.into(),
                });
            }
        }
        Ok(first.clone())
    } else {
        Err(GraphcalError::EvalError {
            message: "match expression has no arms".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        })
    }
}

pub(super) fn expect_scalar(
    inferred: &InferredType,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<Dimension, GraphcalError> {
    match inferred {
        InferredType::Scalar(d) => Ok(d.clone()),
        other => Err(GraphcalError::DimensionMismatch {
            expected: "scalar dimension".to_string(),
            found: format_inferred_type(other, registry),
            help: "expected a scalar value, not an indexed value or struct".to_string(),
            src: src.clone(),
            span: span.into(),
        }),
    }
}

/// Build the Cartesian product of variant name slices across multiple axes.
pub(super) fn cartesian_product<'a>(
    axes: &'a [Vec<VariantName>],
    current: &mut Vec<&'a str>,
    result: &mut std::collections::HashSet<Vec<&'a str>>,
) {
    if current.len() == axes.len() {
        result.insert(current.clone());
        return;
    }
    let axis_idx = current.len();
    for variant in &axes[axis_idx] {
        current.push(variant.as_str());
        cartesian_product(axes, current, result);
        current.pop();
    }
}
