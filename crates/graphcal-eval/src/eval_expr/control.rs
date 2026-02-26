use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{Expr, LetBinding, MatchArm};

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;
use crate::runtime_value::RuntimeValue;

use super::eval_expr;

/// Evaluate an `if` expression.
pub(super) fn eval_if(
    expr: &Expr,
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let cond = eval_expr(
        condition,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?
    .expect_bool("if condition")
    .map_err(|msg| GraphcalError::EvalError {
        message: msg,
        src: src.clone(),
        span: expr.span.into(),
    })?;
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

/// Evaluate a block expression (`{ let x = ...; ... expr }`).
pub(super) fn eval_block(
    stmts: &[LetBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
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

/// Evaluate a `match` expression.
pub(super) fn eval_match(
    expr: &Expr,
    scrutinee: &Expr,
    arms: &[MatchArm],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let scrutinee_val = eval_expr(
        scrutinee,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;

    match &scrutinee_val {
        RuntimeValue::Label { variant, .. } => {
            // Label match (index label pattern matching)
            let matched_arm = arms
                .iter()
                .find(|arm| arm.pattern.variant_name.value.as_str() == variant.as_str())
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("no match arm for label `{variant}`"),
                    src: src.clone(),
                    span: expr.span.into(),
                })?;

            // Labels have no fields -- no bindings to process
            eval_expr(
                &matched_arm.body,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        }
        RuntimeValue::Struct {
            variant,
            fields: scrutinee_fields,
            ..
        } => {
            // Tagged union match
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
        _ => Err(GraphcalError::EvalError {
            message: "match scrutinee must be a label or tagged union".to_string(),
            src: src.clone(),
            span: scrutinee.span.into(),
        }),
    }
}
