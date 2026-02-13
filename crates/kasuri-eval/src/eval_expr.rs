use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use kasuri_syntax::ast::{BinOp, Expr, ExprKind, FnBody, UnaryOp};
use kasuri_syntax::span::Span;

use crate::builtins::BuiltinFunction;
use crate::error::KasuriError;
use crate::registry::Registry;

/// A runtime value: either a scalar (f64 in SI units), a struct, or an indexed collection.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Scalar(f64),
    Struct {
        type_name: String,
        fields: IndexMap<String, Self>,
    },
    /// An indexed collection: maps variant names to values, preserving declaration order.
    Indexed {
        index_name: String,
        entries: IndexMap<String, Self>,
    },
    /// A variant label during `for` comprehension iteration.
    /// Not a "real" value — only exists in `local_values` during loop body evaluation.
    VariantLabel {
        variant: String,
    },
}

impl RuntimeValue {
    /// Extract scalar value, panicking if this is a struct.
    /// (Struct-in-scalar-position is caught by `dim_check`, so this is safe at runtime.)
    fn expect_scalar(&self, context: &str) -> f64 {
        match self {
            Self::Scalar(v) => *v,
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
}

/// Evaluate an expression given a set of resolved values and built-in functions.
/// Returns a `RuntimeValue` (scalar or struct).
///
/// # Errors
///
/// Returns a [`KasuriError`] if the expression references an undefined variable,
/// constant, or function.
#[expect(clippy::if_not_else)] // `!= 0.0` reads more naturally for DSL truthiness
#[expect(clippy::too_many_lines)] // Single match over all ExprKind variants
pub fn eval_expr(
    expr: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, KasuriError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(RuntimeValue::Scalar(*n)),
        ExprKind::UnitLiteral { value, unit } => {
            let (_dim, scale) =
                registry
                    .resolve_unit_expr(unit)
                    .ok_or_else(|| KasuriError::EvalError {
                        message: "unknown unit in literal".to_string(),
                        src: src.clone(),
                        span: unit.span.into(),
                    })?;
            Ok(RuntimeValue::Scalar(*value * scale))
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Scalar(if *b { 1.0 } else { 0.0 })),
        ExprKind::GraphRef(ident) => {
            values
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| KasuriError::EvalError {
                    message: format!("undefined graph reference `@{}`", ident.name),
                    src: src.clone(),
                    span: expr.span.into(),
                })
        }
        ExprKind::ConstRef(ident) => values
            .get(ident.name.as_str())
            .cloned()
            .or_else(|| {
                builtin_consts
                    .get(ident.name.as_str())
                    .map(|v| RuntimeValue::Scalar(*v))
            })
            .ok_or_else(|| KasuriError::EvalError {
                message: format!("undefined constant `{}`", ident.name),
                src: src.clone(),
                span: expr.span.into(),
            }),
        ExprKind::LocalRef(ident) => {
            local_values
                .get(ident.name.as_str())
                .cloned()
                .ok_or_else(|| KasuriError::EvalError {
                    message: format!("undefined local variable `{}`", ident.name),
                    src: src.clone(),
                    span: expr.span.into(),
                })
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let l = eval_expr(
                lhs,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
            .expect_scalar("binary operand");
            let r = eval_expr(
                rhs,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
            .expect_scalar("binary operand");
            Ok(RuntimeValue::Scalar(eval_binop(
                *op, l, r, src, expr.span,
            )?))
        }
        ExprKind::UnaryOp { op, operand } => {
            let v = eval_expr(
                operand,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
            .expect_scalar("unary operand");
            Ok(RuntimeValue::Scalar(match op {
                UnaryOp::Neg => -v,
                UnaryOp::Not => {
                    if v == 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            }))
        }
        ExprKind::FnCall { name, args } => {
            // Aggregation functions over indexed values: sum, min, max, mean, count
            if matches!(name.name.as_str(), "sum" | "min" | "max" | "mean" | "count")
                && args.len() == 1
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
                    return Ok(match name.name.as_str() {
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
                            #[expect(clippy::cast_precision_loss)]
                            let n = entries.len() as f64;
                            let total: f64 = entries
                                .values()
                                .map(|v| v.expect_scalar("mean element"))
                                .sum();
                            RuntimeValue::Scalar(total / n)
                        }
                        "count" => {
                            #[expect(clippy::cast_precision_loss)]
                            let n = entries.len() as f64;
                            RuntimeValue::Scalar(n)
                        }
                        _ => unreachable!(),
                    });
                }
                // If not indexed, fall through to builtins (min/max are 2-arg builtins)
            }

            // Try builtin first
            if let Some(builtin) = builtin_fns.get(name.name.as_str()) {
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
                    &name.name,
                    src,
                    expr.span,
                )?));
            }

            // Try user-defined function
            let fn_def =
                registry
                    .get_function(&name.name)
                    .ok_or_else(|| KasuriError::EvalError {
                        message: format!("unknown function `{}`", name.name),
                        src: src.clone(),
                        span: name.span.into(),
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
            .expect_scalar("if condition");
            if cond != 0.0 {
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
                RuntimeValue::Struct { fields, .. } => {
                    fields
                        .get(&field.name)
                        .cloned()
                        .ok_or_else(|| KasuriError::EvalError {
                            message: format!("no field `{}` on struct", field.name),
                            src: src.clone(),
                            span: field.span.into(),
                        })
                }
                _ => Err(KasuriError::EvalError {
                    message: "field access on non-struct value".to_string(),
                    src: src.clone(),
                    span: inner.span.into(),
                }),
            }
        }
        ExprKind::StructConstruction { type_name, fields } => {
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
                        .get(&field_init.name.name)
                        .or_else(|| values.get(&field_init.name.name))
                        .cloned()
                        .ok_or_else(|| KasuriError::EvalError {
                            message: format!(
                                "undefined variable `{}` for shorthand field",
                                field_init.name.name
                            ),
                            src: src.clone(),
                            span: field_init.name.span.into(),
                        })?
                };
                field_map.insert(field_init.name.name.clone(), val);
            }
            Ok(RuntimeValue::Struct {
                type_name: type_name.name.clone(),
                fields: field_map,
            })
        }

        ExprKind::MapLiteral { entries } => {
            let idx_name = &entries[0].index.name;
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
                result.insert(entry.variant.name.clone(), val);
            }
            Ok(RuntimeValue::Indexed {
                index_name: idx_name.clone(),
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
                    return Err(KasuriError::EvalError {
                        message: "indexing a non-indexed value".to_string(),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                };
                let variant_name = match arg {
                    kasuri_syntax::ast::IndexArg::Variant { variant, .. } => variant.name.clone(),
                    kasuri_syntax::ast::IndexArg::Var(ident) => {
                        let var_val = local_values.get(&ident.name).ok_or_else(|| {
                            KasuriError::EvalError {
                                message: format!("undefined loop variable `{}`", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            }
                        })?;
                        let RuntimeValue::VariantLabel { variant, .. } = var_val else {
                            return Err(KasuriError::EvalError {
                                message: format!("`{}` is not a loop variable", ident.name),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        };
                        variant.clone()
                    }
                };
                current =
                    entries
                        .get(&variant_name)
                        .cloned()
                        .ok_or_else(|| KasuriError::EvalError {
                            message: format!("variant `{variant_name}` not found"),
                            src: src.clone(),
                            span: expr.span.into(),
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
                entries,
            } = source_val
            else {
                return Err(KasuriError::EvalError {
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
            for (variant, val) in &entries {
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
    }
}

/// Evaluate a `for` comprehension by iterating over index variants.
///
/// For single binding `for m: Maneuver { body }`, iterates over Maneuver variants
/// and collects results into `Indexed`.
/// For multi-binding, produces nested `Indexed` values.
#[expect(clippy::too_many_arguments)]
fn eval_for_comp(
    bindings: &[kasuri_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, KasuriError> {
    let binding = &bindings[0];
    let idx_name = &binding.index.name;
    let idx_def = registry
        .get_index(idx_name)
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
        index_name: idx_name.clone(),
        entries,
    })
}

/// Validate that a computed value is finite, returning an `EvalError` if it is NaN or infinite.
fn check_finite(
    value: f64,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, KasuriError> {
    if value.is_nan() {
        Err(KasuriError::EvalError {
            message: format!("invalid argument for {context} (result is NaN)"),
            src: src.clone(),
            span: span.into(),
        })
    } else if value.is_infinite() {
        Err(KasuriError::EvalError {
            message: format!("{context} produced infinite result"),
            src: src.clone(),
            span: span.into(),
        })
    } else {
        Ok(value)
    }
}

#[expect(clippy::float_cmp)] // Intentional: DSL equality/truthiness uses exact comparison
#[expect(clippy::if_not_else)] // `!= r` reads naturally for BinOp::Ne
fn eval_binop(
    op: BinOp,
    l: f64,
    r: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<f64, KasuriError> {
    let result = match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => {
            if r == 0.0 {
                return Err(KasuriError::EvalError {
                    message: "division by zero".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            l / r
        }
        BinOp::Pow => l.powf(r),
        // Comparison and boolean ops always return 0.0 or 1.0 — no check needed.
        BinOp::Eq => {
            return Ok(if l == r { 1.0 } else { 0.0 });
        }
        BinOp::Ne => {
            return Ok(if l != r { 1.0 } else { 0.0 });
        }
        BinOp::Lt => {
            return Ok(if l < r { 1.0 } else { 0.0 });
        }
        BinOp::Gt => {
            return Ok(if l > r { 1.0 } else { 0.0 });
        }
        BinOp::Le => {
            return Ok(if l <= r { 1.0 } else { 0.0 });
        }
        BinOp::Ge => {
            return Ok(if l >= r { 1.0 } else { 0.0 });
        }
        BinOp::And => {
            return Ok(if l != 0.0 && r != 0.0 { 1.0 } else { 0.0 });
        }
        BinOp::Or => {
            return Ok(if l != 0.0 || r != 0.0 { 1.0 } else { 0.0 });
        }
    };

    // Post-check: if inputs were finite but result is not, report an error.
    if l.is_finite() && r.is_finite() && !result.is_finite() {
        if result.is_nan() {
            Err(KasuriError::EvalError {
                message: "invalid arithmetic operation (result is NaN)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        } else {
            Err(KasuriError::EvalError {
                message: "arithmetic overflow (result is infinite)".to_string(),
                src: src.clone(),
                span: span.into(),
            })
        }
    } else {
        Ok(result)
    }
}
