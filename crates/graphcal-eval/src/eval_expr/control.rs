use std::collections::HashMap;

use graphcal_compiler::desugar::resolved_ast::{Expr, MatchArm, MatchPattern};
use graphcal_compiler::registry::declared_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::syntax::names::ResolvedIndexVariant;
use graphcal_compiler::tir::typed::{
    ResolvedConstructorPattern, ResolvedConstructorTarget, ResolvedPatternBinding,
};

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use super::EvalContext;
use super::RuntimeValueMap;
use super::eval_expr;
use super::index_ref_from_path;
use super::index_ref_matches_resolved_or_leaf;

fn resolved_match_label_variant<'a>(
    ctx: &'a EvalContext<'_>,
    pattern: &MatchPattern,
) -> Option<&'a ResolvedIndexVariant> {
    let refs = ctx
        .current_dag
        .and_then(|dag| dag.resolved_collection_refs.as_ref())?;
    refs.match_label_variants
        .get(&pattern.span())
        .or_else(|| match pattern {
            MatchPattern::IndexLabel { index, variant, .. } => refs
                .match_label_variants
                .get(&index.span.merge(variant.span)),
            MatchPattern::Path { path, .. } => refs.match_label_variants.get(&path.span()),
            MatchPattern::Constructor { .. } => None,
        })
}

fn label_pattern_matches(
    ctx: &EvalContext<'_>,
    pattern: &MatchPattern,
    scrutinee_index: &IndexTypeRef,
    scrutinee_variant: &graphcal_compiler::syntax::names::IndexVariantName,
) -> bool {
    if let Some(resolved) = resolved_match_label_variant(ctx, pattern) {
        return index_ref_matches_resolved_or_leaf(scrutinee_index, resolved.index())
            && resolved.variant() == scrutinee_variant;
    }

    match pattern {
        MatchPattern::IndexLabel { index, variant, .. } => {
            index_ref_from_path(ctx, &index.value).matches_ref(scrutinee_index)
                && variant.value == *scrutinee_variant
        }
        MatchPattern::Constructor { .. } | MatchPattern::Path { .. } => false,
    }
}

fn resolved_match_constructor_pattern<'a>(
    ctx: &'a EvalContext<'_>,
    pattern: &MatchPattern,
) -> Option<&'a ResolvedConstructorPattern> {
    let refs = ctx
        .current_dag
        .and_then(|dag| dag.resolved_constructor_refs.as_ref())?;
    refs.match_pattern_constructors
        .get(&pattern.span())
        .or_else(|| match pattern {
            MatchPattern::Constructor { name, .. } => {
                refs.match_pattern_constructors.get(&name.span)
            }
            MatchPattern::Path { path, .. } => refs.match_pattern_constructors.get(&path.span()),
            MatchPattern::IndexLabel { .. } => None,
        })
}

fn runtime_struct_matches_resolved_constructor(
    scrutinee_type: &StructTypeRef,
    target: &ResolvedConstructorTarget,
) -> bool {
    scrutinee_type.name().as_str() == target.variant.name.as_str()
        && scrutinee_type.resolved() == &target.owning_type
}

fn constructor_pattern_matches(
    ctx: &EvalContext<'_>,
    pattern: &MatchPattern,
    scrutinee_type: &StructTypeRef,
) -> bool {
    if let Some(resolved) = resolved_match_constructor_pattern(ctx, pattern) {
        return runtime_struct_matches_resolved_constructor(scrutinee_type, &resolved.target);
    }

    match pattern {
        MatchPattern::Constructor { name, .. } => name.value.as_str() == scrutinee_type.as_str(),
        MatchPattern::IndexLabel { .. } | MatchPattern::Path { .. } => false,
    }
}

fn bind_resolved_constructor_pattern(
    pattern: &ResolvedConstructorPattern,
    scrutinee_fields: &indexmap::IndexMap<
        graphcal_compiler::syntax::names::FieldName,
        RuntimeValue,
    >,
    scrutinee_type: &StructTypeRef,
    arm_locals: &mut HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    for binding in &pattern.bindings {
        match binding {
            ResolvedPatternBinding::Bind { field, local } => {
                let field_val = scrutinee_fields.get(field.value.as_str()).ok_or_else(|| {
                    ctx.eval_error(
                        format!("no field `{}` on type `{scrutinee_type}`", field.value),
                        field.span,
                    )
                })?;
                arm_locals.insert(local.name.as_str().to_string(), field_val.clone());
            }
            ResolvedPatternBinding::Wildcard { .. } => {}
        }
    }
    Ok(())
}

fn bind_syntax_constructor_pattern(
    pattern: &MatchPattern,
    scrutinee_fields: &indexmap::IndexMap<
        graphcal_compiler::syntax::names::FieldName,
        RuntimeValue,
    >,
    scrutinee_type: &StructTypeRef,
    arm_locals: &mut HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    for binding in pattern.bindings() {
        match binding {
            graphcal_compiler::desugar::resolved_ast::PatternBinding::Bind { field, var } => {
                let field_val = scrutinee_fields.get(field.value.as_str()).ok_or_else(|| {
                    ctx.eval_error(
                        format!("no field `{}` on type `{scrutinee_type}`", field.value),
                        field.span,
                    )
                })?;
                arm_locals.insert(var.name.to_string(), field_val.clone());
            }
            graphcal_compiler::desugar::resolved_ast::PatternBinding::Wildcard { .. } => {}
        }
    }
    Ok(())
}

/// Evaluate an `if` expression.
pub(super) fn eval_if(
    expr: &Expr,
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let cond = eval_expr(condition, values, local_values, ctx)?
        .expect_bool("if condition")
        .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
    if cond {
        eval_expr(then_branch, values, local_values, ctx)
    } else {
        eval_expr(else_branch, values, local_values, ctx)
    }
}

/// Evaluate a `match` expression.
pub(super) fn eval_match(
    expr: &Expr,
    scrutinee: &Expr,
    arms: &[MatchArm],
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let scrutinee_val = eval_expr(scrutinee, values, local_values, ctx)?;

    match &scrutinee_val {
        RuntimeValue::Label {
            index_name,
            variant,
        } => {
            // Label match (index label pattern matching)
            let matched_arm = arms
                .iter()
                .find(|arm| label_pattern_matches(ctx, &arm.pattern, index_name, variant))
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for label `{variant}`"), expr.span)
                })?;

            // Labels have no fields -- no bindings to process
            eval_expr(&matched_arm.body, values, local_values, ctx)
        }
        RuntimeValue::Struct {
            type_name,
            fields: scrutinee_fields,
        } => {
            // Tagged union match — type_name keeps the concrete constructor leaf
            // plus, for module-aware values, the owning union's canonical identity.
            let matched_arm = arms
                .iter()
                .find(|arm| constructor_pattern_matches(ctx, &arm.pattern, type_name))
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for variant `{type_name}`"), expr.span)
                })?;

            // Bind pattern variables. Qualified `MatchPattern::Path` arms are
            // selected and bound through the HIR-derived constructor sidecar;
            // standalone/bare constructor arms keep the source syntax bindings.
            let mut arm_locals = local_values.clone();
            if let Some(resolved) = resolved_match_constructor_pattern(ctx, &matched_arm.pattern) {
                bind_resolved_constructor_pattern(
                    resolved,
                    scrutinee_fields,
                    type_name,
                    &mut arm_locals,
                    ctx,
                )?;
            } else {
                bind_syntax_constructor_pattern(
                    &matched_arm.pattern,
                    scrutinee_fields,
                    type_name,
                    &mut arm_locals,
                    ctx,
                )?;
            }

            eval_expr(&matched_arm.body, values, &arm_locals, ctx)
        }
        _ => Err(ctx.eval_error(
            "match scrutinee must be a label or tagged union",
            scrutinee.span,
        )),
    }
}
