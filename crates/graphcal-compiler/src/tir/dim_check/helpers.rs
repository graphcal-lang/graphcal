use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::Expr;
use crate::registry::error::GraphcalError;
use crate::registry::types::{Registry, TypeDef};
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{GenericParamName, IndexName};

use super::{DeclaredType, InferredIndex, InferredStructType, InferredType};
use crate::tir::typed::{ResolvedIndex, ResolvedTypeExpr};

pub(super) fn is_bool_type(ty: &InferredType) -> bool {
    match ty {
        InferredType::Bool => true,
        InferredType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Check if a declared type matches an inferred type.
///
/// Under the n-variant-union model, the inferred type of a constructor
/// expression is *already* the owning union — there is no per-variant
/// type and therefore no widening/subtyping at the type level. Struct
/// equality is by name and type-argument list only.
pub(super) fn types_match(declared: &DeclaredType, inferred: &InferredType) -> bool {
    match (declared, inferred) {
        (DeclaredType::Scalar(d), InferredType::Scalar(i)) => d == i,
        (DeclaredType::Bool, InferredType::Bool) => true,
        (DeclaredType::Int, inferred) if inferred.is_int_like() => true,
        (DeclaredType::Datetime(d), InferredType::Datetime(i)) => d == i,
        (DeclaredType::Label(d), InferredType::Label(i)) => i.matches_ref(d),
        (DeclaredType::Struct(d, d_args), InferredType::Struct(i, i_args)) => {
            i.matches_ref(d)
                && d_args.len() == i_args.len()
                && d_args
                    .iter()
                    .zip(i_args)
                    .all(|(da, ia)| types_match(da, ia))
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
        ) => i_idx.matches_ref(d_idx) && types_match(d_elem, i_elem),
        _ => false,
    }
}

/// Check if a resolved declaration type matches an inferred expression type,
/// preserving canonical index identity when both sides carry it.
pub(super) fn resolved_type_matches_inferred(
    resolved: &ResolvedTypeExpr,
    inferred: &InferredType,
) -> bool {
    match (resolved, inferred) {
        (ResolvedTypeExpr::Dimensionless, InferredType::Scalar(d)) => d.is_dimensionless(),
        (ResolvedTypeExpr::Bool, InferredType::Bool) => true,
        (ResolvedTypeExpr::Int, inferred) => inferred.is_int_like(),
        (ResolvedTypeExpr::Datetime(expected), InferredType::Datetime(actual)) => {
            expected == actual
        }
        (ResolvedTypeExpr::Scalar(expected), InferredType::Scalar(actual)) => expected == actual,
        (ResolvedTypeExpr::Label(expected, resolved, _), InferredType::Label(actual)) => {
            actual.matches_resolved_or_name(expected, resolved.as_ref())
        }
        (ResolvedTypeExpr::Struct(expected, resolved, _), InferredType::Struct(actual, args)) => {
            actual.matches_resolved_or_name(expected, resolved.as_ref()) && args.is_empty()
        }
        (
            ResolvedTypeExpr::GenericStruct {
                name,
                resolved,
                type_args,
                ..
            },
            InferredType::Struct(actual, actual_args),
        ) => {
            actual.matches_resolved_or_name(name, resolved.as_ref())
                && type_args.len() == actual_args.len()
                && type_args
                    .iter()
                    .zip(actual_args)
                    .all(|(expected, actual)| resolved_type_matches_inferred(expected, actual))
        }
        (ResolvedTypeExpr::Indexed { base, indexes }, _) => {
            resolved_indexed_type_matches_inferred(base, indexes, inferred)
        }
        (
            ResolvedTypeExpr::GenericDimParam(_, _)
            | ResolvedTypeExpr::GenericTypeParam(_, _)
            | ResolvedTypeExpr::GenericDimExpr { .. },
            _,
        ) => false,
        _ => false,
    }
}

fn resolved_indexed_type_matches_inferred(
    base: &ResolvedTypeExpr,
    indexes: &[ResolvedIndex],
    inferred: &InferredType,
) -> bool {
    let mut current = inferred;
    for index in indexes {
        let InferredType::Indexed {
            element,
            index: actual,
        } = current
        else {
            return false;
        };
        if !resolved_index_matches_inferred(index, actual) {
            return false;
        }
        current = element;
    }
    resolved_type_matches_inferred(base, current)
}

fn resolved_index_matches_inferred(index: &ResolvedIndex, actual: &InferredIndex) -> bool {
    match index {
        ResolvedIndex::Concrete(expected, resolved, _) => {
            actual.matches_resolved_or_name(expected, resolved.as_ref())
        }
        ResolvedIndex::NatExpr(form, _) => form
            .is_constant()
            .then(|| {
                IndexName::new(crate::registry::types::nat_range_index_name(
                    form.constant(),
                ))
            })
            .is_some_and(|expected| actual.name() == &expected),
        ResolvedIndex::GenericParam(expected, _) => actual.name().as_str() == expected.as_str(),
    }
}

/// Format a declared type for display in diagnostics.
pub(super) fn format_declared_type(dt: &DeclaredType, registry: &Registry) -> String {
    dt.format(&registry.dimensions)
}

/// Look up the definition for an inferred struct identity.
///
/// Prefer canonical module-aware TIR sidecars when the identity carries a
/// resolved owner, then fall back to the legacy leaf-keyed registry for
/// standalone/non-module-aware callers.
pub(super) fn struct_type_def_for_inferred<'a>(
    ty: &InferredStructType,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    registry: &'a Registry,
) -> Option<&'a TypeDef> {
    ty.resolved()
        .and_then(|resolved| {
            dag.and_then(|dag| dag.resolved_type_defs.as_ref())
                .and_then(|defs| defs.struct_types.get(resolved))
        })
        .or_else(|| registry.types.get_type(ty.name().as_str()))
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
            InferredType::Label(index) => Self::Label(index.type_ref().clone()),
            InferredType::Struct(n, args) => {
                Self::Struct(n.type_ref().clone(), args.iter().map(Self::from).collect())
            }
            InferredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: index.type_ref().clone(),
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
            DeclaredType::Label(index) => Self::Label(InferredIndex::from_ref(index.clone())),
            DeclaredType::Struct(n, args) => Self::Struct(
                InferredStructType::from_ref(n.clone()),
                args.iter().map(Self::from).collect(),
            ),
            DeclaredType::Indexed { element, index } => Self::Indexed {
                element: Box::new(Self::from(element.as_ref())),
                index: InferredIndex::from_ref(index.clone()),
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
    field_type_ann: &crate::desugar::resolved_ast::TypeExpr,
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
            .map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: field_type_ann.span.into(),
            })?
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
                    index_sub.insert(param.name.clone(), IndexName::new(name.name().as_str()));
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
    if let crate::desugar::resolved_ast::TypeExprKind::DimExpr(dim_expr) = &field_type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
    {
        if let Some(name) = dim_expr.terms[0]
            .term
            .name
            .value
            .as_bare()
            .map(|atom| atom.as_str())
            && let Some(inferred) = type_sub.get(name)
        {
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
    arms: &[crate::desugar::resolved_ast::MatchArm],
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

/// Build the Cartesian product of variant-key slices across multiple axes.
pub(super) fn cartesian_product<T: Clone + Eq + std::hash::Hash>(
    axes: &[Vec<T>],
    current: &mut Vec<T>,
    result: &mut std::collections::HashSet<Vec<T>>,
) {
    if current.len() == axes.len() {
        result.insert(current.clone());
        return;
    }
    let axis_idx = current.len();
    for variant in &axes[axis_idx] {
        current.push(variant.clone());
        cartesian_product(axes, current, result);
        current.pop();
    }
}
