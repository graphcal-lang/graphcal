#[expect(
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref,
    reason = "evaluation functions pass compilation context through many parameters"
)]
mod arithmetic;
mod collections;
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation functions pass compilation context through many parameters"
)]
mod control;
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation functions pass compilation context through many parameters"
)]
mod functions;

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_syntax::ast::{Expr, ExprKind};
use graphcal_syntax::names::VariantName;

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;

pub use crate::runtime_value::RuntimeValue;

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
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(RuntimeValue::Scalar(*n)),
        ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        ExprKind::StringLiteral(_) => Err(GraphcalError::EvalError {
            message: "unexpected string literal in evaluation context".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
        ExprKind::UnitLiteral { value, unit } => {
            let (_dim, scale) =
                registry
                    .units
                    .resolve_unit_expr(unit)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: "unknown unit in literal".to_string(),
                        src: src.clone(),
                        span: unit.span.into(),
                    })?;
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
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!("undefined graph reference `@{}`", ident.value),
                src: src.clone(),
                span: expr.span.into(),
            }),
        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => values
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

        // --- Arithmetic (delegated) ---
        ExprKind::BinOp { op, lhs, rhs } => arithmetic::eval_binop_expr(
            expr,
            op,
            lhs,
            rhs,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        ExprKind::UnaryOp { op, operand } => arithmetic::eval_unaryop_expr(
            expr,
            op,
            operand,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),

        // --- Function calls (delegated) ---
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            functions::eval_fn_call(
                expr,
                name,
                args,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }

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
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        ExprKind::Block { stmts, expr: body } => control::eval_block(
            stmts,
            body,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        ExprKind::Match { scrutinee, arms } => control::eval_match(
            expr,
            scrutinee,
            arms,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),

        // --- Collections (delegated) ---
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            collections::eval_map_or_table(
                entries,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }
        ExprKind::ForComp { bindings, body } => collections::eval_for_comp_expr(
            bindings,
            body,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        ExprKind::IndexAccess { expr: inner, args } => collections::eval_index_access(
            expr,
            inner,
            args,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
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
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),

        // --- Passthrough (unit/display/cast annotations are handled at the type level) ---
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => eval_expr(
            inner,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),

        // --- Field access ---
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

        // --- Struct construction ---
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
                if registry.types.get_type(type_name.value.as_str()).is_some() {
                    // Single-variant: type_name == variant_name
                    (
                        type_name.value.clone(),
                        VariantName::new(type_name.value.as_str()),
                    )
                } else if let Some((type_def, _)) =
                    registry.types.get_type_by_variant(type_name.value.as_str())
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

        // --- Unfold (must be evaluated at a higher level) ---
        ExprKind::Unfold { .. } => {
            // Unfold is evaluated at a higher level (evaluate_plan in eval.rs)
            // because it needs to insert partial results into the values map
            // for self-referencing via @node_name[prev_i].
            Err(GraphcalError::EvalError {
                message: "Unfold must be evaluated by evaluate_plan, not eval_expr".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            })
        }
    }
}
