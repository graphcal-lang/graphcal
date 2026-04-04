use std::collections::HashMap;

use graphcal_compiler::syntax::ast::{Expr, LetBinding, MatchArm};

use crate::error::GraphcalError;
use crate::runtime_value::RuntimeValue;

use super::EvalContext;
use super::eval_expr;

/// Evaluate an `if` expression.
pub(super) fn eval_if(
    expr: &Expr,
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let cond = eval_expr(condition, values, local_values, ctx)?
        .expect_bool("if condition")
        .map_err(|e| GraphcalError::EvalError {
            message: e.to_string(),
            src: ctx.src.clone(),
            span: expr.span.into(),
        })?;
    if cond {
        eval_expr(then_branch, values, local_values, ctx)
    } else {
        eval_expr(else_branch, values, local_values, ctx)
    }
}

/// Evaluate a block expression (`{ let x = ...; ... expr }`).
pub(super) fn eval_block(
    stmts: &[LetBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let mut block_locals = local_values.clone();
    for binding in stmts {
        let val = eval_expr(&binding.value, values, &block_locals, ctx)?;
        block_locals.insert(binding.name.name.clone(), val);
    }
    eval_expr(body, values, &block_locals, ctx)
}

/// Evaluate a `match` expression.
pub(super) fn eval_match(
    expr: &Expr,
    scrutinee: &Expr,
    arms: &[MatchArm],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let scrutinee_val = eval_expr(scrutinee, values, local_values, ctx)?;

    match &scrutinee_val {
        RuntimeValue::Label { variant, .. } => {
            // Label match (index label pattern matching)
            let matched_arm = arms
                .iter()
                .find(|arm| arm.pattern.variant_name.value.as_str() == variant.as_str())
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("no match arm for label `{variant}`"),
                    src: ctx.src.clone(),
                    span: expr.span.into(),
                })?;

            // Labels have no fields -- no bindings to process
            eval_expr(&matched_arm.body, values, local_values, ctx)
        }
        RuntimeValue::Struct {
            type_name,
            fields: scrutinee_fields,
        } => {
            // Tagged union match — type_name is the concrete variant type name
            let matched_arm = arms
                .iter()
                .find(|arm| arm.pattern.variant_name.value.as_str() == type_name.as_str())
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("no match arm for variant `{type_name}`"),
                    src: ctx.src.clone(),
                    span: expr.span.into(),
                })?;

            // Bind pattern variables
            let mut arm_locals = local_values.clone();
            for binding in &matched_arm.pattern.bindings {
                match binding {
                    graphcal_compiler::syntax::ast::PatternBinding::Bind { field, var } => {
                        let field_val =
                            scrutinee_fields.get(field.value.as_str()).ok_or_else(|| {
                                GraphcalError::EvalError {
                                    message: format!(
                                        "no field `{}` on type `{type_name}`",
                                        field.value
                                    ),
                                    src: ctx.src.clone(),
                                    span: field.span.into(),
                                }
                            })?;
                        arm_locals.insert(var.name.clone(), field_val.clone());
                    }
                    graphcal_compiler::syntax::ast::PatternBinding::Wildcard { .. } => {}
                }
            }

            eval_expr(&matched_arm.body, values, &arm_locals, ctx)
        }
        _ => Err(GraphcalError::EvalError {
            message: "match scrutinee must be a label or tagged union".to_string(),
            src: ctx.src.clone(),
            span: scrutinee.span.into(),
        }),
    }
}
