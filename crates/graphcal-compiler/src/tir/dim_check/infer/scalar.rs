//! Type inference for scalar operations: BinOp, UnaryOp, Convert, DisplayTimezone, AsCast.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::{BinOp, Expr, ExprKind};
use crate::syntax::dimension::{Dimension, Rational};
use crate::syntax::names::{GenericParamName, UnitName};

use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;

use super::super::helpers::{expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Infer the type of a binary operation expression.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive match over all BinOp variants"
)]
pub(super) fn infer_binop(
    expr: &Expr,
    op: &BinOp,
    lhs: &Expr,
    rhs: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let lhs_type = infer_type(lhs, declared_types, local_types, registry, builtin_fns, src)?;
    let rhs_type = infer_type(rhs, declared_types, local_types, registry, builtin_fns, src)?;

    match op {
        // Logical operators: require Bool operands, return Bool
        BinOp::And | BinOp::Or => {
            if lhs_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&lhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: lhs.span.into(),
                });
            }
            if rhs_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&rhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        // Equality: operands must have the same ValueType.
        // Int and Fin(N) are compatible for equality comparison.
        BinOp::Eq | BinOp::Ne => {
            if lhs_type == rhs_type || (lhs_type.is_int_like() && rhs_type.is_int_like()) {
                return Ok(InferredType::Bool);
            }
            Err(GraphcalError::DimensionMismatch {
                expected: format_inferred_type(&lhs_type, registry),
                found: format_inferred_type(&rhs_type, registry),
                help: "equality operands must have the same type".to_string(),
                src: src.clone(),
                span: rhs.span.into(),
            })
        }
        // Ordering comparisons: require same-type scalar or Int/Fin operands, return Bool
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if lhs_type.is_int_like() || rhs_type.is_int_like() {
                if !lhs_type.is_int_like() || !rhs_type.is_int_like() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(&lhs_type, registry),
                        found: format_inferred_type(&rhs_type, registry),
                        help: "comparison operands must have the same type".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Bool);
            }
            // Datetime comparisons: same time scale required
            if let InferredType::Datetime(ls) = &lhs_type
                && let InferredType::Datetime(rs) = &rhs_type
            {
                if ls != rs {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(&lhs_type, registry),
                        found: format_inferred_type(&rhs_type, registry),
                        help: "cannot compare datetimes with different time scales".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Bool);
            }
            let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "comparison operands must have the same dimension".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        // Arithmetic operators: require matching numeric operands (Int or Scalar)
        BinOp::Add | BinOp::Sub => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            // Point-vs-vector rules for Datetime
            if let InferredType::Datetime(ls) = &lhs_type {
                let time_dim = Dimension::base(crate::syntax::dimension::BaseDimId::Prelude(
                    "Time".to_string(),
                ));
                if let InferredType::Datetime(rs) = &rhs_type {
                    // Datetime - Datetime -> Scalar(Time)
                    if *op == BinOp::Sub {
                        if ls != rs {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format_inferred_type(&lhs_type, registry),
                                found: format_inferred_type(&rhs_type, registry),
                                help: "cannot subtract datetimes with different time scales"
                                    .to_string(),
                                src: src.clone(),
                                span: rhs.span.into(),
                            });
                        }
                        return Ok(InferredType::Scalar(time_dim));
                    }
                    // Datetime + Datetime -> error
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Scalar(Time)".to_string(),
                        found: format_inferred_type(&rhs_type, registry),
                        help: "cannot add two datetimes; did you mean to subtract?".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                // Datetime +/- Scalar(Time) -> Datetime
                let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                if rhs_dim != time_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Time".to_string(),
                        found: registry.dimensions.format_dimension(&rhs_dim),
                        help: "can only add/subtract a Time duration to/from a Datetime"
                            .to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Datetime(*ls));
            }
            if let InferredType::Datetime(rs) = &rhs_type {
                // Scalar(Time) + Datetime -> Datetime (only for Add)
                if *op == BinOp::Add {
                    let time_dim = Dimension::base(crate::syntax::dimension::BaseDimId::Prelude(
                        "Time".to_string(),
                    ));
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    if lhs_dim != time_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Time".to_string(),
                            found: registry.dimensions.format_dimension(&lhs_dim),
                            help: "can only add a Time duration to a Datetime".to_string(),
                            src: src.clone(),
                            span: lhs.span.into(),
                        });
                    }
                    return Ok(InferredType::Datetime(*rs));
                }
                // Scalar - Datetime -> error
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&lhs_type, registry),
                    found: format_inferred_type(&rhs_type, registry),
                    help: "cannot subtract a Datetime from a scalar".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "operands of addition and subtraction must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Scalar(lhs_dim))
        }
        BinOp::Mul => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
            Ok(InferredType::Scalar(lhs_dim * rhs_dim))
        }
        BinOp::Div => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
            Ok(InferredType::Scalar(lhs_dim / rhs_dim))
        }
        BinOp::Mod => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            Err(GraphcalError::DimensionMismatch {
                expected: "Int".to_string(),
                found: format!(
                    "{} % {}",
                    format_inferred_type(&lhs_type, registry),
                    format_inferred_type(&rhs_type, registry)
                ),
                help: "modulo operator requires Int operands".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            })
        }
        BinOp::Pow => {
            // Int/Fin ^ Int (literal non-negative) -> Int
            if lhs_type.is_int_like() {
                if let ExprKind::Integer(n) = &rhs.kind {
                    if *n >= 0 {
                        return Ok(InferredType::Int);
                    }
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "non-negative Int exponent".to_string(),
                        found: format!("{n}"),
                        help: "integer power requires a non-negative exponent".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
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

/// Infer the type of a unary operation expression.
pub(super) fn infer_unary(
    op: &crate::syntax::ast::UnaryOp,
    operand: &Expr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let operand_type = infer_type(
        operand,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    match op {
        crate::syntax::ast::UnaryOp::Not => {
            if operand_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&operand_type, registry),
                    help: "logical NOT requires a Bool operand".to_string(),
                    src: src.clone(),
                    span: operand.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        crate::syntax::ast::UnaryOp::Neg => {
            if operand_type == InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "numeric type".to_string(),
                    found: "Bool".to_string(),
                    help: "negation requires a numeric operand,
                               not Bool"
                        .to_string(),
                    src: src.clone(),
                    span: operand.span.into(),
                });
            }
            // Negation preserves the type (Scalar or Int)
            Ok(operand_type)
        }
    }
}

/// Infer the type of a unit conversion expression.
pub(super) fn infer_convert(
    inner: &Expr,
    target: &crate::syntax::ast::UnitExpr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    let expr_dim = expect_scalar(&inner_type, registry, src, inner.span)?;
    let target_dim = registry
        .units
        .resolve_unit_dimension(target)
        .ok_or_else(|| {
            for item in &target.terms {
                if registry.units.get_unit(item.name.value.as_str()).is_none() {
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
            target: registry.dimensions.format_dimension(&target_dim),
            expr_dim: registry.dimensions.format_dimension(&expr_dim),
            src: src.clone(),
            span: target.span.into(),
        });
    }

    Ok(InferredType::Scalar(expr_dim))
}

/// Infer the type of a display timezone expression.
pub(super) fn infer_display_timezone(
    expr: &Expr,
    inner: &Expr,
    timezone: &str,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    if !matches!(&inner_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&inner_type, registry),
            help: format!("timezone display `-> \"{timezone}\"` requires a Datetime expression"),
            src: src.clone(),
            span: inner.span.into(),
        });
    }
    // Validate timezone string is a valid IANA timezone
    if jiff::tz::TimeZone::get(timezone).is_err() {
        return Err(GraphcalError::InvalidTimezone {
            timezone: timezone.to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    Ok(inner_type)
}

/// Infer the type of an `as` cast expression.
pub(super) fn infer_as_cast(
    expr: &Expr,
    inner: &Expr,
    target_type: &crate::syntax::ast::TypeExpr,
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_type(
        inner,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    // Resolve the target type
    let no_dim_params: &[GenericParamName] = &[];
    let no_index_params: &[GenericParamName] = &[];
    let no_nat_params: &[GenericParamName] = &[];
    let resolved_target = crate::tir::typed::resolve_type_expr(
        target_type,
        registry,
        no_dim_params,
        no_index_params,
        no_nat_params,
        src,
    )?;
    let target_declared = crate::tir::typed::resolved_to_declared_type(&resolved_target, src)?;
    let target_inferred = InferredType::from(&target_declared);

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
    let type_def = registry
        .types
        .get_type(source_name.as_str())
        .ok_or_else(|| GraphcalError::UnknownStructType {
            name: source_name.clone(),
            src: src.clone(),
            span: inner.span.into(),
        })?;
    for (i, param) in type_def.generic_params.iter().enumerate() {
        if param.constraint != crate::registry::types::TypeGenericConstraint::Unconstrained {
            // Non-phantom param — must match exactly
            if i < source_args.len() && i < target_args.len() && source_args[i] != target_args[i] {
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
