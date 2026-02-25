//! Type inference for expressions.
//!
//! Contains the main `infer_type` function that walks the AST and determines
//! the type (dimension, Bool, Int, struct, or indexed) of each expression.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{BinOp, Expr, ExprKind};
use graphcal_syntax::dimension::{Dimension, Rational};
use graphcal_syntax::names::{
    FieldName, FnName, GenericParamName, IndexName, StructTypeName, UnitName, VariantName,
};

use crate::error::GraphcalError;
use crate::registry::Registry;
use crate::resolve::is_aggregation_fn;
use crate::tir::ResolvedFnSig;

use super::builtins::infer_fn_dim;
use super::helpers::{
    cartesian_product, check_arm_types_match, check_derived_binop, check_derived_neg,
    declared_to_inferred, expect_scalar, format_inferred_type, resolve_field_type,
};
use super::{DeclaredType, InferredType};

/// Infer the type (dimension or struct) of an expression.
#[expect(
    clippy::too_many_lines,
    reason = "single match over all ExprKind variants"
)]
pub(super) fn infer_type(
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
        ExprKind::StringLiteral(_) => Err(GraphcalError::DimensionMismatch {
            expected: "a numeric or boolean expression".to_string(),
            found: "string literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
            help: "string literals can only be used as arguments to datetime() or epoch()"
                .to_string(),
        }),

        ExprKind::VariantLiteral { index, variant } => {
            // Validate index exists
            let idx_def = registry
                .indexes
                .get_index(index.value.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index.value.clone(),
                    src: src.clone(),
                    span: index.span.into(),
                })?;
            // Validate variant exists in this index
            if !idx_def
                .variants()
                .iter()
                .any(|v| v.as_str() == variant.value.as_str())
            {
                return Err(GraphcalError::UnknownVariant {
                    index_name: index.value.clone(),
                    variant_name: variant.value.clone(),
                    src: src.clone(),
                    span: variant.span.into(),
                });
            }
            Ok(InferredType::Label(index.value.clone()))
        }

        ExprKind::UnitLiteral { unit, .. } => {
            let (dim, _scale) = registry.units.resolve_unit_expr(unit).ok_or_else(|| {
                for item in &unit.terms {
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
                    span: unit.span.into(),
                }
            })?;
            Ok(InferredType::Scalar(dim))
        }

        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => {
            let dt = declared_types.get(ident.value.as_str()).ok_or_else(|| {
                GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                }
            })?;
            Ok(declared_to_inferred(dt))
        }

        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
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
                // Equality: operands must have the same ValueType.
                BinOp::Eq | BinOp::Ne => {
                    if lhs_type != rhs_type {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format_inferred_type(&lhs_type, registry),
                            found: format_inferred_type(&rhs_type, registry),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "equality operands must have the same type".to_string(),
                        });
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
                    // Datetime comparisons: same time scale required
                    if let InferredType::Datetime(ls) = &lhs_type
                        && let InferredType::Datetime(rs) = &rhs_type
                    {
                        if ls != rs {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format_inferred_type(&lhs_type, registry),
                                found: format_inferred_type(&rhs_type, registry),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "cannot compare datetimes with different time scales"
                                    .to_string(),
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
                    // Point-vs-vector rules for Datetime
                    if let InferredType::Datetime(ls) = &lhs_type {
                        let time_dim = Dimension::base(
                            graphcal_syntax::dimension::BaseDimId::Prelude("Time".to_string()),
                        );
                        if let InferredType::Datetime(rs) = &rhs_type {
                            // Datetime - Datetime -> Scalar(Time)
                            if *op == BinOp::Sub {
                                if ls != rs {
                                    return Err(GraphcalError::DimensionMismatch {
                                        expected: format_inferred_type(&lhs_type, registry),
                                        found: format_inferred_type(&rhs_type, registry),
                                        src: src.clone(),
                                        span: rhs.span.into(),
                                        help:
                                            "cannot subtract datetimes with different time scales"
                                                .to_string(),
                                    });
                                }
                                return Ok(InferredType::Scalar(time_dim));
                            }
                            // Datetime + Datetime -> error
                            return Err(GraphcalError::DimensionMismatch {
                                expected: "Scalar(Time)".to_string(),
                                found: format_inferred_type(&rhs_type, registry),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "cannot add two datetimes; did you mean to subtract?"
                                    .to_string(),
                            });
                        }
                        // Datetime +/- Scalar(Time) -> Datetime
                        let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                        if rhs_dim != time_dim {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: "Time".to_string(),
                                found: registry.dimensions.format_dimension(&rhs_dim),
                                src: src.clone(),
                                span: rhs.span.into(),
                                help: "can only add/subtract a Time duration to/from a Datetime"
                                    .to_string(),
                            });
                        }
                        return Ok(InferredType::Datetime(*ls));
                    }
                    if let InferredType::Datetime(rs) = &rhs_type {
                        // Scalar(Time) + Datetime -> Datetime (only for Add)
                        if *op == BinOp::Add {
                            let time_dim = Dimension::base(
                                graphcal_syntax::dimension::BaseDimId::Prelude("Time".to_string()),
                            );
                            let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                            if lhs_dim != time_dim {
                                return Err(GraphcalError::DimensionMismatch {
                                    expected: "Time".to_string(),
                                    found: registry.dimensions.format_dimension(&lhs_dim),
                                    src: src.clone(),
                                    span: lhs.span.into(),
                                    help: "can only add a Time duration to a Datetime".to_string(),
                                });
                            }
                            return Ok(InferredType::Datetime(*rs));
                        }
                        // Scalar - Datetime -> error
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format_inferred_type(&lhs_type, registry),
                            found: format_inferred_type(&rhs_type, registry),
                            src: src.clone(),
                            span: rhs.span.into(),
                            help: "cannot subtract a Datetime from a scalar".to_string(),
                        });
                    }
                    let lhs_dim = expect_scalar(&lhs_type, registry, src, lhs.span)?;
                    let rhs_dim = expect_scalar(&rhs_type, registry, src, rhs.span)?;
                    if lhs_dim != rhs_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: registry.dimensions.format_dimension(&lhs_dim),
                            found: registry.dimensions.format_dimension(&rhs_dim),
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

        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            // Aggregation functions over indexed values: sum, min, max, mean, count
            if is_aggregation_fn(name.value.as_str()) && args.len() == 1 {
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
                        found: registry.dimensions.format_dimension(&arg_dim),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: "to_int() requires a Dimensionless argument".to_string(),
                    });
                }
                return Ok(InferredType::Int);
            }

            // datetime(string_literal) -> Datetime(UTC)
            // datetime(string_literal, string_literal) -> Datetime(UTC)  (with timezone)
            if name.value.as_str() == "datetime" {
                if args.is_empty() || args.len() > 2 {
                    return Err(GraphcalError::EvalError {
                        message: format!("datetime() expects 1 or 2 arguments, got {}", args.len()),
                        src: src.clone(),
                        span: name.span.into(),
                    });
                }
                match &args[0].kind {
                    ExprKind::StringLiteral(_) => {}
                    _ => {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "string literal".to_string(),
                            found: format_inferred_type(
                                &infer_type(
                                    &args[0],
                                    declared_types,
                                    local_types,
                                    registry,
                                    builtin_fns,
                                    resolved_fn_sigs,
                                    src,
                                )?,
                                registry,
                            ),
                            src: src.clone(),
                            span: args[0].span.into(),
                            help: "datetime() requires a string literal argument".to_string(),
                        });
                    }
                }
                if args.len() == 2 {
                    match &args[1].kind {
                        ExprKind::StringLiteral(_) => {}
                        _ => {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: "string literal (IANA timezone)".to_string(),
                                found: format_inferred_type(
                                    &infer_type(
                                        &args[1],
                                        declared_types,
                                        local_types,
                                        registry,
                                        builtin_fns,
                                        resolved_fn_sigs,
                                        src,
                                    )?,
                                    registry,
                                ),
                                src: src.clone(),
                                span: args[1].span.into(),
                                help: "datetime() second argument must be a timezone string literal (e.g. \"Asia/Tokyo\")".to_string(),
                            });
                        }
                    }
                }
                return Ok(InferredType::Datetime(crate::time_scale::TimeScale::UTC));
            }

            // epoch(string_literal, TimeScale) -> Datetime(scale)
            if name.value.as_str() == "epoch" {
                if args.len() != 2 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new("epoch"),
                        expected: 2,
                        got: args.len(),
                        src: src.clone(),
                        span: name.span.into(),
                    });
                }
                // First arg must be a string literal
                if !matches!(&args[0].kind, ExprKind::StringLiteral(_)) {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "string literal".to_string(),
                        found: format_inferred_type(
                            &infer_type(
                                &args[0],
                                declared_types,
                                local_types,
                                registry,
                                builtin_fns,
                                resolved_fn_sigs,
                                src,
                            )?,
                            registry,
                        ),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: "epoch() requires a string literal as its first argument".to_string(),
                    });
                }
                // Second arg must be a time scale identifier
                let ExprKind::ConstRef(scale_ident) = &args[1].kind else {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)"
                            .to_string(),
                        found: format_inferred_type(
                            &infer_type(
                                &args[1],
                                declared_types,
                                local_types,
                                registry,
                                builtin_fns,
                                resolved_fn_sigs,
                                src,
                            )?,
                            registry,
                        ),
                        src: src.clone(),
                        span: args[1].span.into(),
                        help: "epoch() requires a time scale identifier as its second argument"
                            .to_string(),
                    });
                };
                let scale: crate::time_scale::TimeScale =
                    scale_ident.value.as_str().parse().map_err(|_| {
                        GraphcalError::DimensionMismatch {
                            expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)"
                                .to_string(),
                            found: scale_ident.value.to_string(),
                            src: src.clone(),
                            span: args[1].span.into(),
                            help: format!(
                                "unknown time scale `{}`; expected one of: {}",
                                scale_ident.value.as_str(),
                                crate::time_scale::TimeScale::ALL_NAMES.join(", ")
                            ),
                        }
                    })?;
                return Ok(InferredType::Datetime(scale));
            }

            // Time scale conversion: to_utc, to_tai, to_tt, to_tdb, to_et, to_gpst, to_gst, to_bdt, to_qzsst
            if let Some(target_scale) =
                crate::time_scale::time_scale_from_conversion_fn(name.value.as_str())
            {
                if args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: name.value.clone(),
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
                if !matches!(&arg_type, InferredType::Datetime(_)) {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Datetime".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: format!("{}() requires a Datetime argument", name.value.as_str()),
                    });
                }
                return Ok(InferredType::Datetime(target_scale));
            }

            // Datetime extraction functions: year, month, day, etc. -> Int
            if crate::resolve::DATETIME_EXTRACT_FNS.contains(&name.value.as_str()) {
                if args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: name.value.clone(),
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
                if !matches!(&arg_type, InferredType::Datetime(_)) {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Datetime".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: format!("{}() requires a Datetime argument", name.value.as_str()),
                    });
                }
                return Ok(InferredType::Int);
            }

            // Datetime from-numeric constructors: from_jd, from_mjd, from_unix -> Datetime(UTC)
            if crate::resolve::DATETIME_FROM_FNS.contains(&name.value.as_str()) {
                if args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: name.value.clone(),
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
                match &arg_type {
                    InferredType::Scalar(dim) if dim.is_dimensionless() => {}
                    InferredType::Int => {}
                    _ => {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Dimensionless or Int".to_string(),
                            found: format_inferred_type(&arg_type, registry),
                            src: src.clone(),
                            span: args[0].span.into(),
                            help: format!(
                                "{}() requires a dimensionless numeric argument",
                                name.value.as_str()
                            ),
                        });
                    }
                }
                return Ok(InferredType::Datetime(crate::time_scale::TimeScale::UTC));
            }

            // Datetime to-numeric functions: to_jd, to_mjd, to_unix -> Dimensionless
            if crate::resolve::DATETIME_TO_FNS.contains(&name.value.as_str()) {
                if args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: name.value.clone(),
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
                if !matches!(&arg_type, InferredType::Datetime(_)) {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Datetime".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        src: src.clone(),
                        span: args[0].span.into(),
                        help: format!("{}() requires a Datetime argument", name.value.as_str()),
                    });
                }
                return Ok(InferredType::Scalar(Dimension::dimensionless()));
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
                return infer_fn_dim(&func.dim_sig, &arg_dims, args, registry, src)
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
            let (target_dim, _scale) =
                registry.units.resolve_unit_expr(target).ok_or_else(|| {
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

        ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
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
            if !matches!(&inner_type, InferredType::Datetime(_)) {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Datetime".to_string(),
                    found: format_inferred_type(&inner_type, registry),
                    src: src.clone(),
                    span: inner.span.into(),
                    help: format!(
                        "timezone display `-> \"{timezone}\"` requires a Datetime expression"
                    ),
                });
            }
            // Validate timezone string is a valid IANA timezone
            if jiff::tz::TimeZone::get(timezone).is_err() {
                return Err(GraphcalError::InvalidTimezone {
                    timezone: timezone.clone(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            Ok(inner_type)
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
            let type_def = registry
                .types
                .get_type(source_name.as_str())
                .ok_or_else(|| GraphcalError::UnknownStructType {
                    name: source_name.clone(),
                    src: src.clone(),
                    span: inner.span.into(),
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
                    let type_def =
                        registry.types.get_type(type_name.as_str()).ok_or_else(|| {
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
                if let Some(type_def) = registry.types.get_type(type_name.value.as_str()) {
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
                    registry.types.get_type_by_variant(type_name.value.as_str())
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
                        let default_expr =
                            param
                                .default
                                .as_ref()
                                .ok_or_else(|| GraphcalError::EvalError {
                                    message: format!(
                                        "internal: generic parameter `{}` has no default",
                                        param.name
                                    ),
                                    src: src.clone(),
                                    span: type_name.span.into(),
                                })?;
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
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!(
                            "internal: unknown field `{}` in struct `{}`",
                            field_init.name.value, type_name.value
                        ),
                        src: src.clone(),
                        span: field_init.name.span.into(),
                    })?;

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
                let idx_def = registry.indexes.get_index(idx_name).ok_or_else(|| {
                    GraphcalError::UnknownIndex {
                        name: binding.index.value.clone(),
                        src: src.clone(),
                        span: binding.index.span.into(),
                    }
                })?;
                inner_locals.insert(
                    binding.var.name.clone(),
                    match &idx_def.kind {
                        crate::registry::IndexKind::Named { .. } => {
                            InferredType::Label(binding.index.value.clone())
                        }
                        crate::registry::IndexKind::Range { dimension, .. } => {
                            InferredType::Scalar(dimension.clone())
                        }
                    },
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

        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            if entries.is_empty() {
                return Err(GraphcalError::EvalError {
                    message: "empty map literal".to_string(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            let arity = entries[0].keys.len();
            if arity == 0 {
                return Err(GraphcalError::EvalError {
                    message: "map literal entry has no keys".to_string(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            // Validate all entries have the same arity
            for entry in &entries[1..] {
                if entry.keys.len() != arity {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "map literal entries have inconsistent key arity: expected {arity}, found {}",
                            entry.keys.len()
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }
            // Validate index names: all entries must use the same indexes in the same order
            let index_names: Vec<&IndexName> =
                entries[0].keys.iter().map(|k| &k.index.value).collect();
            for entry in &entries[1..] {
                for (i, key) in entry.keys.iter().enumerate() {
                    if key.index.value != *index_names[i] {
                        return Err(GraphcalError::IndexMismatch {
                            expected: index_names[i].clone(),
                            found: key.index.value.clone(),
                            src: src.clone(),
                            span: key.index.span.into(),
                        });
                    }
                }
            }
            // Validate each index exists and collect their variant lists
            let mut axes_variants: Vec<Vec<VariantName>> = Vec::new();
            for key in &entries[0].keys {
                let idx_def = registry
                    .indexes
                    .get_index(key.index.value.as_str())
                    .ok_or_else(|| GraphcalError::UnknownIndex {
                        name: key.index.value.clone(),
                        src: src.clone(),
                        span: key.index.span.into(),
                    })?;
                axes_variants.push(idx_def.variants());
            }
            // Check totality over the Cartesian product
            let mut expected_tuples: std::collections::HashSet<Vec<&str>> =
                std::collections::HashSet::new();
            cartesian_product(&axes_variants, &mut Vec::new(), &mut expected_tuples);
            let mut provided_tuples: std::collections::HashSet<Vec<&str>> =
                std::collections::HashSet::new();
            for entry in entries {
                let tuple: Vec<&str> = entry
                    .keys
                    .iter()
                    .map(|k| k.variant.value.as_str())
                    .collect();
                if !provided_tuples.insert(tuple.clone()) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "duplicate map literal entry for key tuple ({})",
                            entry
                                .keys
                                .iter()
                                .map(|k| format!("{}::{}", k.index.value, k.variant.value))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
                // For multi-axis, validate each variant exists in its respective index.
                // For single-axis, skip this check — extra/missing set difference
                // handles it with more specific error types (ExtraVariants/MissingVariants).
                if arity > 1 {
                    for (i, key) in entry.keys.iter().enumerate() {
                        if !axes_variants[i]
                            .iter()
                            .any(|v| v.as_str() == key.variant.value.as_str())
                        {
                            return Err(GraphcalError::UnknownVariant {
                                index_name: key.index.value.clone(),
                                variant_name: key.variant.value.clone(),
                                src: src.clone(),
                                span: key.variant.span.into(),
                            });
                        }
                    }
                }
            }
            // Check for extra variants (provided but not in expected set)
            let extra: Vec<Vec<&str>> = provided_tuples
                .difference(&expected_tuples)
                .cloned()
                .collect();
            if !extra.is_empty() {
                if arity == 1 {
                    let extra_variants: Vec<VariantName> =
                        extra.iter().map(|t| VariantName::new(t[0])).collect();
                    return Err(GraphcalError::ExtraVariants {
                        index_name: index_names[0].clone(),
                        extra: extra_variants,
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
                let extra_strs: Vec<String> = extra
                    .iter()
                    .map(|t| {
                        t.iter()
                            .enumerate()
                            .map(|(i, v)| format!("{}::{v}", index_names[i]))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .collect();
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "extra entries in map literal: ({})",
                        extra_strs.join("), (")
                    ),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            // Check for missing tuples
            let missing: Vec<Vec<&str>> = expected_tuples
                .difference(&provided_tuples)
                .cloned()
                .collect();
            if !missing.is_empty() {
                if arity == 1 {
                    let missing_variants: Vec<VariantName> =
                        missing.iter().map(|t| VariantName::new(t[0])).collect();
                    return Err(GraphcalError::MissingVariants {
                        index_name: index_names[0].clone(),
                        missing: missing_variants,
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
                let missing_strs: Vec<String> = missing
                    .iter()
                    .map(|t| {
                        t.iter()
                            .enumerate()
                            .map(|(i, v)| format!("{}::{v}", index_names[i]))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .collect();
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "non-exhaustive map literal: missing entries for ({})",
                        missing_strs.join("), (")
                    ),
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
            // Reject nested Indexed as element type (ValueType constraint)
            if matches!(first_type, InferredType::Indexed { .. }) {
                return Err(GraphcalError::EvalError {
                    message: "map literal element type must be a value type, not an indexed type; use tuple keys for multi-axis map literals".to_string(),
                    src: src.clone(),
                    span: entries[0].value.span.into(),
                });
            }
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
            // Wrap in nested Indexed layers (reverse order, matching `for` comprehension)
            let mut result = first_type;
            for idx_name in index_names.iter().rev() {
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: (*idx_name).clone(),
                };
            }
            Ok(result)
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
                        let idx_def =
                            registry
                                .indexes
                                .get_index(idx_name.as_str())
                                .ok_or_else(|| GraphcalError::UnknownIndex {
                                    name: idx_name.clone(),
                                    src: src.clone(),
                                    span: index.span.into(),
                                })?;
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
                            InferredType::Label(label_index) => {
                                if label_index.as_str() != idx_name.as_str() {
                                    return Err(GraphcalError::IndexMismatch {
                                        expected: idx_name,
                                        found: label_index.clone(),
                                        src: src.clone(),
                                        span: ident.span.into(),
                                    });
                                }
                            }
                            InferredType::Struct(type_name, args) => {
                                if type_name.as_str() != idx_name.as_str() || !args.is_empty() {
                                    return Err(GraphcalError::IndexMismatch {
                                        expected: idx_name,
                                        found: IndexName::new(type_name.as_str()),
                                        src: src.clone(),
                                        span: ident.span.into(),
                                    });
                                }
                            }
                            InferredType::Scalar(_) => {
                                // Allow scalar locals to be used as index args
                                // for range indexes (e.g. prev_i, i in Unfold)
                                let idx_def = registry
                                    .indexes
                                    .get_index(idx_name.as_str())
                                    .ok_or_else(|| GraphcalError::UnknownIndex {
                                        name: idx_name.clone(),
                                        src: src.clone(),
                                        span: ident.span.into(),
                                    })?;
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
                    && let Some(idx_def) = registry.indexes.get_index(index.as_str())
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
                    && let Some(idx_def) = registry.indexes.get_index(index.as_str())
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
            // Infer scrutinee type — must be a struct/tagged union value.
            let scrutinee_type = infer_type(
                scrutinee,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )?;

            Ok(match &scrutinee_type {
                InferredType::Label(index_name) => {
                    // Label scrutinee: match on index variants (fieldless, qualified syntax)
                    let index_def =
                        registry
                            .indexes
                            .get_index(index_name.as_str())
                            .ok_or_else(|| GraphcalError::UnknownIndex {
                                name: index_name.clone(),
                                src: src.clone(),
                                span: scrutinee.span.into(),
                            })?;

                    let crate::registry::IndexKind::Named { variants } = &index_def.kind else {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "cannot match on range index `{index_name}`; only named indexes can be matched"
                            ),
                            src: src.clone(),
                            span: scrutinee.span.into(),
                        });
                    };

                    let mut covered: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut arm_types: Vec<InferredType> = Vec::new();

                    for arm in arms {
                        let variant_name_str = arm.pattern.variant_name.value.as_str();

                        // For label patterns, qualified_index must match the index name
                        if let Some(qualified) = &arm.pattern.qualified_index
                            && qualified.value.as_str() != index_name.as_str()
                        {
                            return Err(GraphcalError::IndexMismatch {
                                expected: index_name.clone(),
                                found: qualified.value.clone(),
                                src: src.clone(),
                                span: qualified.span.into(),
                            });
                        }

                        // Check variant belongs to this index
                        if !variants.iter().any(|v| v.as_str() == variant_name_str) {
                            return Err(GraphcalError::UnknownField {
                                type_name: StructTypeName::new(index_name.as_str()),
                                field_name: FieldName::new(variant_name_str),
                                src: src.clone(),
                                span: arm.pattern.variant_name.span.into(),
                            });
                        }

                        // Check for duplicate arms
                        if !covered.insert(variant_name_str.to_string()) {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "duplicate match arm for variant `{variant_name_str}`"
                                ),
                                src: src.clone(),
                                span: arm.pattern.span.into(),
                            });
                        }

                        // Labels are fieldless — no bindings allowed
                        if !arm.pattern.bindings.is_empty() {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "index label variant `{index_name}::{variant_name_str}` has no fields to bind"
                                ),
                                src: src.clone(),
                                span: arm.pattern.span.into(),
                            });
                        }

                        // Infer arm body type
                        let arm_type = infer_type(
                            &arm.body,
                            declared_types,
                            local_types,
                            registry,
                            builtin_fns,
                            resolved_fn_sigs,
                            src,
                        )?;
                        arm_types.push(arm_type);
                    }

                    // Check exhaustiveness: all variants must be covered
                    for variant in variants {
                        if !covered.contains(variant.as_str()) {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "non-exhaustive match: variant `{index_name}::{}` not covered",
                                    variant.as_str()
                                ),
                                src: src.clone(),
                                span: expr.span.into(),
                            });
                        }
                    }

                    // All arm types must match
                    check_arm_types_match(&arm_types, arms, registry, src, expr)?
                }

                InferredType::Struct(type_name, scrutinee_type_args) => {
                    let type_def =
                        registry.types.get_type(type_name.as_str()).ok_or_else(|| {
                            GraphcalError::UnknownStructType {
                                name: type_name.clone(),
                                src: src.clone(),
                                span: scrutinee.span.into(),
                            }
                        })?;

                    let mut covered: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut arm_types: Vec<InferredType> = Vec::new();

                    for arm in arms {
                        let variant_name_str = arm.pattern.variant_name.value.as_str();
                        if let Some(qualified) = &arm.pattern.qualified_index
                            && qualified.value.as_str() != type_name.as_str()
                        {
                            return Err(GraphcalError::IndexMismatch {
                                expected: IndexName::new(type_name.as_str()),
                                found: qualified.value.clone(),
                                src: src.clone(),
                                span: qualified.span.into(),
                            });
                        }

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
                                message: format!(
                                    "duplicate match arm for variant `{variant_name_str}`"
                                ),
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
                                        scrutinee_type_args,
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
                    check_arm_types_match(&arm_types, arms, registry, src, expr)?
                }

                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "cannot match on type `{}`; expected a tagged union or label value",
                            format_inferred_type(&scrutinee_type, registry)
                        ),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    });
                }
            })
        }
    }
}
