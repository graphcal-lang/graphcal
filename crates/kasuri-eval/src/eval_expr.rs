use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::{BinOp, Expr, ExprKind, UnaryOp};

use crate::builtins::BuiltinFunction;
use crate::error::KasuriError;

/// Evaluate an expression given a set of resolved values and built-in functions.
/// Used by both the const evaluator and the runtime evaluator.
///
/// # Errors
///
/// Returns a [`KasuriError`] if the expression references an undefined variable,
/// constant, or function.
#[expect(clippy::implicit_hasher)] // Internal function always uses default HashMap
#[expect(clippy::if_not_else)] // `!= 0.0` reads more naturally for DSL truthiness
pub fn eval_expr(
    expr: &Expr,
    values: &HashMap<String, f64>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<f64, KasuriError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(*n),
        ExprKind::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        ExprKind::GraphRef(ident) => {
            values
                .get(ident.name.as_str())
                .copied()
                .ok_or_else(|| KasuriError::EvalError {
                    message: format!("undefined graph reference `@{}`", ident.name),
                    src: src.clone(),
                    span: expr.span.into(),
                })
        }
        ExprKind::ConstRef(ident) => values
            .get(ident.name.as_str())
            .or_else(|| builtin_consts.get(ident.name.as_str()))
            .copied()
            .ok_or_else(|| KasuriError::EvalError {
                message: format!("undefined constant `{}`", ident.name),
                src: src.clone(),
                span: expr.span.into(),
            }),
        ExprKind::BinOp { op, lhs, rhs } => {
            let l = eval_expr(lhs, values, builtin_consts, builtin_fns, src)?;
            let r = eval_expr(rhs, values, builtin_consts, builtin_fns, src)?;
            Ok(eval_binop(*op, l, r))
        }
        ExprKind::UnaryOp { op, operand } => {
            let v = eval_expr(operand, values, builtin_consts, builtin_fns, src)?;
            Ok(match op {
                UnaryOp::Neg => -v,
                UnaryOp::Not => {
                    if v == 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            })
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
                .map(|a| eval_expr(a, values, builtin_consts, builtin_fns, src))
                .collect::<Result<_, _>>()?;
            Ok((builtin.eval)(&arg_values))
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = eval_expr(condition, values, builtin_consts, builtin_fns, src)?;
            if cond != 0.0 {
                eval_expr(then_branch, values, builtin_consts, builtin_fns, src)
            } else {
                eval_expr(else_branch, values, builtin_consts, builtin_fns, src)
            }
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
