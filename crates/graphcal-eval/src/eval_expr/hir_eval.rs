use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::builtin::{AggregationFn, BuiltinFnName};
use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::hir::{self, ConstRef, FunctionRef};
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::registry::types::{IndexDef, IndexKind};
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::model::DagTIR;
use indexmap::IndexMap;
use miette::NamedSource;

use crate::decl_key::RuntimeDeclKey;
use crate::presentation_evidence::{PresentationInstance, PresentationInstanceMap};
use crate::runtime_presentation::EvaluatedRuntimeValue;

use super::builtin_call::{
    DatetimeConstructorFn, DatetimeExtractFn, DatetimeFromFn, DatetimeToFn, EvalBuiltinRule,
    TypeConversionFn, eval_rule_for_builtin,
};
use super::{
    EvalContext, RuntimeValueMap, checked_finite_quantity, checked_unit_scaled_value,
    imported_binding_value, index_ref_matches_resolved, resolve_unit_scale,
};

pub type HirLocalValueMap<'a> = hir::LocalEnv<'a, EvaluatedRuntimeValue>;

type ResolvedDeclKey = graphcal_compiler::syntax::decl_name::ResolvedDeclName;

fn presentation_instance_for(
    presentation_values: Option<&PresentationInstanceMap>,
    key: &RuntimeDeclKey,
) -> PresentationInstance {
    presentation_values
        .and_then(|presentations| presentations.get(key))
        .map_or_else(PresentationInstance::default, Clone::clone)
}

#[expect(
    clippy::option_if_let_else,
    reason = "a missing sparse sidecar explicitly means that this value has no call identity"
)]
fn take_presentation_instance(
    presentation_values: &mut PresentationInstanceMap,
    key: &RuntimeDeclKey,
) -> PresentationInstance {
    match presentation_values.remove(key) {
        Some(presentation) => presentation,
        None => PresentationInstance::None,
    }
}

/// Evaluate an already-lowered HIR expression.
///
/// Module-aware TIR construction stores HIR for const/default/node expressions.
/// This evaluator consumes canonical declaration, constructor, index-variant,
/// inline-DAG, local, and built-in references directly instead of consulting
/// span-keyed syntax-AST metadata.
pub fn eval_hir_expr(
    expr: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_hir_expr_evaluated(expr, values, None, local_values, ctx)
        .map(EvaluatedRuntimeValue::into_value)
}

/// Evaluate one HIR expression while preserving concrete presentation-call
/// identities through value-preserving expression forms.
pub fn eval_hir_expr_with_presentation(
    expr: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    eval_hir_expr_evaluated(expr, values, Some(presentation_values), local_values, ctx)
}

fn eval_hir_expr_evaluated(
    expr: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    ctx.cancellation.checkpoint()?;
    // Recursion choke point: evaluation recurses once per tree level
    // (unbounded for left-nested operator chains).
    graphcal_compiler::stack::with_stack_growth(|| {
        eval_hir_expr_inner(expr, values, presentation_values, local_values, ctx)
    })
}

#[expect(clippy::too_many_lines, reason = "exhaustive HIR ExprKind evaluation")]
fn eval_hir_expr_inner(
    expr: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    // Reject missing, contextual, or deferred facts before any operand work.
    ctx.expression_fact(expr)?;
    match expr.kind() {
        // Error nodes exist only in tolerant lowering for IDE consumers; the
        // batch pipeline rejects them before evaluation.
        hir::ExprKind::Error { .. } => {
            Err(ctx.eval_error("unresolved reference reached evaluation", expr.span))
        }
        hir::ExprKind::Number(n) => checked_finite_quantity(*n, "numeric literal", expr.span, ctx)
            .map(EvaluatedRuntimeValue::plain),
        hir::ExprKind::Integer(n) => Ok(EvaluatedRuntimeValue::plain(RuntimeValue::Int(*n))),
        hir::ExprKind::Bool(b) => Ok(EvaluatedRuntimeValue::plain(RuntimeValue::Bool(*b))),
        hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::OffsetDateTimeLiteral(_)
        | hir::ExprKind::CivilDateTimeLiteral(_)
        | hir::ExprKind::ZonedDateTimeLiteral(_)
        | hir::ExprKind::IanaTimeZoneLiteral(_) => Err(ctx.eval_error(
            "unexpected contextual literal in evaluation context",
            expr.span,
        )),
        hir::ExprKind::TypeSystemRef(name) => Err(ctx.eval_error(
            format!(
                "unexpected type-system name `{:?}` in evaluation context",
                name.value
            ),
            name.span,
        )),
        hir::ExprKind::QuantityLiteral { value, unit } => {
            let scale = resolve_unit_scale(unit, values, ctx)?;
            let value = checked_unit_scaled_value(*value, scale, expr.span, ctx)?;
            let presentation = super::presentation::scaled(unit, scale, ctx)?;
            Ok(EvaluatedRuntimeValue::new(value, presentation))
        }
        hir::ExprKind::GraphRef(target) => {
            let runtime_target = ctx.current_dag.runtime_decl_identity(&target.value);
            let key = RuntimeDeclKey::resolved(runtime_target);
            let value = resolve_hir_graph_ref(&target.value, target.span, values, ctx)?;
            let presentation = presentation_instance_for(presentation_values, &key);
            Ok(EvaluatedRuntimeValue::new(
                clone_hir_graph_ref_value(value),
                presentation,
            ))
        }
        hir::ExprKind::ConstRef(target) => {
            let value = eval_hir_const_ref(expr, target, values, local_values, ctx)?;
            let presentation = match &target.value {
                ConstRef::Decl(target) => presentation_instance_for(
                    presentation_values,
                    &RuntimeDeclKey::resolved(ctx.current_dag.runtime_decl_identity(target)),
                ),
                ConstRef::Builtin(_) | ConstRef::Constructor(_) | ConstRef::GenericNatParam(_) => {
                    PresentationInstance::None
                }
            };
            Ok(EvaluatedRuntimeValue::new(value, presentation))
        }
        hir::ExprKind::LocalRef(local) => local_values
            .get(local.value)
            .cloned()
            .ok_or_else(|| ctx.eval_error("undefined local variable", local.span)),
        hir::ExprKind::BinOp { op, lhs, rhs } => {
            eval_hir_binop(expr.span, *op, lhs, rhs, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain)
        }
        hir::ExprKind::UnaryOp { op, operand } => {
            eval_hir_unary(expr.span, *op, operand, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain)
        }
        hir::ExprKind::FnCall { callee, args, .. } => {
            eval_hir_fn_call(expr, callee, args, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain)
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = eval_hir_expr(condition, values, local_values, ctx)?
                .expect_bool("if condition")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            if cond {
                eval_hir_expr_evaluated(then_branch, values, presentation_values, local_values, ctx)
            } else {
                eval_hir_expr_evaluated(else_branch, values, presentation_values, local_values, ctx)
            }
        }
        hir::ExprKind::Convert {
            expr: inner,
            target,
        } => {
            let value = eval_hir_expr(inner, values, local_values, ctx)?;
            Ok(EvaluatedRuntimeValue::new(
                value,
                super::presentation::pending(target, ctx),
            ))
        }
        hir::ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => {
            let value = eval_hir_expr(inner, values, local_values, ctx)?;
            Ok(EvaluatedRuntimeValue::new(
                value,
                PresentationInstance::Timezone(timezone.clone()),
            ))
        }
        hir::ExprKind::FieldAccess { expr: inner, field } => {
            let inner_val =
                eval_hir_expr_evaluated(inner, values, presentation_values, local_values, ctx)?;
            let (inner_val, presentation) = inner_val.into_parts();
            let value = eval_hir_field_access(inner_val, inner, field, ctx)?;
            let presentation = presentation
                .project_field(&field.value)
                .map_err(|error| ctx.internal_error(error.to_string(), field.span))?;
            Ok(EvaluatedRuntimeValue::new(value, presentation))
        }
        hir::ExprKind::ConstructorCall { fields, .. } => {
            eval_hir_constructor_call(expr, fields, values, presentation_values, local_values, ctx)
        }
        hir::ExprKind::MapLiteral { entries } => eval_hir_map_literal(
            checked_value_type(expr, ctx)?,
            expr.span,
            entries,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        hir::ExprKind::ForComp { bindings, body } => eval_hir_for_comp(
            expr,
            bindings,
            body,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        hir::ExprKind::IndexAccess { expr: inner, args } => eval_hir_index_access(
            expr.span,
            inner,
            args.as_slice(),
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        hir::ExprKind::Scan {
            source,
            init,
            acc,
            val,
            body,
        } => eval_hir_scan(
            source,
            init,
            acc,
            val,
            body,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        hir::ExprKind::Unfold { .. } => {
            eval_hir_unfold(expr, values, presentation_values, local_values, ctx)
        }
        hir::ExprKind::KeyForm { kind, arg, .. } => eval_hir_key_form(
            *kind,
            checked_key_axis(expr, ctx)?,
            arg,
            expr.span,
            values,
            local_values,
            ctx,
        )
        .map(EvaluatedRuntimeValue::plain),
        hir::ExprKind::Match { scrutinee, arms } => eval_hir_match(
            expr,
            scrutinee,
            arms,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        hir::ExprKind::VariantLiteral(variant) => Ok(EvaluatedRuntimeValue::plain(
            RuntimeValue::resolved_label(&variant.variant),
        )),
        hir::ExprKind::DagCall {
            target,
            args,
            output,
            ..
        } => eval_hir_dag_call(
            expr,
            target,
            args,
            output,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
    }
}

fn resolve_hir_graph_ref<'a>(
    target: &ResolvedDeclKey,
    target_span: Span,
    values: &'a RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<&'a RuntimeValue, GraphcalError> {
    let runtime_target = ctx.current_dag.runtime_decl_identity(target);
    values
        .get(&RuntimeDeclKey::resolved(runtime_target))
        .ok_or_else(|| {
            ctx.eval_error(
                format!("undefined graph reference `@{target}`"),
                target_span,
            )
        })
}

fn clone_hir_graph_ref_value(value: &RuntimeValue) -> RuntimeValue {
    #[cfg(test)]
    record_hir_cloned_runtime_nodes(value);
    value.clone()
}

fn clone_hir_index_access_result(value: &RuntimeValue) -> RuntimeValue {
    #[cfg(test)]
    record_hir_cloned_runtime_nodes(value);
    value.clone()
}

#[cfg(test)]
fn record_hir_cloned_runtime_nodes(value: &RuntimeValue) {
    HIR_CLONED_RUNTIME_NODES.with(|count| {
        count.set(
            count
                .get()
                .saturating_add(runtime_value_tree_node_count(value)),
        );
    });
}

#[cfg(test)]
std::thread_local! {
    static HIR_CLONED_RUNTIME_NODES: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn runtime_value_tree_node_count(value: &RuntimeValue) -> usize {
    match value {
        RuntimeValue::Struct { fields, .. } => fields
            .values()
            .map(runtime_value_tree_node_count)
            .fold(1, usize::saturating_add),
        RuntimeValue::Indexed { entries, .. } => entries
            .values()
            .map(runtime_value_tree_node_count)
            .fold(1, usize::saturating_add),
        _ => 1,
    }
}

#[cfg(test)]
fn reset_hir_cloned_runtime_node_count() {
    HIR_CLONED_RUNTIME_NODES.with(|count| count.set(0));
}

#[cfg(test)]
fn take_hir_cloned_runtime_node_count() -> usize {
    HIR_CLONED_RUNTIME_NODES.with(|count| count.replace(0))
}

fn eval_hir_const_ref(
    expr: &hir::Expr,
    target: &graphcal_compiler::syntax::span::Spanned<ConstRef>,
    values: &RuntimeValueMap,
    _local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &target.value {
        ConstRef::Decl(resolved) => values
            .get(&RuntimeDeclKey::resolved(
                ctx.current_dag.runtime_decl_identity(resolved),
            ))
            .cloned()
            .ok_or_else(|| ctx.eval_error(format!("undefined constant `{resolved}`"), target.span)),
        ConstRef::Constructor(_) => eval_hir_nullary_constructor(expr, ctx),
        ConstRef::Builtin(builtin) => {
            checked_finite_quantity(builtin.value(), "built-in constant", target.span, ctx)
        }
        ConstRef::GenericNatParam(param) => Err(ctx.eval_error(
            format!("generic Nat parameter `{}` is not bound", param.name),
            target.span,
        )),
    }
}

fn checked_value_type<'a>(
    expr: &hir::Expr,
    ctx: &'a EvalContext<'_>,
) -> Result<&'a DeclaredType, GraphcalError> {
    match &ctx.expression_fact(expr)?.fact {
        graphcal_compiler::tir::expression_facts::ExpressionFact::Value {
            checked_type, ..
        } => Ok(checked_type),
        graphcal_compiler::tir::expression_facts::ExpressionFact::Contextual(_) => {
            Err(ctx.internal_error("expression has no retained value type", expr.span))
        }
    }
}

fn checked_key_axis<'a>(
    expr: &hir::Expr,
    ctx: &'a EvalContext<'_>,
) -> Result<&'a IndexTypeRef, GraphcalError> {
    match checked_value_type(expr, ctx)? {
        DeclaredType::Key(index) => Ok(index),
        _ => Err(ctx.internal_error("key expression has no retained axis", expr.span)),
    }
}

fn checked_constructor<'a>(
    expr: &hir::Expr,
    ctx: &'a EvalContext<'_>,
) -> Result<&'a graphcal_compiler::tir::expression_facts::ConstructorApplication, GraphcalError> {
    match &ctx.expression_fact(expr)?.fact {
        graphcal_compiler::tir::expression_facts::ExpressionFact::Value {
            constructor: Some(application),
            ..
        } => {
            crate::pipeline_metrics::record(
                crate::pipeline_metrics::Event::ConstructorFactConsumption,
            );
            Ok(application)
        }
        _ => Err(ctx.internal_error(
            "constructor expression has no retained application",
            expr.span,
        )),
    }
}

fn eval_hir_nullary_constructor(
    expr: &hir::Expr,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let application = checked_constructor(expr, ctx)?;
    Ok(RuntimeValue::Struct {
        type_name: application.runtime_type.clone(),
        constructor: application.constructor.clone(),
        generic_args: application.generic_args.clone(),
        fields: IndexMap::new(),
    })
}

fn eval_hir_binop(
    span: Span,
    op: graphcal_compiler::desugar::desugared_ast::BinOp,
    lhs: &hir::Expr,
    rhs: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::desugar::desugared_ast::BinOp;
    match op {
        BinOp::And => {
            let l = eval_hir_expr(lhs, values, local_values, ctx)?
                .expect_bool("AND operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let r = eval_hir_expr(rhs, values, local_values, ctx)?
                .expect_bool("AND operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(l && r))
        }
        BinOp::Or => {
            let l = eval_hir_expr(lhs, values, local_values, ctx)?
                .expect_bool("OR operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let r = eval_hir_expr(rhs, values, local_values, ctx)?
                .expect_bool("OR operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(l || r))
        }
        BinOp::Eq | BinOp::Ne => {
            let l = eval_hir_expr(lhs, values, local_values, ctx)?;
            let r = eval_hir_expr(rhs, values, local_values, ctx)?;
            super::arithmetic::eval_equality_values(op, &l, &r, ctx, span)
        }
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            let l = eval_hir_expr(lhs, values, local_values, ctx)?;
            let r = eval_hir_expr(rhs, values, local_values, ctx)?;
            super::arithmetic::eval_ordering_values(op, &l, &r, ctx, span)
        }
        BinOp::Pow(exponent) => eval_hir_power(span, exponent, lhs, rhs, values, local_values, ctx),
        _ => {
            let l = eval_hir_expr(lhs, values, local_values, ctx)?;
            let r = eval_hir_expr(rhs, values, local_values, ctx)?;
            if let (RuntimeValue::Int(li), RuntimeValue::Int(ri)) = (&l, &r) {
                return super::arithmetic::eval_int_binop(op, *li, *ri, ctx, span)
                    .map(RuntimeValue::Int);
            }
            match (&l, &r) {
                (RuntimeValue::Datetime(le), RuntimeValue::Datetime(re)) if op == BinOp::Sub => {
                    let seconds = super::datetime::checked_epoch_difference_seconds(*le, *re)
                        .map_err(|error| ctx.eval_error(error.to_string(), span))?;
                    return checked_finite_quantity(seconds, "datetime difference", span, ctx);
                }
                (RuntimeValue::Datetime(_), RuntimeValue::Datetime(_)) => {
                    return Err(ctx.eval_error("cannot add two datetimes", span));
                }
                (RuntimeValue::Datetime(e), RuntimeValue::Quantity(secs)) => {
                    let result = match op {
                        BinOp::Add => super::datetime::checked_epoch_add_seconds(*e, secs.get()),
                        BinOp::Sub => {
                            super::datetime::checked_epoch_subtract_seconds(*e, secs.get())
                        }
                        _ => {
                            return Err(ctx.eval_error(
                                format!("unsupported operator {op:?} for Datetime and quantity"),
                                span,
                            ));
                        }
                    };
                    return result
                        .map(RuntimeValue::Datetime)
                        .map_err(|error| ctx.eval_error(error.to_string(), span));
                }
                (RuntimeValue::Quantity(secs), RuntimeValue::Datetime(e)) if op == BinOp::Add => {
                    return super::datetime::checked_epoch_add_seconds(*e, secs.get())
                        .map(RuntimeValue::Datetime)
                        .map_err(|error| ctx.eval_error(error.to_string(), span));
                }
                (RuntimeValue::Quantity(_), RuntimeValue::Datetime(_)) => {
                    return Err(ctx.eval_error("cannot subtract a Datetime from a quantity", span));
                }
                _ => {}
            }
            if matches!(l, RuntimeValue::Complex(_)) || matches!(r, RuntimeValue::Complex(_)) {
                return super::complex::evaluate_binary(op, &l, &r).map_err(|error| {
                    if error.is_internal_invariant() {
                        ctx.internal_error(error.to_string(), span)
                    } else {
                        ctx.eval_error(error.to_string(), span)
                    }
                });
            }
            let lv = l
                .expect_quantity("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_quantity("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            super::arithmetic::eval_quantity_binop(op, lv, rv, ctx, span)
                .and_then(|value| checked_finite_quantity(value, "quantity operation", span, ctx))
        }
    }
}

fn eval_hir_power(
    span: Span,
    exponent: graphcal_compiler::syntax::ast::PowerExponent,
    base: &hir::Expr,
    exponent_expr: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::desugar::desugared_ast::BinOp;
    use graphcal_compiler::syntax::ast::PowerExponent;

    let base = eval_hir_expr(base, values, local_values, ctx)?;
    let op = BinOp::Pow(exponent);
    match (base, exponent) {
        (RuntimeValue::Int(base), PowerExponent::Exact(exact)) => {
            if !exact.is_integer() {
                return Err(ctx.internal_error(
                    "fractional exact exponent reached Int power evaluation",
                    span,
                ));
            }
            super::arithmetic::eval_int_binop(op, base, exact.numerator(), ctx, span)
                .map(RuntimeValue::Int)
        }
        (RuntimeValue::Int(base), _) => {
            let runtime_exponent = eval_hir_expr(exponent_expr, values, local_values, ctx)?;
            let RuntimeValue::Int(runtime_exponent) = runtime_exponent else {
                return Err(
                    ctx.internal_error("non-Int exponent reached Int power evaluation", span)
                );
            };
            super::arithmetic::eval_int_binop(op, base, runtime_exponent, ctx, span)
                .map(RuntimeValue::Int)
        }
        (RuntimeValue::Quantity(base), PowerExponent::Exact(exact)) => {
            super::arithmetic::eval_exact_quantity_power(base.get(), exact, ctx, span)
                .and_then(|value| checked_finite_quantity(value, "quantity power", span, ctx))
        }
        (RuntimeValue::Quantity(base), _) => {
            let runtime_exponent = eval_hir_expr(exponent_expr, values, local_values, ctx)?
                .expect_quantity("power exponent")
                .map_err(|error| ctx.internal_error(error.to_string(), span))?;
            super::arithmetic::eval_quantity_binop(op, base.get(), runtime_exponent, ctx, span)
                .and_then(|value| checked_finite_quantity(value, "quantity power", span, ctx))
        }
        (other, _) => Err(ctx.internal_error(
            format!("non-numeric base reached power evaluation: {other:?}"),
            span,
        )),
    }
}

fn eval_hir_unary(
    span: Span,
    op: graphcal_compiler::desugar::desugared_ast::UnaryOp,
    operand: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match op {
        graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg => {
            let v = eval_hir_expr(operand, values, local_values, ctx)?;
            match v {
                RuntimeValue::Int(i) => i
                    .checked_neg()
                    .map(RuntimeValue::Int)
                    .ok_or_else(|| ctx.eval_error("integer negation overflow", span)),
                RuntimeValue::Complex(value) => super::complex::negate(value)
                    .map(RuntimeValue::Complex)
                    .map_err(|error| ctx.eval_error(error.to_string(), span)),
                _ => checked_finite_quantity(
                    -v.expect_quantity("unary negation")
                        .map_err(|e| ctx.eval_error(e.to_string(), span))?,
                    "unary negation",
                    span,
                    ctx,
                ),
            }
        }
        graphcal_compiler::desugar::desugared_ast::UnaryOp::Not => {
            let v = eval_hir_expr(operand, values, local_values, ctx)?
                .expect_bool("logical NOT")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(!v))
        }
    }
}

fn expect_hir_builtin_arity(
    name: BuiltinFnName,
    args: &[hir::Expr],
    expected: usize,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    if args.len() == expected {
        return Ok(());
    }
    Err(ctx.internal_error(
        format!(
            "{}() received {} argument(s) after dim-check accepted arity {expected}",
            name.as_str(),
            args.len()
        ),
        span,
    ))
}

#[expect(
    clippy::too_many_lines,
    reason = "function dispatch handles all built-in call forms"
)]
fn eval_hir_fn_call(
    expr: &hir::Expr,
    callee: &graphcal_compiler::syntax::span::Spanned<FunctionRef>,
    args: &[hir::Expr],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let (name, epoch_scale) = match &callee.value {
        FunctionRef::Builtin(name) => (*name, None),
        FunctionRef::Epoch { scale } => (BuiltinFnName::Epoch, Some(scale.value)),
        FunctionRef::External(ext) => {
            return eval_hir_extern_fn(expr, ext, args, values, local_values, ctx);
        }
    };
    match eval_rule_for_builtin(name) {
        EvalBuiltinRule::Complex(function) => {
            expect_hir_builtin_arity(name, args, function.arity(), callee.span, ctx)?;
            let arguments = args
                .iter()
                .map(|argument| eval_hir_expr(argument, values, local_values, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            super::complex::evaluate_builtin(function, &arguments).map_err(|error| {
                if error.is_internal_invariant() {
                    ctx.internal_error(error.to_string(), expr.span)
                } else {
                    ctx.eval_error(error.to_string(), expr.span)
                }
            })
        }
        EvalBuiltinRule::CollectionAggregation(kind) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Indexed {
                index_name,
                entries,
            } = arg_val
            else {
                return Err(ctx.internal_error(
                    format!("{}() received a non-indexed argument", name.as_str()),
                    args[0].span,
                ));
            };
            if matches!(kind, AggregationFn::Argmin | AggregationFn::Argmax) {
                return eval_hir_extremum_key(kind, &index_name, &entries, expr.span, ctx);
            }
            eval_hir_aggregation_fn(kind, &entries, expr.span, ctx.src)
        }
        EvalBuiltinRule::LinearAlgebra(function) => {
            expect_hir_builtin_arity(name, args, function.arity(), callee.span, ctx)?;
            let arguments = args
                .iter()
                .map(|argument| eval_hir_expr(argument, values, local_values, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            super::linear_algebra::evaluate(function, arguments, ctx).map_err(|error| {
                error.cancellation().map_or_else(
                    || {
                        if error.is_internal_invariant() {
                            ctx.internal_error(error.to_string(), expr.span)
                        } else {
                            ctx.eval_error(error.to_string(), expr.span)
                        }
                    },
                    GraphcalError::from,
                )
            })
        }
        EvalBuiltinRule::TypeConversion(kind) => {
            eval_hir_conversion_fn(kind, expr.span, args, values, local_values, ctx)
        }
        EvalBuiltinRule::TimeScaleConversion(scale) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    args[0].span,
                ));
            };
            Ok(RuntimeValue::Datetime(
                epoch.to_time_scale(scale.to_hifitime()),
            ))
        }
        EvalBuiltinRule::DatetimeConstructor(kind) => {
            eval_hir_datetime_constructor(kind, epoch_scale, expr.span, args, ctx.src)
        }
        EvalBuiltinRule::DatetimeExtract(kind) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    args[0].span,
                ));
            };
            let fields = super::datetime::GregorianFields::from_epoch(epoch).map_err(|error| {
                ctx.internal_error(
                    format!("invalid declared-scale Gregorian fields: {error}"),
                    args[0].span,
                )
            })?;
            let result = match kind {
                DatetimeExtractFn::Year => fields.year(),
                DatetimeExtractFn::Month => fields.month(),
                DatetimeExtractFn::Day => fields.day(),
                DatetimeExtractFn::Hour => fields.hour(),
                DatetimeExtractFn::Minute => fields.minute(),
                DatetimeExtractFn::Second => fields.second(),
                DatetimeExtractFn::Weekday => fields.iso_weekday(),
                DatetimeExtractFn::DayOfYear => fields.day_of_year(),
            };
            Ok(RuntimeValue::Int(result))
        }
        EvalBuiltinRule::DatetimeFromNumeric(kind) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let num = match arg_val {
                RuntimeValue::Quantity(v) => v.get(),
                RuntimeValue::Int(v) => {
                    exact_numeric_datetime_arg(v, name.as_str(), args[0].span, ctx)?
                }
                _ => {
                    return Err(ctx.internal_error(
                        format!("{}() received non-numeric argument", name.as_str()),
                        args[0].span,
                    ));
                }
            };
            let kind = match kind {
                DatetimeFromFn::Jd => super::datetime::NumericEpochKind::JulianDate,
                DatetimeFromFn::Mjd => super::datetime::NumericEpochKind::ModifiedJulianDate,
                DatetimeFromFn::Unix => super::datetime::NumericEpochKind::UnixSeconds,
            };
            super::datetime::checked_epoch_from_numeric(num, kind)
                .map(RuntimeValue::Datetime)
                .map_err(|error| ctx.eval_error(error.to_string(), args[0].span))
        }
        EvalBuiltinRule::DatetimeToNumeric(kind) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    args[0].span,
                ));
            };
            let result = match kind {
                DatetimeToFn::Jd => epoch.to_jde_utc_days(),
                DatetimeToFn::Mjd => epoch.to_mjd_utc_days(),
                DatetimeToFn::Unix => epoch.to_unix_seconds(),
            };
            checked_finite_quantity(result, "datetime conversion", args[0].span, ctx)
        }
        EvalBuiltinRule::RegistryFunction => {
            eval_hir_builtin_fn(expr, name, args, values, local_values, ctx)
        }
    }
}

/// Evaluate a key introduction form.
///
/// `key` positions are proven in bounds by the checker; `fin_key` performs
/// its runtime range check here; the coordinate searches scan the axis's
/// coordinates with the documented policies.
fn eval_hir_key_form(
    kind: graphcal_compiler::syntax::ast::KeyFormKind,
    axis_ref: &IndexTypeRef,
    arg: &hir::Expr,
    span: Span,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::syntax::ast::KeyFormKind;

    let arg_val = eval_hir_expr(arg, values, local_values, ctx)?;
    match kind {
        KeyFormKind::Static => {
            // Bounds were discharged at compile time.
            let RuntimeValue::Int(position) = arg_val else {
                return Err(ctx.internal_error("key() received a non-Int position", arg.span));
            };
            Ok(RuntimeValue::Int(position))
        }
        KeyFormKind::Fin => {
            let RuntimeValue::Int(position) = arg_val else {
                return Err(ctx.internal_error("fin_key() received a non-Int position", arg.span));
            };
            let finite = axis_ref.finite_index().ok_or_else(|| {
                ctx.internal_error("fin_key() has no retained concrete Fin axis", span)
            })?;
            let size = finite.cardinality().get() as u64;
            let in_range = u64::try_from(position).is_ok_and(|position| position < size);
            if !in_range {
                return Err(ctx.eval_error(
                    format!("fin_key: {position} out of bounds for Fin({size})"),
                    span,
                ));
            }
            Ok(RuntimeValue::Int(position))
        }
        KeyFormKind::Floor | KeyFormKind::Ceil | KeyFormKind::Nearest => {
            let quantity = arg_val
                .expect_quantity("coordinate search argument")
                .map_err(|e| ctx.eval_error(e.to_string(), arg.span))?;
            let definition = index_def_for_ref(axis_ref, ctx).ok_or_else(|| {
                ctx.internal_error(
                    format!("index `{axis_ref}` has no registered definition"),
                    span,
                )
            })?;
            let IndexKind::Coordinate(data) = &definition.kind else {
                return Err(
                    ctx.internal_error("coordinate search received a non-coordinate axis", span)
                );
            };
            let count = data.cardinality();
            let mut best: Option<(usize, f64)> = None;
            for position in 0..count {
                let coordinate = data.coordinate_value(position);
                let candidate = match kind {
                    KeyFormKind::Floor if coordinate <= quantity => Some((position, coordinate)),
                    KeyFormKind::Ceil if coordinate >= quantity => Some((position, coordinate)),
                    KeyFormKind::Nearest => Some((position, coordinate)),
                    _ => None,
                };
                let Some((position, coordinate)) = candidate else {
                    continue;
                };
                let better = match (&best, kind) {
                    (None, _) => true,
                    // Nearest: strictly closer wins, so midpoint ties keep
                    // the earlier position (toward the axis start).
                    (Some((_, incumbent)), KeyFormKind::Nearest) => {
                        (coordinate - quantity).abs() < (incumbent - quantity).abs()
                    }
                    // Floor: the greatest coordinate at or below the target.
                    (Some((_, incumbent)), KeyFormKind::Floor) => coordinate > *incumbent,
                    // Ceil: the smallest coordinate at or above the target.
                    (Some((_, incumbent)), _) => coordinate < *incumbent,
                };
                if better {
                    best = Some((position, coordinate));
                }
            }
            let Some((position, _)) = best else {
                return Err(ctx.eval_error(
                    format!(
                        "{}: no coordinate of `{axis_ref}` is {} the target",
                        kind.as_str(),
                        if kind == KeyFormKind::Floor {
                            "at or below"
                        } else {
                            "at or above"
                        },
                    ),
                    span,
                ));
            };
            RuntimeValue::coordinate_label(
                axis_ref.clone(),
                position,
                data.coordinate_value(position),
            )
            .map_err(|error| ctx.eval_error(error.to_string(), span))
        }
    }
}

/// Evaluate `argmin`/`argmax`: find the extremum entry and reify its entry
/// key as the matching key runtime value for the reduced axis.
fn eval_hir_extremum_key(
    kind: AggregationFn,
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let entry_key = super::aggregations::extremum_entry_key(kind, entries).map_err(|error| {
        let message = error.to_string();
        if error.is_internal_invariant() {
            ctx.internal_error(message, span)
        } else {
            ctx.eval_error(message, span)
        }
    })?;
    runtime_key_for_entry(index_name, &entry_key, span, ctx)
}

/// Reify an [`IndexEntryKey`] of `index_name` as the key runtime value:
/// a label for named axes, a coordinate label for coordinate axes, and a
/// position integer for `Fin` axes.
fn runtime_key_for_entry(
    index_name: &IndexTypeRef,
    entry_key: &IndexEntryKey,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match entry_key {
        IndexEntryKey::Named(variant) => Ok(RuntimeValue::Label {
            index_name: index_name.clone(),
            variant: variant.clone(),
        }),
        IndexEntryKey::Position(position) => {
            if index_name.finite_index().is_some() {
                let position = i64::try_from(*position).map_err(|_| {
                    ctx.internal_error(
                        format!("key position {position} cannot be represented as Int"),
                        span,
                    )
                })?;
                return Ok(RuntimeValue::Int(position));
            }
            let index_def = index_def_for_ref(index_name, ctx).ok_or_else(|| {
                ctx.internal_error(
                    format!("index `{index_name}` has no registered definition"),
                    span,
                )
            })?;
            match &index_def.kind {
                IndexKind::Coordinate(data) => {
                    let position = usize::try_from(*position).map_err(|_| {
                        ctx.internal_error(
                            format!("coordinate position {position} exceeds the platform range"),
                            span,
                        )
                    })?;
                    RuntimeValue::coordinate_label(
                        index_name.clone(),
                        position,
                        data.coordinate_value(position),
                    )
                    .map_err(|error| ctx.internal_error(error.to_string(), span))
                }
                _ => Err(ctx.internal_error(
                    format!("position key on non-coordinate, non-finite index `{index_name}`"),
                    span,
                )),
            }
        }
    }
}

fn eval_hir_aggregation_fn(
    kind: AggregationFn,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    super::aggregations::aggregate_indexed_values(kind, entries).map_err(|error| {
        let message = error.to_string();
        if error.is_internal_invariant() {
            GraphcalError::InternalError {
                message,
                src: src.clone(),
                span: span.into(),
            }
        } else {
            GraphcalError::EvalError {
                message,
                src: src.clone(),
                span: span.into(),
            }
        }
    })
}

fn eval_hir_conversion_fn(
    kind: TypeConversionFn,
    span: Span,
    args: &[hir::Expr],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let name = match kind {
        TypeConversionFn::ToFloat => BuiltinFnName::ToFloat,
        TypeConversionFn::ToInt => BuiltinFnName::ToInt,
        TypeConversionFn::Coord => BuiltinFnName::Coord,
    };
    expect_hir_builtin_arity(name, args, 1, span, ctx)?;
    match kind {
        TypeConversionFn::ToFloat => {
            let arg = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Int(i) = arg else {
                return Err(
                    ctx.internal_error("to_float() received non-Int argument", args[0].span)
                );
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "explicit Int to float conversion"
            )]
            checked_finite_quantity(i as f64, "to_float()", args[0].span, ctx)
        }
        TypeConversionFn::ToInt => {
            let arg = eval_hir_expr(&args[0], values, local_values, ctx)?;
            // A Fin-axis key is represented as its position integer: to_int()
            // on a key is the identity at runtime, checked at the type level.
            if let RuntimeValue::Int(position) = arg {
                return Ok(RuntimeValue::Int(position));
            }
            let f = arg
                .expect_quantity("to_int argument")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            super::conversions::exact_f64_to_i64(f)
                .map(RuntimeValue::Int)
                .map_err(|error| {
                    let rounding_help = if matches!(
                        &error,
                        super::conversions::ExactIntConversionError::NonInteger { .. }
                    ) {
                        "; apply trunc(), floor(), ceil(), or round() explicitly before to_int()"
                    } else {
                        ""
                    };
                    ctx.eval_error(format!("to_int() argument {error}{rounding_help}"), span)
                })
        }
        TypeConversionFn::Coord => {
            let arg = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::CoordinateLabel { value, .. } = arg else {
                return Err(ctx.internal_error(
                    "coord() received a non-coordinate-key argument",
                    args[0].span,
                ));
            };
            Ok(RuntimeValue::Quantity(value))
        }
    }
}

fn exact_numeric_datetime_arg(
    value: i64,
    fn_name: &str,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    super::numeric::exact_i64_to_f64(value).map_err(|_| {
        ctx.eval_error(
            format!("{fn_name}() integer argument {value} is too large for exact conversion"),
            span,
        )
    })
}

fn eval_hir_datetime_constructor(
    kind: DatetimeConstructorFn,
    epoch_scale: Option<TimeScale>,
    span: Span,
    args: &[hir::Expr],
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match kind {
        DatetimeConstructorFn::Datetime => {
            if !(1..=2).contains(&args.len()) {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "datetime() received {} argument(s) after dim-check accepted arity 1..2",
                        args.len()
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let epoch = match args {
                [arg] => {
                    let hir::ExprKind::OffsetDateTimeLiteral(datetime) = arg.kind() else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unparsed offset literal".to_string(),
                            src: src.clone(),
                            span: arg.span.into(),
                        });
                    };
                    super::datetime::datetime_from_offset(*datetime)
                }
                [datetime_arg, timezone_arg] => {
                    let hir::ExprKind::ZonedDateTimeLiteral(datetime) = datetime_arg.kind() else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unresolved zoned literal".to_string(),
                            src: src.clone(),
                            span: datetime_arg.span.into(),
                        });
                    };
                    let hir::ExprKind::IanaTimeZoneLiteral(time_zone_id) = timezone_arg.kind()
                    else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unvalidated timezone argument"
                                .to_string(),
                            src: src.clone(),
                            span: timezone_arg.span.into(),
                        });
                    };
                    if datetime.time_zone() != time_zone_id {
                        return Err(GraphcalError::InternalError {
                            message: "resolved datetime timezone does not match its argument"
                                .to_string(),
                            src: src.clone(),
                            span: timezone_arg.span.into(),
                        });
                    }
                    super::datetime::datetime_from_zoned(datetime)
                }
                _ => {
                    return Err(GraphcalError::InternalError {
                        message: "datetime arity changed after validation".to_string(),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            };
            Ok(RuntimeValue::Datetime(epoch))
        }
        DatetimeConstructorFn::Epoch => {
            let [arg] = args else {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "epoch() received {} argument(s) after dim-check accepted arity 1",
                        args.len()
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            let hir::ExprKind::CivilDateTimeLiteral(datetime) = arg.kind() else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received an unparsed civil literal".to_string(),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            };
            let Some(scale) = epoch_scale else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() reached evaluation without a static time scale".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            let epoch =
                super::datetime::epoch_from_civil_datetime(*datetime, scale).map_err(|error| {
                    GraphcalError::InternalError {
                        message: format!("validated epoch literal failed evaluation: {error}"),
                        src: src.clone(),
                        span: arg.span.into(),
                    }
                })?;
            Ok(RuntimeValue::Datetime(epoch))
        }
    }
}

/// The concrete index an extern call bound to one of its index variables:
/// the index identity plus its typed entry keys, in
/// declaration order. Result arrays are rebuilt over exactly these keys.
#[derive(Clone)]
struct BoundExternIndex {
    index_name: graphcal_compiler::registry::declared_type::IndexTypeRef,
    keys: Vec<IndexEntryKey>,
}

impl BoundExternIndex {
    fn matches(&self, other: &Self) -> bool {
        self.index_name.matches_ref(&other.index_name) && self.keys == other.keys
    }
}

enum FlattenedExternArrayValues {
    Quantity(Vec<f64>),
    Bool(Vec<bool>),
    Int(Vec<i64>),
}

impl FlattenedExternArrayValues {
    fn append(&mut self, other: Self) -> Result<(), ()> {
        match (self, other) {
            (Self::Quantity(values), Self::Quantity(other)) => values.extend(other),
            (Self::Bool(values), Self::Bool(other)) => values.extend(other),
            (Self::Int(values), Self::Int(other)) => values.extend(other),
            _ => return Err(()),
        }
        Ok(())
    }
}

struct FlattenedExternArray {
    axes: Vec<BoundExternIndex>,
    values: FlattenedExternArrayValues,
}

fn flatten_extern_array(
    value: &RuntimeValue,
    expected: &graphcal_compiler::function_signature::ScalarValueKind,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<FlattenedExternArray, GraphcalError> {
    use graphcal_compiler::function_signature::ScalarValueKind;

    match (expected, value) {
        (ScalarValueKind::Quantity(_), RuntimeValue::Quantity(value)) => Ok(FlattenedExternArray {
            axes: Vec::new(),
            values: FlattenedExternArrayValues::Quantity(vec![value.get()]),
        }),
        (ScalarValueKind::Bool, RuntimeValue::Bool(value)) => Ok(FlattenedExternArray {
            axes: Vec::new(),
            values: FlattenedExternArrayValues::Bool(vec![*value]),
        }),
        (ScalarValueKind::Int, RuntimeValue::Int(value)) => Ok(FlattenedExternArray {
            axes: Vec::new(),
            values: FlattenedExternArrayValues::Int(vec![*value]),
        }),
        (
            _,
            RuntimeValue::Indexed {
                index_name,
                entries,
            },
        ) => {
            let mut children = entries
                .values()
                .map(|value| flatten_extern_array(value, expected, ctx, span))
                .collect::<Result<Vec<_>, _>>()?;
            let first = children.first().ok_or_else(|| {
                ctx.internal_error("extern array unexpectedly had an empty axis", span)
            })?;
            if children.iter().skip(1).any(|child| {
                child.axes.len() != first.axes.len()
                    || child
                        .axes
                        .iter()
                        .zip(&first.axes)
                        .any(|(left, right)| !left.matches(right))
            }) {
                return Err(ctx.eval_error(
                    "extern array is ragged or changes typed indexes between branches",
                    span,
                ));
            }
            let mut first = children.remove(0);
            for child in children {
                first.values.append(child.values).map_err(|()| {
                    ctx.eval_error("extern array mixes scalar element kinds", span)
                })?;
            }
            let mut axes = Vec::with_capacity(first.axes.len().saturating_add(1));
            axes.push(BoundExternIndex {
                index_name: index_name.clone(),
                keys: entries.keys().cloned().collect(),
            });
            axes.extend(first.axes);
            Ok(FlattenedExternArray {
                axes,
                values: first.values,
            })
        }
        (ScalarValueKind::Quantity(_), _) => {
            Err(ctx.eval_error("extern array elements must be quantities", span))
        }
        (ScalarValueKind::Bool, _) => {
            Err(ctx.eval_error("extern array elements must be Bool values", span))
        }
        (ScalarValueKind::Int, _) => {
            Err(ctx.eval_error("extern array elements must be Int values", span))
        }
    }
}

fn rebuild_extern_array<T>(
    bound_axes: &[BoundExternIndex],
    values: &[T],
    make_leaf: impl Fn(&T) -> RuntimeValue + Copy,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    match bound_axes.split_first() {
        None => match values {
            [value] => Ok(make_leaf(value)),
            _ => {
                Err(ctx.internal_error("extern array leaf did not contain exactly one value", span))
            }
        },
        Some((axis, remaining)) => {
            let child_len = remaining
                .iter()
                .try_fold(1_usize, |size, axis| size.checked_mul(axis.keys.len()));
            let Some(child_len) = child_len else {
                return Err(ctx.eval_error("extern result shape cardinality overflowed", span));
            };
            let chunks = values.chunks_exact(child_len);
            if !chunks.remainder().is_empty() || chunks.len() != axis.keys.len() {
                return Err(
                    ctx.internal_error("extern result buffer did not match its bound shape", span)
                );
            }
            let values = axis
                .keys
                .iter()
                .cloned()
                .zip(chunks)
                .map(|(key, values)| {
                    rebuild_extern_array(remaining, values, make_leaf, ctx, span)
                        .map(|value| (key, value))
                })
                .collect::<Result<IndexMap<_, _>, _>>()?;
            Ok(RuntimeValue::Indexed {
                index_name: axis.index_name.clone(),
                entries: values,
            })
        }
    }
}

/// Evaluate an extern (plugin) function call through the embedder-injected
/// host function registry.
///
/// The host ABI is `fn(&[HostFnValue]) -> Result<HostFnValue, HostFnError>`:
/// quantities cross as SI `f64`s (Int arguments convert exactly, Bool arguments
/// become `1.0`/`0.0`), arrays cross as row-major buffers with an ordered
/// shape, and the result converts back per the declared result kind — each
/// result axis is rebuilt over the same typed index keys supplied by an input.
/// A closure error or a non-finite quantity becomes a per-node
/// evaluation failure naming the plugin alias and function; dependents
/// report `DependencyFailed` through the ordinary per-node fault isolation.
#[expect(
    clippy::too_many_lines,
    reason = "single linear path: lookup, argument conversion, call, result conversion"
)]
fn eval_hir_extern_fn(
    expr: &hir::Expr,
    ext: &hir::ExternFnRef,
    args: &[hir::Expr],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::function_signature::{ScalarValueKind, ValueKind};

    use crate::host_abi::{
        ValidatedHostArrayValues, ValidatedHostFieldValue, ValidatedHostResult, decode_result,
        encode_bool, encode_int, validate_quantity,
    };
    use crate::host_fns::{HostArray, HostFnValue};

    let Some(registry) = ctx.host_fns() else {
        return Err(ctx.eval_error(
            format!("extern function `{ext}` cannot be evaluated in this context (no host function registry)"),
            expr.span,
        ));
    };
    let key = ext.key();
    let Some(host_fn) = registry.get(&key) else {
        return Err(ctx.eval_error(
            format!(
                "extern function `{}` (plugin \"{}\") is not provided by the host",
                ext.name, ext.plugin
            ),
            expr.span,
        ));
    };
    let Some(function) = ctx.tir.extern_functions().get(&key) else {
        return Err(ctx.internal_error(
            format!("extern function `{ext}` has no resolved signature after dimension checking"),
            expr.span,
        ));
    };
    let signature = &function.signature;
    if args.len() != signature.arity() {
        return Err(ctx.eval_error(
            format!(
                "extern function `{ext}` expects {} argument(s) but got {}",
                signature.arity(),
                args.len()
            ),
            expr.span,
        ));
    }

    let mut bound_indexes: std::collections::HashMap<
        graphcal_compiler::syntax::index_name::IndexVarName,
        BoundExternIndex,
    > = std::collections::HashMap::new();
    let mut arg_values: Vec<HostFnValue> = Vec::with_capacity(args.len());
    for (param, arg) in signature.params().iter().zip(args) {
        let value = eval_hir_expr(arg, values, local_values, ctx)?;
        let converted = match (&param.kind, value) {
            (ValueKind::Scalar(ScalarValueKind::Quantity(_)), value) => {
                let value = value
                    .expect_quantity("extern function argument")
                    .map_err(|error| ctx.eval_error(error.to_string(), arg.span))?;
                let value = validate_quantity(value).map_err(|error| {
                    ctx.eval_error(
                        format!(
                            "extern function `{ext}` received an invalid quantity for parameter `{}`: {error}",
                            param.name
                        ),
                        arg.span,
                    )
                })?;
                HostFnValue::F64(value.get())
            }
            (ValueKind::Scalar(ScalarValueKind::Int), RuntimeValue::Int(value)) => {
                let value = encode_int(value).map_err(|error| {
                    ctx.eval_error(
                        format!(
                            "extern function `{ext}` received an invalid Int for parameter `{}`: {error}",
                            param.name
                        ),
                        arg.span,
                    )
                })?;
                HostFnValue::F64(value)
            }
            (ValueKind::Scalar(ScalarValueKind::Bool), RuntimeValue::Bool(value)) => {
                HostFnValue::F64(encode_bool(value))
            }
            (ValueKind::Indexed { element, indexes }, value) => {
                let flattened = flatten_extern_array(&value, element, ctx, arg.span)?;
                if flattened.axes.len() != indexes.len() {
                    return Err(ctx.internal_error(
                        format!(
                            "extern function `{ext}` parameter `{}` received rank {}, expected rank {} after dimension checking",
                            param.name,
                            flattened.axes.len(),
                            indexes.len()
                        ),
                        arg.span,
                    ));
                }
                for (index, bound) in indexes.iter().zip(&flattened.axes) {
                    match bound_indexes.entry(index.clone()) {
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(bound.clone());
                        }
                        std::collections::hash_map::Entry::Occupied(previous)
                            if !previous.get().matches(bound) =>
                        {
                            return Err(ctx.internal_error(
                                format!(
                                    "extern function `{ext}` received inconsistent typed axes for index variable `{index}` after dimension checking"
                                ),
                                arg.span,
                            ));
                        }
                        std::collections::hash_map::Entry::Occupied(_) => {}
                    }
                }
                let shape = flattened.axes.iter().map(|axis| axis.keys.len()).collect();
                let encoded_values = match flattened.values {
                    FlattenedExternArrayValues::Quantity(values) => values
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| {
                            validate_quantity(value)
                                .map(graphcal_compiler::finite_value::FiniteQuantity::get)
                                .map_err(|error| {
                                ctx.eval_error(
                                    format!(
                                        "extern function `{ext}` parameter `{}` has an invalid quantity at flat array index {index}: {error}",
                                        param.name
                                    ),
                                    arg.span,
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    FlattenedExternArrayValues::Bool(values) => {
                        values.into_iter().map(encode_bool).collect()
                    }
                    FlattenedExternArrayValues::Int(values) => values
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| {
                            encode_int(value).map_err(|error| {
                                ctx.eval_error(
                                    format!(
                                        "extern function `{ext}` parameter `{}` has an invalid Int at flat array index {index}: {error}",
                                        param.name
                                    ),
                                    arg.span,
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };
                let array = HostArray::try_new(shape, encoded_values).map_err(|error| {
                    ctx.internal_error(
                        format!("failed to flatten extern array argument: {error}"),
                        arg.span,
                    )
                })?;
                HostFnValue::Array(array)
            }
            (ValueKind::Scalar(_) | ValueKind::Struct(_), _) => {
                return Err(ctx.internal_error(
                    format!(
                        "extern function `{ext}` parameter `{}` received a value of the wrong kind after dimension checking",
                        param.name
                    ),
                    arg.span,
                ));
            }
        };
        arg_values.push(converted);
    }

    let result = host_fn(&arg_values).map_err(|err| {
        ctx.eval_error(
            format!(
                "extern function `{}` (plugin \"{}\") failed: {}",
                ext, ext.plugin, err.message
            ),
            expr.span,
        )
    })?;

    let decoded = decode_result(signature.result(), &result).map_err(|error| {
        ctx.eval_error(
            format!("extern function `{ext}` returned an invalid ABI result: {error}"),
            expr.span,
        )
    })?;

    match decoded {
        ValidatedHostResult::Quantity { value, .. } => Ok(RuntimeValue::Quantity(value)),
        ValidatedHostResult::Int(value) => Ok(RuntimeValue::Int(value)),
        ValidatedHostResult::Bool(value) => Ok(RuntimeValue::Bool(value)),
        ValidatedHostResult::Array(array) => {
            let axes = array
                .indexes()
                .iter()
                .map(|index| {
                    bound_indexes.get(index).cloned().ok_or_else(|| {
                        ctx.internal_error(
                            format!(
                                "extern function `{ext}` result index variable `{index}` was not bound by any argument"
                            ),
                            expr.span,
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let expected_shape = axes.iter().map(|axis| axis.keys.len()).collect::<Vec<_>>();
            if array.shape() != expected_shape {
                return Err(ctx.eval_error(
                    format!(
                        "extern function `{ext}` returned shape {:?}, expected {expected_shape:?}",
                        array.shape()
                    ),
                    expr.span,
                ));
            }
            match array.values() {
                ValidatedHostArrayValues::Quantity { values, .. } => rebuild_extern_array(
                    &axes,
                    values,
                    |value| RuntimeValue::Quantity(*value),
                    ctx,
                    expr.span,
                ),
                ValidatedHostArrayValues::Bool(values) => rebuild_extern_array(
                    &axes,
                    values,
                    |value| RuntimeValue::Bool(*value),
                    ctx,
                    expr.span,
                ),
                ValidatedHostArrayValues::Int(values) => rebuild_extern_array(
                    &axes,
                    values,
                    |value| RuntimeValue::Int(*value),
                    ctx,
                    expr.span,
                ),
            }
        }
        ValidatedHostResult::Struct(fields) => {
            let Some(result_struct) = &function.result_struct else {
                return Err(ctx.internal_error(
                    format!(
                        "extern function `{ext}` declares a struct result without a resolved record type"
                    ),
                    expr.span,
                ));
            };
            let fields = fields
                .iter()
                .map(|field| {
                    let value = match field.value() {
                        ValidatedHostFieldValue::Bool(value) => RuntimeValue::Bool(*value),
                        ValidatedHostFieldValue::Int(value) => RuntimeValue::Int(*value),
                        ValidatedHostFieldValue::Quantity(value) => RuntimeValue::Quantity(*value),
                    };
                    (field.name().clone(), value)
                })
                .collect::<indexmap::IndexMap<_, _>>();
            Ok(RuntimeValue::Struct {
                type_name: result_struct.resolved.clone(),
                constructor: result_struct.constructor.clone(),
                generic_args: Vec::new(),
                fields,
            })
        }
    }
}

fn eval_hir_builtin_fn(
    expr: &hir::Expr,
    name: BuiltinFnName,
    args: &[hir::Expr],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let builtin = ctx
        .builtin_fns
        .get(&name)
        .ok_or_else(|| ctx.eval_error(format!("unknown function `{name}`"), expr.span))?;
    let arg_values: Vec<f64> = args
        .iter()
        .map(|arg| {
            let rv = eval_hir_expr(arg, values, local_values, ctx)?;
            rv.expect_quantity("function argument")
                .map_err(|e| ctx.eval_error(e.to_string(), arg.span))
        })
        .collect::<Result<_, _>>()?;
    let result = builtin
        .eval(&arg_values)
        .map_err(|error| ctx.eval_error(format!("builtin function `{name}` {error}"), expr.span))?;
    checked_finite_quantity(
        super::arithmetic::check_finite(result, name.as_str(), ctx, expr.span)?,
        name.as_str(),
        expr.span,
        ctx,
    )
}

fn eval_hir_field_access(
    inner_val: RuntimeValue,
    inner: &hir::Expr,
    field: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::type_name::FieldName,
    >,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match inner_val {
        RuntimeValue::Struct {
            type_name,
            constructor,
            generic_args,
            fields,
        } => {
            let DeclaredType::Struct(expected, expected_args) = checked_value_type(inner, ctx)?
            else {
                return Err(ctx.internal_error(
                    "field access has no retained struct operand type",
                    inner.span,
                ));
            };
            let expected_runtime = ctx
                .current_dag
                .runtime_struct_type_identity(expected.resolved());
            // Validate the actual tag against the retained expected type, never
            // resolve a source name or infer a constructor application here.
            let definition = ctx
                .tir
                .struct_type_def(expected.resolved())
                .ok_or_else(|| {
                    ctx.internal_error(
                        "retained field operand has no nominal definition",
                        inner.span,
                    )
                })?;
            let exposes_field = definition.union_members().is_some_and(|members| {
                members.iter().any(|member| {
                    member.name() == constructor
                        && member
                            .fields()
                            .iter()
                            .any(|declared| declared.name() == &field.value)
                })
            });
            fields
                .get(&field.value)
                .filter(|_| type_name == expected_runtime && generic_args == *expected_args && exposes_field)
                .cloned()
                .ok_or_else(|| {
                    ctx.eval_error(
                        format!("no field `{}` on `{type_name}::{constructor}`; expected `{expected_runtime}`", field.value),
                        field.span,
                    )
                })
        }
        _ => Err(ctx.eval_error("field access on non-struct value", inner.span)),
    }
}

fn eval_hir_constructor_call(
    expr: &hir::Expr,
    fields: &[hir::expr::FieldInit],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let application = checked_constructor(expr, ctx)?;
    let constructor_name = &application.constructor;
    let owning_type = StructTypeRef::from_resolved(application.definition.clone());
    let mut field_map = IndexMap::new();
    let mut field_presentations = HashMap::new();
    for field_init in fields {
        let evaluated = eval_hir_expr_evaluated(
            &field_init.value,
            values,
            presentation_values,
            local_values,
            ctx,
        )?;
        let (val, presentation) = evaluated.into_parts();
        if application
            .required_constraints
            .contains(&field_init.name.value)
            && let Some(field_constraints) = ctx.struct_field_constraints()
        {
            let key =
                graphcal_compiler::tir::typed::model::StructFieldConstraintKey::for_application(
                    owning_type.clone(),
                    application.generic_args.clone(),
                    constructor_name.clone(),
                    field_init.name.value.clone(),
                );
            let constraint = field_constraints.get(&key).ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "required field constraint `{constructor_name}.{}` is missing",
                        field_init.name.value
                    ),
                    field_init.value.span,
                )
            })?;
            if let Err(violation) = crate::domain_check::check_domain_constraint(&val, constraint) {
                return Err(ctx.eval_error(
                    format!(
                        "field `{constructor_name}.{}` {}",
                        field_init.name.value, violation.message
                    ),
                    field_init.value.span,
                ));
            }
        }
        field_map.insert(field_init.name.value.clone(), val);
        if !presentation.is_none() {
            field_presentations.insert(field_init.name.value.clone(), presentation);
        }
    }
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Struct {
            type_name: application.runtime_type.clone(),
            constructor: constructor_name.clone(),
            generic_args: application.generic_args.clone(),
            fields: field_map,
        },
        PresentationInstance::fields(field_presentations),
    ))
}

fn index_def_for_ref<'a>(
    index_ref: &IndexTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<std::borrow::Cow<'a, IndexDef>> {
    ctx.tir.index_def(index_ref)
}

fn ensure_index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::syntax::index_name::ResolvedIndexName,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    if index_ref_matches_resolved(actual, expected) {
        return Ok(());
    }
    Err(ctx.eval_error(
        format!(
            "index argument belongs to `{}`, but value is indexed by `{}`",
            expected.as_str(),
            actual.display_name()
        ),
        span,
    ))
}

fn map_entry_variant_for_axis(
    key: &hir::expr::MapEntryKey,
    axis: &IndexTypeRef,
    ctx: &EvalContext<'_>,
) -> Result<IndexEntryKey, GraphcalError> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => {
            ensure_index_ref_matches_resolved(
                axis,
                variant.variant.index(),
                variant.variant_span,
                ctx,
            )?;
            Ok(IndexEntryKey::named(variant.variant.variant().clone()))
        }
        hir::expr::MapEntryKey::FinitePosition { position, .. } => {
            Ok(IndexEntryKey::position(position.value))
        }
    }
}

fn map_entry_key_span(key: &hir::expr::MapEntryKey) -> Span {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => variant.path_span(),
        hir::expr::MapEntryKey::FinitePosition { position, .. } => position.span,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "recursive map evaluation keeps values and sparse presentation entries reordered atomically"
)]
fn eval_hir_map_literal(
    checked_type: &DeclaredType,
    map_span: Span,
    entries: &[hir::expr::MapEntry],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let first = entries
        .first()
        .ok_or_else(|| ctx.internal_error("empty map literal", map_span))?;
    let first_key = first.keys.first();
    let arity = first.keys.len();
    let DeclaredType::Indexed { element, index } = checked_type else {
        return Err(ctx.internal_error("map has no retained indexed type", map_span));
    };
    let idx_name = index.clone();

    if arity == 1 {
        let idx_def = index_def_for_ref(&idx_name, ctx).ok_or_else(|| {
            ctx.internal_error(
                format!("unknown index `{idx_name}`"),
                map_entry_key_span(first_key),
            )
        })?;
        let mut evaluated = IndexMap::new();
        for entry in entries {
            let key = entry.keys.first();
            let variant = map_entry_variant_for_axis(key, &idx_name, ctx)?;
            let value = eval_hir_expr_evaluated(
                &entry.value,
                values,
                presentation_values,
                local_values,
                ctx,
            )?;
            evaluated.insert(variant, value);
        }
        let mut result = IndexMap::new();
        let mut presentations = IndexMap::new();
        for variant in idx_def.entry_keys() {
            let evaluated = evaluated.swap_remove(&variant).ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "map literal for index `{idx_name}` is missing entry for variant `{variant}`"
                    ),
                    map_span,
                )
            })?;
            let (value, presentation) = evaluated.into_parts();
            result.insert(variant.clone(), value);
            if !presentation.is_none() {
                presentations.insert(variant, presentation);
            }
        }
        return Ok(EvaluatedRuntimeValue::new(
            RuntimeValue::Indexed {
                index_name: idx_name,
                entries: result,
            },
            PresentationInstance::entries(presentations),
        ));
    }

    let idx_def = index_def_for_ref(&idx_name, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("unknown index `{idx_name}`"),
            map_entry_key_span(first_key),
        )
    })?;
    let variants = idx_def.entry_keys();
    let mut outer = IndexMap::new();
    let mut presentations = IndexMap::new();
    for variant in &variants {
        let mut sub_entries = Vec::new();
        for entry in entries {
            let first_entry_key = entry.keys.first();
            if map_entry_variant_for_axis(first_entry_key, &idx_name, ctx)? != *variant {
                continue;
            }
            let keys = graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(
                entry.keys.as_slice()[1..].to_vec(),
            )
            .map_err(|_| {
                ctx.internal_error(
                    "multi-axis map literal entry lost all keys",
                    entry.value.span,
                )
            })?;
            sub_entries.push(hir::expr::MapEntry {
                keys,
                value: entry.value.clone(),
            });
        }
        if sub_entries.is_empty() {
            return Err(ctx.internal_error(
                format!(
                    "map literal for index `{idx_name}` is missing entries for variant `{variant}`"
                ),
                map_span,
            ));
        }
        let evaluated = eval_hir_map_literal(
            element,
            map_span,
            &sub_entries,
            values,
            presentation_values,
            local_values,
            ctx,
        )?;
        let (inner, presentation) = evaluated.into_parts();
        outer.insert(variant.clone(), inner);
        if !presentation.is_none() {
            presentations.insert(variant.clone(), presentation);
        }
    }
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed {
            index_name: idx_name,
            entries: outer,
        },
        PresentationInstance::entries(presentations),
    ))
}

fn eval_hir_for_comp(
    expr: &hir::Expr,
    bindings: &[hir::expr::ForBinding],
    body: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    match &ctx.expression_fact(expr)?.fact {
        graphcal_compiler::tir::expression_facts::ExpressionFact::Value {
            shape: graphcal_compiler::tir::expression_facts::ExpressionShape::Concrete(_),
            ..
        } => {}
        _ => {
            return Err(ctx.internal_error(
                "materialized expression has no concrete checked shape",
                expr.span,
            ));
        }
    }
    eval_hir_for_comp_bindings(
        checked_value_type(expr, ctx)?,
        bindings,
        body,
        values,
        presentation_values,
        local_values,
        ctx,
    )
}

fn eval_hir_for_comp_bindings(
    checked_type: &DeclaredType,
    bindings: &[hir::expr::ForBinding],
    body: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let DeclaredType::Indexed { element, index } = checked_type else {
        return Err(ctx.internal_error(
            "comprehension binding has no retained indexed type",
            binding.local.span,
        ));
    };
    let idx_name = index.clone();
    let error_span = binding.local.span;

    let idx_def = index_def_for_ref(&idx_name, ctx)
        .ok_or_else(|| ctx.internal_error(format!("unknown index `{idx_name}`"), error_span))?;

    let remaining = &bindings[1..];
    let variants = idx_def.entry_keys();
    let mut entries = IndexMap::new();
    let mut presentations = IndexMap::new();
    let mut inner_locals = local_values.child(Vec::new());
    for (position, variant) in variants.iter().enumerate() {
        let binding_value = match (&idx_def.kind, variant) {
            (IndexKind::Named { .. } | IndexKind::RequiredNamed, IndexEntryKey::Named(name)) => {
                RuntimeValue::Label {
                    index_name: idx_name.clone(),
                    variant: name.clone(),
                }
            }
            (IndexKind::Coordinate(data), IndexEntryKey::Position(_)) => {
                RuntimeValue::coordinate_label(
                    idx_name.clone(),
                    position,
                    data.coordinate_value(position),
                )
                .map_err(|error| ctx.internal_error(error.to_string(), error_span))?
            }
            (IndexKind::Finite { .. }, IndexEntryKey::Position(_)) => {
                RuntimeValue::Int(i64::try_from(position).map_err(|_| {
                    ctx.internal_error(
                        format!("Fin position {position} is too large for i64"),
                        error_span,
                    )
                })?)
            }
            (IndexKind::RequiredCoordinate { .. }, _) => {
                return Err(
                    ctx.internal_error("RequiredCoordinate should have been bound", error_span)
                );
            }
            (IndexKind::Named { .. } | IndexKind::RequiredNamed, IndexEntryKey::Position(_))
            | (IndexKind::Coordinate(_) | IndexKind::Finite { .. }, IndexEntryKey::Named(_)) => {
                return Err(ctx.internal_error(
                    "registry entry-key category does not match its index kind",
                    error_span,
                ));
            }
        };
        inner_locals.bind(
            binding.local.id,
            EvaluatedRuntimeValue::plain(binding_value),
        );
        let evaluated = if remaining.is_empty() {
            eval_hir_expr_evaluated(body, values, presentation_values, &inner_locals, ctx)?
        } else {
            eval_hir_for_comp_bindings(
                element,
                remaining,
                body,
                values,
                presentation_values,
                &inner_locals,
                ctx,
            )?
        };
        let (value, presentation) = evaluated.into_parts();
        entries.insert(variant.clone(), value);
        if !presentation.is_none() {
            presentations.insert(variant.clone(), presentation);
        }
    }
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed {
            index_name: idx_name,
            entries,
        },
        PresentationInstance::entries(presentations),
    ))
}

#[expect(
    clippy::too_many_lines,
    clippy::single_match_else,
    reason = "single pattern dispatch keeps borrowed graph references and every index-argument category explicit"
)]
fn eval_hir_index_access(
    span: Span,
    inner: &hir::Expr,
    args: &[hir::expr::IndexArg],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let (base_value, base_presentation) = match inner.kind() {
        hir::ExprKind::GraphRef(target) => {
            // This replaces the checkpoint normally performed by
            // `eval_hir_expr(inner, ...)` while retaining a reference to the
            // stored value instead of deep-cloning it before traversal.
            ctx.cancellation.checkpoint()?;
            let value = std::borrow::Cow::Borrowed(resolve_hir_graph_ref(
                &target.value,
                target.span,
                values,
                ctx,
            )?);
            let presentation = presentation_instance_for(
                presentation_values,
                &RuntimeDeclKey::resolved(ctx.current_dag.runtime_decl_identity(&target.value)),
            );
            (value, presentation)
        }
        _ => {
            let evaluated =
                eval_hir_expr_evaluated(inner, values, presentation_values, local_values, ctx)?;
            let (value, presentation) = evaluated.into_parts();
            (std::borrow::Cow::Owned(value), presentation)
        }
    };
    let mut current = base_value.as_ref();
    let mut selected_keys = Vec::with_capacity(args.len());
    for arg in args {
        let RuntimeValue::Indexed {
            index_name,
            entries,
        } = current
        else {
            return Err(ctx.eval_error("indexing a non-indexed value", span));
        };
        let entry_key = match arg {
            hir::expr::IndexArg::Variant(variant) => {
                ensure_index_ref_matches_resolved(
                    index_name,
                    variant.variant.index(),
                    variant.path_span(),
                    ctx,
                )?;
                IndexEntryKey::named(variant.variant.variant().clone())
            }
            hir::expr::IndexArg::Var(local) => {
                let var_val = local_values
                    .get(local.value)
                    .ok_or_else(|| ctx.eval_error("undefined loop variable", local.span))?;
                match var_val.value() {
                    RuntimeValue::Label {
                        index_name: label_index,
                        variant,
                    } => {
                        if !index_name.matches_ref(label_index) {
                            return Err(ctx.eval_error(
                                "index argument belongs to a different index",
                                local.span,
                            ));
                        }
                        IndexEntryKey::named(variant.clone())
                    }
                    RuntimeValue::CoordinateLabel {
                        index_name: label_index,
                        position,
                        ..
                    } => {
                        if !index_name.matches_ref(label_index) {
                            return Err(ctx.eval_error(
                                format!(
                                    "index argument belongs to `{}`, but value is indexed by `{}`",
                                    label_index.display_name(),
                                    index_name.display_name()
                                ),
                                local.span,
                            ));
                        }
                        IndexEntryKey::position(u64::try_from(*position).map_err(|_| {
                            ctx.internal_error("coordinate position does not fit u64", local.span)
                        })?)
                    }
                    RuntimeValue::Int(n) => {
                        if *n < 0 {
                            return Err(ctx.eval_error(
                                format!("index variable evaluated to negative value: {n}"),
                                local.span,
                            ));
                        }
                        IndexEntryKey::position(u64::try_from(*n).map_err(|_| {
                            ctx.eval_error(format!("index variable is too large: {n}"), local.span)
                        })?)
                    }
                    _ => return Err(ctx.eval_error("value is not a loop variable", local.span)),
                }
            }
            hir::expr::IndexArg::Expr(index_expr) => {
                let val = eval_hir_expr(index_expr, values, local_values, ctx)?;
                match val {
                    // A key value selects the entry it names; the checker has
                    // already proven the axis identity.
                    RuntimeValue::Label { variant, .. } => IndexEntryKey::named(variant),
                    RuntimeValue::CoordinateLabel { position, .. } => {
                        IndexEntryKey::position(u64::try_from(position).map_err(|_| {
                            ctx.internal_error(
                                format!("coordinate key position {position} is unrepresentable"),
                                index_expr.span,
                            )
                        })?)
                    }
                    RuntimeValue::Int(n) => {
                        if n < 0 {
                            return Err(ctx.eval_error(
                                format!("index expression evaluated to negative value: {n}"),
                                index_expr.span,
                            ));
                        }
                        IndexEntryKey::position(u64::try_from(n).map_err(|_| {
                            ctx.eval_error(
                                format!("index expression is too large: {n}"),
                                index_expr.span,
                            )
                        })?)
                    }
                    _ => {
                        return Err(ctx.eval_error(
                            "index expression must evaluate to an integer or an index key",
                            index_expr.span,
                        ));
                    }
                }
            }
        };
        current = entries
            .get(&entry_key)
            .ok_or_else(|| ctx.eval_error(format!("index entry `{entry_key}` not found"), span))?;
        selected_keys.push(entry_key);
    }
    let presentation = base_presentation
        .project_indexes(&selected_keys)
        .map_err(|error| ctx.internal_error(error.to_string(), span))?;
    Ok(EvaluatedRuntimeValue::new(
        clone_hir_index_access_result(current),
        presentation,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "scan evaluation is called from expression destructuring"
)]
fn eval_hir_scan(
    source: &hir::Expr,
    init: &hir::Expr,
    acc: &hir::LocalDef,
    val: &hir::LocalDef,
    body: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let (source_val, source_presentation) =
        eval_hir_expr_evaluated(source, values, presentation_values, local_values, ctx)?
            .into_parts();
    let RuntimeValue::Indexed {
        index_name,
        entries: source_entries,
    } = source_val
    else {
        return Err(ctx.eval_error("scan source must be an indexed value", source.span));
    };
    if source_entries
        .values()
        .next()
        .is_some_and(|item| matches!(item, RuntimeValue::Indexed { .. }))
    {
        return Err(ctx.internal_error(
            "multi-axis source reached scan evaluation after rank-one type checking",
            source.span,
        ));
    }
    let evaluated_init =
        eval_hir_expr_evaluated(init, values, presentation_values, local_values, ctx)?;
    let (_, initial_presentation) = evaluated_init.clone().into_parts();
    let mut accumulated = evaluated_init;
    let mut result_entries = IndexMap::new();
    let mut presentations = IndexMap::new();
    let mut scan_locals = local_values.child(Vec::new());
    for (variant, item) in &source_entries {
        scan_locals.bind(acc.id, accumulated);
        let item_presentation = source_presentation
            .project_indexes_ref(std::slice::from_ref(variant))
            .cloned()
            .map_err(|error| ctx.internal_error(error.to_string(), source.span))?;
        scan_locals.bind(
            val.id,
            EvaluatedRuntimeValue::new(item.clone(), item_presentation),
        );
        accumulated =
            eval_hir_expr_evaluated(body, values, presentation_values, &scan_locals, ctx)?
                .with_default_presentation(&initial_presentation);
        let (value, evidence) = accumulated.clone().into_parts();
        result_entries.insert(variant.clone(), value);
        if !evidence.is_none() {
            presentations.insert(variant.clone(), evidence);
        }
    }
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed {
            index_name,
            entries: result_entries,
        },
        PresentationInstance::entries(presentations),
    ))
}

fn eval_hir_unfold(
    expr: &hir::Expr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let hir::ExprKind::Unfold {
        recurrence,
        init,
        body,
    } = expr.kind()
    else {
        return Err(ctx.internal_error("unfold evaluator received another operation", expr.span));
    };
    let axis = &recurrence.axis;
    let DeclaredType::Indexed { index, .. } = checked_value_type(expr, ctx)? else {
        return Err(ctx.internal_error("unfold has no retained indexed type", expr.span));
    };
    let index_ref = index.clone();
    let idx_def = index_def_for_ref(&index_ref, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("missing resolved unfold axis `{}`", axis.value),
            axis.span,
        )
    })?;
    let variants = idx_def.entry_keys();
    let coordinate_data = idx_def.coordinate_data().ok_or_else(|| {
        ctx.eval_error(
            format!(
                "unfold requires a coordinate index, but `{index_ref}` is not coordinate-valued"
            ),
            axis.span,
        )
    })?;
    let evaluated_init =
        eval_hir_expr_evaluated(init, values, presentation_values, local_values, ctx)?;
    let mut previous_state = evaluated_init.clone();
    let (init_value, init_presentation) = evaluated_init.into_parts();
    let mut result_entries = IndexMap::new();
    let mut presentations = IndexMap::new();
    if !init_presentation.is_none() {
        presentations.insert(variants[0].clone(), init_presentation.clone());
    }
    result_entries.insert(variants[0].clone(), init_value);

    let mut unfold_locals = local_values.child(Vec::new());
    for (position, variant) in variants.iter().enumerate().skip(1) {
        let previous_position = position
            .checked_sub(1)
            .ok_or_else(|| ctx.internal_error("unfold step has no previous position", axis.span))?;
        unfold_locals.bind(recurrence.previous_state.id, previous_state);
        unfold_locals.bind(
            recurrence.previous_index.id,
            EvaluatedRuntimeValue::plain(
                RuntimeValue::coordinate_label(
                    index_ref.clone(),
                    previous_position,
                    coordinate_data.coordinate_value(previous_position),
                )
                .map_err(|error| ctx.internal_error(error.to_string(), axis.span))?,
            ),
        );
        unfold_locals.bind(
            recurrence.current_index.id,
            EvaluatedRuntimeValue::plain(
                RuntimeValue::coordinate_label(
                    index_ref.clone(),
                    position,
                    coordinate_data.coordinate_value(position),
                )
                .map_err(|error| ctx.internal_error(error.to_string(), axis.span))?,
            ),
        );
        previous_state =
            eval_hir_expr_evaluated(body, values, presentation_values, &unfold_locals, ctx)?
                .with_default_presentation(&init_presentation);
        let (value, evidence) = previous_state.clone().into_parts();
        result_entries.insert(variant.clone(), value);
        if !evidence.is_none() {
            presentations.insert(variant.clone(), evidence);
        }
    }
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed {
            index_name: index_ref,
            entries: result_entries,
        },
        PresentationInstance::entries(presentations),
    ))
}

fn evaluated_match_field(
    field: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::type_name::FieldName,
    >,
    type_name: &graphcal_compiler::syntax::type_name::ResolvedStructTypeName,
    fields: &IndexMap<graphcal_compiler::syntax::type_name::FieldName, RuntimeValue>,
    presentation: &PresentationInstance,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let value = fields.get(&field.value).ok_or_else(|| {
        ctx.eval_error(
            format!("no field `{}` on type `{type_name}`", field.value),
            field.span,
        )
    })?;
    let evidence = presentation
        .project_field_ref(&field.value)
        .cloned()
        .map_err(|error| ctx.internal_error(error.to_string(), field.span))?;
    Ok(EvaluatedRuntimeValue::new(value.clone(), evidence))
}

fn eval_hir_match(
    expr: &hir::Expr,
    scrutinee: &hir::Expr,
    arms: &[hir::expr::MatchArm],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let span = expr.span;
    let constructor_matches = &ctx.expression_fact(expr)?.constructor_matches;
    let (scrutinee_val, scrutinee_presentation) =
        eval_hir_expr_evaluated(scrutinee, values, presentation_values, local_values, ctx)?
            .into_parts();
    match &scrutinee_val {
        RuntimeValue::Label {
            index_name,
            variant,
        } => {
            let matched_arm = arms
                .iter()
                .find(|arm| match &arm.pattern {
                    hir::expr::MatchPattern::IndexLabel { variant: pat, .. } => {
                        index_ref_matches_resolved(index_name, pat.variant.index())
                            && pat.variant.variant() == variant
                    }
                    hir::expr::MatchPattern::Constructor { .. } => false,
                })
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for label `{variant}`"), span)
                })?;
            eval_hir_expr_evaluated(
                &matched_arm.body,
                values,
                presentation_values,
                local_values,
                ctx,
            )
        }
        RuntimeValue::Struct {
            type_name,
            constructor: value_constructor,
            fields: scrutinee_fields,
            ..
        } => {
            let matched_arm = arms
                .iter()
                .find_map(|arm| match &arm.pattern {
                    hir::expr::MatchPattern::Constructor { constructor, .. } => {
                        constructor_matches.get(&constructor.value).map_or_else(
                            || {
                                Some(Err(ctx.internal_error(
                                    "match arm has no retained constructor target",
                                    arm.span,
                                )))
                            },
                            |target| {
                                (*value_constructor == target.constructor
                                    && *type_name == target.runtime_type)
                                    .then_some(Ok(arm))
                            },
                        )
                    }
                    hir::expr::MatchPattern::IndexLabel { .. } => None,
                })
                .transpose()?
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for variant `{type_name}`"), span)
                })?;
            let mut arm_locals = local_values.child(Vec::new());
            let hir::expr::MatchPattern::Constructor { bindings, .. } = &matched_arm.pattern else {
                return Err(
                    ctx.internal_error("matched non-constructor arm for struct", matched_arm.span)
                );
            };
            for binding in bindings {
                match binding {
                    hir::expr::PatternBinding::Bind { field, local } => {
                        arm_locals.bind(
                            local.id,
                            evaluated_match_field(
                                field,
                                type_name,
                                scrutinee_fields,
                                &scrutinee_presentation,
                                ctx,
                            )?,
                        );
                    }
                    hir::expr::PatternBinding::Wildcard { .. } => {}
                }
            }
            eval_hir_expr_evaluated(
                &matched_arm.body,
                values,
                presentation_values,
                &arm_locals,
                ctx,
            )
        }
        _ => Err(ctx.eval_error(
            "match scrutinee must be a label or tagged union",
            scrutinee.span,
        )),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "inline-call evaluation keeps the checked DAG environment, semantic values, and presentation sidecars in one transaction"
)]
fn eval_hir_dag_call(
    call: &hir::Expr,
    target: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::dag_id::DagId>,
    args: &[hir::expr::ParamBinding],
    output: &graphcal_compiler::syntax::span::Spanned<ResolvedDeclKey>,
    caller_values: &RuntimeValueMap,
    caller_presentations: Option<&PresentationInstanceMap>,
    caller_locals: &HirLocalValueMap,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let call_span = call.span;
    let plan = ctx.execution_plan()?;
    let callable = plan
        .callable(&target.value)
        .map_err(|error| ctx.internal_error(error.to_string(), target.span))?;
    let checked = &plan.checked_execution_facts;
    let scope = crate::execution_scope::CheckedExecutionScope::new(ctx.tir, checked, &target.value)
        .map_err(|error| ctx.internal_error(error.to_string(), target.span))?;
    let dag_facts = scope.facts();

    let mut frame = crate::execution_frame::ExecutionFrame::new(
        plan,
        &target.value,
        crate::execution_frame::FailurePolicy::Propagate,
    )
    .map_err(|error| ctx.internal_error(error.to_string(), call_span))?;
    for binding in args {
        let key = super::dag_decl_runtime_key(&binding.target.value);
        let (value, evidence) = eval_hir_expr_evaluated(
            &binding.value,
            caller_values,
            caller_presentations,
            caller_locals,
            ctx,
        )?
        .into_parts();
        frame.bind(&key, value, ctx.src, binding.value.span)?;
        if !evidence.is_none() {
            frame.presentations.insert(key, evidence);
        }
    }
    seed_inline_dag_imported_values(
        &callable.imports.runtime,
        &mut frame.values,
        &mut frame.presentations,
        caller_values,
        caller_presentations,
        ctx,
    );

    let empty_hir_locals = HirLocalValueMap::root();
    frame.run(
        ctx.tir,
        dag_facts.source(),
        &ctx.cancellation,
        |entry, frame| {
            let context = ctx
                .for_dag(entry.scope.dag(), entry.scope.facts().source())?
                .for_decl(entry.key.as_resolved());
            eval_hir_expr_evaluated(
                entry.expression,
                &frame.values,
                Some(&frame.presentations),
                &empty_hir_locals,
                &context,
            )
        },
    )?;
    let mut dag_presentations = frame.presentations;
    let dag_values = frame.values;

    check_inline_plan_asserts(
        &callable.execution_dags,
        &dag_values,
        target,
        output.span,
        ctx,
    )?;

    let output_key = super::dag_decl_runtime_key(&output.value);
    let output_value = dag_values.get(&output_key).cloned().ok_or_else(|| {
        ctx.internal_error(
            format!(
                "dag `{}` has no projected value `{}` after evaluation (should have been caught by dim-check)",
                target.value,
                output.value.as_str()
            ),
            output.span,
        )
    })?;
    let output_presentation = take_presentation_instance(&mut dag_presentations, &output_key);
    let presentation = super::presentation::resolve_frame(
        output_presentation,
        &dag_values,
        ctx,
        &callable.execution_dags,
    )?;
    #[cfg(test)]
    record_call_retention(&dag_values, &presentation);
    Ok(EvaluatedRuntimeValue::new(output_value, presentation))
}

#[cfg(test)]
fn record_call_retention(values: &RuntimeValueMap, presentation: &PresentationInstance) {
    use crate::pipeline_metrics::{Event, record_many};
    record_many(
        Event::CallFrameValueNodes,
        u64::try_from(
            values
                .values()
                .map(runtime_value_tree_node_count)
                .sum::<usize>(),
        )
        .unwrap_or(u64::MAX),
    );
    record_many(
        Event::CallOutputEvidenceNodes,
        u64::try_from(presentation.retained_nodes()).unwrap_or(u64::MAX),
    );
}

/// Only explicit prepared runtime imports may consult the caller or root frame.
/// Supplied/current values and retained checked constants always win.
fn seed_inline_dag_imported_values(
    imports: &[RuntimeDeclKey],
    dag_values: &mut RuntimeValueMap,
    dag_presentations: &mut PresentationInstanceMap,
    caller_values: &RuntimeValueMap,
    caller_presentations: Option<&PresentationInstanceMap>,
    ctx: &EvalContext<'_>,
) {
    for key in imports {
        if dag_values.contains_key(key) {
            continue;
        }
        if let Some(value) = imported_binding_value(key.as_resolved(), caller_values, ctx) {
            dag_values.insert(key.clone(), value.clone());
            let presentation = if key.as_resolved().owner() == ctx.current_dag.dag_id() {
                caller_presentations.and_then(|instances| instances.get(key))
            } else if key.as_resolved().owner() == ctx.tir.root_dag_id() {
                ctx.root_presentation_instances
                    .and_then(|instances| instances.get(key))
            } else {
                None
            };
            if let Some(presentation) = presentation
                && !presentation.is_none()
            {
                dag_presentations.insert(key.clone(), presentation.clone());
            }
        }
    }
}

fn check_inline_plan_asserts(
    owners: &[graphcal_compiler::dag_id::DagId],
    values: &RuntimeValueMap,
    target: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::dag_id::DagId>,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    let checked = &ctx.execution_plan()?.checked_execution_facts;
    owners.iter().try_for_each(|owner| {
        let scope = crate::execution_scope::CheckedExecutionScope::new(ctx.tir, checked, owner)
            .map_err(|error| ctx.internal_error(error.to_string(), span))?;
        let context = ctx.for_dag(scope.dag(), scope.facts().source())?;
        check_inline_dag_asserts(scope.dag(), values, &context, target, span, ctx)
    })
}

/// Check the asserts of an inline-instantiated dag body (#812).
///
/// An inline call site is a fresh instantiation (sugar for a synthetic
/// include), so the dag's asserts are checked here too — instantiating
/// inline must not silently skip the dag's invariants. Unlike the include
/// path, an expression has no reporting surface, so a FAIL or ERROR fails
/// the calling expression (fault-isolated to the calling declaration).
/// `#[expected_fail]` inversion applies as usual.
fn check_inline_dag_asserts(
    dag_tir: &DagTIR,
    dag_values: &RuntimeValueMap,
    dag_ctx: &EvalContext<'_>,
    target: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::dag_id::DagId>,
    call_span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    let empty_hir_locals = HirLocalValueMap::root();
    let callable = ctx
        .execution_plan()?
        .callable(dag_tir.dag_id())
        .map_err(|error| ctx.internal_error(error.to_string(), call_span))?;
    for (name, cat) in dag_tir.source_order() {
        if !matches!(cat, DeclCategory::Assert) {
            continue;
        }
        let key = dag_tir
            .lookup_decl_identity(name)
            .into_bound()
            .map_err(|probe| ctx.internal_error(probe.to_string(), call_span))?;
        let body = dag_tir.assert_body(&key).ok_or_else(|| {
            ctx.internal_error(
                format!("TIR assertion entry missing for DAG assertion `{name}`"),
                call_span,
            )
        })?;
        let ef = callable
            .expected_fail
            .get(&RuntimeDeclKey::resolved(key.clone()));
        let result =
            crate::assertion_eval::evaluate_assert_with_expected_fail(body, ef, &mut |expr| {
                eval_hir_expr(expr, dag_values, &empty_hir_locals, &dag_ctx.for_decl(&key))
            });
        match result {
            crate::eval::types::AssertResult::Pass => {}
            crate::eval::types::AssertResult::Fail { message } => {
                return Err(ctx.eval_error(
                    format!(
                        "assertion `{name}` failed in inline call of dag `{}` ({message})",
                        target.value.name()
                    ),
                    call_span,
                ));
            }
            crate::eval::types::AssertResult::Error { message } => {
                return Err(ctx.eval_error(
                    format!(
                        "assertion `{name}` errored in inline call of dag `{}` ({message})",
                        target.value.name()
                    ),
                    call_span,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use miette::Diagnostic;

    use super::*;
    use crate::eval::compile_and_eval;

    #[test]
    fn indexed_graph_ref_borrows_collection_before_selecting_entry() {
        reset_hir_cloned_runtime_node_count();
        let direct_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = @source;
";
        compile_and_eval(direct_copy).unwrap();
        assert_eq!(
            take_hir_cloned_runtime_node_count(),
            21,
            "the clone observer must count the root, four rows, and 16 leaves"
        );

        let elementwise_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    @source[row, column]
};
";
        compile_and_eval(elementwise_copy).unwrap();
        assert_eq!(
            take_hir_cloned_runtime_node_count(),
            16,
            "index traversal must clone only the 16 selected leaves"
        );
    }

    #[test]
    fn empty_aggregation_is_reported_as_x001() {
        let entries = IndexMap::new();
        let src = NamedSource::new("test.gcl", Arc::new(String::new()));

        for function in [
            AggregationFn::Sum,
            AggregationFn::Product,
            AggregationFn::Minimum,
            AggregationFn::Maximum,
            AggregationFn::Mean,
            AggregationFn::RootSumSquare,
            AggregationFn::Count,
        ] {
            let error =
                eval_hir_aggregation_fn(function, &entries, Span::new(0, 0), &src).unwrap_err();
            assert!(matches!(&error, GraphcalError::InternalError { .. }));
            let code = error.code().map(|code| code.to_string());
            assert_eq!(code.as_deref(), Some("graphcal::X001"));
        }
    }
}
