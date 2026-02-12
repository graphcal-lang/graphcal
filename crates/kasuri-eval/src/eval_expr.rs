use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use kasuri_syntax::ast::{BinOp, Expr, ExprKind, UnaryOp};

use crate::builtins::BuiltinFunction;
use crate::error::KasuriError;
use crate::registry::Registry;

/// A runtime value: either a scalar (f64 in SI units) or a struct with named fields.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Scalar(f64),
    Struct {
        type_name: String,
        fields: IndexMap<String, Self>,
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
#[expect(clippy::implicit_hasher)] // Internal function always uses default HashMap
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
            Ok(RuntimeValue::Scalar(eval_binop(*op, l, r)))
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
            let builtin =
                builtin_fns
                    .get(name.name.as_str())
                    .ok_or_else(|| KasuriError::EvalError {
                        message: format!("unknown function `{}`", name.name),
                        src: src.clone(),
                        span: name.span.into(),
                    })?;
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
            Ok(RuntimeValue::Scalar((builtin.eval)(&arg_values)))
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
                RuntimeValue::Scalar(_) => Err(KasuriError::EvalError {
                    message: "field access on scalar value".to_string(),
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
    }
}

#[expect(clippy::float_cmp)] // Intentional: DSL equality/truthiness uses exact comparison
#[expect(clippy::if_not_else)] // `!= r` reads naturally for BinOp::Ne
fn eval_binop(op: BinOp, l: f64, r: f64) -> f64 {
    match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => l / r,
        BinOp::Pow => l.powf(r),
        BinOp::Eq => {
            if l == r {
                1.0
            } else {
                0.0
            }
        }
        BinOp::Ne => {
            if l != r {
                1.0
            } else {
                0.0
            }
        }
        BinOp::Lt => {
            if l < r {
                1.0
            } else {
                0.0
            }
        }
        BinOp::Gt => {
            if l > r {
                1.0
            } else {
                0.0
            }
        }
        BinOp::Le => {
            if l <= r {
                1.0
            } else {
                0.0
            }
        }
        BinOp::Ge => {
            if l >= r {
                1.0
            } else {
                0.0
            }
        }
        BinOp::And => {
            if l != 0.0 && r != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        BinOp::Or => {
            if l != 0.0 || r != 0.0 {
                1.0
            } else {
                0.0
            }
        }
    }
}
