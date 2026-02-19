use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{BinOp, Expr, ExprKind};
use graphcal_syntax::dimension::{Dimension, Rational};
use graphcal_syntax::names::{
    FieldName, FnName, GenericParamName, IndexName, StructTypeName, UnitName, VariantName,
};

use crate::builtins::{DimSignature, builtin_functions};
use crate::error::GraphcalError;
use crate::registry::Registry;
use crate::tir::ResolvedFnSig;

/// The declared type of a const/param/node: either a scalar with a dimension, a bool, or a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeName, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexName,
    },
}

/// The inferred type of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeName, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexName,
    },
    /// A loop variable bound by `for m: Maneuver`.
    /// Used only in `local_types` — not a "real" value type.
    LoopVar(IndexName),
}

/// Check dimensions for all declarations in a file.
///
/// For each const/param/node, infers the dimension of the RHS expression
/// and verifies it matches the declared type annotation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
/// Check dimensions for all declarations using a TIR.
///
/// Uses `tir.build_declared_types()` (derived from `resolved_decl_types`) to validate that
/// every RHS expression matches its declared type annotation.
///
/// This is a pure validation step — returns `()` on success.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
#[expect(
    clippy::too_many_lines,
    reason = "dimension checking for all declaration kinds including assert tolerance"
)]
pub fn check_dimensions_tir(
    tir: &crate::tir::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let declared_types = tir.build_declared_types(src)?;

    // Validate expressions against declared types
    let empty_locals: HashMap<String, InferredType> = HashMap::new();
    let all_decls = tir
        .consts
        .iter()
        .chain(tir.params.iter())
        .chain(tir.nodes.iter());

    for (name, type_ann, value_expr, _span) in all_decls {
        let declared = &declared_types[name.as_str()];
        let inferred = infer_type(
            value_expr,
            &declared_types,
            &empty_locals,
            &tir.registry,
            &builtin_fns,
            &tir.resolved_fn_sigs,
            src,
        )?;

        if !types_match(declared, &inferred) {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_declared_type(declared, &tir.registry),
                inferred: format_inferred_type(&inferred, &tir.registry),
                src: src.clone(),
                span: type_ann.span.into(),
            });
        }
    }

    // Validate assert bodies
    for (_name, body, span) in &tir.asserts {
        match body {
            graphcal_syntax::ast::AssertBody::Expr(body_expr) => {
                let inferred = infer_type(
                    body_expr,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    &builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;
                if inferred != InferredType::Bool {
                    return Err(GraphcalError::AssertBodyNotBool {
                        found: format_inferred_type(&inferred, &tir.registry),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
            }
            graphcal_syntax::ast::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                is_relative,
            } => {
                let actual_type = infer_type(
                    actual,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    &builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;
                let expected_type = infer_type(
                    expected,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    &builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;
                let tolerance_type = infer_type(
                    tolerance,
                    &declared_types,
                    &empty_locals,
                    &tir.registry,
                    &builtin_fns,
                    &tir.resolved_fn_sigs,
                    src,
                )?;

                // actual and expected must have the same dimension
                let actual_dim = expect_scalar(&actual_type, &tir.registry, src, actual.span)?;
                let expected_dim =
                    expect_scalar(&expected_type, &tir.registry, src, expected.span)?;
                if actual_dim != expected_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: tir.registry.format_dimension(&actual_dim),
                        found: tir.registry.format_dimension(&expected_dim),
                        src: src.clone(),
                        span: expected.span.into(),
                        help: "actual and expected in tolerance assertion must have the same dimension".to_string(),
                    });
                }

                // tolerance: same dimension (absolute) or dimensionless/Int (relative %)
                let tolerance_ok = if *is_relative {
                    // Relative tolerance: accept Int or Dimensionless scalar
                    matches!(tolerance_type, InferredType::Int)
                        || matches!(&tolerance_type, InferredType::Scalar(d) if d.is_dimensionless())
                } else {
                    let tolerance_dim =
                        expect_scalar(&tolerance_type, &tir.registry, src, tolerance.span)?;
                    tolerance_dim == actual_dim
                };
                if !tolerance_ok {
                    let (expected_str, help_str) = if *is_relative {
                        (
                            "Dimensionless".to_string(),
                            "relative tolerance (%) must be dimensionless".to_string(),
                        )
                    } else {
                        (
                            tir.registry.format_dimension(&actual_dim),
                            "absolute tolerance must have the same dimension as actual/expected"
                                .to_string(),
                        )
                    };
                    return Err(GraphcalError::DimensionMismatch {
                        expected: expected_str,
                        found: format_inferred_type(&tolerance_type, &tir.registry),
                        src: src.clone(),
                        span: tolerance.span.into(),
                        help: help_str,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Check that an override expression has the correct dimension for the given param.
///
/// # Errors
///
/// Returns a [`GraphcalError::DimensionMismatch`] if the expression's inferred
/// dimension does not match the declared type of the param.
pub fn check_override_dimension(
    expr: &Expr,
    param_name: &str,
    declared_types: &HashMap<String, DeclaredType>,
    registry: &Registry,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, InferredType> = HashMap::new();

    let declared = &declared_types[param_name];
    let inferred = infer_type(
        expr,
        declared_types,
        &empty_locals,
        registry,
        &builtin_fns,
        resolved_fn_sigs,
        src,
    )?;

    if !types_match(declared, &inferred) {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_declared_type(declared, registry),
            found: format_inferred_type(&inferred, registry),
            src: src.clone(),
            span: expr.span.into(),
            help: format!(
                "override for `{param_name}` must have dimension {}",
                format_declared_type(declared, registry)
            ),
        });
    }
    Ok(())
}

/// Check if a declared type matches an inferred type.
fn types_match(declared: &DeclaredType, inferred: &InferredType) -> bool {
    match (declared, inferred) {
        (DeclaredType::Scalar(d), InferredType::Scalar(i)) => d == i,
        (DeclaredType::Bool, InferredType::Bool) | (DeclaredType::Int, InferredType::Int) => true,
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
fn format_declared_type(dt: &DeclaredType, registry: &Registry) -> String {
    match dt {
        DeclaredType::Scalar(d) => registry.format_dimension(d),
        DeclaredType::Bool => "Bool".to_string(),
        DeclaredType::Int => "Int".to_string(),
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
fn format_inferred_type(it: &InferredType, registry: &Registry) -> String {
    match it {
        InferredType::Scalar(d) => registry.format_dimension(d),
        InferredType::Bool => "Bool".to_string(),
        InferredType::Int => "Int".to_string(),
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
        InferredType::LoopVar(idx) => format!("<loop var: {idx}>"),
    }
}

/// Convert a `DeclaredType` to the corresponding `InferredType`.
fn declared_to_inferred(dt: &DeclaredType) -> InferredType {
    match dt {
        DeclaredType::Scalar(d) => InferredType::Scalar(d.clone()),
        DeclaredType::Bool => InferredType::Bool,
        DeclaredType::Int => InferredType::Int,
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
fn resolve_field_type(
    field_type_ann: &graphcal_syntax::ast::TypeExpr,
    type_def: &crate::registry::TypeDef,
    type_args: &[InferredType],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::registry::TypeGenericConstraint;

    if type_def.generic_params.is_empty() {
        // Non-generic type: resolve the field type_ann using the registry
        let dim =
            registry
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
fn check_derived_binop(
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
    let type_def =
        registry
            .get_type(lhs_name.as_str())
            .ok_or_else(|| GraphcalError::UnknownStructType {
                name: lhs_name.clone(),
                src: src.clone(),
                span: lhs_span.into(),
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
fn check_derived_neg(
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

fn expect_scalar(
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

/// Infer the type (dimension or struct) of an expression.
#[expect(
    clippy::too_many_lines,
    reason = "single match over all ExprKind variants"
)]
fn infer_type(
    expr: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_) => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ExprKind::Integer(_) => Ok(InferredType::Int),
        ExprKind::Bool(_) => Ok(InferredType::Bool),

        ExprKind::UnitLiteral { unit, .. } => {
            let (dim, _scale) = registry.resolve_unit_expr(unit).ok_or_else(|| {
                for item in &unit.terms {
                    if registry.get_unit(item.name.value.as_str()).is_none() {
                        return GraphcalError::UnknownUnit {
                            name: item.name.value.clone(),
                            src: src.clone(),
                            span: item.name.span.into(),
                        };
                    }
                }
                GraphcalError::UnknownUnit {
                    name: UnitName::new("unknown"),
                    src: src.clone(),
                    span: unit.span.into(),
                }
            })?;
            Ok(InferredType::Scalar(dim))
        }

        ExprKind::ConstRef(ident) => {
            let dt = declared_types.get(ident.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                }
            })?;
            Ok(declared_to_inferred(dt))
        }

        ExprKind::GraphRef(ident) => {
            let dt = declared_types.get(ident.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownGraphRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                }
            })?;
            Ok(declared_to_inferred(dt))
        }

        ExprKind::LocalRef(ident) => {
            local_types
                .get(&ident.name)
                .cloned()
                .ok_or_else(|| GraphcalError::UnknownLocalRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
        }

        ExprKind::BinOp { op, lhs, rhs } => {
            let lhs_type = infer_type(
                lhs,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            let rhs_type = infer_type(
                rhs,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;

            match op {
                // Logical operators: require Bool operands, return Bool
                BinOp::And | BinOp::Or => {
                    if lhs_type != InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Bool".to_string(),
                            found: format_inferred_type(&lhs_type, registry),
                            src: src.clone(),
                            span: lhs.span.into(),
                            help: "boolean operators require Bool operands".to_string(),
                        });
                    }
                    if rhs_type != InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Bool".to_string(),
                            found: format_inferred_type(&rhs_type, registry),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "boolean operators require Bool operands".to_string(),
                        });
                    }
                    Ok(InferredType::Bool)
                }
                // Equality: both operands must be same type (Bool, Int, or same-dimension Scalar)
                BinOp::Eq | BinOp::Ne => {
                    if lhs_type == InferredType::Bool
                        || rhs_type == InferredType::Bool
                        || lhs_type == InferredType::Int
                        || rhs_type == InferredType::Int
                    {
                        if lhs_type != rhs_type {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format_inferred_type(&lhs_type, registry),
                                found: format_inferred_type(&rhs_type, registry),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "equality operands must have the same type".to_string(),
                            });
                        }
                    } else {
                        let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                        let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                        if lhs_dim != rhs_dim {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: registry.format_dimension(&lhs_dim),
                                found: registry.format_dimension(&rhs_dim),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "comparison operands must have the same dimension"
                                    .to_string(),
                            });
                        }
                    }
                    Ok(InferredType::Bool)
                }
                // Ordering comparisons: require same-type scalar or Int operands, return Bool
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    if lhs_type == InferredType::Int || rhs_type == InferredType::Int {
                        if lhs_type != rhs_type {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format_inferred_type(&lhs_type, registry),
                                found: format_inferred_type(&rhs_type, registry),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "comparison operands must have the same type".to_string(),
                            });
                        }
                        return Ok(InferredType::Bool);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                    if lhs_dim != rhs_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: registry.format_dimension(&lhs_dim),
                            found: registry.format_dimension(&rhs_dim),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "comparison operands must have the same dimension".to_string(),
                        });
                    }
                    Ok(InferredType::Bool)
                }
                // Arithmetic operators: require matching numeric operands (Int or Scalar)
                // or structs with derive(Add)/derive(Sub)
                BinOp::Add | BinOp::Sub => {
                    if lhs_type == InferredType::Int && rhs_type == InferredType::Int {
                        return Ok(InferredType::Int);
                    }
                    // Check for derive(Add)/derive(Sub) on struct types
                    let derive_op = if *op == BinOp::Add {
                        graphcal_syntax::ast::DeriveOp::Add
                    } else {
                        graphcal_syntax::ast::DeriveOp::Sub
                    };
                    if let Some(result) = check_derived_binop(
                        &lhs_type, &rhs_type, derive_op, registry, src, lhs.span, rhs.span,
                    )? {
                        return Ok(result);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                    if lhs_dim != rhs_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: registry.format_dimension(&lhs_dim),
                            found: registry.format_dimension(&rhs_dim),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help:
                                "operands of addition and subtraction must have the same dimension"
                                    .to_string(),
                        });
                    }
                    Ok(InferredType::Scalar(lhs_dim))
                }
                BinOp::Mul => {
                    if lhs_type == InferredType::Int && rhs_type == InferredType::Int {
                        return Ok(InferredType::Int);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                    Ok(InferredType::Scalar(lhs_dim * rhs_dim))
                }
                BinOp::Div => {
                    if lhs_type == InferredType::Int && rhs_type == InferredType::Int {
                        return Ok(InferredType::Int);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                    Ok(InferredType::Scalar(lhs_dim / rhs_dim))
                }
                BinOp::Mod => {
                    if lhs_type == InferredType::Int && rhs_type == InferredType::Int {
                        return Ok(InferredType::Int);
                    }
                    Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format!(
                            "{} % {}",
                            format_inferred_type(&lhs_type, registry),
                            format_inferred_type(&rhs_type, registry)
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                        help: "modulo operator requires Int operands".to_string(),
                    })
                }
                BinOp::Pow => {
                    // Int ^ Int (literal non-negative) -> Int
                    if lhs_type == InferredType::Int {
                        if let ExprKind::Integer(n) = &rhs.kind {
                            if *n >= 0 {
                                return Ok(InferredType::Int);
                            }
                            return Err(GraphcalError::DimensionMismatch {
                                expected: "non-negative Int exponent".to_string(),
                                found: format!("{n}"),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "integer power requires a non-negative exponent".to_string(),
                            });
                        }
                        return Err(GraphcalError::NonLiteralExponent {
                            src: src.clone(),
                            span: rhs.span.into(),
                        });
                    }
                    // Scalar ^ ... (existing logic)
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                    if let ExprKind::Number(n) = &rhs.kind {
                        if n.fract() == 0.0 {
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "guarded by fract() == 0.0 check"
                            )]
                            let exp = *n as i32;
                            Ok(InferredType::Scalar(lhs_dim.pow(Rational::from_int(exp))))
                        } else {
                            #[expect(
                                clippy::float_cmp,
                                reason = "checking exact 0.5 literal for square-root exponent"
                            )]
                            if *n == 0.5 {
                                Ok(InferredType::Scalar(lhs_dim.pow(Rational::new(1, 2))))
                            } else {
                                Err(GraphcalError::NonLiteralExponent {
                                    src: src.clone(),
                                    span: rhs.span.into(),
                                })
                            }
                        }
                    } else if let ExprKind::Integer(n) = &rhs.kind {
                        // Scalar ^ integer_literal
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "exponent values are small integers"
                        )]
                        let exp = *n as i32;
                        Ok(InferredType::Scalar(lhs_dim.pow(Rational::from_int(exp))))
                    } else if rhs_dim.is_dimensionless() {
                        if lhs_dim.is_dimensionless() {
                            Ok(InferredType::Scalar(Dimension::dimensionless()))
                        } else {
                            Err(GraphcalError::NonLiteralExponent {
                                src: src.clone(),
                                span: rhs.span.into(),
                            })
                        }
                    } else {
                        Err(GraphcalError::NonLiteralExponent {
                            src: src.clone(),
                            span: rhs.span.into(),
                        })
                    }
                }
            }
        }

        ExprKind::UnaryOp { op, operand } => {
            let operand_type = infer_type(
                operand,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            match op {
                graphcal_syntax::ast::UnaryOp::Not => {
                    if operand_type != InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Bool".to_string(),
                            found: format_inferred_type(&operand_type, registry),
                            src: src.clone(),
                            span: operand.span.into(),
                            help: "logical NOT requires a Bool operand".to_string(),
                        });
                    }
                    Ok(InferredType::Bool)
                }
                graphcal_syntax::ast::UnaryOp::Neg => {
                    if operand_type == InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "numeric type".to_string(),
                            found: "Bool".to_string(),
                            src: src.clone(),
                            span: operand.span.into(),
                            help: "negation requires a numeric operand, not Bool".to_string(),
                        });
                    }
                    // Check for derive(Neg) on struct types
                    if let Some(result) =
                        check_derived_neg(&operand_type, registry, src, operand.span)?
                    {
                        return Ok(result);
                    }
                    // Negation preserves the type (Scalar or Int)
                    Ok(operand_type)
                }
            }
        }

        ExprKind::FnCall { name, args } => {
            // Aggregation functions over indexed values: sum, min, max, mean, count
            if matches!(
                name.value.as_str(),
                "sum" | "min" | "max" | "mean" | "count"
            ) && args.len() == 1
            {
                let arg_type = infer_type(
                    &args[0],
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;
                if let InferredType::Indexed { element, .. } = arg_type {
                    return Ok(if name.value.as_str() == "count" {
                        InferredType::Scalar(Dimension::dimensionless())
                    } else {
                        *element
                    });
                }
                // If not indexed, fall through to builtins (min/max are 2-arg builtins too)
            }

            // Conversion builtins: to_float(Int) -> Dimensionless, to_int(Dimensionless) -> Int
            if name.value.as_str() == "to_float" {
                if args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new("to_float"),
                        expected: 1,
                        got: args.len(),
                        src: src.clone(),
                        span: name.span.into(),
                    });
                }
                let arg_type = infer_type(
                    &args[0],
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;
                if arg_type != InferredType::Int {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: "to_float() requires an Int argument".to_string(),
                    });
                }
                return Ok(InferredType::Scalar(Dimension::dimensionless()));
            }
            if name.value.as_str() == "to_int" {
                if args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new("to_int"),
                        expected: 1,
                        got: args.len(),
                        src: src.clone(),
                        span: name.span.into(),
                    });
                }
                let arg_type = infer_type(
                    &args[0],
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;
                let arg_dim = expect_scalar(&arg_type, registry, src, args[0].span)?;
                if !arg_dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: registry.format_dimension(&arg_dim),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: "to_int() requires a Dimensionless argument".to_string(),
                    });
                }
                return Ok(InferredType::Int);
            }

            // Try builtin first
            if let Some(func) = builtin_fns.get(name.value.as_str()) {
                let arg_dims: Vec<Dimension> = args
                    .iter()
                    .map(|a| {
                        let t = infer_type(
                            a,
                            declared_types,
                            local_types,
                            registry,
                            builtin_fns,
                            resolved_fn_sigs,
                            src,
                        )?;
                        expect_scalar(&t, registry, src, a.span)
                    })
                    .collect::<Result<_, _>>()?;
                return infer_fn_dim(func.dim_sig, &arg_dims, args, registry, src)
                    .map(InferredType::Scalar);
            }

            // Try user-defined function via resolved signatures
            let fn_name_key = FnName::new(name.value.as_str());
            let sig = resolved_fn_sigs.get(&fn_name_key).ok_or_else(|| {
                GraphcalError::UnknownFunction {
                    name: name.value.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                }
            })?;

            // Arity check
            if args.len() != sig.params.len() {
                return Err(GraphcalError::WrongArity {
                    name: name.value.clone(),
                    expected: sig.params.len(),
                    got: args.len(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }

            // Infer arg types
            let arg_types: Vec<InferredType> = args
                .iter()
                .map(|a| {
                    infer_type(
                        a,
                        declared_types,
                        local_types,
                        registry,
                        builtin_fns,
                        resolved_fn_sigs,
                        src,
                    )
                })
                .collect::<Result<_, _>>()?;

            if sig.generic_dim_params.is_empty() && sig.generic_index_params.is_empty() {
                // Non-generic: check each param type using resolved signature
                for (i, param) in sig.params.iter().enumerate() {
                    let expected =
                        crate::tir::resolved_to_declared_type(&param.resolved_type, src)?;
                    let expected_inferred = declared_to_inferred(&expected);
                    if arg_types[i] != expected_inferred {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format_inferred_type(&expected_inferred, registry),
                            found: format_inferred_type(&arg_types[i], registry),
                            src: src.clone(),
                            span: args[i].span.into(),
                            help: format!(
                                "parameter `{}` expects {expected_inferred:?}",
                                param.name
                            ),
                        });
                    }
                }
                // Resolve return type
                let ret = crate::tir::resolved_to_declared_type(&sig.return_type, src)?;
                Ok(declared_to_inferred(&ret))
            } else {
                // Generic: unify generic params from arg types
                let mut dim_sub: HashMap<GenericParamName, Dimension> = HashMap::new();
                let mut index_sub: HashMap<GenericParamName, IndexName> = HashMap::new();
                for (i, param) in sig.params.iter().enumerate() {
                    crate::tir::unify_resolved_type(
                        &param.resolved_type,
                        &arg_types[i],
                        &mut dim_sub,
                        &mut index_sub,
                        registry,
                        src,
                        args[i].span,
                    )?;
                }
                // Resolve return type with substitution
                let ret_type = crate::tir::substitute_resolved_type(
                    &sig.return_type,
                    &dim_sub,
                    &index_sub,
                    src,
                )?;
                Ok(ret_type)
            }
        }

        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond_type = infer_type(
                condition,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            if cond_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&cond_type, registry),
                    src: src.clone(),
                    span: condition.span.into(),
                    help: "if/else condition must be Bool".to_string(),
                });
            }

            let then_type = infer_type(
                then_branch,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            let else_type = infer_type(
                else_branch,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;

            if then_type != else_type {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&then_type, registry),
                    found: format_inferred_type(&else_type, registry),
                    src: src.clone(),
                    span: else_branch.span.into(),
                    help: "both branches of if/else must have the same dimension".to_string(),
                });
            }

            Ok(then_type)
        }

        ExprKind::Convert {
            expr: inner,
            target,
        } => {
            let inner_type = infer_type(
                inner,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            let expr_dim = expect_scalar(&inner_type, registry, src, inner.span)?;
            let (target_dim, _scale) = registry.resolve_unit_expr(target).ok_or_else(|| {
                for item in &target.terms {
                    if registry.get_unit(item.name.value.as_str()).is_none() {
                        return GraphcalError::UnknownUnit {
                            name: item.name.value.clone(),
                            src: src.clone(),
                            span: item.name.span.into(),
                        };
                    }
                }
                GraphcalError::UnknownUnit {
                    name: UnitName::new("unknown"),
                    src: src.clone(),
                    span: target.span.into(),
                }
            })?;

            if expr_dim != target_dim {
                return Err(GraphcalError::ConversionDimensionMismatch {
                    target: registry.format_dimension(&target_dim),
                    expr_dim: registry.format_dimension(&expr_dim),
                    src: src.clone(),
                    span: target.span.into(),
                });
            }

            Ok(InferredType::Scalar(expr_dim))
        }

        ExprKind::AsCast {
            expr: inner,
            target_type,
        } => {
            let inner_type = infer_type(
                inner,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            // Resolve the target type
            let no_dim_params: &[GenericParamName] = &[];
            let no_index_params: &[GenericParamName] = &[];
            let resolved_target = crate::tir::resolve_type_expr(
                target_type,
                registry,
                no_dim_params,
                no_index_params,
                src,
            )?;
            let target_declared = crate::tir::resolved_to_declared_type(&resolved_target, src)?;
            let target_inferred = declared_to_inferred(&target_declared);

            // Both must be structs with the same name
            let InferredType::Struct(source_name, source_args) = &inner_type else {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "`as` cast requires a struct type, got {}",
                        format_inferred_type(&inner_type, registry)
                    ),
                    src: src.clone(),
                    span: inner.span.into(),
                });
            };
            let InferredType::Struct(target_name, target_args) = &target_inferred else {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "`as` cast target must be a struct type, got {}",
                        format_inferred_type(&target_inferred, registry)
                    ),
                    src: src.clone(),
                    span: target_type.span.into(),
                });
            };
            if source_name != target_name {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "`as` cast requires same struct type, got `{source_name}` and `{target_name}`"
                    ),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            // Verify non-phantom type args are identical (Dim and Index params must match)
            let type_def = registry.get_type(source_name.as_str()).ok_or_else(|| {
                GraphcalError::UnknownStructType {
                    name: source_name.clone(),
                    src: src.clone(),
                    span: inner.span.into(),
                }
            })?;
            for (i, param) in type_def.generic_params.iter().enumerate() {
                if param.constraint != crate::registry::TypeGenericConstraint::Unconstrained {
                    // Non-phantom param — must match exactly
                    if i < source_args.len()
                        && i < target_args.len()
                        && source_args[i] != target_args[i]
                    {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "`as` cast can only change phantom (Type) parameters; \
                                 parameter `{}` (constraint {:?}) differs: {} vs {}",
                                param.name,
                                param.constraint,
                                format_inferred_type(&source_args[i], registry),
                                format_inferred_type(&target_args[i], registry),
                            ),
                            src: src.clone(),
                            span: expr.span.into(),
                        });
                    }
                }
            }
            Ok(target_inferred)
        }

        ExprKind::Block { stmts, expr: body } => {
            let mut block_locals = local_types.clone();
            for binding in stmts {
                // Check for duplicate let bindings
                if let Some(existing) = block_locals.get(&binding.name.name) {
                    // Find the span of the first binding (search stmts processed so far)
                    let first_span = stmts
                        .iter()
                        .find(|b| b.name.name == binding.name.name && b.span != binding.span)
                        .map_or(binding.span, |b| b.name.span);
                    let _ = existing; // suppress unused warning
                    return Err(GraphcalError::DuplicateLetBinding {
                        name: binding.name.name.clone(),
                        src: src.clone(),
                        duplicate: binding.name.span.into(),
                        first: first_span.into(),
                    });
                }

                let rhs_type = infer_type(
                    &binding.value,
                    declared_types,
                    &block_locals,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;

                // If type annotation provided, check it matches
                if let Some(type_ann) = &binding.type_ann {
                    let resolved =
                        crate::tir::resolve_type_expr(type_ann, registry, &[], &[], src)?;
                    let ann_type = crate::tir::resolved_to_declared_type(&resolved, src)?;
                    let ann_inferred = declared_to_inferred(&ann_type);
                    if ann_inferred != rhs_type {
                        return Err(GraphcalError::DimensionMismatchInAnnotation {
                            declared: format_inferred_type(&ann_inferred, registry),
                            inferred: format_inferred_type(&rhs_type, registry),
                            src: src.clone(),
                            span: type_ann.span.into(),
                        });
                    }
                }

                block_locals.insert(binding.name.name.clone(), rhs_type);
            }
            infer_type(
                body,
                declared_types,
                &block_locals,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )
        }

        ExprKind::FieldAccess { expr: inner, field } => {
            let inner_type = infer_type(
                inner,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            match &inner_type {
                InferredType::Struct(type_name, type_args) => {
                    let type_def = registry.get_type(type_name.as_str()).ok_or_else(|| {
                        GraphcalError::UnknownStructType {
                            name: type_name.clone(),
                            src: src.clone(),
                            span: inner.span.into(),
                        }
                    })?;
                    // Field access is only allowed on single-variant (struct sugar) types
                    if !type_def.is_single_variant() {
                        return Err(GraphcalError::NotAStruct {
                            name: format!(
                                "multi-variant type `{type_name}` (use `match` to access fields)"
                            ),
                            src: src.clone(),
                            span: inner.span.into(),
                        });
                    }
                    let variant =
                        type_def
                            .variants
                            .first()
                            .ok_or_else(|| GraphcalError::NotAStruct {
                                name: type_name.to_string(),
                                src: src.clone(),
                                span: inner.span.into(),
                            })?;
                    let field_def = variant
                        .fields
                        .iter()
                        .find(|f| f.name.as_str() == field.value.as_str())
                        .ok_or_else(|| GraphcalError::UnknownField {
                            type_name: type_name.clone(),
                            field_name: field.value.clone(),
                            src: src.clone(),
                            span: field.span.into(),
                        })?;
                    resolve_field_type(&field_def.type_ann, type_def, type_args, registry, src)
                }
                _ => Err(GraphcalError::NotAStruct {
                    name: format_inferred_type(&inner_type, registry),
                    src: src.clone(),
                    span: inner.span.into(),
                }),
            }
        }

        ExprKind::StructConstruction {
            type_name,
            type_args: constructor_type_args,
            fields,
        } => {
            // Look up by type name first (single-variant / struct sugar),
            // then by variant name (multi-variant tagged union)
            let (type_def, variant_def) =
                if let Some(type_def) = registry.get_type(type_name.value.as_str()) {
                    // Single-variant: type_name == variant_name
                    let variant = type_def.variants.first().ok_or_else(|| {
                        GraphcalError::UnknownStructType {
                            name: type_name.value.clone(),
                            src: src.clone(),
                            span: type_name.span.into(),
                        }
                    })?;
                    (type_def, variant)
                } else if let Some((type_def, variant)) =
                    registry.get_type_by_variant(type_name.value.as_str())
                {
                    (type_def, variant)
                } else {
                    return Err(GraphcalError::UnknownStructType {
                        name: type_name.value.clone(),
                        src: src.clone(),
                        span: type_name.span.into(),
                    });
                };
            let owning_type_name = type_def.name.clone();

            // Resolve constructor type args for generic structs
            let resolved_type_args: Vec<InferredType> =
                if constructor_type_args.is_empty() && type_def.generic_params.is_empty() {
                    vec![]
                } else if !type_def.generic_params.is_empty() {
                    let total_params = type_def.generic_params.len();
                    let required_count = type_def
                        .generic_params
                        .iter()
                        .take_while(|p| p.default.is_none())
                        .count();
                    if constructor_type_args.len() < required_count
                        || constructor_type_args.len() > total_params
                    {
                        let hint = if required_count == total_params {
                            format!("{total_params}")
                        } else {
                            format!("{required_count}..{total_params}")
                        };
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "type `{}` expects {hint} type argument(s), got {}",
                                type_name.value,
                                constructor_type_args.len()
                            ),
                            src: src.clone(),
                            span: type_name.span.into(),
                        });
                    }
                    let no_dim_params: &[GenericParamName] = &[];
                    let no_index_params: &[GenericParamName] = &[];
                    let mut args = Vec::with_capacity(total_params);
                    for arg in constructor_type_args {
                        let resolved = crate::tir::resolve_type_expr(
                            arg,
                            registry,
                            no_dim_params,
                            no_index_params,
                            src,
                        )?;
                        let dt = crate::tir::resolved_to_declared_type(&resolved, src)?;
                        args.push(declared_to_inferred(&dt));
                    }
                    // Fill in defaults for remaining params
                    for param in type_def
                        .generic_params
                        .iter()
                        .skip(constructor_type_args.len())
                    {
                        let default_expr = param.default.as_ref().expect(
                            "params without defaults should have been caught by count check above",
                        );
                        let resolved = crate::tir::resolve_type_expr(
                            default_expr,
                            registry,
                            no_dim_params,
                            no_index_params,
                            src,
                        )?;
                        let dt = crate::tir::resolved_to_declared_type(&resolved, src)?;
                        args.push(declared_to_inferred(&dt));
                    }
                    args
                } else {
                    vec![]
                };

            // Check for extra fields
            let def_field_names: std::collections::HashSet<&str> =
                variant_def.fields.iter().map(|f| f.name.as_str()).collect();
            let provided_names: Vec<&str> = fields.iter().map(|f| f.name.value.as_str()).collect();
            let extra: Vec<FieldName> = provided_names
                .iter()
                .filter(|n| !def_field_names.contains(**n))
                .map(|n| FieldName::new(*n))
                .collect();
            if !extra.is_empty() {
                return Err(GraphcalError::ExtraFields {
                    type_name: type_name.value.clone(),
                    extra,
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }

            // Check for missing fields
            let provided_set: std::collections::HashSet<&str> =
                provided_names.iter().copied().collect();
            let missing: Vec<FieldName> = variant_def
                .fields
                .iter()
                .filter(|f| !provided_set.contains(f.name.as_str()))
                .map(|f| f.name.clone())
                .collect();
            if !missing.is_empty() {
                return Err(GraphcalError::MissingFields {
                    type_name: type_name.value.clone(),
                    missing,
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }

            // Type-check each field's value
            for field_init in fields {
                let field_def = variant_def
                    .fields
                    .iter()
                    .find(|f| f.name.as_str() == field_init.name.value.as_str())
                    .expect("extra fields already checked");

                let value_type = if let Some(value_expr) = &field_init.value {
                    infer_type(
                        value_expr,
                        declared_types,
                        local_types,
                        registry,
                        builtin_fns,
                        resolved_fn_sigs,
                        src,
                    )?
                } else {
                    // Shorthand: look up the local variable with the same name
                    local_types
                        .get(field_init.name.value.as_str())
                        .cloned()
                        .ok_or_else(|| GraphcalError::UnknownLocalRef {
                            name: field_init.name.value.to_string(),
                            src: src.clone(),
                            span: field_init.name.span.into(),
                        })?
                };

                let expected_field_type = resolve_field_type(
                    &field_def.type_ann,
                    type_def,
                    &resolved_type_args,
                    registry,
                    src,
                )?;
                if value_type != expected_field_type {
                    return Err(GraphcalError::FieldDimensionMismatch {
                        type_name: type_name.value.clone(),
                        field_name: field_init.name.value.clone(),
                        expected: format_inferred_type(&expected_field_type, registry),
                        found: format_inferred_type(&value_type, registry),
                        src: src.clone(),
                        span: field_init.name.span.into(),
                    });
                }
            }

            Ok(InferredType::Struct(owning_type_name, resolved_type_args))
        }

        ExprKind::ForComp { bindings, body } => {
            // Add loop variables to local_types, infer body type, wrap in Indexed layers
            let mut inner_locals = local_types.clone();
            for binding in bindings {
                let idx_name = binding.index.value.as_str();
                if registry.get_index(idx_name).is_none() {
                    return Err(GraphcalError::UnknownIndex {
                        name: binding.index.value.clone(),
                        src: src.clone(),
                        span: binding.index.span.into(),
                    });
                }
                inner_locals.insert(
                    binding.var.name.clone(),
                    InferredType::LoopVar(binding.index.value.clone()),
                );
            }
            let body_type = infer_type(
                body,
                declared_types,
                &inner_locals,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            // Wrap body type with index layers (outermost binding first)
            let mut result = body_type;
            for binding in bindings.iter().rev() {
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: binding.index.value.clone(),
                };
            }
            Ok(result)
        }

        ExprKind::MapLiteral { entries } => {
            if entries.is_empty() {
                return Err(GraphcalError::EvalError {
                    message: "empty map literal".to_string(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            // All entries must have the same index name
            let idx_name = &entries[0].index.value;
            // Validate index exists
            let idx_def = registry.get_index(idx_name.as_str()).ok_or_else(|| {
                GraphcalError::UnknownIndex {
                    name: entries[0].index.value.clone(),
                    src: src.clone(),
                    span: entries[0].index.span.into(),
                }
            })?;
            // Validate all entries use the same index
            for entry in entries {
                if entry.index.value != *idx_name {
                    return Err(GraphcalError::IndexMismatch {
                        expected: entries[0].index.value.clone(),
                        found: entry.index.value.clone(),
                        src: src.clone(),
                        span: entry.index.span.into(),
                    });
                }
            }
            // Check totality: all variants present, no extras
            let variants = idx_def.variants();
            let declared_variants: std::collections::HashSet<&str> =
                variants.iter().map(VariantName::as_str).collect();
            let provided_variants: std::collections::HashSet<&str> =
                entries.iter().map(|e| e.variant.value.as_str()).collect();
            let missing: Vec<VariantName> = declared_variants
                .difference(&provided_variants)
                .map(|s| VariantName::new(*s))
                .collect();
            let extra: Vec<VariantName> = provided_variants
                .difference(&declared_variants)
                .map(|s| VariantName::new(*s))
                .collect();
            if !missing.is_empty() {
                return Err(GraphcalError::MissingVariants {
                    index_name: entries[0].index.value.clone(),
                    missing,
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            if !extra.is_empty() {
                return Err(GraphcalError::ExtraVariants {
                    index_name: entries[0].index.value.clone(),
                    extra,
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            // Infer element type from first entry, check all entries match
            let first_type = infer_type(
                &entries[0].value,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            for entry in &entries[1..] {
                let entry_type = infer_type(
                    &entry.value,
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;
                if entry_type != first_type {
                    return Err(GraphcalError::DimensionMismatchInAnnotation {
                        declared: format_inferred_type(&first_type, registry),
                        inferred: format_inferred_type(&entry_type, registry),
                        src: src.clone(),
                        span: entry.value.span.into(),
                    });
                }
            }
            Ok(InferredType::Indexed {
                element: Box::new(first_type),
                index: entries[0].index.value.clone(),
            })
        }

        ExprKind::IndexAccess { expr: inner, args } => {
            let inner_type = infer_type(
                inner,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            // Peel off one index layer per argument
            let mut current = inner_type;
            for arg in args {
                let InferredType::Indexed {
                    element,
                    index: idx_name,
                } = current
                else {
                    return Err(GraphcalError::EvalError {
                        message: "indexing a non-indexed value".to_string(),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                };
                // Validate the argument matches the index
                match arg {
                    graphcal_syntax::ast::IndexArg::Variant { index, variant } => {
                        if index.value.as_str() != idx_name.as_str() {
                            return Err(GraphcalError::IndexMismatch {
                                expected: idx_name,
                                found: index.value.clone(),
                                src: src.clone(),
                                span: index.span.into(),
                            });
                        }
                        // Validate variant exists
                        let idx_def = registry
                            .get_index(idx_name.as_str())
                            .expect("index validated");
                        if !idx_def
                            .variants()
                            .iter()
                            .any(|v| v.as_str() == variant.value.as_str())
                        {
                            return Err(GraphcalError::UnknownVariant {
                                index_name: idx_name,
                                variant_name: variant.value.clone(),
                                src: src.clone(),
                                span: variant.span.into(),
                            });
                        }
                    }
                    graphcal_syntax::ast::IndexArg::Var(ident) => {
                        // Must be a loop variable with matching index
                        let var_type = local_types.get(&ident.name).ok_or_else(|| {
                            GraphcalError::UnknownLocalRef {
                                name: ident.name.clone(),
                                src: src.clone(),
                                span: ident.span.into(),
                            }
                        })?;
                        match var_type {
                            InferredType::LoopVar(var_idx) => {
                                if *var_idx != idx_name {
                                    return Err(GraphcalError::IndexMismatch {
                                        expected: idx_name,
                                        found: var_idx.clone(),
                                        src: src.clone(),
                                        span: ident.span.into(),
                                    });
                                }
                            }
                            InferredType::Scalar(_) => {
                                // Allow scalar locals to be used as index args
                                // for range indexes (e.g. prev_i, i in Unfold)
                                let idx_def = registry
                                    .get_index(idx_name.as_str())
                                    .expect("index validated");
                                if !idx_def.is_range() {
                                    return Err(GraphcalError::EvalError {
                                        message: format!("`{}` is not a loop variable", ident.name),
                                        src: src.clone(),
                                        span: ident.span.into(),
                                    });
                                }
                            }
                            _ => {
                                return Err(GraphcalError::EvalError {
                                    message: format!("`{}` is not a loop variable", ident.name),
                                    src: src.clone(),
                                    span: ident.span.into(),
                                });
                            }
                        }
                    }
                }
                current = *element;
            }
            Ok(current)
        }

        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => {
            // source must be indexed, init must be scalar matching element type
            let source_type = infer_type(
                source,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            let InferredType::Indexed { element, index } = source_type else {
                return Err(GraphcalError::EvalError {
                    message: "scan source must be an indexed value".to_string(),
                    src: src.clone(),
                    span: source.span.into(),
                });
            };
            let init_type = infer_type(
                init,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            // init and element must have the same type
            if init_type != *element {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&element, registry),
                    found: format_inferred_type(&init_type, registry),
                    src: src.clone(),
                    span: init.span.into(),
                    help: "scan init value must match element type of source".to_string(),
                });
            }
            // Bind acc and val as locals with element type
            let mut scan_locals = local_types.clone();
            scan_locals.insert(acc_name.name.clone(), *element.clone());
            scan_locals.insert(val_name.name.clone(), *element.clone());
            let body_type = infer_type(
                body,
                declared_types,
                &scan_locals,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            if body_type != *element {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&element, registry),
                    found: format_inferred_type(&body_type, registry),
                    src: src.clone(),
                    span: body.span.into(),
                    help: "scan body must return the same type as the accumulator".to_string(),
                });
            }
            // scan produces an indexed result with the same index
            Ok(InferredType::Indexed { element, index })
        }

        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => {
            // Unfold: unfold(init, |prev_i, i| body)
            // The node's declared type should be T[RangeIndex].
            // init has type T, body must also return T.
            // prev_name and curr_name are bound as loop variables.
            let init_type = infer_type(
                init,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;

            // Look up the declared type to find the range index and its dimension.
            // The declared type for the node should be Something[RangeIndex].
            // We need to find the range index to bind prev_name/curr_name with the right dimension.
            // For now, bind them as Dimensionless scalars — they will be refined
            // when the range index dimension is known from context.
            let mut scan_locals = local_types.clone();
            scan_locals.insert(
                prev_name.name.clone(),
                InferredType::Scalar(graphcal_syntax::dimension::Dimension::dimensionless()),
            );
            scan_locals.insert(
                curr_name.name.clone(),
                InferredType::Scalar(graphcal_syntax::dimension::Dimension::dimensionless()),
            );

            // Try to find the range index dimension from the declared types context.
            // The dim_check caller binds self-name → declared type.
            // We check if the declared type for this node is Indexed with a range index.
            // Walk declared_types to find a matching range index.
            for dt in declared_types.values() {
                if let DeclaredType::Indexed { index, .. } = dt
                    && let Some(idx_def) = registry.get_index(index.as_str())
                    && idx_def.is_range()
                {
                    if let crate::registry::IndexKind::Range { dimension, .. } = &idx_def.kind {
                        scan_locals.insert(
                            prev_name.name.clone(),
                            InferredType::Scalar(dimension.clone()),
                        );
                        scan_locals.insert(
                            curr_name.name.clone(),
                            InferredType::Scalar(dimension.clone()),
                        );
                    }
                    break;
                }
            }

            let body_type = infer_type(
                body,
                declared_types,
                &scan_locals,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            if body_type != init_type {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&init_type, registry),
                    found: format_inferred_type(&body_type, registry),
                    src: src.clone(),
                    span: body.span.into(),
                    help: "time scan body must return the same type as the init value".to_string(),
                });
            }

            // The result type is Indexed { element: init_type, index: <range_index> }
            // We need to find the range index name from the declared type context.
            // For now, return just the init_type and let the annotation check handle the wrapping.
            // If we can find the index from declared types, use it.
            for dt in declared_types.values() {
                if let DeclaredType::Indexed { index, .. } = dt
                    && let Some(idx_def) = registry.get_index(index.as_str())
                    && idx_def.is_range()
                {
                    return Ok(InferredType::Indexed {
                        element: Box::new(init_type),
                        index: index.clone(),
                    });
                }
            }

            // Fallback: return init_type (will fail annotation check if declared as indexed)
            Ok(init_type)
        }

        ExprKind::Match {
            scrutinee, arms, ..
        } => {
            // Infer scrutinee type — must be a struct (tagged union) type
            let scrutinee_type = infer_type(
                scrutinee,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;
            let (type_name, scrutinee_type_args) = match &scrutinee_type {
                InferredType::Struct(name, args) => (name.clone(), args.clone()),
                _ => {
                    return Err(GraphcalError::NotAStruct {
                        name: format_inferred_type(&scrutinee_type, registry),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    });
                }
            };
            let type_def = registry.get_type(type_name.as_str()).ok_or_else(|| {
                GraphcalError::UnknownStructType {
                    name: type_name.clone(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                }
            })?;

            // Track which variants are covered for exhaustiveness
            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut arm_types: Vec<InferredType> = Vec::new();

            for arm in arms {
                let variant_name_str = arm.pattern.variant_name.value.as_str();

                // Check variant belongs to this type
                let variant = type_def.get_variant(variant_name_str).ok_or_else(|| {
                    GraphcalError::UnknownField {
                        type_name: type_name.clone(),
                        field_name: FieldName::new(variant_name_str),
                        src: src.clone(),
                        span: arm.pattern.variant_name.span.into(),
                    }
                })?;

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for variant `{variant_name_str}`"),
                        src: src.clone(),
                        span: arm.pattern.span.into(),
                    });
                }

                // Bind pattern variables as locals
                let mut arm_locals = local_types.clone();
                for binding in &arm.pattern.bindings {
                    match binding {
                        graphcal_syntax::ast::PatternBinding::Bind { field, var } => {
                            let field_def = variant
                                .fields
                                .iter()
                                .find(|f| f.name.as_str() == field.value.as_str())
                                .ok_or_else(|| GraphcalError::UnknownField {
                                    type_name: type_name.clone(),
                                    field_name: field.value.clone(),
                                    src: src.clone(),
                                    span: field.span.into(),
                                })?;
                            let field_type = resolve_field_type(
                                &field_def.type_ann,
                                type_def,
                                &scrutinee_type_args,
                                registry,
                                src,
                            )?;
                            arm_locals.insert(var.name.clone(), field_type);
                        }
                        graphcal_syntax::ast::PatternBinding::Wildcard { .. } => {
                            // Wildcard: no binding needed
                        }
                    }
                }

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    &arm_locals,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;
                arm_types.push(arm_type);
            }

            // Check exhaustiveness: all variants must be covered
            for variant in &type_def.variants {
                if !covered.contains(variant.name.as_str()) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "non-exhaustive match: variant `{}` not covered",
                            variant.name.as_str()
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }

            // All arm types must match
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
                // Empty match (empty type) — should not happen in practice
                Err(GraphcalError::EvalError {
                    message: "match expression has no arms".to_string(),
                    src: src.clone(),
                    span: expr.span.into(),
                })
            }
        }
    }
}

/// Infer the result dimension of a built-in function call given its `DimSignature`.
fn infer_fn_dim(
    sig: DimSignature,
    arg_dims: &[Dimension],
    args: &[Expr],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    use graphcal_syntax::dimension::BaseDimId;

    match sig {
        DimSignature::AllDimensionless => {
            for (dim, arg) in arg_dims.iter().zip(args) {
                if !dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: registry.format_dimension(dim),
                        src: src.clone(),
                        span: arg.span.into(),
                        help: "this function requires Dimensionless arguments".to_string(),
                    });
                }
            }
            Ok(Dimension::dimensionless())
        }
        DimSignature::AngleToDimensionless => {
            let angle = Dimension::base(BaseDimId(7));
            if arg_dims[0] != angle {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Angle".to_string(),
                    found: registry.format_dimension(&arg_dims[0]),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "trigonometric functions require an Angle argument".to_string(),
                });
            }
            Ok(Dimension::dimensionless())
        }
        DimSignature::DimensionlessToAngle => {
            if !arg_dims[0].is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.format_dimension(&arg_dims[0]),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "inverse trigonometric functions require a Dimensionless argument"
                        .to_string(),
                });
            }
            Ok(Dimension::base(BaseDimId(7)))
        }
        DimSignature::Sqrt => {
            // Result dimension is arg^(1/2)
            Ok(arg_dims[0].pow(Rational::new(1, 2)))
        }
        DimSignature::Passthrough => Ok(arg_dims[0].clone()),
        DimSignature::SameDimension => {
            if arg_dims[0] != arg_dims[1] {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.format_dimension(&arg_dims[0]),
                    found: registry.format_dimension(&arg_dims[1]),
                    src: src.clone(),
                    span: args[1].span.into(),
                    help: "both arguments must have the same dimension".to_string(),
                });
            }
            Ok(arg_dims[0].clone())
        }
        DimSignature::SameDimensionToAngle => {
            if arg_dims[0] != arg_dims[1] {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.format_dimension(&arg_dims[0]),
                    found: registry.format_dimension(&arg_dims[1]),
                    src: src.clone(),
                    span: args[1].span.into(),
                    help: "both arguments must have the same dimension".to_string(),
                });
            }
            Ok(Dimension::base(BaseDimId(7)))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use graphcal_syntax::dimension::BaseDimId;
    use graphcal_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn check(source: &str) -> Result<HashMap<String, DeclaredType>, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        let src = make_src(source);
        let ir = crate::ir::lower(&file, &src)?;
        let tir = crate::tir::type_resolve(ir, &src)?;
        check_dimensions_tir(&tir, &src)?;
        tir.build_declared_types(&src)
    }

    #[test]
    fn check_dimensionless_const() {
        let types = check("const G0: Dimensionless = 9.80665;").unwrap();
        assert_eq!(
            types["G0"],
            DeclaredType::Scalar(Dimension::dimensionless())
        );
    }

    #[test]
    fn check_dimensionless_arithmetic() {
        let types =
            check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
        assert_eq!(types["y"], DeclaredType::Scalar(Dimension::dimensionless()));
    }

    #[test]
    fn check_length_unit_literal() {
        let types = check("param alt: Length = 400.0 km;").unwrap();
        let length = Dimension::base(BaseDimId(0));
        assert_eq!(types["alt"], DeclaredType::Scalar(length));
    }

    #[test]
    fn check_velocity_from_division() {
        let source = "param dist: Length = 100.0 km;\nparam time: Time = 2.0 hour;\nnode speed: Velocity = @dist / @time;";
        let types = check(source).unwrap();
        let velocity = Dimension::base(BaseDimId(0)) / Dimension::base(BaseDimId(1));
        assert_eq!(types["speed"], DeclaredType::Scalar(velocity));
    }

    #[test]
    fn check_add_dimension_mismatch() {
        let source = "param x: Length = 1.0 m;\nparam y: Time = 1.0 s;\nnode z: Length = @x + @y;";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_annotation_mismatch() {
        let source = "param x: Length = 1.0 m;\nnode y: Time = @x;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_conversion_same_dimension() {
        let source =
            "param speed: Velocity = 100.0 m / s;\nnode speed_kmh: Velocity = @speed -> km / hour;";
        let types = check(source).unwrap();
        let velocity = Dimension::base(BaseDimId(0)) / Dimension::base(BaseDimId(1));
        assert_eq!(types["speed_kmh"], DeclaredType::Scalar(velocity));
    }

    #[test]
    fn check_conversion_wrong_dimension() {
        let source = "param x: Length = 1.0 m;\nnode y: Length = @x -> s;";
        let err = check(source).unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::ConversionDimensionMismatch { .. }
        ));
    }

    #[test]
    fn check_sqrt_dimension() {
        let source = "param area: Area = 100.0 m;\nnode side: Length = sqrt(@area);";
        // Note: area should be m^2, but we declared it with m (Length).
        // sqrt(Length) = Length^(1/2) which doesn't match Length.
        let err = check(source).unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::DimensionMismatchInAnnotation { .. }
        ));
    }

    #[test]
    fn check_builtin_sin_requires_angle() {
        let source = "param x: Length = 1.0 m;\nnode y: Dimensionless = sin(@x);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_if_branches_same_dim() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };";
        check(source).unwrap();
    }

    #[test]
    fn check_if_branches_different_dim() {
        let source = "param x: Length = 1.0 m;\nnode y: Length = if true { @x } else { 0.0 };";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_multiplication_creates_new_dim() {
        let source = "param mass: Mass = 10.0 kg;\nparam accel: Acceleration = 9.8 m / s^2;\nnode force: Force = @mass * @accel;";
        check(source).unwrap();
    }

    #[test]
    fn check_power_with_literal() {
        let source = "param r: Length = 5.0 m;\nnode area: Area = @r ^ 2.0;";
        // Area is Length^2, r^2 = Length^2
        // But we need PI * r^2 for circle area — just testing r^2 = Area
        check(source).unwrap();
    }

    // --- User-defined function tests ---

    #[test]
    fn check_non_generic_fn_call() {
        let source = "fn add_lengths(a: Length, b: Length) -> Length = a + b;\nparam x: Length = 1.0 m;\nparam y: Length = 2.0 m;\nnode z: Length = add_lengths(@x, @y);";
        check(source).unwrap();
    }

    #[test]
    fn check_non_generic_fn_dim_mismatch() {
        let source = "fn add_lengths(a: Length, b: Length) -> Length = a + b;\nparam x: Length = 1.0 m;\nparam t: Time = 1.0 s;\nnode z: Length = add_lengths(@x, @t);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_non_generic_fn_return_type() {
        // Function returns Velocity but we annotate as Length
        let source = "fn speed(d: Length, t: Time) -> Velocity = d / t;\nparam d: Length = 10.0 m;\nparam t: Time = 2.0 s;\nnode v: Length = speed(@d, @t);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_generic_fn_call() {
        let source = "fn double<D: Dim>(x: D) -> D = x + x;\nparam alt: Length = 100.0 km;\nnode doubled: Length = double(@alt);";
        check(source).unwrap();
    }

    #[test]
    fn check_generic_fn_multi_param() {
        let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;\nparam x: Length = 100.0 km;\nparam y: Length = 200.0 km;\nnode mid: Length = lerp(@x, @y, 0.5);";
        check(source).unwrap();
    }

    #[test]
    fn check_generic_fn_consistency_error() {
        // a: D binds D=Length, b: D expects Length but gets Time
        let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;\nparam x: Length = 100.0 km;\nparam t: Time = 1.0 s;\nnode bad: Length = lerp(@x, @t, 0.5);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_generic_fn_infers_return_type() {
        // Return type D should be inferred as Velocity
        let source = "fn identity<D: Dim>(x: D) -> D = x;\nparam v: Velocity = 10.0 m / s;\nnode w: Velocity = identity(@v);";
        check(source).unwrap();
    }

    #[test]
    fn check_generic_fn_wrong_annotation() {
        // identity returns Velocity (D=Velocity) but annotation says Length
        let source = "fn identity<D: Dim>(x: D) -> D = x;\nparam v: Velocity = 10.0 m / s;\nnode w: Length = identity(@v);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_fn_wrong_arity() {
        let source =
            "fn f(a: Length) -> Length = a;\nparam x: Length = 1.0 m;\nnode y: Length = f(@x, @x);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::WrongArity { .. }));
    }

    #[test]
    fn check_fn_unknown_function() {
        let source = "param x: Length = 1.0 m;\nnode y: Length = no_such_fn(@x);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
    }

    // --- Indexed type tests ---

    #[test]
    fn check_indexed_param_map_literal() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};";
        let types = check(source).unwrap();
        let velocity = Dimension::base(BaseDimId(0)) / Dimension::base(BaseDimId(1));
        assert_eq!(
            types["dv"],
            DeclaredType::Indexed {
                element: Box::new(DeclaredType::Scalar(velocity)),
                index: IndexName::new("Maneuver"),
            }
        );
    }

    #[test]
    fn check_for_comprehension() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node doubled: Velocity[Maneuver] = for m: Maneuver { @dv[m] + @dv[m] };";
        check(source).unwrap();
    }

    #[test]
    fn check_for_comprehension_type_mismatch() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node bad: Length[Maneuver] = for m: Maneuver { @dv[m] };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_index_access_with_variant() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node first: Velocity = @dv[Maneuver::Departure];";
        check(source).unwrap();
    }

    #[test]
    fn check_map_literal_missing_variant() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
};";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::MissingVariants { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_map_literal_extra_variant() {
        let source = "\
index Maneuver = { Departure, Correction }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::ExtraVariants { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_index_mismatch_in_for() {
        let source = "\
index Phase = { Coast, Burn }
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node bad: Velocity[Phase] = for p: Phase { @dv[p] };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::IndexMismatch { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_sum_aggregation() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node total_dv: Velocity = sum(@dv);";
        check(source).unwrap();
    }

    #[test]
    fn check_count_aggregation() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node n: Dimensionless = count(@dv);";
        check(source).unwrap();
    }

    #[test]
    fn check_mean_aggregation() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node avg_dv: Velocity = mean(@dv);";
        check(source).unwrap();
    }

    #[test]
    fn check_scan() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node cum_dv: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc + val);";
        check(source).unwrap();
    }

    #[test]
    fn check_scan_type_mismatch() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 m, |acc, val| acc + val);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_unknown_index_in_type_annotation() {
        let source = "param x: Velocity[NoSuchIndex] = 1.0 m / s;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownIndex { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_generic_index_fn() {
        // fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node total_dv: Velocity = total(@dv);";
        check(source).unwrap();
    }

    #[test]
    fn check_generic_index_fn_wrong_return() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node bad: Length = total(@dv);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_for_with_sum() {
        // sum over a for comprehension
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node total: Velocity = sum(for m: Maneuver { @dv[m] });";
        check(source).unwrap();
    }

    // --- Comparison dimension mismatch ---

    #[test]
    fn check_comparison_dimension_mismatch() {
        let source = "\
param x: Length = 1.0 m;
param t: Time = 1.0 s;
node bad: Dimensionless = if @x > @t { 1.0 } else { 0.0 };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- Boolean operator dimension errors ---

    #[test]
    fn check_boolean_and_lhs_dimensioned() {
        let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = @x && true;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_boolean_or_rhs_dimensioned() {
        let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = true || @x;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- Power / exponent edge cases ---

    #[test]
    fn check_power_half_exponent() {
        // x ^ 0.5 on dimensionless should work
        let source = "param x: Dimensionless = 4.0;\nnode y: Dimensionless = @x ^ 0.5;";
        check(source).unwrap();
    }

    #[test]
    fn check_power_non_literal_exponent_dimensioned_base() {
        // dimensioned ^ non-literal → NonLiteralExponent
        let source = "\
param x: Length = 1.0 m;
param n: Dimensionless = 2.0;
node bad: Area = @x ^ @n;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::NonLiteralExponent { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_power_dimensionless_base_non_literal_exponent() {
        // dimensionless ^ dimensionless (non-literal) → ok
        let source = "\
param x: Dimensionless = 2.0;
param n: Dimensionless = 3.0;
node y: Dimensionless = @x ^ @n;";
        check(source).unwrap();
    }

    #[test]
    fn check_power_bad_fractional_exponent() {
        // x ^ 0.3 → NonLiteralExponent (not 0.5 and not integer)
        let source = "param x: Length = 1.0 m;\nnode bad: Length = @x ^ 0.3;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::NonLiteralExponent { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_power_dimensioned_exponent() {
        // anything ^ dimensioned → NonLiteralExponent
        let source = "\
param x: Dimensionless = 2.0;
param n: Length = 1.0 m;
node bad: Dimensionless = @x ^ @n;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::NonLiteralExponent { .. }),
            "got: {err:?}"
        );
    }

    // --- If condition must be dimensionless ---

    #[test]
    fn check_if_condition_dimensioned() {
        let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = if @x { 1.0 } else { 0.0 };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- Unknown dimension in type annotation ---

    #[test]
    fn check_unknown_dimension_in_type() {
        let source = "param x: NoSuchDimension = 1.0;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownDimension { .. }),
            "got: {err:?}"
        );
    }

    // --- expect_scalar error: struct used where scalar expected ---

    #[test]
    fn check_struct_in_arithmetic() {
        let source = "\
type Orbit { altitude: Length, speed: Velocity }
param o: Orbit = Orbit { altitude: 400.0 km, speed: 7.6 km / s };
node bad: Length = @o + 1.0 m;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- FieldAccess on non-struct ---

    #[test]
    fn check_field_access_on_scalar() {
        let source = "\
param x: Length = 1.0 m;
node bad: Length = @x.foo;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::NotAStruct { .. }),
            "got: {err:?}"
        );
    }

    // --- Struct extra fields ---

    #[test]
    fn check_struct_extra_fields() {
        let source = "\
type Orbit { altitude: Length, speed: Velocity }
node o: Orbit = Orbit { altitude: 400.0 km, speed: 7.6 km / s, bonus: 1.0 };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::ExtraFields { .. }),
            "got: {err:?}"
        );
    }

    // --- Block let-binding type annotation mismatch ---

    #[test]
    fn check_block_let_type_mismatch() {
        let source = "\
param x: Length = 1.0 m;
node y: Dimensionless = {
    let a: Time = @x;
    1.0
};";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    // --- types_match wildcard: mismatched kinds ---

    #[test]
    fn check_types_match_struct_vs_scalar() {
        // Declared as a struct type but expression evaluates to scalar → mismatch
        let source = "\
type Orbit { altitude: Length, speed: Velocity }
param x: Dimensionless = 1.0;
node o: Orbit = @x;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    // --- ForComp with unknown index ---

    #[test]
    fn check_for_comp_unknown_index() {
        let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = for m: NoSuchIndex { @x };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownIndex { .. }),
            "got: {err:?}"
        );
    }

    // --- Scan body type mismatch ---

    #[test]
    fn check_scan_body_type_mismatch() {
        let source = "\
index Maneuver = { Departure, Correction, Insertion }
param dv: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km / s,
    Maneuver::Correction: 0.5 km / s,
    Maneuver::Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc * val);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::DimensionMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- Scan on non-indexed value ---

    #[test]
    fn check_scan_on_scalar() {
        let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = scan(@x, 0.0, |acc, val| acc + val);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::EvalError { .. }),
            "got: {err:?}"
        );
    }

    // --- Map literal dimension inconsistency ---

    #[test]
    fn check_map_literal_inconsistent_element_dims() {
        let source = "\
index Phase = { Coast, Burn }
param x: Dimensionless[Phase] = {
    Phase::Coast: 1.0,
    Phase::Burn: 2.0 m,
};";
        let err = check(source).unwrap_err();
        // The map entries have different dimensions: first is Dimensionless, second is Length
        assert!(
            matches!(
                err,
                GraphcalError::DimensionMismatchInAnnotation { .. }
                    | GraphcalError::DimensionMismatch { .. }
            ),
            "got: {err:?}"
        );
    }

    // --- Index access with unknown variant ---

    #[test]
    fn check_index_access_unknown_variant() {
        let source = "\
index Phase = { Coast, Burn }
param x: Dimensionless[Phase] = {
    Phase::Coast: 1.0,
    Phase::Burn: 2.0,
};
node bad: Dimensionless = @x[Phase::NoSuch];";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownVariant { .. }),
            "got: {err:?}"
        );
    }

    // --- Indexing a non-indexed value ---

    #[test]
    fn check_index_access_on_scalar() {
        let source = "\
index Phase = { Coast, Burn }
param x: Dimensionless = 1.0;
node bad: Dimensionless = @x[Phase::Coast];";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::EvalError { .. }),
            "got: {err:?}"
        );
    }

    // --- Index access with wrong index name ---

    #[test]
    fn check_index_access_wrong_index() {
        let source = "\
index Phase = { Coast, Burn }
index Stage = { First, Second }
param x: Dimensionless[Phase] = {
    Phase::Coast: 1.0,
    Phase::Burn: 2.0,
};
node bad: Dimensionless = @x[Stage::First];";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::IndexMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through if/else sub-expressions ---

    #[test]
    fn check_if_error_in_condition() {
        // Error inside condition sub-expression (unknown unit)
        let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if (1.0 foobar > 0.0) { 1.0 } else { 0.0 };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_if_error_in_then_branch() {
        // Error in then-branch sub-expression
        let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if true { 1.0 foobar } else { 0.0 };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_if_error_in_else_branch() {
        // Error in else-branch sub-expression
        let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if true { 0.0 } else { 1.0 foobar };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through convert sub-expression ---

    #[test]
    fn check_convert_error_in_inner() {
        // Error inside the inner expression of a convert
        let source = "\
node bad: Length = (1.0 foobar) -> m;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through block binding ---

    #[test]
    fn check_block_error_in_binding() {
        // Error inside a let-binding value
        let source = "\
node bad: Dimensionless = {
    let a = 1.0 foobar;
    1.0
};";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through field access inner expression ---

    #[test]
    fn check_field_access_error_in_inner() {
        let source = "\
type Orbit { altitude: Length, speed: Velocity }
node bad: Length = (1.0 foobar).altitude;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through struct construction field value ---

    #[test]
    fn check_struct_construction_error_in_field_value() {
        let source = "\
type Orbit { altitude: Length, speed: Velocity }
node o: Orbit = Orbit { altitude: 1.0 foobar, speed: 7.6 km / s };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through for comprehension body ---

    #[test]
    fn check_for_comp_error_in_body() {
        let source = "\
index Phase = { Coast, Burn }
node bad: Dimensionless[Phase] = for p: Phase { 1.0 foobar };";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through aggregation arg ---

    #[test]
    fn check_aggregation_error_in_arg() {
        let source = "\
node bad: Dimensionless = sum(1.0 foobar);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through generic fn args ---

    #[test]
    fn check_generic_fn_error_in_arg() {
        let source = "\
fn identity<D: Dim>(x: D) -> D = x;
node bad: Dimensionless = identity(1.0 foobar);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through scan source/init ---

    #[test]
    fn check_scan_error_in_source() {
        let source = "\
index Phase = { Coast, Burn }
node bad: Dimensionless[Phase] = scan(1.0 foobar, 0.0, |acc, val| acc + val);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Error propagation through map literal entry ---

    #[test]
    fn check_map_literal_error_in_entry() {
        let source = "\
index Phase = { Coast, Burn }
node bad: Dimensionless[Phase] = {
    Phase::Coast: 1.0 foobar,
    Phase::Burn: 2.0,
};";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::UnknownUnit { .. }),
            "got: {err:?}"
        );
    }

    // --- Map literal with mixed index names ---

    #[test]
    fn check_map_literal_mixed_index_names() {
        let source = "\
index Phase = { Coast, Burn }
index Stage = { First, Second }
param x: Dimensionless[Phase] = {
    Phase::Coast: 1.0,
    Stage::Second: 2.0,
};";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, GraphcalError::IndexMismatch { .. }),
            "got: {err:?}"
        );
    }

    // --- Block let-binding with valid type annotation ---

    #[test]
    fn check_block_let_type_annotation_ok() {
        let source = "\
param x: Length = 1.0 m;
node y: Length = {
    let a: Length = @x;
    a
};";
        check(source).unwrap();
    }
}
