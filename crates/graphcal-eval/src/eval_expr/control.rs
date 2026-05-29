use std::collections::HashMap;

use graphcal_compiler::desugar::resolved_ast::{Expr, MatchArm, MatchPattern};
use graphcal_compiler::registry::declared_type::IndexTypeRef;
use graphcal_compiler::syntax::names::{IndexName, NamePath, ResolvedIndexVariant, ScopedName};

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use super::EvalContext;
use super::eval_expr;
use super::index_ref_matches_resolved_or_legacy;

fn legacy_index_ref_from_path(path: &NamePath) -> IndexTypeRef {
    IndexTypeRef::legacy(IndexName::from(path.leaf().clone()))
}

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
        return index_ref_matches_resolved_or_legacy(scrutinee_index, resolved.index())
            && resolved.variant() == scrutinee_variant;
    }

    match pattern {
        MatchPattern::IndexLabel { index, variant, .. } => {
            legacy_index_ref_from_path(&index.value).matches_ref(scrutinee_index)
                && variant.value == *scrutinee_variant
        }
        MatchPattern::Constructor { .. } | MatchPattern::Path { .. } => false,
    }
}

/// Evaluate an `if` expression.
pub(super) fn eval_if(
    expr: &Expr,
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    values: &HashMap<ScopedName, RuntimeValue>,
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
    values: &HashMap<ScopedName, RuntimeValue>,
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
            // Tagged union match — type_name is the concrete variant type name
            let matched_arm = arms
                .iter()
                .find(|arm| match &arm.pattern {
                    MatchPattern::Constructor { name, .. } => {
                        name.value.as_str() == type_name.as_str()
                    }
                    MatchPattern::IndexLabel { .. } | MatchPattern::Path { .. } => false,
                })
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for variant `{type_name}`"), expr.span)
                })?;

            let MatchPattern::Constructor { bindings, .. } = &matched_arm.pattern else {
                return Err(ctx.eval_error(
                    "internal: selected non-constructor arm for struct match",
                    matched_arm.span,
                ));
            };

            // Bind pattern variables
            let mut arm_locals = local_values.clone();
            for binding in bindings {
                match binding {
                    graphcal_compiler::desugar::resolved_ast::PatternBinding::Bind {
                        field,
                        var,
                    } => {
                        let field_val =
                            scrutinee_fields.get(field.value.as_str()).ok_or_else(|| {
                                ctx.eval_error(
                                    format!("no field `{}` on type `{type_name}`", field.value),
                                    field.span,
                                )
                            })?;
                        arm_locals.insert(var.name.to_string(), field_val.clone());
                    }
                    graphcal_compiler::desugar::resolved_ast::PatternBinding::Wildcard {
                        ..
                    } => {}
                }
            }

            eval_expr(&matched_arm.body, values, &arm_locals, ctx)
        }
        _ => Err(ctx.eval_error(
            "match scrutinee must be a label or tagged union",
            scrutinee.span,
        )),
    }
}
