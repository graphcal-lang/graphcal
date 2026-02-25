use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::Expr;
use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{GenericParamName, IndexName, VariantName};

use graphcal_registry::error::GraphcalError;
use graphcal_registry::registry::Registry;

use super::{DeclaredType, InferredType};

pub(super) fn is_bool_type(ty: &InferredType) -> bool {
    match ty {
        InferredType::Bool => true,
        InferredType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Check if a declared type matches an inferred type.
pub(super) fn types_match(declared: &DeclaredType, inferred: &InferredType) -> bool {
    match (declared, inferred) {
        (DeclaredType::Scalar(d), InferredType::Scalar(i)) => d == i,
        (DeclaredType::Bool, InferredType::Bool) | (DeclaredType::Int, InferredType::Int) => true,
        (DeclaredType::Datetime(d), InferredType::Datetime(i)) => d == i,
        (DeclaredType::Label(d), InferredType::Label(i)) => d == i,
        (DeclaredType::Struct(d, d_args), InferredType::Struct(i, i_args)) => {
            d == i
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
        ) => d_idx == i_idx && types_match(d_elem, i_elem),
        _ => false,
    }
}

/// Format a declared type for display in diagnostics.
pub(super) fn format_declared_type(dt: &DeclaredType, registry: &Registry) -> String {
    match dt {
        DeclaredType::Scalar(d) => registry.dimensions.format_dimension(d),
        DeclaredType::Bool => "Bool".to_string(),
        DeclaredType::Int => "Int".to_string(),
        DeclaredType::Datetime(scale) => {
            if scale.is_utc() {
                "Datetime".to_string()
            } else {
                format!("Datetime<{scale}>")
            }
        }
        DeclaredType::Label(index) => format!("Label({index})"),
        DeclaredType::Struct(name, args) => {
            if args.is_empty() {
                name.to_string()
            } else {
                let args_str: Vec<String> = args
                    .iter()
                    .map(|a| format_declared_type(a, registry))
                    .collect();
                format!("{name}<{}>", args_str.join(", "))
            }
        }
        DeclaredType::Indexed { element, index } => {
            format!("{}[{index}]", format_declared_type(element, registry))
        }
    }
}

/// Format an inferred type for display in diagnostics.
pub(super) fn format_inferred_type(it: &InferredType, registry: &Registry) -> String {
    match it {
        InferredType::Scalar(d) => registry.dimensions.format_dimension(d),
        InferredType::Bool => "Bool".to_string(),
        InferredType::Int => "Int".to_string(),
        InferredType::Datetime(scale) => {
            if scale.is_utc() {
                "Datetime".to_string()
            } else {
                format!("Datetime<{scale}>")
            }
        }
        InferredType::Label(index) => format!("Label({index})"),
        InferredType::Struct(name, args) => {
            if args.is_empty() {
                name.to_string()
            } else {
                let args_str: Vec<String> = args
                    .iter()
                    .map(|a| format_inferred_type(a, registry))
                    .collect();
                format!("{name}<{}>", args_str.join(", "))
            }
        }
        InferredType::Indexed { element, index } => {
            format!("{}[{index}]", format_inferred_type(element, registry))
        }
    }
}

/// Convert a `DeclaredType` to the corresponding `InferredType`.
pub(super) fn declared_to_inferred(dt: &DeclaredType) -> InferredType {
    match dt {
        DeclaredType::Scalar(d) => InferredType::Scalar(d.clone()),
        DeclaredType::Bool => InferredType::Bool,
        DeclaredType::Int => InferredType::Int,
        DeclaredType::Datetime(scale) => InferredType::Datetime(*scale),
        DeclaredType::Label(index) => InferredType::Label(index.clone()),
        DeclaredType::Struct(n, args) => {
            InferredType::Struct(n.clone(), args.iter().map(declared_to_inferred).collect())
        }
        DeclaredType::Indexed { element, index } => InferredType::Indexed {
            element: Box::new(declared_to_inferred(element)),
            index: index.clone(),
        },
    }
}

/// Resolve a field's `TypeExpr` to an `InferredType`, substituting generic type params.
///
/// For non-generic types (empty `type_args`), the field's `type_ann` is resolved directly
/// using the registry. For generic types, generic params in the field type are substituted
/// with the corresponding concrete type args.
pub(super) fn resolve_field_type(
    field_type_ann: &graphcal_syntax::ast::TypeExpr,
    type_def: &graphcal_registry::registry::TypeDef,
    type_args: &[InferredType],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use graphcal_registry::registry::TypeGenericConstraint;

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
            TypeGenericConstraint::Dim => {
                if let InferredType::Scalar(dim) = arg {
                    dim_sub.insert(param.name.clone(), dim.clone());
                }
            }
            TypeGenericConstraint::Index => {
                if let InferredType::Struct(name, _) = arg {
                    index_sub.insert(param.name.clone(), IndexName::new(name.as_str()));
                }
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
    if let graphcal_syntax::ast::TypeExprKind::DimExpr(dim_expr) = &field_type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
    {
        let name = &dim_expr.terms[0].term.name.name;
        if let Some(inferred) = type_sub.get(name.as_str()) {
            return Ok(inferred.clone());
        }
    }

    // Resolve using TIR type resolution with generic params in scope, then substitute
    let resolved =
        crate::tir::resolve_type_expr(field_type_ann, registry, &dim_params, &index_params, src)?;
    crate::tir::substitute_resolved_type(&resolved, &dim_sub, &index_sub, src)
}

/// Helper: extract scalar dimension from `InferredType`, returning error if struct.
/// Check if a binary operation (Add/Sub) is valid via derive on a struct type.
/// Returns `Some(result_type)` if the operation is derived, `None` if not a struct type.
pub(super) fn check_derived_binop(
    lhs_type: &InferredType,
    rhs_type: &InferredType,
    derive_op: graphcal_syntax::ast::DeriveOp,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    lhs_span: graphcal_syntax::span::Span,
    rhs_span: graphcal_syntax::span::Span,
) -> Result<Option<InferredType>, GraphcalError> {
    let InferredType::Struct(lhs_name, lhs_args) = lhs_type else {
        return Ok(None);
    };
    let InferredType::Struct(rhs_name, rhs_args) = rhs_type else {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(lhs_type, registry),
            found: format_inferred_type(rhs_type, registry),
            src: src.clone(),
            span: rhs_span.into(),
            help: "both operands must be the same struct type".to_string(),
        });
    };
    if lhs_name != rhs_name || lhs_args != rhs_args {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(lhs_type, registry),
            found: format_inferred_type(rhs_type, registry),
            src: src.clone(),
            span: rhs_span.into(),
            help: "both operands must be the same struct type with the same type arguments"
                .to_string(),
        });
    }
    let type_def = registry.types.get_type(lhs_name.as_str()).ok_or_else(|| {
        GraphcalError::UnknownStructType {
            name: lhs_name.clone(),
            src: src.clone(),
            span: lhs_span.into(),
        }
    })?;
    let op_name = match derive_op {
        graphcal_syntax::ast::DeriveOp::Add => "Add",
        graphcal_syntax::ast::DeriveOp::Sub => "Sub",
        graphcal_syntax::ast::DeriveOp::Neg => "Neg",
    };
    if !type_def.derives.contains(&derive_op) {
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` does not derive `{op_name}`, cannot use `{}` operator",
                lhs_name,
                if derive_op == graphcal_syntax::ast::DeriveOp::Add {
                    "+"
                } else {
                    "-"
                }
            ),
            src: src.clone(),
            span: lhs_span.into(),
        });
    }
    Ok(Some(lhs_type.clone()))
}

/// Check if unary negation is valid via derive(Neg) on a struct type.
/// Returns `Some(result_type)` if the operation is derived, `None` if not a struct type.
pub(super) fn check_derived_neg(
    operand_type: &InferredType,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    operand_span: graphcal_syntax::span::Span,
) -> Result<Option<InferredType>, GraphcalError> {
    let InferredType::Struct(name, _args) = operand_type else {
        return Ok(None);
    };
    let type_def =
        registry
            .types
            .get_type(name.as_str())
            .ok_or_else(|| GraphcalError::UnknownStructType {
                name: name.clone(),
                src: src.clone(),
                span: operand_span.into(),
            })?;
    if !type_def
        .derives
        .contains(&graphcal_syntax::ast::DeriveOp::Neg)
    {
        return Err(GraphcalError::EvalError {
            message: format!("type `{name}` does not derive `Neg`, cannot use unary `-` operator"),
            src: src.clone(),
            span: operand_span.into(),
        });
    }
    Ok(Some(operand_type.clone()))
}

/// Check that all match arm types are identical, returning the common type.
pub(super) fn check_arm_types_match(
    arm_types: &[InferredType],
    arms: &[graphcal_syntax::ast::MatchArm],
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
                    src: src.clone(),
                    span: arms[i].body.span.into(),
                    help: "all match arms must return the same type".to_string(),
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
    span: graphcal_syntax::span::Span,
) -> Result<Dimension, GraphcalError> {
    match inferred {
        InferredType::Scalar(d) => Ok(d.clone()),
        other => Err(GraphcalError::DimensionMismatch {
            expected: "scalar dimension".to_string(),
            found: format_inferred_type(other, registry),
            src: src.clone(),
            span: span.into(),
            help: "expected a scalar value, not an indexed value or struct".to_string(),
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
