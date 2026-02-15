use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_syntax::ast::{BinOp, Expr, ExprKind, FnBody, UnaryOp};
use graphcal_syntax::names::{FieldName, IndexName, StructTypeName, VariantName};
use graphcal_syntax::span::Span;

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;

/// A runtime value: either a scalar (f64 in SI units), a bool, a struct, or an indexed collection.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Scalar(f64),
    Bool(bool),
    Int(i64),
    Struct {
        type_name: StructTypeName,
        variant: VariantName,
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values, preserving declaration order.
    Indexed {
        index_name: IndexName,
        entries: IndexMap<VariantName, Self>,
    },
    /// A variant label during `for` comprehension iteration.
    /// Not a "real" value — only exists in `local_values` during loop body evaluation.
    VariantLabel {
        variant: VariantName,
    },
}

impl RuntimeValue {
    /// Extract scalar value, panicking if this is not a scalar.
    /// (Type mismatches are caught by `dim_check`, so this is safe at runtime.)
    fn expect_scalar(&self, context: &str) -> f64 {
        match self {
            Self::Scalar(v) => *v,
            Self::Bool(_) => {
                panic!("expected scalar for {context}, got Bool")
            }
            Self::Int(i) => {
                panic!("expected scalar for {context}, got Int({i})")
            }
            Self::Struct { type_name, .. } => {
                panic!("expected scalar for {context}, got struct `{type_name}`")
            }
            Self::Indexed { index_name, .. } => {
                panic!("expected scalar for {context}, got indexed value `{index_name}[...]`")
            }
            Self::VariantLabel { variant, .. } => {
                panic!("expected scalar for {context}, got variant label `{variant}`")
            }
        }
    }

    /// Extract boolean value, panicking if this is not a Bool.
    /// (Type mismatches are caught by `dim_check`, so this is safe at runtime.)
    fn expect_bool(&self, context: &str) -> bool {
        match self {
            Self::Bool(b) => *b,
            other => panic!("expected Bool for {context}, got {other:?}"),
        }
    }
}

/// Evaluate an expression given a set of resolved values and built-in functions.
/// Returns a `RuntimeValue` (scalar or struct).
///
/// # Errors
///
/// Returns a [`GraphcalError`] if the expression references an undefined variable,
/// constant, or function.
#[expect(
    clippy::too_many_lines,
    reason = "single match over all ExprKind variants"
)]
pub fn eval_expr(
    expr: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(RuntimeValue::Scalar(*n)),
        ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        ExprKind::UnitLiteral { value, unit } => {
            let (_dim, scale) =
                registry
                    .resolve_unit_expr(unit)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: "unknown unit in literal".to_string(),
                        src: src.clone(),
                        span: unit.span.into(),
                    })?;
            Ok(RuntimeValue::Scalar(*value * scale))
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        ExprKind::GraphRef(ident) => {
            values
                .get(ident.value.as_str())
                .cloned()
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("undefined graph reference `@{}`", ident.value),
                    src: src.clone(),
                    span: expr.span.into(),
                })
        }
        ExprKind::ConstRef(ident) => values
            .get(ident.value.as_str())
            .cloned()
            .or_else(|| {
                builtin_consts
                    .get(ident.value.as_str())
                    .map(|v| RuntimeValue::Scalar(*v))
            })
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!("undefined constant `{}`", ident.value),
                src: src.clone(),
                span: expr.span.into(),
            }),
        ExprKind::LocalRef(ident) => {
            local_values
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("undefined local variable `{}`", ident.name),
                    src: src.clone(),
                    span: expr.span.into(),
                })
        }
        ExprKind::BinOp { op, lhs, rhs } => match op {
            // Logical operators: Bool operands, Bool result
            BinOp::And => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("AND operand");
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("AND operand");
                Ok(RuntimeValue::Bool(l && r))
            }
            BinOp::Or => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("OR operand");
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("OR operand");
                Ok(RuntimeValue::Bool(l || r))
            }
            // Equality: can compare Bool==Bool, Int==Int, or Scalar==Scalar
            BinOp::Eq | BinOp::Ne => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let is_eq = *op == BinOp::Eq;
                match (&l, &r) {
                    (RuntimeValue::Bool(lb), RuntimeValue::Bool(rb)) => {
                        Ok(RuntimeValue::Bool(if is_eq { lb == rb } else { lb != rb }))
                    }
                    (RuntimeValue::Int(li), RuntimeValue::Int(ri)) => {
                        Ok(RuntimeValue::Bool(if is_eq { li == ri } else { li != ri }))
                    }
                    _ => {
                        let lv = l.expect_scalar("comparison operand");
                        let rv = r.expect_scalar("comparison operand");
                        Ok(RuntimeValue::Bool(eval_comparison(*op, lv, rv)))
                    }
                }
            }
            // Ordering comparisons: Int or Scalar operands, Bool result
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                    let result = match op {
                        BinOp::Lt => li < ri,
                        BinOp::Gt => li > ri,
                        BinOp::Le => li <= ri,
                        BinOp::Ge => li >= ri,
                        _ => unreachable!(),
                    };
                    return Ok(RuntimeValue::Bool(result));
                }
                let lv = l.expect_scalar("comparison operand");
                let rv = r.expect_scalar("comparison operand");
                Ok(RuntimeValue::Bool(eval_comparison(*op, lv, rv)))
            }
            // Arithmetic operators: Int or Scalar operands
            _ => {
                let l = eval_expr(
                    lhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let r = eval_expr(
                    rhs,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                    return Ok(RuntimeValue::Int(eval_int_binop(
                        *op, *li, *ri, src, expr.span,
                    )?));
                }
                let lv = l.expect_scalar("binary operand");
                let rv = r.expect_scalar("binary operand");
                Ok(RuntimeValue::Scalar(eval_binop(
                    *op, lv, rv, src, expr.span,
                )?))
            }
        },
        ExprKind::UnaryOp { op, operand } => match op {
            UnaryOp::Neg => {
                let v = eval_expr(
                    operand,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                match v {
                    RuntimeValue::Int(i) => {
                        let negated = i.checked_neg().ok_or_else(|| GraphcalError::EvalError {
                            message: "integer negation overflow".to_string(),
                            src: src.clone(),
                            span: expr.span.into(),
                        })?;
                        Ok(RuntimeValue::Int(negated))
                    }
                    _ => Ok(RuntimeValue::Scalar(-v.expect_scalar("unary negation"))),
                }
            }
            UnaryOp::Not => {
                let v = eval_expr(
                    operand,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?
                .expect_bool("logical NOT");
                Ok(RuntimeValue::Bool(!v))
            }
        },
        ExprKind::FnCall { name, args } => {
            // Aggregation functions over indexed values: sum, min, max, mean, count
            if matches!(
                name.value.as_str(),
                "sum" | "min" | "max" | "mean" | "count"
            ) && args.len() == 1
            {
                let arg_val = eval_expr(
                    &args[0],
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                if let RuntimeValue::Indexed { entries, .. } = arg_val {
                    return Ok(match name.value.as_str() {
                        "sum" => {
                            let total: f64 = entries
                                .values()
                                .map(|v| v.expect_scalar("sum element"))
                                .sum();
                            RuntimeValue::Scalar(total)
                        }
                        "min" => {
                            let min = entries
                                .values()
                                .map(|v| v.expect_scalar("min element"))
                                .fold(f64::INFINITY, f64::min);
                            RuntimeValue::Scalar(min)
                        }
                        "max" => {
                            let max = entries
                                .values()
                                .map(|v| v.expect_scalar("max element"))
                                .fold(f64::NEG_INFINITY, f64::max);
                            RuntimeValue::Scalar(max)
                        }
                        "mean" => {
                            #[expect(
                                clippy::cast_precision_loss,
                                reason = "indexed collection length fits in f64"
                            )]
                            let n = entries.len() as f64;
                            let total: f64 = entries
                                .values()
                                .map(|v| v.expect_scalar("mean element"))
                                .sum();
                            RuntimeValue::Scalar(total / n)
                        }
                        "count" => {
                            #[expect(
                                clippy::cast_precision_loss,
                                reason = "indexed collection length fits in f64"
                            )]
                            let n = entries.len() as f64;
                            RuntimeValue::Scalar(n)
                        }
                        _ => unreachable!(),
                    });
                }
                // If not indexed, fall through to builtins (min/max are 2-arg builtins)
            }

            // Conversion builtins: to_float and to_int
            if name.value.as_str() == "to_float" {
                let arg = eval_expr(
                    &args[0],
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let RuntimeValue::Int(i) = arg else {
                    panic!("to_float expects Int argument (checked by dim_check)");
                };
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "explicit conversion from Int to float"
                )]
                return Ok(RuntimeValue::Scalar(i as f64));
            }
            if name.value.as_str() == "to_int" {
                let arg = eval_expr(
                    &args[0],
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                let f = arg.expect_scalar("to_int argument");
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "explicit truncating conversion from float to Int"
                )]
                return Ok(RuntimeValue::Int(f as i64));
            }

            // Try builtin first
            if let Some(builtin) = builtin_fns.get(name.value.as_str()) {
                let arg_values: Vec<f64> = args
                    .iter()
                    .map(|a| {
                        eval_expr(
                            a,
                            values,
                            local_values,
                            builtin_consts,
                            builtin_fns,
                            registry,
                            src,
                        )
                        .map(|rv| rv.expect_scalar("function argument"))
                    })
                    .collect::<Result<_, _>>()?;
                let result = (builtin.eval)(&arg_values);
                return Ok(RuntimeValue::Scalar(check_finite(
                    result,
                    name.value.as_str(),
                    src,
                    expr.span,
                )?));
            }

            // Try user-defined function
            let fn_def = registry.get_function(name.value.as_str()).ok_or_else(|| {
                GraphcalError::EvalError {
                    message: format!("unknown function `{}`", name.value),
                    src: src.clone(),
                    span: name.span.into(),
                }
            })?;

            // Evaluate arguments
            let arg_values: Vec<RuntimeValue> = args
                .iter()
                .map(|a| {
                    eval_expr(
                        a,
                        values,
                        local_values,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )
                })
                .collect::<Result<_, _>>()?;

            // Build fn_locals from param names + arg values
            let mut fn_locals: HashMap<String, RuntimeValue> = HashMap::new();
            for (param, val) in fn_def.params.iter().zip(arg_values) {
                fn_locals.insert(param.name.clone(), val);
            }

            // Evaluate body: pass `values` for ConstRef access (user consts like GM_EARTH).
            // Purity is enforced by the resolver's @ prohibition -- no GraphRef nodes
            // exist in function bodies, so passing values is safe.
            let body = fn_def.body.clone();
            match &body {
                FnBody::Short(expr) => eval_expr(
                    expr,
                    values,
                    &fn_locals,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                ),
                FnBody::Block { stmts, expr } => {
                    let mut block_locals = fn_locals;
                    for binding in stmts {
                        let val = eval_expr(
                            &binding.value,
                            values,
                            &block_locals,
                            builtin_consts,
                            builtin_fns,
                            registry,
                            src,
                        )?;
                        block_locals.insert(binding.name.name.clone(), val);
                    }
                    eval_expr(
                        expr,
                        values,
                        &block_locals,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )
                }
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = eval_expr(
                condition,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
            .expect_bool("if condition");
            if cond {
                eval_expr(
                    then_branch,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )
            } else {
                eval_expr(
                    else_branch,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )
            }
        }
        ExprKind::Convert { expr: inner, .. } => eval_expr(
            inner,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        ExprKind::Block { stmts, expr: body } => {
            let mut block_locals = local_values.clone();
            for binding in stmts {
                let val = eval_expr(
                    &binding.value,
                    values,
                    &block_locals,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                block_locals.insert(binding.name.name.clone(), val);
            }
            eval_expr(
                body,
                values,
                &block_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }
        ExprKind::FieldAccess { expr: inner, field } => {
            let inner_val = eval_expr(
                inner,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            match inner_val {
                RuntimeValue::Struct { fields, .. } => fields
                    .get(field.value.as_str())
                    .cloned()
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("no field `{}` on struct", field.value),
                        src: src.clone(),
                        span: field.span.into(),
                    }),
                _ => Err(GraphcalError::EvalError {
                    message: "field access on non-struct value".to_string(),
                    src: src.clone(),
                    span: inner.span.into(),
                }),
            }
        }
        ExprKind::StructConstruction {
            type_name, fields, ..
        } => {
            let mut field_map = IndexMap::new();
            for field_init in fields {
                let val = if let Some(value_expr) = &field_init.value {
                    eval_expr(
                        value_expr,
                        values,
                        local_values,
                        builtin_consts,
                        builtin_fns,
                        registry,
                        src,
                    )?
                } else {
                    // Shorthand: look up name in local scope, then graph scope
                    local_values
                        .get(field_init.name.value.as_str())
                        .or_else(|| values.get(field_init.name.value.as_str()))
                        .cloned()
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!(
                                "undefined variable `{}` for shorthand field",
                                field_init.name.value
                            ),
                            src: src.clone(),
                            span: field_init.name.span.into(),
                        })?
                };
                field_map.insert(field_init.name.value.clone(), val);
            }
            // Resolve owning type and variant names
            let (owning_type, variant_name) =
                if registry.get_type(type_name.value.as_str()).is_some() {
                    // Single-variant: type_name == variant_name
                    (
                        type_name.value.clone(),
                        VariantName::new(type_name.value.as_str()),
                    )
                } else if let Some((type_def, _)) =
                    registry.get_type_by_variant(type_name.value.as_str())
                {
                    (
                        type_def.name.clone(),
                        VariantName::new(type_name.value.as_str()),
                    )
                } else {
                    return Err(GraphcalError::EvalError {
                        message: format!("unknown type or variant `{}`", type_name.value),
                        src: src.clone(),
                        span: type_name.span.into(),
                    });
                };
            Ok(RuntimeValue::Struct {
                type_name: owning_type,
                variant: variant_name,
                fields: field_map,
            })
        }

        ExprKind::MapLiteral { entries } => {
            let idx_name = entries[0].index.value.clone();
            let mut result = IndexMap::new();
            for entry in entries {
                let val = eval_expr(
                    &entry.value,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                result.insert(entry.variant.value.clone(), val);
            }
            Ok(RuntimeValue::Indexed {
                index_name: idx_name,
                entries: result,
            })
        }

        ExprKind::ForComp { bindings, body } => {
            // Evaluate body for each combination of index variants
            eval_for_comp(
                bindings,
                body,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }

        ExprKind::IndexAccess { expr: inner, args } => {
            let mut current = eval_expr(
                inner,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            for arg in args {
                let RuntimeValue::Indexed { entries, .. } = current else {
                    return Err(GraphcalError::EvalError {
                        message: "indexing a non-indexed value".to_string(),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                };
                let variant_name: VariantName = match arg {
                    graphcal_syntax::ast::IndexArg::Variant { variant, .. } => {
                        variant.value.clone()
                    }
                    graphcal_syntax::ast::IndexArg::Var(ident) => {
                        let var_val = local_values.get(&ident.name).ok_or_else(|| {
                            GraphcalError::EvalError {
                                message: format!("undefined loop variable `{}`", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            }
                        })?;
                        let RuntimeValue::VariantLabel { variant, .. } = var_val else {
                            return Err(GraphcalError::EvalError {
                                message: format!("`{}` is not a loop variable", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        };
                        variant.clone()
                    }
                };
                current = entries.get(variant_name.as_str()).cloned().ok_or_else(|| {
                    GraphcalError::EvalError {
                        message: format!("variant `{variant_name}` not found"),
                        src: src.clone(),
                        span: expr.span.into(),
                    }
                })?;
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
            let source_val = eval_expr(
                source,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            let RuntimeValue::Indexed {
                index_name,
                entries: source_entries,
            } = source_val
            else {
                return Err(GraphcalError::EvalError {
                    message: "scan source must be an indexed value".to_string(),
                    src: src.clone(),
                    span: source.span.into(),
                });
            };
            let init_val = eval_expr(
                init,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;

            let mut acc = init_val;
            let mut result_entries = IndexMap::new();
            for (variant, val) in &source_entries {
                let mut scan_locals = local_values.clone();
                scan_locals.insert(acc_name.name.clone(), acc);
                scan_locals.insert(val_name.name.clone(), val.clone());
                let body_val = eval_expr(
                    body,
                    values,
                    &scan_locals,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                result_entries.insert(variant.clone(), body_val.clone());
                acc = body_val;
            }
            Ok(RuntimeValue::Indexed {
                index_name,
                entries: result_entries,
            })
        }

        ExprKind::Match { scrutinee, arms } => {
            let scrutinee_val = eval_expr(
                scrutinee,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            let RuntimeValue::Struct {
                variant,
                fields: scrutinee_fields,
                ..
            } = &scrutinee_val
            else {
                return Err(GraphcalError::EvalError {
                    message: "match scrutinee must be a struct/tagged union value".to_string(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                });
            };

            // Find the matching arm
            let matched_arm = arms
                .iter()
                .find(|arm| arm.pattern.variant_name.value.as_str() == variant.as_str())
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("no match arm for variant `{variant}`"),
                    src: src.clone(),
                    span: expr.span.into(),
                })?;

            // Bind pattern variables
            let mut arm_locals = local_values.clone();
            for binding in &matched_arm.pattern.bindings {
                match binding {
                    graphcal_syntax::ast::PatternBinding::Bind { field, var } => {
                        let field_val =
                            scrutinee_fields.get(field.value.as_str()).ok_or_else(|| {
                                GraphcalError::EvalError {
                                    message: format!(
                                        "no field `{}` on variant `{variant}`",
                                        field.value
                                    ),
                                    src: src.clone(),
                                    span: field.span.into(),
                                }
                            })?;
                        arm_locals.insert(var.name.clone(), field_val.clone());
                    }
                    graphcal_syntax::ast::PatternBinding::Wildcard { .. } => {}
                }
            }

            // Evaluate the matched arm body
            eval_expr(
                &matched_arm.body,
                values,
                &arm_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }
    }
}

/// Evaluate a `for` comprehension by iterating over index variants.
///
/// For single binding `for m: Maneuver { body }`, iterates over Maneuver variants
/// and collects results into `Indexed`.
/// For multi-binding, produces nested `Indexed` values.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through evaluation context to recursive calls"
)]
fn eval_for_comp(
    bindings: &[graphcal_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let idx_name = binding.index.value.clone();
    let idx_def = registry
        .get_index(idx_name.as_str())
        .expect("index validated by dim_check");
    let remaining = &bindings[1..];

    let mut entries = IndexMap::new();
    for variant in &idx_def.variants {
        let mut inner_locals = local_values.clone();
        inner_locals.insert(
            binding.var.name.clone(),
            RuntimeValue::VariantLabel {
                variant: variant.clone(),
            },
        );
        let val = if remaining.is_empty() {
            eval_expr(
                body,
                values,
                &inner_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
        } else {
            eval_for_comp(
                remaining,
                body,
                values,
                &inner_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
        };
        entries.insert(variant.clone(), val);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries,
    })
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
fn check_finite(
    value: f64,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, GraphcalError> {
    if value.is_nan() {
        Err(GraphcalError::EvalError {
            message: format!("invalid argument for {context} (result is NaN)"),
            src: src.clone(),
            span: span.into(),
        })
    } else if value.is_infinite() {
        Err(GraphcalError::EvalError {
            message: format!("{context} produced infinite result"),
            src: src.clone(),
            span: span.into(),
        })
    } else {
        Ok(value)
    }
}

/// Evaluate a comparison operator on two f64 values.
#[expect(clippy::float_cmp, reason = "DSL equality uses exact comparison")]
fn eval_comparison(op: BinOp, l: f64, r: f64) -> bool {
    match op {
        BinOp::Eq => l == r,
        BinOp::Ne => l != r,
        BinOp::Lt => l < r,
        BinOp::Gt => l > r,
        BinOp::Le => l <= r,
        BinOp::Ge => l >= r,
        _ => panic!("eval_comparison called with non-comparison operator"),
    }
}

/// Evaluate an arithmetic binary operator on two i64 values with checked arithmetic.
fn eval_int_binop(
    op: BinOp,
    l: i64,
    r: i64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<i64, GraphcalError> {
    match op {
        BinOp::Add => l.checked_add(r),
        BinOp::Sub => l.checked_sub(r),
        BinOp::Mul => l.checked_mul(r),
        BinOp::Div => {
            if r == 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l.checked_div(r)
        }
        BinOp::Mod => {
            if r == 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer modulo by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l.checked_rem(r)
        }
        BinOp::Pow => {
            if r < 0 {
                return Err(GraphcalError::EvalError {
                    message: "integer exponent must be non-negative".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let exp = u32::try_from(r).map_err(|_| GraphcalError::EvalError {
                message: "integer exponent too large".to_string(),
                src: src.clone(),
                span: span.into(),
            })?;
            l.checked_pow(exp)
        }
        _ => panic!("eval_int_binop called with non-arithmetic operator"),
    }
    .ok_or_else(|| GraphcalError::EvalError {
        message: "integer arithmetic overflow".to_string(),
        src: src.clone(),
        span: span.into(),
    })
}

/// Evaluate an arithmetic binary operator on two f64 values.
fn eval_binop(
    op: BinOp,
    l: f64,
    r: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, GraphcalError> {
    let result = match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => {
            if r == 0.0 {
                return Err(GraphcalError::EvalError {
                    message: "division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l / r
        }
        BinOp::Pow => l.powf(r),
        _ => panic!("eval_binop called with non-arithmetic operator"),
    };

    // Post-check: if inputs were finite but result is not, report an error.
    if l.is_finite() && r.is_finite() && !result.is_finite() {
        if result.is_nan() {
            Err(GraphcalError::EvalError {
                message: "invalid arithmetic operation (result is NaN)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        } else {
            Err(GraphcalError::EvalError {
                message: "arithmetic overflow (result is infinite)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        }
    } else {
        Ok(result)
    }
}
