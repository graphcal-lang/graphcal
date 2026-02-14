use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{BinOp, DeclKind, Expr, ExprKind, File, MulDivOp, TypeExprKind};
use graphcal_syntax::dimension::{Dimension, Rational};
use graphcal_syntax::names::{
    DimName, FieldName, FnName, GenericParamName, IndexName, StructTypeName, UnitName, VariantName,
};

use crate::builtins::{DimSignature, builtin_constants, builtin_functions};
use crate::error::GraphcalError;
use crate::registry::Registry;

/// The declared type of a const/param/node: either a scalar with a dimension, a bool, or a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredType {
    Scalar(Dimension),
    Bool,
    Int,
    Struct(StructTypeName),
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
    Struct(StructTypeName),
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
pub fn check_dimensions(
    file: &File,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<String, DeclaredType>, GraphcalError> {
    check_dimensions_with_imports(file, registry, src, &HashMap::new())
}

/// Check dimensions with pre-populated declared types from imports.
///
/// `imported_types` maps imported declaration names to their declared types.
/// These are added to `declared_types` before checking the file's own declarations.
pub fn check_dimensions_with_imports(
    file: &File,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    imported_types: &HashMap<String, DeclaredType>,
) -> Result<HashMap<String, DeclaredType>, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    // Collect declared types for all consts/params/nodes
    let mut declared_types: HashMap<String, DeclaredType> = HashMap::new();

    // Include imported types
    for (name, dt) in imported_types {
        declared_types.insert(name.clone(), dt.clone());
    }

    // Built-in constants are Dimensionless
    for name in builtin_consts.keys() {
        declared_types.insert(
            (*name).to_string(),
            DeclaredType::Scalar(Dimension::DIMENSIONLESS),
        );
    }

    // First pass: resolve declared type annotations
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Fn(_)
            | DeclKind::Index(_)
            | DeclKind::Use(_) => {}
            DeclKind::Const(c) => {
                let dt = resolve_type_annotation(&c.type_ann, registry, src)?;
                declared_types.insert(c.name.value.to_string(), dt);
            }
            DeclKind::Param(p) => {
                let dt = resolve_type_annotation(&p.type_ann, registry, src)?;
                declared_types.insert(p.name.value.to_string(), dt);
            }
            DeclKind::Node(n) => {
                let dt = resolve_type_annotation(&n.type_ann, registry, src)?;
                declared_types.insert(n.name.value.to_string(), dt);
            }
        }
    }

    // Second pass: infer types and check against annotations
    let empty_locals: HashMap<String, InferredType> = HashMap::new();
    for decl in &file.declarations {
        let (name, type_ann, value_expr) = match &decl.kind {
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Fn(_)
            | DeclKind::Index(_)
            | DeclKind::Use(_) => {
                continue;
            }
            DeclKind::Const(c) => (c.name.value.as_str(), &c.type_ann, &c.value),
            DeclKind::Param(p) => (p.name.value.as_str(), &p.type_ann, &p.value),
            DeclKind::Node(n) => (n.name.value.as_str(), &n.type_ann, &n.value),
        };

        let declared = &declared_types[name];
        let inferred = infer_type(
            value_expr,
            &declared_types,
            &empty_locals,
            registry,
            &builtin_fns,
            src,
        )?;

        if !types_match(declared, &inferred) {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_declared_type(declared),
                inferred: format_inferred_type(&inferred),
                src: src.clone(),
                span: type_ann.span.into(),
            });
        }
    }

    Ok(declared_types)
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
        src,
    )?;

    if !types_match(declared, &inferred) {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_declared_type(declared),
            found: format_inferred_type(&inferred),
            src: src.clone(),
            span: expr.span.into(),
            help: format!(
                "override for `{param_name}` must have dimension {}",
                format_declared_type(declared)
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
        (DeclaredType::Struct(d), InferredType::Struct(i)) => d == i,
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
fn format_declared_type(dt: &DeclaredType) -> String {
    match dt {
        DeclaredType::Scalar(d) => format!("{d}"),
        DeclaredType::Bool => "Bool".to_string(),
        DeclaredType::Int => "Int".to_string(),
        DeclaredType::Struct(name) => name.to_string(),
        DeclaredType::Indexed { element, index } => {
            format!("{}[{index}]", format_declared_type(element))
        }
    }
}

/// Format an inferred type for display in diagnostics.
fn format_inferred_type(it: &InferredType) -> String {
    match it {
        InferredType::Scalar(d) => format!("{d}"),
        InferredType::Bool => "Bool".to_string(),
        InferredType::Int => "Int".to_string(),
        InferredType::Struct(name) => name.to_string(),
        InferredType::Indexed { element, index } => {
            format!("{}[{index}]", format_inferred_type(element))
        }
        InferredType::LoopVar(idx) => format!("<loop var: {idx}>"),
    }
}

/// Convert a `DeclaredType` to the corresponding `InferredType`.
fn declared_to_inferred(dt: &DeclaredType) -> InferredType {
    match dt {
        DeclaredType::Scalar(d) => InferredType::Scalar(*d),
        DeclaredType::Bool => InferredType::Bool,
        DeclaredType::Int => InferredType::Int,
        DeclaredType::Struct(n) => InferredType::Struct(n.clone()),
        DeclaredType::Indexed { element, index } => InferredType::Indexed {
            element: Box::new(declared_to_inferred(element)),
            index: index.clone(),
        },
    }
}

/// Resolve a type annotation to a `DeclaredType`.
///
/// Checks the struct registry first (for single-term `DimExpr` that match a struct name),
/// then falls back to dimension resolution.
pub fn resolve_type_annotation(
    type_ann: &graphcal_syntax::ast::TypeExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclaredType, GraphcalError> {
    match &type_ann.kind {
        TypeExprKind::Bool => Ok(DeclaredType::Bool),
        TypeExprKind::Int => Ok(DeclaredType::Int),
        TypeExprKind::Indexed { base, indexes } => {
            // Resolve the base type, then wrap with each index (right to left for nesting)
            let mut result = resolve_type_annotation(base, registry, src)?;
            for idx in indexes.iter().rev() {
                let idx_name = &idx.name;
                if registry.get_index(idx_name).is_none() {
                    return Err(GraphcalError::UnknownIndex {
                        name: idx.as_index_name(),
                        src: src.clone(),
                        span: idx.span.into(),
                    });
                }
                result = DeclaredType::Indexed {
                    element: Box::new(result),
                    index: idx.as_index_name(),
                };
            }
            Ok(result)
        }
        _ => {
            // Check if this is a single-term DimExpr that matches a struct name
            if let TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
                && dim_expr.terms.len() == 1
                && dim_expr.terms[0].term.power.is_none()
            {
                let name = &dim_expr.terms[0].term.name.name;
                if registry.get_struct(name).is_some() {
                    return Ok(DeclaredType::Struct(StructTypeName::new(name)));
                }
            }

            // Fall back to dimension resolution
            let dim = registry
                .resolve_type_expr(type_ann)
                .ok_or_else(|| unknown_dim_in_type(type_ann, src))?;
            Ok(DeclaredType::Scalar(dim))
        }
    }
}

/// Produce a helpful error when a type annotation references an unknown dimension.
fn unknown_dim_in_type(
    type_ann: &graphcal_syntax::ast::TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    // Try to find the first unknown dimension name in the type expression
    if let graphcal_syntax::ast::TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
        && let Some(item) = dim_expr.terms.first()
    {
        return GraphcalError::UnknownDimension {
            name: item.term.name.as_dim_name(),
            src: src.clone(),
            span: item.term.span.into(),
        };
    }
    GraphcalError::UnknownDimension {
        name: DimName::new("unknown"),
        src: src.clone(),
        span: type_ann.span.into(),
    }
}

/// Helper: extract scalar dimension from `InferredType`, returning error if struct.
fn expect_scalar(
    inferred: &InferredType,
    src: &NamedSource<Arc<String>>,
    span: graphcal_syntax::span::Span,
) -> Result<Dimension, GraphcalError> {
    match inferred {
        InferredType::Scalar(d) => Ok(*d),
        other => Err(GraphcalError::DimensionMismatch {
            expected: "scalar dimension".to_string(),
            found: format_inferred_type(other),
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
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_) => Ok(InferredType::Scalar(Dimension::DIMENSIONLESS)),
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
            let lhs_type =
                infer_type(lhs, declared_types, local_types, registry, builtin_fns, src)?;
            let rhs_type =
                infer_type(rhs, declared_types, local_types, registry, builtin_fns, src)?;

            match op {
                // Logical operators: require Bool operands, return Bool
                BinOp::And | BinOp::Or => {
                    if lhs_type != InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Bool".to_string(),
                            found: format_inferred_type(&lhs_type),
                            src: src.clone(),
                            span: lhs.span.into(),
                            help: "boolean operators require Bool operands".to_string(),
                        });
                    }
                    if rhs_type != InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Bool".to_string(),
                            found: format_inferred_type(&rhs_type),
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
                                expected: format_inferred_type(&lhs_type),
                                found: format_inferred_type(&rhs_type),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "equality operands must have the same type".to_string(),
                            });
                        }
                    } else {
                        let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
                        let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;
                        if lhs_dim != rhs_dim {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!("{lhs_dim}"),
                                found: format!("{rhs_dim}"),
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
                                expected: format_inferred_type(&lhs_type),
                                found: format_inferred_type(&rhs_type),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "comparison operands must have the same type".to_string(),
                            });
                        }
                        return Ok(InferredType::Bool);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;
                    if lhs_dim != rhs_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format!("{lhs_dim}"),
                            found: format!("{rhs_dim}"),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "comparison operands must have the same dimension".to_string(),
                        });
                    }
                    Ok(InferredType::Bool)
                }
                // Arithmetic operators: require matching numeric operands (Int or Scalar)
                BinOp::Add | BinOp::Sub => {
                    if lhs_type == InferredType::Int && rhs_type == InferredType::Int {
                        return Ok(InferredType::Int);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;
                    if lhs_dim != rhs_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format!("{lhs_dim}"),
                            found: format!("{rhs_dim}"),
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
                    let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;
                    Ok(InferredType::Scalar(lhs_dim * rhs_dim))
                }
                BinOp::Div => {
                    if lhs_type == InferredType::Int && rhs_type == InferredType::Int {
                        return Ok(InferredType::Int);
                    }
                    let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;
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
                            format_inferred_type(&lhs_type),
                            format_inferred_type(&rhs_type)
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
                    let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;
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
                            Ok(InferredType::Scalar(Dimension::DIMENSIONLESS))
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
                src,
            )?;
            match op {
                graphcal_syntax::ast::UnaryOp::Not => {
                    if operand_type != InferredType::Bool {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Bool".to_string(),
                            found: format_inferred_type(&operand_type),
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
                    src,
                )?;
                if let InferredType::Indexed { element, .. } = arg_type {
                    return Ok(if name.value.as_str() == "count" {
                        InferredType::Scalar(Dimension::DIMENSIONLESS)
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
                    src,
                )?;
                if arg_type != InferredType::Int {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format_inferred_type(&arg_type),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: "to_float() requires an Int argument".to_string(),
                    });
                }
                return Ok(InferredType::Scalar(Dimension::DIMENSIONLESS));
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
                    src,
                )?;
                let arg_dim = expect_scalar(&arg_type, src, args[0].span)?;
                if !arg_dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: format!("{arg_dim}"),
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
                        let t =
                            infer_type(a, declared_types, local_types, registry, builtin_fns, src)?;
                        expect_scalar(&t, src, a.span)
                    })
                    .collect::<Result<_, _>>()?;
                return infer_fn_dim(func.dim_sig, &arg_dims, args, src).map(InferredType::Scalar);
            }

            // Try user-defined function
            let fn_def = registry.get_function(name.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownFunction {
                    name: name.value.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                }
            })?;

            // Arity check
            if args.len() != fn_def.params.len() {
                return Err(GraphcalError::WrongArity {
                    name: name.value.clone(),
                    expected: fn_def.params.len(),
                    got: args.len(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }

            // Infer arg types
            let arg_types: Vec<InferredType> = args
                .iter()
                .map(|a| infer_type(a, declared_types, local_types, registry, builtin_fns, src))
                .collect::<Result<_, _>>()?;

            if fn_def.generic_params.is_empty() {
                // Non-generic: resolve each param type and check
                for (i, param) in fn_def.params.iter().enumerate() {
                    let expected = resolve_type_annotation(&param.type_expr, registry, src)?;
                    let expected_inferred = declared_to_inferred(&expected);
                    if arg_types[i] != expected_inferred {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format_inferred_type(&expected_inferred),
                            found: format_inferred_type(&arg_types[i]),
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
                let ret = resolve_type_annotation(&fn_def.return_type_expr, registry, src)?;
                Ok(declared_to_inferred(&ret))
            } else {
                // Generic: unify generic params from arg types
                let dim_param_names: Vec<GenericParamName> = fn_def
                    .generic_params
                    .iter()
                    .filter(|g| g.constraint == crate::registry::FnGenericConstraint::Dim)
                    .map(|g| g.name.clone())
                    .collect();
                let index_param_names: Vec<GenericParamName> = fn_def
                    .generic_params
                    .iter()
                    .filter(|g| g.constraint == crate::registry::FnGenericConstraint::Index)
                    .map(|g| g.name.clone())
                    .collect();
                let mut dim_sub: HashMap<GenericParamName, Dimension> = HashMap::new();
                let mut index_sub: HashMap<GenericParamName, IndexName> = HashMap::new();
                for (i, param) in fn_def.params.iter().enumerate() {
                    unify_type_expr_generic(
                        &param.type_expr,
                        &arg_types[i],
                        &dim_param_names,
                        &index_param_names,
                        &mut dim_sub,
                        &mut index_sub,
                        registry,
                        src,
                        args[i].span,
                    )?;
                }
                // Resolve return type with substitution
                let ret_type = resolve_type_with_substitution(
                    &fn_def.return_type_expr,
                    &dim_sub,
                    &index_sub,
                    registry,
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
                src,
            )?;
            if cond_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&cond_type),
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
                src,
            )?;
            let else_type = infer_type(
                else_branch,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                src,
            )?;

            if then_type != else_type {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&then_type),
                    found: format_inferred_type(&else_type),
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
                src,
            )?;
            let expr_dim = expect_scalar(&inner_type, src, inner.span)?;
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
                    target: format!("{target_dim}"),
                    expr_dim: format!("{expr_dim}"),
                    src: src.clone(),
                    span: target.span.into(),
                });
            }

            Ok(InferredType::Scalar(expr_dim))
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
                    src,
                )?;

                // If type annotation provided, check it matches
                if let Some(type_ann) = &binding.type_ann {
                    let ann_type = resolve_type_annotation(type_ann, registry, src)?;
                    let ann_inferred = declared_to_inferred(&ann_type);
                    if ann_inferred != rhs_type {
                        return Err(GraphcalError::DimensionMismatchInAnnotation {
                            declared: format_inferred_type(&ann_inferred),
                            inferred: format_inferred_type(&rhs_type),
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
                src,
            )?;
            match &inner_type {
                InferredType::Struct(type_name) => {
                    let struct_def = registry.get_struct(type_name.as_str()).ok_or_else(|| {
                        GraphcalError::UnknownStructType {
                            name: type_name.clone(),
                            src: src.clone(),
                            span: inner.span.into(),
                        }
                    })?;
                    let field_def = struct_def
                        .fields
                        .iter()
                        .find(|f| f.name.as_str() == field.value.as_str())
                        .ok_or_else(|| GraphcalError::UnknownField {
                            type_name: type_name.clone(),
                            field_name: field.value.clone(),
                            src: src.clone(),
                            span: field.span.into(),
                        })?;
                    Ok(InferredType::Scalar(field_def.dimension))
                }
                _ => Err(GraphcalError::NotAStruct {
                    name: format_inferred_type(&inner_type),
                    src: src.clone(),
                    span: inner.span.into(),
                }),
            }
        }

        ExprKind::StructConstruction { type_name, fields } => {
            let struct_def = registry
                .get_struct(type_name.value.as_str())
                .ok_or_else(|| GraphcalError::UnknownStructType {
                    name: type_name.value.clone(),
                    src: src.clone(),
                    span: type_name.span.into(),
                })?;

            // Check for extra fields
            let def_field_names: std::collections::HashSet<&str> =
                struct_def.fields.iter().map(|f| f.name.as_str()).collect();
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
            let missing: Vec<FieldName> = struct_def
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
                let field_def = struct_def
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

                let value_dim = expect_scalar(&value_type, src, field_init.name.span)?;
                if value_dim != field_def.dimension {
                    return Err(GraphcalError::FieldDimensionMismatch {
                        type_name: type_name.value.clone(),
                        field_name: field_init.name.value.clone(),
                        expected: format!("{}", field_def.dimension),
                        found: format!("{value_dim}"),
                        src: src.clone(),
                        span: field_init.name.span.into(),
                    });
                }
            }

            Ok(InferredType::Struct(type_name.value.clone()))
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
            let declared_variants: std::collections::HashSet<&str> =
                idx_def.variants.iter().map(VariantName::as_str).collect();
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
                src,
            )?;
            for entry in &entries[1..] {
                let entry_type = infer_type(
                    &entry.value,
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    src,
                )?;
                if entry_type != first_type {
                    return Err(GraphcalError::DimensionMismatchInAnnotation {
                        declared: format_inferred_type(&first_type),
                        inferred: format_inferred_type(&entry_type),
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
                            .variants
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
                        let InferredType::LoopVar(var_idx) = var_type else {
                            return Err(GraphcalError::EvalError {
                                message: format!("`{}` is not a loop variable", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        };
                        if *var_idx != idx_name {
                            return Err(GraphcalError::IndexMismatch {
                                expected: idx_name,
                                found: var_idx.clone(),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
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
                src,
            )?;
            // init and element must have the same type
            if init_type != *element {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&element),
                    found: format_inferred_type(&init_type),
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
                src,
            )?;
            if body_type != *element {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&element),
                    found: format_inferred_type(&body_type),
                    src: src.clone(),
                    span: body.span.into(),
                    help: "scan body must return the same type as the accumulator".to_string(),
                });
            }
            // scan produces an indexed result with the same index
            Ok(InferredType::Indexed { element, index })
        }
    }
}

/// Unify a parameter's type expression against an actual inferred type,
/// binding generic dimension and index names.
///
/// For example, if `type_expr` is `D` and `actual` is `Scalar(Length)`, binds `D = Length`.
/// If `type_expr` is `D[I]` and `actual` is `Indexed { Scalar(Velocity), "Maneuver" }`,
/// binds `D = Velocity_dim` and `I = "Maneuver"`.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "complex generic unification requires many parameters and match arms"
)]
fn unify_type_expr_generic(
    type_expr: &graphcal_syntax::ast::TypeExpr,
    actual: &InferredType,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    dim_sub: &mut HashMap<GenericParamName, Dimension>,
    index_sub: &mut HashMap<GenericParamName, IndexName>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: graphcal_syntax::span::Span,
) -> Result<(), GraphcalError> {
    match &type_expr.kind {
        TypeExprKind::Indexed { base, indexes } => {
            // Peel off index layers from actual type, binding index generics
            let mut current = actual;
            for idx in indexes.iter().rev() {
                let idx_name = &idx.name;
                let InferredType::Indexed {
                    element,
                    index: actual_idx,
                } = current
                else {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format!("indexed type with index `{idx_name}`"),
                        found: format_inferred_type(current),
                        src: src.clone(),
                        span: span.into(),
                        help: "expected an indexed value".to_string(),
                    });
                };
                // If this is a generic index param, bind it
                if let Some(generic_param) = index_params.iter().find(|p| p.as_str() == idx_name) {
                    if let Some(prev) = index_sub.get(generic_param) {
                        if *prev != *actual_idx {
                            return Err(GraphcalError::IndexMismatch {
                                expected: prev.clone(),
                                found: actual_idx.clone(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    } else {
                        index_sub.insert(generic_param.clone(), actual_idx.clone());
                    }
                } else if *idx_name != actual_idx.as_str() {
                    // Concrete index name — must match exactly
                    return Err(GraphcalError::IndexMismatch {
                        expected: IndexName::new(idx_name),
                        found: actual_idx.clone(),
                        src: src.clone(),
                        span: span.into(),
                    });
                } else {
                    // Concrete index name matches — OK
                }
                current = element;
            }
            // Now unify the base type against the peeled element
            unify_type_expr_generic(
                base,
                current,
                dim_params,
                index_params,
                dim_sub,
                index_sub,
                registry,
                src,
                span,
            )
        }
        TypeExprKind::Bool => {
            if *actual != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(actual),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Bool argument".to_string(),
                });
            }
            Ok(())
        }
        TypeExprKind::Int => {
            if *actual != InferredType::Int {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: format_inferred_type(actual),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Int argument".to_string(),
                });
            }
            Ok(())
        }
        TypeExprKind::Dimensionless => {
            let actual_dim = expect_scalar(actual, src, span)?;
            if !actual_dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: format!("{actual_dim}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Dimensionless argument".to_string(),
                });
            }
            Ok(())
        }
        TypeExprKind::DimExpr(dim_expr) => {
            let actual_dim = expect_scalar(actual, src, span)?;

            // Check if this is a single generic param (most common case: `D` or `D^n`)
            if dim_expr.terms.len() == 1 {
                let item = &dim_expr.terms[0];
                let name = &item.term.name.name;
                if let Some(generic_param) = dim_params.iter().find(|p| p.as_str() == name) {
                    let exp = item.term.power.unwrap_or(1);
                    let bound_dim = if exp == 1 {
                        actual_dim
                    } else {
                        actual_dim.pow(Rational::new(1, exp))
                    };
                    if let Some(prev) = dim_sub.get(generic_param) {
                        if *prev != bound_dim {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!("{prev}"),
                                found: format!("{bound_dim}"),
                                src: src.clone(),
                                span: span.into(),
                                help: format!(
                                    "generic `{name}` was bound to {prev} but this argument requires {bound_dim}"
                                ),
                            });
                        }
                    } else {
                        dim_sub.insert(generic_param.clone(), bound_dim);
                    }
                    return Ok(());
                }
            }

            // General case: resolve terms
            let mut expected_dim = Dimension::DIMENSIONLESS;
            for item in &dim_expr.terms {
                let name = &item.term.name.name;
                let exp = item.term.power.unwrap_or(1);

                let term_dim =
                    if let Some(generic_param) = dim_params.iter().find(|p| p.as_str() == name) {
                        if let Some(prev) = dim_sub.get(generic_param) {
                            prev.pow(Rational::from_int(exp))
                        } else {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!("generic `{name}` (unresolved)"),
                                found: format!("{actual_dim}"),
                                src: src.clone(),
                                span: span.into(),
                                help: format!(
                                    "generic `{name}` could not be inferred from this argument"
                                ),
                            });
                        }
                    } else {
                        let base = registry.get_dimension(name).ok_or_else(|| {
                            GraphcalError::UnknownDimension {
                                name: DimName::new(name),
                                src: src.clone(),
                                span: item.term.span.into(),
                            }
                        })?;
                        base.pow(Rational::from_int(exp))
                    };

                expected_dim = match item.op {
                    MulDivOp::Mul => expected_dim * term_dim,
                    MulDivOp::Div => expected_dim / term_dim,
                };
            }

            if expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("{expected_dim}"),
                    found: format!("{actual_dim}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "dimension mismatch in function argument".to_string(),
                });
            }
            Ok(())
        }
    }
}

/// Resolve a type expression to an `InferredType`, substituting generic dim and index names.
fn resolve_type_with_substitution(
    type_expr: &graphcal_syntax::ast::TypeExpr,
    dim_sub: &HashMap<GenericParamName, Dimension>,
    index_sub: &HashMap<GenericParamName, IndexName>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &type_expr.kind {
        TypeExprKind::Dimensionless => Ok(InferredType::Scalar(Dimension::DIMENSIONLESS)),
        TypeExprKind::Bool => Ok(InferredType::Bool),
        TypeExprKind::Int => Ok(InferredType::Int),
        TypeExprKind::DimExpr(dim_expr) => {
            let mut result = Dimension::DIMENSIONLESS;
            for item in &dim_expr.terms {
                let name = &item.term.name.name;
                let exp = item.term.power.unwrap_or(1);

                let base = if let Some(dim) = dim_sub.get(name.as_str()) {
                    *dim
                } else if let Some(dim) = registry.get_dimension(name) {
                    *dim
                } else {
                    return Err(GraphcalError::UnknownDimension {
                        name: DimName::new(name),
                        src: src.clone(),
                        span: item.term.span.into(),
                    });
                };

                let term_dim = base.pow(Rational::from_int(exp));
                result = match item.op {
                    MulDivOp::Mul => result * term_dim,
                    MulDivOp::Div => result / term_dim,
                };
            }
            Ok(InferredType::Scalar(result))
        }
        TypeExprKind::Indexed { base, indexes } => {
            let mut result =
                resolve_type_with_substitution(base, dim_sub, index_sub, registry, src)?;
            for idx in indexes.iter().rev() {
                let idx_name = &idx.name;
                // Look up in index substitution, then use as-is
                let resolved_idx = index_sub
                    .get(idx_name.as_str())
                    .cloned()
                    .unwrap_or_else(|| IndexName::new(idx_name));
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: resolved_idx,
                };
            }
            Ok(result)
        }
    }
}

/// Infer the result dimension of a built-in function call given its `DimSignature`.
fn infer_fn_dim(
    sig: DimSignature,
    arg_dims: &[Dimension],
    args: &[Expr],
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    use graphcal_syntax::dimension::BaseDim;

    match sig {
        DimSignature::AllDimensionless => {
            for (dim, arg) in arg_dims.iter().zip(args) {
                if !dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: format!("{dim}"),
                        src: src.clone(),
                        span: arg.span.into(),
                        help: "this function requires Dimensionless arguments".to_string(),
                    });
                }
            }
            Ok(Dimension::DIMENSIONLESS)
        }
        DimSignature::AngleToDimensionless => {
            let angle = Dimension::base(BaseDim::Angle);
            if arg_dims[0] != angle {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Angle".to_string(),
                    found: format!("{}", arg_dims[0]),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "trigonometric functions require an Angle argument".to_string(),
                });
            }
            Ok(Dimension::DIMENSIONLESS)
        }
        DimSignature::DimensionlessToAngle => {
            if !arg_dims[0].is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: format!("{}", arg_dims[0]),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "inverse trigonometric functions require a Dimensionless argument"
                        .to_string(),
                });
            }
            Ok(Dimension::base(BaseDim::Angle))
        }
        DimSignature::Sqrt => {
            // Result dimension is arg^(1/2)
            Ok(arg_dims[0].pow(Rational::new(1, 2)))
        }
        DimSignature::Passthrough => Ok(arg_dims[0]),
        DimSignature::SameDimension => {
            if arg_dims[0] != arg_dims[1] {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("{}", arg_dims[0]),
                    found: format!("{}", arg_dims[1]),
                    src: src.clone(),
                    span: args[1].span.into(),
                    help: "both arguments must have the same dimension".to_string(),
                });
            }
            Ok(arg_dims[0])
        }
        DimSignature::SameDimensionToAngle => {
            if arg_dims[0] != arg_dims[1] {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("{}", arg_dims[0]),
                    found: format!("{}", arg_dims[1]),
                    src: src.clone(),
                    span: args[1].span.into(),
                    help: "both arguments must have the same dimension".to_string(),
                });
            }
            Ok(Dimension::base(BaseDim::Angle))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use crate::prelude::load_prelude;
    use graphcal_syntax::parser::Parser;

    fn make_registry() -> Registry {
        let mut r = Registry::new();
        load_prelude(&mut r);
        r
    }

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn check(source: &str) -> Result<HashMap<String, DeclaredType>, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        let mut registry = make_registry();
        let src = make_src(source);

        // Register indexes, struct types, and user-defined functions (mirrors eval.rs pipeline)
        for decl in &file.declarations {
            if let DeclKind::Index(idx) = &decl.kind {
                registry.register_index(crate::registry::IndexDef {
                    name: idx.name.value.clone(),
                    variants: idx.variants.iter().map(|v| v.value.clone()).collect(),
                });
            }
        }
        for decl in &file.declarations {
            if let DeclKind::Type(t) = &decl.kind {
                let fields = t
                    .fields
                    .iter()
                    .map(|f| {
                        let dim = registry.resolve_type_expr(&f.type_ann).unwrap();
                        crate::registry::StructField {
                            name: f.name.value.clone(),
                            dimension: dim,
                        }
                    })
                    .collect();
                registry.register_struct(crate::registry::StructDef {
                    name: t.name.value.clone(),
                    fields,
                });
            }
        }
        for decl in &file.declarations {
            if let DeclKind::Fn(fn_decl) = &decl.kind {
                registry.register_function(crate::registry::FnDef {
                    name: fn_decl.name.value.clone(),
                    generic_params: fn_decl
                        .generic_params
                        .iter()
                        .map(|g| crate::registry::FnGenericParam {
                            name: g.name.value.clone(),
                            constraint: match g.constraint {
                                graphcal_syntax::ast::GenericConstraint::Dim => {
                                    crate::registry::FnGenericConstraint::Dim
                                }
                                graphcal_syntax::ast::GenericConstraint::Index => {
                                    crate::registry::FnGenericConstraint::Index
                                }
                            },
                        })
                        .collect(),
                    params: fn_decl
                        .params
                        .iter()
                        .map(|p| crate::registry::FnParamDef {
                            name: p.name.name.clone(),
                            type_expr: p.type_ann.clone(),
                        })
                        .collect(),
                    return_type_expr: fn_decl.return_type.clone(),
                    body: fn_decl.body.clone(),
                    span: decl.span,
                });
            }
        }

        check_dimensions(&file, &registry, &src)
    }

    #[test]
    fn check_dimensionless_const() {
        let types = check("const G0: Dimensionless = 9.80665;").unwrap();
        assert_eq!(types["G0"], DeclaredType::Scalar(Dimension::DIMENSIONLESS));
    }

    #[test]
    fn check_dimensionless_arithmetic() {
        let types =
            check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
        assert_eq!(types["y"], DeclaredType::Scalar(Dimension::DIMENSIONLESS));
    }

    #[test]
    fn check_length_unit_literal() {
        let types = check("param alt: Length = 400.0 km;").unwrap();
        let length = Dimension::base(graphcal_syntax::dimension::BaseDim::Length);
        assert_eq!(types["alt"], DeclaredType::Scalar(length));
    }

    #[test]
    fn check_velocity_from_division() {
        let source = "param dist: Length = 100.0 km;\nparam time: Time = 2.0 hour;\nnode speed: Velocity = @dist / @time;";
        let types = check(source).unwrap();
        let velocity = Dimension::base(graphcal_syntax::dimension::BaseDim::Length)
            / Dimension::base(graphcal_syntax::dimension::BaseDim::Time);
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
        let velocity = Dimension::base(graphcal_syntax::dimension::BaseDim::Length)
            / Dimension::base(graphcal_syntax::dimension::BaseDim::Time);
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
        let velocity = Dimension::base(graphcal_syntax::dimension::BaseDim::Length)
            / Dimension::base(graphcal_syntax::dimension::BaseDim::Time);
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
