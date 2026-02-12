use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::{BinOp, DeclKind, Expr, ExprKind, File, MulDivOp, TypeExprKind};
use kasuri_syntax::dimension::{Dimension, Rational};

use crate::builtins::{DimSignature, builtin_constants, builtin_functions};
use crate::error::KasuriError;
use crate::registry::Registry;

/// The declared type of a const/param/node: either a scalar with a dimension, or a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredType {
    Scalar(Dimension),
    Struct(String),
}

/// The inferred type of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    Scalar(Dimension),
    Struct(String),
}

/// Check dimensions for all declarations in a file.
///
/// For each const/param/node, infers the dimension of the RHS expression
/// and verifies it matches the declared type annotation.
///
/// # Errors
///
/// Returns a [`KasuriError`] if dimensions are inconsistent.
pub fn check_dimensions(
    file: &File,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<String, DeclaredType>, KasuriError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    // Collect declared types for all consts/params/nodes
    let mut declared_types: HashMap<String, DeclaredType> = HashMap::new();

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
            DeclKind::Dimension(_) | DeclKind::Unit(_) | DeclKind::Type(_) | DeclKind::Fn(_) => {}
            DeclKind::Const(c) => {
                let dt = resolve_type_annotation(&c.type_ann, registry, src)?;
                declared_types.insert(c.name.name.clone(), dt);
            }
            DeclKind::Param(p) => {
                let dt = resolve_type_annotation(&p.type_ann, registry, src)?;
                declared_types.insert(p.name.name.clone(), dt);
            }
            DeclKind::Node(n) => {
                let dt = resolve_type_annotation(&n.type_ann, registry, src)?;
                declared_types.insert(n.name.name.clone(), dt);
            }
        }
    }

    // Second pass: infer types and check against annotations
    let empty_locals: HashMap<String, InferredType> = HashMap::new();
    for decl in &file.declarations {
        let (name, type_ann, value_expr) = match &decl.kind {
            DeclKind::Dimension(_) | DeclKind::Unit(_) | DeclKind::Type(_) | DeclKind::Fn(_) => {
                continue;
            }
            DeclKind::Const(c) => (&c.name.name, &c.type_ann, &c.value),
            DeclKind::Param(p) => (&p.name.name, &p.type_ann, &p.value),
            DeclKind::Node(n) => (&n.name.name, &n.type_ann, &n.value),
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

        match (declared, &inferred) {
            (DeclaredType::Scalar(d), InferredType::Scalar(i)) => {
                if d != i {
                    return Err(KasuriError::DimensionMismatchInAnnotation {
                        declared: format!("{d}"),
                        inferred: format!("{i}"),
                        src: src.clone(),
                        span: type_ann.span.into(),
                    });
                }
            }
            (DeclaredType::Struct(d_name), InferredType::Struct(i_name)) => {
                if d_name != i_name {
                    return Err(KasuriError::DimensionMismatchInAnnotation {
                        declared: d_name.clone(),
                        inferred: i_name.clone(),
                        src: src.clone(),
                        span: type_ann.span.into(),
                    });
                }
            }
            (DeclaredType::Scalar(d), InferredType::Struct(i_name)) => {
                return Err(KasuriError::DimensionMismatchInAnnotation {
                    declared: format!("{d}"),
                    inferred: i_name.clone(),
                    src: src.clone(),
                    span: type_ann.span.into(),
                });
            }
            (DeclaredType::Struct(d_name), InferredType::Scalar(i)) => {
                return Err(KasuriError::DimensionMismatchInAnnotation {
                    declared: d_name.clone(),
                    inferred: format!("{i}"),
                    src: src.clone(),
                    span: type_ann.span.into(),
                });
            }
        }
    }

    Ok(declared_types)
}

/// Resolve a type annotation to a `DeclaredType`.
///
/// Checks the struct registry first (for single-term `DimExpr` that match a struct name),
/// then falls back to dimension resolution.
fn resolve_type_annotation(
    type_ann: &kasuri_syntax::ast::TypeExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclaredType, KasuriError> {
    // Check if this is a single-term DimExpr that matches a struct name
    if let kasuri_syntax::ast::TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
    {
        let name = &dim_expr.terms[0].term.name.name;
        if registry.get_struct(name).is_some() {
            return Ok(DeclaredType::Struct(name.clone()));
        }
    }

    // Fall back to dimension resolution
    let dim = registry
        .resolve_type_expr(type_ann)
        .ok_or_else(|| unknown_dim_in_type(type_ann, src))?;
    Ok(DeclaredType::Scalar(dim))
}

/// Produce a helpful error when a type annotation references an unknown dimension.
fn unknown_dim_in_type(
    type_ann: &kasuri_syntax::ast::TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> KasuriError {
    // Try to find the first unknown dimension name in the type expression
    if let kasuri_syntax::ast::TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
        && let Some(item) = dim_expr.terms.first()
    {
        return KasuriError::UnknownDimension {
            name: item.term.name.name.clone(),
            src: src.clone(),
            span: item.term.span.into(),
        };
    }
    KasuriError::UnknownDimension {
        name: "unknown".to_string(),
        src: src.clone(),
        span: type_ann.span.into(),
    }
}

/// Helper: extract scalar dimension from `InferredType`, returning error if struct.
fn expect_scalar(
    inferred: &InferredType,
    src: &NamedSource<Arc<String>>,
    span: kasuri_syntax::span::Span,
) -> Result<Dimension, KasuriError> {
    match inferred {
        InferredType::Scalar(d) => Ok(*d),
        InferredType::Struct(name) => Err(KasuriError::NotAStruct {
            name: name.clone(),
            src: src.clone(),
            span: span.into(),
        }),
    }
}

/// Infer the type (dimension or struct) of an expression.
#[expect(clippy::too_many_lines)]
fn infer_type(
    expr: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, KasuriError> {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Bool(_) => {
            Ok(InferredType::Scalar(Dimension::DIMENSIONLESS))
        }

        ExprKind::UnitLiteral { unit, .. } => {
            let (dim, _scale) = registry.resolve_unit_expr(unit).ok_or_else(|| {
                for item in &unit.terms {
                    if registry.get_unit(&item.name.name).is_none() {
                        return KasuriError::UnknownUnit {
                            name: item.name.name.clone(),
                            src: src.clone(),
                            span: item.name.span.into(),
                        };
                    }
                }
                KasuriError::UnknownUnit {
                    name: "unknown".to_string(),
                    src: src.clone(),
                    span: unit.span.into(),
                }
            })?;
            Ok(InferredType::Scalar(dim))
        }

        ExprKind::ConstRef(ident) => {
            let dt =
                declared_types
                    .get(&ident.name)
                    .ok_or_else(|| KasuriError::UnknownConstRef {
                        name: ident.name.clone(),
                        src: src.clone(),
                        span: ident.span.into(),
                    })?;
            Ok(match dt {
                DeclaredType::Scalar(d) => InferredType::Scalar(*d),
                DeclaredType::Struct(n) => InferredType::Struct(n.clone()),
            })
        }

        ExprKind::GraphRef(ident) => {
            let dt =
                declared_types
                    .get(&ident.name)
                    .ok_or_else(|| KasuriError::UnknownGraphRef {
                        name: ident.name.clone(),
                        src: src.clone(),
                        span: ident.span.into(),
                    })?;
            Ok(match dt {
                DeclaredType::Scalar(d) => InferredType::Scalar(*d),
                DeclaredType::Struct(n) => InferredType::Struct(n.clone()),
            })
        }

        ExprKind::LocalRef(ident) => {
            local_types
                .get(&ident.name)
                .cloned()
                .ok_or_else(|| KasuriError::UnknownLocalRef {
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
            let lhs_dim = expect_scalar(&lhs_type, src, lhs.span)?;
            let rhs_dim = expect_scalar(&rhs_type, src, rhs.span)?;

            match op {
                BinOp::Add | BinOp::Sub => {
                    if lhs_dim != rhs_dim {
                        return Err(KasuriError::DimensionMismatch {
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
                BinOp::Mul => Ok(InferredType::Scalar(lhs_dim * rhs_dim)),
                BinOp::Div => Ok(InferredType::Scalar(lhs_dim / rhs_dim)),
                BinOp::Pow => {
                    if let ExprKind::Number(n) = &rhs.kind {
                        if n.fract() == 0.0 {
                            #[expect(clippy::cast_possible_truncation)]
                            let exp = *n as i32;
                            Ok(InferredType::Scalar(lhs_dim.pow(Rational::from_int(exp))))
                        } else {
                            #[expect(clippy::float_cmp)]
                            if *n == 0.5 {
                                Ok(InferredType::Scalar(lhs_dim.pow(Rational::new(1, 2))))
                            } else {
                                Err(KasuriError::NonLiteralExponent {
                                    src: src.clone(),
                                    span: rhs.span.into(),
                                })
                            }
                        }
                    } else if rhs_dim.is_dimensionless() {
                        if lhs_dim.is_dimensionless() {
                            Ok(InferredType::Scalar(Dimension::DIMENSIONLESS))
                        } else {
                            Err(KasuriError::NonLiteralExponent {
                                src: src.clone(),
                                span: rhs.span.into(),
                            })
                        }
                    } else {
                        Err(KasuriError::NonLiteralExponent {
                            src: src.clone(),
                            span: rhs.span.into(),
                        })
                    }
                }
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    if lhs_dim != rhs_dim {
                        return Err(KasuriError::DimensionMismatch {
                            expected: format!("{lhs_dim}"),
                            found: format!("{rhs_dim}"),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "comparison operands must have the same dimension".to_string(),
                        });
                    }
                    Ok(InferredType::Scalar(Dimension::DIMENSIONLESS))
                }
                BinOp::And | BinOp::Or => {
                    if !lhs_dim.is_dimensionless() {
                        return Err(KasuriError::DimensionMismatch {
                            expected: "Dimensionless".to_string(),
                            found: format!("{lhs_dim}"),
                            src: src.clone(),
                            span: lhs.span.into(),
                            help: "boolean operators require Dimensionless operands".to_string(),
                        });
                    }
                    if !rhs_dim.is_dimensionless() {
                        return Err(KasuriError::DimensionMismatch {
                            expected: "Dimensionless".to_string(),
                            found: format!("{rhs_dim}"),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "boolean operators require Dimensionless operands".to_string(),
                        });
                    }
                    Ok(InferredType::Scalar(Dimension::DIMENSIONLESS))
                }
            }
        }

        ExprKind::UnaryOp { operand, .. } => infer_type(
            operand,
            declared_types,
            local_types,
            registry,
            builtin_fns,
            src,
        ),

        ExprKind::FnCall { name, args } => {
            // Try builtin first
            if let Some(func) = builtin_fns.get(name.name.as_str()) {
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
            let fn_def =
                registry
                    .get_function(&name.name)
                    .ok_or_else(|| KasuriError::UnknownFunction {
                        name: name.name.clone(),
                        src: src.clone(),
                        span: name.span.into(),
                    })?;

            // Arity check
            if args.len() != fn_def.params.len() {
                return Err(KasuriError::WrongArity {
                    name: name.name.clone(),
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
                    let expected_inferred = match &expected {
                        DeclaredType::Scalar(d) => InferredType::Scalar(*d),
                        DeclaredType::Struct(n) => InferredType::Struct(n.clone()),
                    };
                    if arg_types[i] != expected_inferred {
                        let expected_str = match &expected_inferred {
                            InferredType::Scalar(d) => format!("{d}"),
                            InferredType::Struct(n) => n.clone(),
                        };
                        let actual_str = match &arg_types[i] {
                            InferredType::Scalar(d) => format!("{d}"),
                            InferredType::Struct(n) => n.clone(),
                        };
                        return Err(KasuriError::DimensionMismatch {
                            expected: expected_str,
                            found: actual_str,
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
                Ok(match ret {
                    DeclaredType::Scalar(d) => InferredType::Scalar(d),
                    DeclaredType::Struct(n) => InferredType::Struct(n),
                })
            } else {
                // Generic: unify generic params from arg types
                let mut substitution: HashMap<String, Dimension> = HashMap::new();
                for (i, param) in fn_def.params.iter().enumerate() {
                    let actual_dim = expect_scalar(&arg_types[i], src, args[i].span)?;
                    unify_type_expr(
                        &param.type_expr,
                        actual_dim,
                        &fn_def.generic_params,
                        &mut substitution,
                        registry,
                        src,
                        args[i].span,
                    )?;
                }
                // Resolve return type with substitution
                let ret_dim = resolve_type_with_substitution(
                    &fn_def.return_type_expr,
                    &substitution,
                    registry,
                    src,
                )?;
                Ok(InferredType::Scalar(ret_dim))
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
            let cond_dim = expect_scalar(&cond_type, src, condition.span)?;
            if !cond_dim.is_dimensionless() {
                return Err(KasuriError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: format!("{cond_dim}"),
                    src: src.clone(),
                    span: condition.span.into(),
                    help: "if/else condition must be Dimensionless".to_string(),
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
                let then_str = match &then_type {
                    InferredType::Scalar(d) => format!("{d}"),
                    InferredType::Struct(n) => n.clone(),
                };
                let else_str = match &else_type {
                    InferredType::Scalar(d) => format!("{d}"),
                    InferredType::Struct(n) => n.clone(),
                };
                return Err(KasuriError::DimensionMismatch {
                    expected: then_str,
                    found: else_str,
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
                    if registry.get_unit(&item.name.name).is_none() {
                        return KasuriError::UnknownUnit {
                            name: item.name.name.clone(),
                            src: src.clone(),
                            span: item.name.span.into(),
                        };
                    }
                }
                KasuriError::UnknownUnit {
                    name: "unknown".to_string(),
                    src: src.clone(),
                    span: target.span.into(),
                }
            })?;

            if expr_dim != target_dim {
                return Err(KasuriError::ConversionDimensionMismatch {
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
                    return Err(KasuriError::DuplicateLetBinding {
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
                    let ann_inferred = match &ann_type {
                        DeclaredType::Scalar(d) => InferredType::Scalar(*d),
                        DeclaredType::Struct(n) => InferredType::Struct(n.clone()),
                    };
                    if ann_inferred != rhs_type {
                        let ann_str = match &ann_inferred {
                            InferredType::Scalar(d) => format!("{d}"),
                            InferredType::Struct(n) => n.clone(),
                        };
                        let rhs_str = match &rhs_type {
                            InferredType::Scalar(d) => format!("{d}"),
                            InferredType::Struct(n) => n.clone(),
                        };
                        return Err(KasuriError::DimensionMismatchInAnnotation {
                            declared: ann_str,
                            inferred: rhs_str,
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
                    let struct_def = registry.get_struct(type_name).ok_or_else(|| {
                        KasuriError::UnknownStructType {
                            name: type_name.clone(),
                            src: src.clone(),
                            span: inner.span.into(),
                        }
                    })?;
                    let field_def = struct_def
                        .fields
                        .iter()
                        .find(|f| f.name == field.name)
                        .ok_or_else(|| KasuriError::UnknownField {
                            type_name: type_name.clone(),
                            field_name: field.name.clone(),
                            src: src.clone(),
                            span: field.span.into(),
                        })?;
                    Ok(InferredType::Scalar(field_def.dimension))
                }
                InferredType::Scalar(_) => Err(KasuriError::NotAStruct {
                    name: format!("{inner_type:?}"),
                    src: src.clone(),
                    span: inner.span.into(),
                }),
            }
        }

        ExprKind::StructConstruction { type_name, fields } => {
            let struct_def = registry.get_struct(&type_name.name).ok_or_else(|| {
                KasuriError::UnknownStructType {
                    name: type_name.name.clone(),
                    src: src.clone(),
                    span: type_name.span.into(),
                }
            })?;

            // Check for extra fields
            let def_field_names: std::collections::HashSet<&str> =
                struct_def.fields.iter().map(|f| f.name.as_str()).collect();
            let provided_names: Vec<&str> = fields.iter().map(|f| f.name.name.as_str()).collect();
            let extra: Vec<String> = provided_names
                .iter()
                .filter(|n| !def_field_names.contains(**n))
                .map(|n| (*n).to_string())
                .collect();
            if !extra.is_empty() {
                return Err(KasuriError::ExtraFields {
                    type_name: type_name.name.clone(),
                    extra,
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }

            // Check for missing fields
            let provided_set: std::collections::HashSet<&str> =
                provided_names.iter().copied().collect();
            let missing: Vec<String> = struct_def
                .fields
                .iter()
                .filter(|f| !provided_set.contains(f.name.as_str()))
                .map(|f| f.name.clone())
                .collect();
            if !missing.is_empty() {
                return Err(KasuriError::MissingFields {
                    type_name: type_name.name.clone(),
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
                    .find(|f| f.name == field_init.name.name)
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
                        .get(&field_init.name.name)
                        .cloned()
                        .ok_or_else(|| KasuriError::UnknownLocalRef {
                            name: field_init.name.name.clone(),
                            src: src.clone(),
                            span: field_init.name.span.into(),
                        })?
                };

                let value_dim = expect_scalar(&value_type, src, field_init.name.span)?;
                if value_dim != field_def.dimension {
                    return Err(KasuriError::FieldDimensionMismatch {
                        type_name: type_name.name.clone(),
                        field_name: field_init.name.name.clone(),
                        expected: format!("{}", field_def.dimension),
                        found: format!("{value_dim}"),
                        src: src.clone(),
                        span: field_init.name.span.into(),
                    });
                }
            }

            Ok(InferredType::Struct(type_name.name.clone()))
        }
    }
}

/// Unify a parameter's type expression against an actual dimension, binding generic names.
///
/// For example, if `type_expr` is `D` and `actual` is `Length`, binds `D = Length`.
/// If `type_expr` is `D^2` and `actual` is `Area` (= Length^2), binds `D = Length`.
/// Non-generic terms are resolved normally and checked for equality.
fn unify_type_expr(
    type_expr: &kasuri_syntax::ast::TypeExpr,
    actual: Dimension,
    generic_params: &[String],
    substitution: &mut HashMap<String, Dimension>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: kasuri_syntax::span::Span,
) -> Result<(), KasuriError> {
    match &type_expr.kind {
        TypeExprKind::Dimensionless => {
            if !actual.is_dimensionless() {
                return Err(KasuriError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: format!("{actual}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Dimensionless argument".to_string(),
                });
            }
            Ok(())
        }
        TypeExprKind::DimExpr(dim_expr) => {
            // Check if this is a single generic param (most common case: `D` or `D^n`)
            if dim_expr.terms.len() == 1 {
                let item = &dim_expr.terms[0];
                let name = &item.term.name.name;
                if generic_params.contains(name) {
                    let exp = item.term.power.unwrap_or(1);
                    // Solve: D^exp = actual => D = actual^(1/exp)
                    let bound_dim = if exp == 1 {
                        actual
                    } else {
                        actual.pow(Rational::new(1, exp))
                    };
                    // Check consistency with previous binding
                    if let Some(prev) = substitution.get(name) {
                        if *prev != bound_dim {
                            return Err(KasuriError::DimensionMismatch {
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
                        substitution.insert(name.clone(), bound_dim);
                    }
                    return Ok(());
                }
            }

            // General case: compute the "expected" dimension by resolving non-generic
            // terms concretely and generic terms via substitution/binding.
            // Walk terms, accumulating into expected_dim.
            let mut expected_dim = Dimension::DIMENSIONLESS;
            for item in &dim_expr.terms {
                let name = &item.term.name.name;
                let exp = item.term.power.unwrap_or(1);

                let term_dim = if generic_params.contains(name) {
                    // Generic term: try to bind or check consistency
                    if let Some(prev) = substitution.get(name) {
                        prev.pow(Rational::from_int(exp))
                    } else {
                        // Cannot determine generic from compound expression unless
                        // we solve a system of equations. For Phase 3, we require
                        // that each generic param appears as a standalone param first
                        // so it's already bound. If not bound, error.
                        return Err(KasuriError::DimensionMismatch {
                            expected: format!("generic `{name}` (unresolved)"),
                            found: format!("{actual}"),
                            src: src.clone(),
                            span: span.into(),
                            help: format!(
                                "generic `{name}` could not be inferred from this argument"
                            ),
                        });
                    }
                } else {
                    // Concrete dimension name
                    let base = registry.get_dimension(name).ok_or_else(|| {
                        KasuriError::UnknownDimension {
                            name: name.clone(),
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

            if expected_dim != actual {
                return Err(KasuriError::DimensionMismatch {
                    expected: format!("{expected_dim}"),
                    found: format!("{actual}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "dimension mismatch in function argument".to_string(),
                });
            }
            Ok(())
        }
    }
}

/// Resolve a type expression to a concrete dimension, substituting generic names.
fn resolve_type_with_substitution(
    type_expr: &kasuri_syntax::ast::TypeExpr,
    substitution: &HashMap<String, Dimension>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, KasuriError> {
    match &type_expr.kind {
        TypeExprKind::Dimensionless => Ok(Dimension::DIMENSIONLESS),
        TypeExprKind::DimExpr(dim_expr) => {
            let mut result = Dimension::DIMENSIONLESS;
            for item in &dim_expr.terms {
                let name = &item.term.name.name;
                let exp = item.term.power.unwrap_or(1);

                let base = if let Some(dim) = substitution.get(name) {
                    *dim
                } else if let Some(dim) = registry.get_dimension(name) {
                    *dim
                } else {
                    return Err(KasuriError::UnknownDimension {
                        name: name.clone(),
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
) -> Result<Dimension, KasuriError> {
    use kasuri_syntax::dimension::BaseDim;

    match sig {
        DimSignature::AllDimensionless => {
            for (dim, arg) in arg_dims.iter().zip(args) {
                if !dim.is_dimensionless() {
                    return Err(KasuriError::DimensionMismatch {
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
                return Err(KasuriError::DimensionMismatch {
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
                return Err(KasuriError::DimensionMismatch {
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
                return Err(KasuriError::DimensionMismatch {
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
                return Err(KasuriError::DimensionMismatch {
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
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::prelude::load_prelude;
    use kasuri_syntax::parser::Parser;

    fn make_registry() -> Registry {
        let mut r = Registry::new();
        load_prelude(&mut r);
        r
    }

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn check(source: &str) -> Result<HashMap<String, DeclaredType>, KasuriError> {
        let file = Parser::new(source).parse_file().unwrap();
        let mut registry = make_registry();
        let src = make_src(source);

        // Register user-defined functions (mirrors eval.rs pipeline)
        for decl in &file.declarations {
            if let DeclKind::Fn(fn_decl) = &decl.kind {
                registry.register_function(crate::registry::FnDef {
                    name: fn_decl.name.name.clone(),
                    generic_params: fn_decl
                        .generic_params
                        .iter()
                        .map(|g| g.name.name.clone())
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
        let types = check("param alt: Length = 400 km;").unwrap();
        let length = Dimension::base(kasuri_syntax::dimension::BaseDim::Length);
        assert_eq!(types["alt"], DeclaredType::Scalar(length));
    }

    #[test]
    fn check_velocity_from_division() {
        let source = "param dist: Length = 100 km;\nparam time: Time = 2.0 hour;\nnode speed: Velocity = @dist / @time;";
        let types = check(source).unwrap();
        let velocity = Dimension::base(kasuri_syntax::dimension::BaseDim::Length)
            / Dimension::base(kasuri_syntax::dimension::BaseDim::Time);
        assert_eq!(types["speed"], DeclaredType::Scalar(velocity));
    }

    #[test]
    fn check_add_dimension_mismatch() {
        let source = "param x: Length = 1.0 m;\nparam y: Time = 1.0 s;\nnode z: Length = @x + @y;";
        let err = check(source).unwrap_err();
        assert!(matches!(err, KasuriError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_annotation_mismatch() {
        let source = "param x: Length = 1.0 m;\nnode y: Time = @x;";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, KasuriError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_conversion_same_dimension() {
        let source =
            "param speed: Velocity = 100 m / s;\nnode speed_kmh: Velocity = @speed -> km / hour;";
        let types = check(source).unwrap();
        let velocity = Dimension::base(kasuri_syntax::dimension::BaseDim::Length)
            / Dimension::base(kasuri_syntax::dimension::BaseDim::Time);
        assert_eq!(types["speed_kmh"], DeclaredType::Scalar(velocity));
    }

    #[test]
    fn check_conversion_wrong_dimension() {
        let source = "param x: Length = 1.0 m;\nnode y: Length = @x -> s;";
        let err = check(source).unwrap_err();
        assert!(matches!(
            err,
            KasuriError::ConversionDimensionMismatch { .. }
        ));
    }

    #[test]
    fn check_sqrt_dimension() {
        let source = "param area: Area = 100 m;\nnode side: Length = sqrt(@area);";
        // Note: area should be m^2, but we declared it with m (Length).
        // sqrt(Length) = Length^(1/2) which doesn't match Length.
        let err = check(source).unwrap_err();
        assert!(matches!(
            err,
            KasuriError::DimensionMismatchInAnnotation { .. }
        ));
    }

    #[test]
    fn check_builtin_sin_requires_angle() {
        let source = "param x: Length = 1.0 m;\nnode y: Dimensionless = sin(@x);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, KasuriError::DimensionMismatch { .. }));
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
        assert!(matches!(err, KasuriError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_multiplication_creates_new_dim() {
        let source = "param mass: Mass = 10 kg;\nparam accel: Acceleration = 9.8 m / s^2;\nnode force: Force = @mass * @accel;";
        check(source).unwrap();
    }

    #[test]
    fn check_power_with_literal() {
        let source = "param r: Length = 5 m;\nnode area: Area = @r ^ 2.0;";
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
        assert!(matches!(err, KasuriError::DimensionMismatch { .. }));
    }

    #[test]
    fn check_non_generic_fn_return_type() {
        // Function returns Velocity but we annotate as Length
        let source = "fn speed(d: Length, t: Time) -> Velocity = d / t;\nparam d: Length = 10.0 m;\nparam t: Time = 2.0 s;\nnode v: Length = speed(@d, @t);";
        let err = check(source).unwrap_err();
        assert!(
            matches!(err, KasuriError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_generic_fn_call() {
        let source = "fn double<D: Dim>(x: D) -> D = x + x;\nparam alt: Length = 100 km;\nnode doubled: Length = double(@alt);";
        check(source).unwrap();
    }

    #[test]
    fn check_generic_fn_multi_param() {
        let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;\nparam x: Length = 100 km;\nparam y: Length = 200 km;\nnode mid: Length = lerp(@x, @y, 0.5);";
        check(source).unwrap();
    }

    #[test]
    fn check_generic_fn_consistency_error() {
        // a: D binds D=Length, b: D expects Length but gets Time
        let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;\nparam x: Length = 100 km;\nparam t: Time = 1.0 s;\nnode bad: Length = lerp(@x, @t, 0.5);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, KasuriError::DimensionMismatch { .. }));
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
            matches!(err, KasuriError::DimensionMismatchInAnnotation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn check_fn_wrong_arity() {
        let source =
            "fn f(a: Length) -> Length = a;\nparam x: Length = 1.0 m;\nnode y: Length = f(@x, @x);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, KasuriError::WrongArity { .. }));
    }

    #[test]
    fn check_fn_unknown_function() {
        let source = "param x: Length = 1.0 m;\nnode y: Length = no_such_fn(@x);";
        let err = check(source).unwrap_err();
        assert!(matches!(err, KasuriError::UnknownFunction { .. }));
    }
}
