#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "evaluation functions pass compilation context through EvalContext"
)]
mod arithmetic;
mod collections;
mod control;
mod functions;

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::syntax::ast::{Expr, ExprKind, MulDivOp, UnitExpr};

use crate::builtins::BuiltinFunction;
use crate::declared_type::DeclaredType;
use crate::error::GraphcalError;
use crate::registry::{Registry, UnitScale};

pub use crate::runtime_value::RuntimeValue;

/// Immutable evaluation environment shared across all expression evaluations.
///
/// Bundles built-in constants, built-in functions, the type/unit registry,
/// and source information for diagnostics, plus optional unfold context
/// for evaluating `unfold(...)` expressions inline.
pub struct EvalContext<'a> {
    pub builtin_consts: &'a HashMap<&'a str, f64>,
    pub builtin_fns: &'a HashMap<&'a str, BuiltinFunction>,
    pub registry: &'a Registry,
    pub src: &'a NamedSource<Arc<String>>,
    /// When set, enables inline evaluation of `ExprKind::Unfold` expressions.
    /// Contains the name of the node being evaluated and the declared types map.
    pub unfold_context: Option<UnfoldContext<'a>>,
}

/// Context required to evaluate an `unfold(...)` expression inline.
///
/// Provides the self-referencing node name and declared types needed
/// to look up the range index for iterative evaluation.
pub struct UnfoldContext<'a> {
    pub self_name: &'a str,
    pub declared_types: &'a HashMap<String, DeclaredType>,
}

impl EvalContext<'_> {
    /// Build a `GraphcalError::EvalError` using this context's source.
    pub fn eval_error(&self, message: impl Into<String>, span: graphcal_compiler::syntax::span::Span) -> GraphcalError {
        GraphcalError::EvalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }

    /// Build a `GraphcalError::InternalError` using this context's source.
    pub fn internal_error(&self, message: impl Into<String>, span: graphcal_compiler::syntax::span::Span) -> GraphcalError {
        GraphcalError::InternalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
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
#[expect(clippy::too_many_lines, reason = "large match on ExprKind variants")]
pub fn eval_expr(
    expr: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(RuntimeValue::Scalar(*n)),
        ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        ExprKind::StringLiteral(_) => Err(ctx.eval_error(
            "unexpected string literal in evaluation context",
            expr.span,
        )),
        ExprKind::UnitLiteral { value, unit } => {
            let scale = resolve_unit_scale(unit, values, local_values, ctx)?;
            Ok(RuntimeValue::Scalar(*value * scale))
        }
        ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        ExprKind::VariantLiteral { index, variant } => Ok(RuntimeValue::Label {
            index_name: index.value.clone(),
            variant: variant.value.clone(),
        }),
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => values
            .get(ident.value.as_str())
            .cloned()
            .ok_or_else(|| {
                ctx.eval_error(format!("undefined graph reference `@{}`", ident.value), expr.span)
            }),
        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => values
            .get(ident.value.as_str())
            .cloned()
            .or_else(|| {
                ctx.builtin_consts
                    .get(ident.value.as_str())
                    .map(|v| RuntimeValue::Scalar(*v))
            })
            .or_else(|| {
                // Nat generic params are stored as local values (e.g., `N` from `N: Nat`)
                // and may be referenced in expression position as ConstRef (uppercase).
                local_values.get(ident.value.as_str()).cloned()
            })
            .ok_or_else(|| {
                ctx.eval_error(format!("undefined constant `{}`", ident.value), expr.span)
            }),
        ExprKind::LocalRef(ident) => local_values
            .get(ident.name.as_str())
            .cloned()
            .ok_or_else(|| {
                ctx.eval_error(format!("undefined local variable `{}`", ident.name), expr.span)
            }),

        // --- Arithmetic (delegated) ---
        ExprKind::BinOp { op, lhs, rhs } => {
            arithmetic::eval_binop_expr(expr, op, lhs, rhs, values, local_values, ctx)
        }
        ExprKind::UnaryOp { op, operand } => {
            arithmetic::eval_unaryop_expr(expr, op, operand, values, local_values, ctx)
        }

        // --- Function calls (delegated) ---
        ExprKind::FnCall {
            name,
            type_args,
            args,
        }
        | ExprKind::QualifiedFnCall {
            name,
            type_args,
            args,
            ..
        } => functions::eval_fn_call(expr, name, type_args, args, values, local_values, ctx),

        // --- Control flow (delegated) ---
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => control::eval_if(
            expr,
            condition,
            then_branch,
            else_branch,
            values,
            local_values,
            ctx,
        ),
        ExprKind::Block { stmts, expr: body } => {
            control::eval_block(stmts, body, values, local_values, ctx)
        }
        ExprKind::Match { scrutinee, arms } => {
            control::eval_match(expr, scrutinee, arms, values, local_values, ctx)
        }

        // --- Collections (delegated) ---
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            collections::eval_map_or_table(entries, values, local_values, ctx)
        }
        ExprKind::ForComp { bindings, body } => {
            collections::eval_for_comp_expr(bindings, body, values, local_values, ctx)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            collections::eval_index_access(expr, inner, args, values, local_values, ctx)
        }
        ExprKind::Scan {
            source,
            init,
            acc_name,
            val_name,
            body,
        } => collections::eval_scan(
            source,
            init,
            acc_name,
            val_name,
            body,
            values,
            local_values,
            ctx,
        ),

        // --- Passthrough (unit/display/cast annotations are handled at the type level) ---
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => eval_expr(inner, values, local_values, ctx),

        // --- Field access ---
        ExprKind::FieldAccess { expr: inner, field } => {
            let inner_val = eval_expr(inner, values, local_values, ctx)?;
            match inner_val {
                RuntimeValue::Struct { fields, .. } => fields
                    .get(field.value.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        ctx.eval_error(format!("no field `{}` on struct", field.value), field.span)
                    }),
                _ => Err(ctx.eval_error("field access on non-struct value", inner.span)),
            }
        }

        // --- Struct construction ---
        ExprKind::StructConstruction {
            type_name, fields, ..
        } => {
            let mut field_map = IndexMap::new();
            for field_init in fields {
                let val = if let Some(value_expr) = &field_init.value {
                    eval_expr(value_expr, values, local_values, ctx)?
                } else {
                    // Shorthand: look up name in local scope, then graph scope
                    local_values
                        .get(field_init.name.value.as_str())
                        .or_else(|| values.get(field_init.name.value.as_str()))
                        .cloned()
                        .ok_or_else(|| ctx.eval_error(
                            format!(
                                "undefined variable `{}` for shorthand field",
                                field_init.name.value
                            ),
                            field_init.name.span,
                        ))?
                };
                field_map.insert(field_init.name.value.clone(), val);
            }
            Ok(RuntimeValue::Struct {
                type_name: type_name.value.clone(),
                fields: field_map,
            })
        }

        // --- Unfold ---
        ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } => collections::eval_unfold(expr, init, prev_name, curr_name, body, values, ctx),

        // TupleMatch is desugared to If/BinOp(Eq) chains before evaluation.
        #[expect(clippy::unreachable, reason = "invariant: desugared before eval")]
        ExprKind::TupleMatch { .. } => unreachable!("TupleMatch should be desugared before eval"),
    }
}

/// Resolve a `UnitExpr` to its compound scale factor at runtime.
///
/// For static units, this is equivalent to `registry.units.resolve_unit_expr()`.
/// For dynamic units, the scale expression is evaluated using the current `values`
/// and `local_values` maps, then multiplied by the base unit's static scale.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a unit is unknown or a dynamic scale expression
/// fails to evaluate to a scalar.
pub fn resolve_unit_scale(
    unit: &UnitExpr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    let mut compound_scale = 1.0;
    for item in &unit.terms {
        let info = ctx
            .registry
            .units
            .get_unit(item.name.value.as_str())
            .ok_or_else(|| {
                ctx.eval_error(format!("unknown unit `{}`", item.name.value), item.name.span)
            })?;
        let unit_scale = match &info.scale {
            UnitScale::Static(s) => *s,
            UnitScale::Dynamic {
                scale_expr,
                base_unit_scale,
            } => {
                let scale_val = eval_expr(scale_expr, values, local_values, ctx)?;
                let RuntimeValue::Scalar(scale_f64) = scale_val else {
                    return Err(ctx.eval_error(
                        "dynamic unit scale expression must evaluate to a scalar",
                        scale_expr.span,
                    ));
                };
                scale_f64 * base_unit_scale
            }
        };
        let exp = item.power.unwrap_or(1);
        let powered_scale = unit_scale.powi(exp);
        match item.op {
            MulDivOp::Mul => compound_scale *= powered_scale,
            MulDivOp::Div => compound_scale /= powered_scale,
        }
    }
    Ok(compound_scale)
}
