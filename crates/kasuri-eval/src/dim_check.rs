use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::{BinOp, DeclKind, Expr, ExprKind, File};
use kasuri_syntax::dimension::{Dimension, Rational};

use crate::builtins::{DimSignature, builtin_constants, builtin_functions};
use crate::error::KasuriError;
use crate::registry::Registry;

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
) -> Result<HashMap<String, Dimension>, KasuriError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    // Collect declared dimensions for all consts/params/nodes
    let mut declared_dims: HashMap<String, Dimension> = HashMap::new();

    // Built-in constants are Dimensionless
    for name in builtin_consts.keys() {
        declared_dims.insert((*name).to_string(), Dimension::DIMENSIONLESS);
    }

    // First pass: resolve declared type annotations
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(_) | DeclKind::Unit(_) => {}
            DeclKind::Const(c) => {
                let dim = registry
                    .resolve_type_expr(&c.type_ann)
                    .ok_or_else(|| unknown_dim_in_type(&c.type_ann, src))?;
                declared_dims.insert(c.name.name.clone(), dim);
            }
            DeclKind::Param(p) => {
                let dim = registry
                    .resolve_type_expr(&p.type_ann)
                    .ok_or_else(|| unknown_dim_in_type(&p.type_ann, src))?;
                declared_dims.insert(p.name.name.clone(), dim);
            }
            DeclKind::Node(n) => {
                let dim = registry
                    .resolve_type_expr(&n.type_ann)
                    .ok_or_else(|| unknown_dim_in_type(&n.type_ann, src))?;
                declared_dims.insert(n.name.name.clone(), dim);
            }
        }
    }

    // Second pass: infer dimensions and check against annotations
    for decl in &file.declarations {
        let (name, type_ann, value_expr) = match &decl.kind {
            DeclKind::Dimension(_) | DeclKind::Unit(_) => continue,
            DeclKind::Const(c) => (&c.name.name, &c.type_ann, &c.value),
            DeclKind::Param(p) => (&p.name.name, &p.type_ann, &p.value),
            DeclKind::Node(n) => (&n.name.name, &n.type_ann, &n.value),
        };

        let declared = declared_dims[name];
        let inferred = infer_dim(value_expr, &declared_dims, registry, &builtin_fns, src)?;

        if declared != inferred {
            return Err(KasuriError::DimensionMismatchInAnnotation {
                declared: format!("{declared}"),
                inferred: format!("{inferred}"),
                src: src.clone(),
                span: type_ann.span.into(),
            });
        }
    }

    Ok(declared_dims)
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

/// Infer the dimension of an expression.
#[expect(clippy::too_many_lines)]
fn infer_dim(
    expr: &Expr,
    declared_dims: &HashMap<String, Dimension>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, KasuriError> {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Bool(_) => Ok(Dimension::DIMENSIONLESS),

        ExprKind::UnitLiteral { unit, .. } => {
            let (dim, _scale) = registry.resolve_unit_expr(unit).ok_or_else(|| {
                // Find first unknown unit in the expression
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
            Ok(dim)
        }

        ExprKind::ConstRef(ident) => {
            declared_dims
                .get(&ident.name)
                .copied()
                .ok_or_else(|| KasuriError::UnknownConstRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
        }

        ExprKind::GraphRef(ident) => {
            declared_dims
                .get(&ident.name)
                .copied()
                .ok_or_else(|| KasuriError::UnknownGraphRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
        }

        ExprKind::BinOp { op, lhs, rhs } => {
            let lhs_dim = infer_dim(lhs, declared_dims, registry, builtin_fns, src)?;
            let rhs_dim = infer_dim(rhs, declared_dims, registry, builtin_fns, src)?;

            match op {
                BinOp::Add | BinOp::Sub => {
                    if lhs_dim != rhs_dim {
                        return Err(KasuriError::DimensionMismatch {
                            expected: format!("{lhs_dim}"),
                            found: format!("{rhs_dim}"),
                            src: src.clone(),
                            span: rhs.span.into(),
                        });
                    }
                    Ok(lhs_dim)
                }
                BinOp::Mul => Ok(lhs_dim * rhs_dim),
                BinOp::Div => Ok(lhs_dim / rhs_dim),
                BinOp::Pow => {
                    // RHS must be a numeric literal for dimensional analysis
                    if let ExprKind::Number(n) = &rhs.kind {
                        // Check if it's an integer
                        if n.fract() == 0.0 {
                            #[expect(clippy::cast_possible_truncation)]
                            let exp = *n as i32;
                            Ok(lhs_dim.pow(Rational::from_int(exp)))
                        } else {
                            // Fractional exponent: use rational approximation
                            // For now, only support 0.5 (sqrt)
                            #[expect(clippy::float_cmp)]
                            if *n == 0.5 {
                                Ok(lhs_dim.pow(Rational::new(1, 2)))
                            } else {
                                Err(KasuriError::NonLiteralExponent {
                                    src: src.clone(),
                                    span: rhs.span.into(),
                                })
                            }
                        }
                    } else if rhs_dim.is_dimensionless() {
                        // If LHS is dimensionless and RHS is dimensionless, result is dimensionless
                        if lhs_dim.is_dimensionless() {
                            Ok(Dimension::DIMENSIONLESS)
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
                // Comparison operators return Dimensionless (boolean)
                // but both sides must have the same dimension
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    if lhs_dim != rhs_dim {
                        return Err(KasuriError::DimensionMismatch {
                            expected: format!("{lhs_dim}"),
                            found: format!("{rhs_dim}"),
                            src: src.clone(),
                            span: rhs.span.into(),
                        });
                    }
                    Ok(Dimension::DIMENSIONLESS)
                }
                BinOp::And | BinOp::Or => {
                    // Both sides must be Dimensionless (boolean logic)
                    if !lhs_dim.is_dimensionless() {
                        return Err(KasuriError::DimensionMismatch {
                            expected: "Dimensionless".to_string(),
                            found: format!("{lhs_dim}"),
                            src: src.clone(),
                            span: lhs.span.into(),
                        });
                    }
                    if !rhs_dim.is_dimensionless() {
                        return Err(KasuriError::DimensionMismatch {
                            expected: "Dimensionless".to_string(),
                            found: format!("{rhs_dim}"),
                            src: src.clone(),
                            span: rhs.span.into(),
                        });
                    }
                    Ok(Dimension::DIMENSIONLESS)
                }
            }
        }

        ExprKind::UnaryOp { operand, .. } => {
            // Unary neg and not preserve dimension
            infer_dim(operand, declared_dims, registry, builtin_fns, src)
        }

        ExprKind::FnCall { name, args } => {
            let func = builtin_fns.get(name.name.as_str()).ok_or_else(|| {
                KasuriError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                }
            })?;

            let arg_dims: Vec<Dimension> = args
                .iter()
                .map(|a| infer_dim(a, declared_dims, registry, builtin_fns, src))
                .collect::<Result<_, _>>()?;

            infer_fn_dim(func.dim_sig, &arg_dims, args, src)
        }

        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond_dim = infer_dim(condition, declared_dims, registry, builtin_fns, src)?;
            if !cond_dim.is_dimensionless() {
                return Err(KasuriError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: format!("{cond_dim}"),
                    src: src.clone(),
                    span: condition.span.into(),
                });
            }

            let then_dim = infer_dim(then_branch, declared_dims, registry, builtin_fns, src)?;
            let else_dim = infer_dim(else_branch, declared_dims, registry, builtin_fns, src)?;

            if then_dim != else_dim {
                return Err(KasuriError::DimensionMismatch {
                    expected: format!("{then_dim}"),
                    found: format!("{else_dim}"),
                    src: src.clone(),
                    span: else_branch.span.into(),
                });
            }

            Ok(then_dim)
        }

        ExprKind::Convert {
            expr: inner,
            target,
        } => {
            let expr_dim = infer_dim(inner, declared_dims, registry, builtin_fns, src)?;
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

            Ok(expr_dim)
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

    fn check(source: &str) -> Result<HashMap<String, Dimension>, KasuriError> {
        let file = Parser::new(source).parse_file().unwrap();
        let registry = make_registry();
        let src = make_src(source);
        check_dimensions(&file, &registry, &src)
    }

    #[test]
    fn check_dimensionless_const() {
        let dims = check("const G0: Dimensionless = 9.80665;").unwrap();
        assert_eq!(dims["G0"], Dimension::DIMENSIONLESS);
    }

    #[test]
    fn check_dimensionless_arithmetic() {
        let dims =
            check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
        assert_eq!(dims["y"], Dimension::DIMENSIONLESS);
    }

    #[test]
    fn check_length_unit_literal() {
        let dims = check("param alt: Length = 400 km;").unwrap();
        let length = Dimension::base(kasuri_syntax::dimension::BaseDim::Length);
        assert_eq!(dims["alt"], length);
    }

    #[test]
    fn check_velocity_from_division() {
        let source = "param dist: Length = 100 km;\nparam time: Time = 2.0 hour;\nnode speed: Velocity = @dist / @time;";
        let dims = check(source).unwrap();
        let velocity = Dimension::base(kasuri_syntax::dimension::BaseDim::Length)
            / Dimension::base(kasuri_syntax::dimension::BaseDim::Time);
        assert_eq!(dims["speed"], velocity);
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
        let dims = check(source).unwrap();
        let velocity = Dimension::base(kasuri_syntax::dimension::BaseDim::Length)
            / Dimension::base(kasuri_syntax::dimension::BaseDim::Time);
        assert_eq!(dims["speed_kmh"], velocity);
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
}
