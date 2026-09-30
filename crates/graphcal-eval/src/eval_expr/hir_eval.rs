use std::collections::HashMap;
use std::sync::Arc;

use crate::runtime_value::{
    IndexAxis, IndexedValue, KeyElement, KeyValue, RuntimeValue, StructValue,
};
use graphcal_compiler::builtin::{
    AggregationFn, BuiltinFn, ConversionFn, DatetimeConstructorFn, DatetimeField, DatetimeFn,
    DatetimeFromNumericFn, DatetimeToNumericFn, KeyAggregation, ScalarFn, ValueAggregation,
};
use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::hir::{self, FunctionRef};
use graphcal_compiler::registry::checked_type::{CheckedType, IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::span::{Span, Spanned};
use graphcal_compiler::tir::texpr::{
    ContextualLiteral, TArg, TConstRef, TExpr, TExprKind, TFieldInit, TIndexArg, TMatchArm,
    TMatchPattern, TParamBinding,
};
use graphcal_compiler::tir::typed::evaluation_unit::{DeclarationBody, ScopedTree};
use indexmap::IndexMap;
use miette::NamedSource;

use crate::presentation_evidence::{PresentationInstance, PresentationInstanceMap};
use crate::runtime_presentation::EvaluatedRuntimeValue;
use graphcal_compiler::resolved_name::ResolvedDeclName;

use super::arithmetic::{Comparison, OrderingOp};
use super::{
    EvalContext, EvalSession, RuntimeValueMap, checked_finite_quantity, checked_unit_scaled_value,
    imported_binding_value, index_ref_matches_resolved, resolve_unit_scale,
};

pub type HirLocalValueMap<'a> = hir::LocalEnv<'a, EvaluatedRuntimeValue>;

fn presentation_instance_for(
    presentation_values: Option<&PresentationInstanceMap>,
    key: &ResolvedDeclName,
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
    key: &ResolvedDeclName,
) -> PresentationInstance {
    match presentation_values.remove(key) {
        Some(presentation) => presentation,
        None => PresentationInstance::None,
    }
}

/// Evaluate the root tree of an evaluation unit in the scope it was handed
/// out with.
pub fn eval_root<T: std::borrow::Borrow<TExpr>>(
    root: &ScopedTree<'_, T>,
    values: &RuntimeValueMap,
    session: &EvalSession<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_texpr(
        root.tree(),
        values,
        &HirLocalValueMap::root(),
        &session.enter(root),
    )
}

/// Evaluate the root tree of an evaluation unit in the scope it was handed
/// out with, preserving concrete presentation-call identities.
pub fn eval_root_with_presentation<T: std::borrow::Borrow<TExpr>>(
    root: &ScopedTree<'_, T>,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    session: &EvalSession<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    eval_texpr_with_presentation(
        root.tree(),
        values,
        presentation_values,
        &HirLocalValueMap::root(),
        &session.enter(root),
    )
}

/// Evaluate `subtree`, a subtree of `root`, in `root`'s scope with `locals`
/// bound, for tests that forge a local binding.
#[cfg(test)]
pub fn eval_subtree_for_test<T: std::borrow::Borrow<TExpr>>(
    root: &ScopedTree<'_, T>,
    subtree: &TExpr,
    values: &RuntimeValueMap,
    locals: &HirLocalValueMap<'_>,
    session: &EvalSession<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_texpr(subtree, values, locals, &session.enter(root))
}

/// Evaluate a checked, executable expression tree.
///
/// Checking published the tree only once every type in it was concrete and
/// every static obligation discharged, so evaluation reads each node's checked
/// type and nominal facts from the node itself. Declaration, constructor,
/// index-variant, inline-DAG, local, and built-in references are canonical.
///
/// `expr` is the root of a declaration's (or another evaluated root's) tree:
/// the availability of every dependency of the whole tree, including
/// unselected branches, is determined once before any of it is evaluated.
fn eval_texpr(
    expr: &TExpr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    ctx.check_dependencies(expr)?;
    eval_value(expr, values, local_values, ctx)
}

/// Evaluate a subtree of a root whose dependency availability has already
/// been determined by [`eval_texpr`] or [`eval_texpr_with_presentation`].
fn eval_value(
    expr: &TExpr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_texpr_evaluated(expr, values, None, local_values, ctx)
        .map(EvaluatedRuntimeValue::into_value)
}

/// Evaluate one checked root tree while preserving concrete presentation-call
/// identities through value-preserving expression forms. Like [`eval_texpr`],
/// it determines the availability of the whole tree's dependencies once.
fn eval_texpr_with_presentation(
    expr: &TExpr,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    ctx.check_dependencies(expr)?;
    eval_texpr_evaluated(expr, values, Some(presentation_values), local_values, ctx)
}

fn eval_texpr_evaluated(
    expr: &TExpr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    ctx.cancellation.checkpoint()?;
    // Recursion choke point: evaluation recurses once per tree level
    // (unbounded for left-nested operator chains).
    graphcal_compiler::stack::with_stack_growth(|| {
        eval_texpr_inner(expr, values, presentation_values, local_values, ctx)
    })
}

/// A function argument the callee evaluates as a value.
fn value_arg<'a>(arg: &'a TArg, ctx: &EvalContext<'_>) -> Result<&'a TExpr, GraphcalError> {
    match arg {
        TArg::Value(value) => Ok(value),
        TArg::Contextual(literal) => Err(ctx.internal_error(
            format!(
                "contextual metadata is not an executable value: {:?}",
                literal.id()
            ),
            literal.span(),
        )),
    }
}

/// The source span of a function argument.
const fn arg_span(arg: &TArg) -> Span {
    match arg {
        TArg::Value(value) => value.span(),
        TArg::Contextual(literal) => literal.span(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive typed expression evaluation"
)]
fn eval_texpr_inner(
    expr: &TExpr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let span = expr.span();
    match expr.kind() {
        TExprKind::Number(n) => checked_finite_quantity(*n, "numeric literal", span, ctx)
            .map(EvaluatedRuntimeValue::plain),
        TExprKind::Integer(n) => Ok(EvaluatedRuntimeValue::plain(RuntimeValue::Int(*n))),
        TExprKind::Bool(b) => Ok(EvaluatedRuntimeValue::plain(RuntimeValue::Bool(*b))),
        TExprKind::Quantity { value, unit } => {
            let scale = resolve_unit_scale(unit, values, ctx)?;
            let value = checked_unit_scaled_value(*value, scale, span, ctx)?;
            let presentation = super::presentation::scaled(unit, scale, ctx);
            Ok(EvaluatedRuntimeValue::new(value, presentation))
        }
        TExprKind::GraphRef(target) => {
            let key = ctx.resolve(&target.value);
            let value = resolve_graph_ref(&target.value, target.span, values, ctx)?;
            let presentation = presentation_instance_for(presentation_values, &key);
            Ok(EvaluatedRuntimeValue::new(
                clone_graph_ref_value(value),
                presentation,
            ))
        }
        TExprKind::Const(target) => {
            let value = eval_const_ref(target, values, ctx)?;
            let presentation = match &target.value {
                TConstRef::Decl(target) => {
                    presentation_instance_for(presentation_values, &ctx.resolve(target))
                }
                TConstRef::Builtin(_) | TConstRef::Constructor(_) => PresentationInstance::None,
            };
            Ok(EvaluatedRuntimeValue::new(value, presentation))
        }
        TExprKind::Local(local) => local_values
            .get(local.value)
            .cloned()
            .ok_or_else(|| ctx.eval_error("undefined local variable", local.span)),
        TExprKind::Binary { op, lhs, rhs } => match expr.ty() {
            // Additive Fin-key arithmetic `k + c : Key<Fin(N + c)>`.
            CheckedType::Key(target) => {
                eval_key_shift(span, target, lhs, rhs, values, local_values, ctx)
                    .map(EvaluatedRuntimeValue::plain)
            }
            _ => eval_binop(span, *op, lhs, rhs, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain),
        },
        TExprKind::Unary { op, operand } => {
            eval_unary(span, *op, operand, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain)
        }
        TExprKind::Call { callee, args } => {
            eval_fn_call(span, callee, args, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain)
        }
        TExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = eval_value(condition, values, local_values, ctx)?
                .expect_bool("if condition")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            if cond {
                eval_texpr_evaluated(then_branch, values, presentation_values, local_values, ctx)
            } else {
                eval_texpr_evaluated(else_branch, values, presentation_values, local_values, ctx)
            }
        }
        TExprKind::Convert {
            expr: inner,
            target,
        } => {
            let value = eval_value(inner, values, local_values, ctx)?;
            Ok(EvaluatedRuntimeValue::new(
                value,
                super::presentation::pending(target, ctx),
            ))
        }
        TExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => {
            let value = eval_value(inner, values, local_values, ctx)?;
            Ok(EvaluatedRuntimeValue::new(
                value,
                PresentationInstance::Timezone(timezone.clone()),
            ))
        }
        TExprKind::Field { expr: inner, field } => {
            let inner_val =
                eval_texpr_evaluated(inner, values, presentation_values, local_values, ctx)?;
            let (inner_val, presentation) = inner_val.into_parts();
            let value = eval_field_access(inner_val, inner, field, ctx)?;
            let presentation = presentation
                .project_field(&field.value)
                .map_err(|error| ctx.internal_error(error.to_string(), field.span))?;
            Ok(EvaluatedRuntimeValue::new(value, presentation))
        }
        TExprKind::Construct {
            application,
            fields,
        } => eval_constructor_call(
            expr.span(),
            application,
            fields,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        TExprKind::Map { entries } => {
            let entries = entries
                .iter()
                .map(|entry| {
                    let (first, rest) = entry.keys.split_first();
                    (first, rest, &entry.value)
                })
                .collect::<Vec<_>>();
            eval_map_literal(
                expr.ty(),
                span,
                &entries,
                values,
                presentation_values,
                local_values,
                ctx,
            )
        }
        TExprKind::For { bindings, body } => eval_for_comp_bindings(
            expr.ty(),
            bindings,
            body,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        TExprKind::Index { expr: inner, args } => eval_index_access(
            span,
            inner,
            args.as_slice(),
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        TExprKind::Scan {
            source,
            init,
            acc,
            val,
            body,
        } => eval_scan(
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
        TExprKind::Unfold { .. } => {
            eval_unfold(expr, values, presentation_values, local_values, ctx)
        }
        TExprKind::Key { kind, arg, .. } => {
            let CheckedType::Key(axis) = expr.ty() else {
                return Err(ctx.internal_error("key expression has no retained axis", span));
            };
            eval_key_form(*kind, axis, arg, span, values, local_values, ctx)
                .map(EvaluatedRuntimeValue::plain)
        }
        TExprKind::Match { scrutinee, arms } => eval_match(
            span,
            scrutinee,
            arms,
            values,
            presentation_values,
            local_values,
            ctx,
        ),
        TExprKind::Variant(variant) => {
            named_key(&variant.variant, span, ctx).map(EvaluatedRuntimeValue::plain)
        }
        TExprKind::DagCall {
            target,
            args,
            output,
            ..
        } => eval_dag_call(
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

fn resolve_graph_ref<'a>(
    target: &hir::LocalDecl,
    target_span: Span,
    values: &'a RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<&'a RuntimeValue, GraphcalError> {
    let runtime_target = ctx.resolve(target);
    values.get(&runtime_target).ok_or_else(|| {
        ctx.eval_error(
            format!("undefined graph reference `@{target}`"),
            target_span,
        )
    })
}

fn clone_graph_ref_value(value: &RuntimeValue) -> RuntimeValue {
    #[cfg(test)]
    record_cloned_runtime_nodes(value);
    value.clone()
}

fn clone_index_access_result(value: &RuntimeValue) -> RuntimeValue {
    #[cfg(test)]
    record_cloned_runtime_nodes(value);
    value.clone()
}

#[cfg(test)]
fn record_cloned_runtime_nodes(value: &RuntimeValue) {
    CLONED_RUNTIME_NODES.with(|count| {
        count.set(
            count
                .get()
                .saturating_add(runtime_value_tree_node_count(value)),
        );
    });
}

#[cfg(test)]
std::thread_local! {
    static CLONED_RUNTIME_NODES: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn runtime_value_tree_node_count(value: &RuntimeValue) -> usize {
    match value {
        RuntimeValue::Struct(value) => value
            .fields()
            .map(|(_, field)| runtime_value_tree_node_count(field))
            .fold(1, usize::saturating_add),
        RuntimeValue::Indexed(entries) => entries
            .values()
            .iter()
            .map(runtime_value_tree_node_count)
            .fold(1, usize::saturating_add),
        _ => 1,
    }
}

#[cfg(test)]
fn reset_cloned_runtime_node_count() {
    CLONED_RUNTIME_NODES.with(|count| count.set(0));
}

#[cfg(test)]
fn take_cloned_runtime_node_count() -> usize {
    CLONED_RUNTIME_NODES.with(|count| count.replace(0))
}

fn eval_const_ref(
    target: &Spanned<TConstRef>,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &target.value {
        TConstRef::Decl(resolved) => values
            .get(&ctx.resolve(resolved))
            .cloned()
            .ok_or_else(|| ctx.eval_error(format!("undefined constant `{resolved}`"), target.span)),
        TConstRef::Constructor(application) => nullary_constructor(application, target.span, ctx),
        TConstRef::Builtin(builtin) => {
            checked_finite_quantity(builtin.value(), "built-in constant", target.span, ctx)
        }
    }
}

fn nullary_constructor(
    application: &graphcal_compiler::tir::texpr::ConstructorApplication,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::ConstructorFactConsumption);
    apply_constructor(application, Vec::new(), span, ctx)
}

/// Apply a checked constructor to its evaluated fields.
fn apply_constructor(
    application: &graphcal_compiler::tir::texpr::ConstructorApplication,
    fields: Vec<(
        graphcal_compiler::syntax::type_name::FieldName,
        RuntimeValue,
    )>,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    StructValue::try_from_application(application, fields)
        .map(RuntimeValue::Struct)
        .map_err(|error| ctx.internal_error(error.to_string(), span))
}

/// Evaluate `k + c` on a `Fin` key: the key at position `k + c` of the wider
/// target axis `Fin(N + c)`, which the checker derived from the static addend.
fn eval_key_shift(
    span: Span,
    target: &IndexTypeRef,
    lhs: &TExpr,
    rhs: &TExpr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let (RuntimeValue::Key(key), RuntimeValue::Int(addend)) = (
        eval_value(lhs, values, local_values, ctx)?,
        eval_value(rhs, values, local_values, ctx)?,
    ) else {
        return Err(ctx.internal_error("key arithmetic needs a key and an Int addend", span));
    };
    let axis = index_axis_for_ref(target, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("key axis `{target}` has no concrete definition"),
            span,
        )
    })?;
    usize::try_from(addend)
        .ok()
        .and_then(|addend| key.position().checked_add(addend))
        .and_then(|position| KeyValue::at(axis, position))
        .map(RuntimeValue::Key)
        .ok_or_else(|| {
            ctx.internal_error(
                format!("key shifted by {addend} left its checked axis `{target}`"),
                span,
            )
        })
}

fn eval_binop(
    span: Span,
    op: graphcal_compiler::desugar::desugared_ast::BinOp,
    lhs: &TExpr,
    rhs: &TExpr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::desugar::desugared_ast::BinOp;
    let compare = |comparison| {
        let l = eval_value(lhs, values, local_values, ctx)?;
        let r = eval_value(rhs, values, local_values, ctx)?;
        super::arithmetic::eval_comparison_values(comparison, &l, &r, ctx, span)
    };
    match op {
        BinOp::And => {
            let l = eval_value(lhs, values, local_values, ctx)?
                .expect_bool("AND operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let r = eval_value(rhs, values, local_values, ctx)?
                .expect_bool("AND operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(l && r))
        }
        BinOp::Or => {
            let l = eval_value(lhs, values, local_values, ctx)?
                .expect_bool("OR operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let r = eval_value(rhs, values, local_values, ctx)?
                .expect_bool("OR operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(l || r))
        }
        BinOp::Eq => compare(Comparison::Eq),
        BinOp::Ne => compare(Comparison::Ne),
        BinOp::Lt => compare(Comparison::Ord(OrderingOp::Lt)),
        BinOp::Gt => compare(Comparison::Ord(OrderingOp::Gt)),
        BinOp::Le => compare(Comparison::Ord(OrderingOp::Le)),
        BinOp::Ge => compare(Comparison::Ord(OrderingOp::Ge)),
        BinOp::Pow(exponent) => eval_power(span, exponent, lhs, rhs, values, local_values, ctx),
        _ => {
            let l = eval_value(lhs, values, local_values, ctx)?;
            let r = eval_value(rhs, values, local_values, ctx)?;
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

fn eval_power(
    span: Span,
    exponent: graphcal_compiler::syntax::ast::PowerExponent,
    base: &TExpr,
    exponent_expr: &TExpr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::desugar::desugared_ast::BinOp;
    use graphcal_compiler::syntax::ast::PowerExponent;

    let base = eval_value(base, values, local_values, ctx)?;
    let op = BinOp::Pow(exponent);
    match (base, exponent) {
        (RuntimeValue::Int(base), PowerExponent::Exact(exact)) => {
            if !exact.is_integer() {
                return Err(ctx.internal_error(
                    "fractional exact exponent reached Int power evaluation",
                    span,
                ));
            }
            super::arithmetic::eval_int_binop(op, base, exact.num(), ctx, span)
                .map(RuntimeValue::Int)
        }
        (RuntimeValue::Int(base), _) => {
            let runtime_exponent = eval_value(exponent_expr, values, local_values, ctx)?;
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
            let runtime_exponent = eval_value(exponent_expr, values, local_values, ctx)?
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

fn eval_unary(
    span: Span,
    op: graphcal_compiler::desugar::desugared_ast::UnaryOp,
    operand: &TExpr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match op {
        graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg => {
            let v = eval_value(operand, values, local_values, ctx)?;
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
            let v = eval_value(operand, values, local_values, ctx)?
                .expect_bool("logical NOT")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            Ok(RuntimeValue::Bool(!v))
        }
    }
}

fn expect_builtin_arity(
    function: BuiltinFn,
    args: &[TArg],
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    let expected = function.entry().arity();
    if expected.accepts(args.len()) {
        return Ok(());
    }
    Err(ctx.internal_error(
        format!(
            "{function}() received {} argument(s) after dim-check accepted arity {expected}",
            args.len()
        ),
        span,
    ))
}

#[expect(
    clippy::too_many_lines,
    reason = "function dispatch handles all built-in call forms"
)]
fn eval_fn_call(
    span: Span,
    callee: &Spanned<FunctionRef>,
    args: &[TArg],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let (name, epoch_scale) = match &callee.value {
        FunctionRef::Builtin(builtin) => (builtin.function(), None),
        FunctionRef::Epoch { scale } => (BuiltinFn::EPOCH, Some(scale.value)),
        FunctionRef::External(ext) => {
            return eval_extern_fn(span, ext, args, values, local_values, ctx);
        }
    };
    // The single arity re-check for every built-in family; family kernels
    // below rely on the accepted argument count.
    expect_builtin_arity(name, args, callee.span, ctx)?;
    match name {
        BuiltinFn::Complex(function) => {
            let arguments = args
                .iter()
                .map(|argument| eval_value(value_arg(argument, ctx)?, values, local_values, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            super::complex::evaluate_builtin(function, &arguments).map_err(|error| {
                if error.is_internal_invariant() {
                    ctx.internal_error(error.to_string(), span)
                } else {
                    ctx.eval_error(error.to_string(), span)
                }
            })
        }
        BuiltinFn::Aggregation(kind) => {
            let arg_val = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let RuntimeValue::Indexed(indexed) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received a non-indexed argument", name.as_str()),
                    arg_span(&args[0]),
                ));
            };
            match kind {
                AggregationFn::Key(function) => eval_extremum_key(function, &indexed, span, ctx),
                AggregationFn::Value(function) => {
                    eval_aggregation_fn(function, &indexed, span, ctx.src)
                }
            }
        }
        BuiltinFn::LinearAlgebra(function) => {
            let arguments = args
                .iter()
                .map(|argument| eval_value(value_arg(argument, ctx)?, values, local_values, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            super::linear_algebra::evaluate(function, arguments, ctx).map_err(|error| {
                error.cancellation().map_or_else(
                    || {
                        if error.is_internal_invariant() {
                            ctx.internal_error(error.to_string(), span)
                        } else {
                            ctx.eval_error(error.to_string(), span)
                        }
                    },
                    GraphcalError::from,
                )
            })
        }
        BuiltinFn::Conversion(kind) => {
            eval_conversion_fn(kind, span, args, values, local_values, ctx)
        }
        BuiltinFn::Datetime(DatetimeFn::ScaleConversion(conversion)) => {
            let arg = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    arg_span(&args[0]),
                ));
            };
            Ok(RuntimeValue::Datetime(
                epoch.to_time_scale(conversion.target().to_hifitime()),
            ))
        }
        BuiltinFn::Datetime(DatetimeFn::Constructor(kind)) => {
            eval_datetime_constructor(kind, epoch_scale, span, args, ctx.src)
        }
        BuiltinFn::Datetime(DatetimeFn::Field(kind)) => {
            let arg_val = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    arg_span(&args[0]),
                ));
            };
            let fields = super::datetime::GregorianFields::from_epoch(epoch).map_err(|error| {
                ctx.internal_error(
                    format!("invalid declared-scale Gregorian fields: {error}"),
                    arg_span(&args[0]),
                )
            })?;
            let result = match kind {
                DatetimeField::Year => fields.year(),
                DatetimeField::Month => fields.month(),
                DatetimeField::Day => fields.day(),
                DatetimeField::Hour => fields.hour(),
                DatetimeField::Minute => fields.minute(),
                DatetimeField::Second => fields.second(),
                DatetimeField::Weekday => fields.iso_weekday(),
                DatetimeField::DayOfYear => fields.day_of_year(),
            };
            Ok(RuntimeValue::Int(result))
        }
        BuiltinFn::Datetime(DatetimeFn::FromNumeric(kind)) => {
            let arg_val = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let num = match arg_val {
                RuntimeValue::Quantity(v) => v.get(),
                RuntimeValue::Int(v) => {
                    exact_numeric_datetime_arg(v, name.as_str(), arg_span(&args[0]), ctx)?
                }
                _ => {
                    return Err(ctx.internal_error(
                        format!("{}() received non-numeric argument", name.as_str()),
                        arg_span(&args[0]),
                    ));
                }
            };
            let kind = match kind {
                DatetimeFromNumericFn::Jd => super::datetime::NumericEpochKind::JulianDate,
                DatetimeFromNumericFn::Mjd => super::datetime::NumericEpochKind::ModifiedJulianDate,
                DatetimeFromNumericFn::Unix => super::datetime::NumericEpochKind::UnixSeconds,
            };
            super::datetime::checked_epoch_from_numeric(num, kind)
                .map(RuntimeValue::Datetime)
                .map_err(|error| ctx.eval_error(error.to_string(), arg_span(&args[0])))
        }
        BuiltinFn::Datetime(DatetimeFn::ToNumeric(kind)) => {
            let arg_val = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    arg_span(&args[0]),
                ));
            };
            let result = match kind {
                DatetimeToNumericFn::Jd => epoch.to_jde_utc_days(),
                DatetimeToNumericFn::Mjd => epoch.to_mjd_utc_days(),
                DatetimeToNumericFn::Unix => epoch.to_unix_seconds(),
            };
            checked_finite_quantity(result, "datetime conversion", arg_span(&args[0]), ctx)
        }
        BuiltinFn::Scalar(function) => {
            eval_builtin_fn(span, function, args, values, local_values, ctx)
        }
    }
}

/// Evaluate a key introduction form.
///
/// `key` positions are proven in bounds by the checker; `fin_key` performs
/// its runtime range check here; the coordinate searches scan the axis's
/// coordinates with the documented policies.
fn eval_key_form(
    kind: graphcal_compiler::syntax::ast::KeyFormKind,
    axis_ref: &IndexTypeRef,
    arg: &TExpr,
    span: Span,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::syntax::ast::KeyFormKind;

    let arg_val = eval_value(arg, values, local_values, ctx)?;
    let axis = index_axis_for_ref(axis_ref, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("key axis `{axis_ref}` has no concrete definition"),
            span,
        )
    })?;
    match kind {
        KeyFormKind::Static => {
            // Bounds were discharged at compile time.
            let RuntimeValue::Int(position) = arg_val else {
                return Err(ctx.internal_error("key() received a non-Int position", arg.span()));
            };
            usize::try_from(position)
                .ok()
                .and_then(|position| KeyValue::at(axis, position))
                .map(RuntimeValue::Key)
                .ok_or_else(|| {
                    ctx.internal_error(
                        format!("static key position {position} escaped its checked range"),
                        arg.span(),
                    )
                })
        }
        KeyFormKind::Fin => {
            let RuntimeValue::Int(position) = arg_val else {
                return Err(ctx.internal_error("fin_key() received a non-Int position", arg.span()));
            };
            usize::try_from(position)
                .ok()
                .and_then(|position| KeyValue::at(axis, position))
                .map(RuntimeValue::Key)
                .ok_or_else(|| {
                    ctx.eval_error(
                        format!("fin_key: {position} out of bounds for {axis_ref}"),
                        span,
                    )
                })
        }
        KeyFormKind::Floor | KeyFormKind::Ceil | KeyFormKind::Nearest => {
            let quantity = arg_val
                .expect_quantity("coordinate search argument")
                .map_err(|e| ctx.eval_error(e.to_string(), arg.span()))?;
            let keys = KeyValue::all(&axis);
            let mut best: Option<(&KeyValue, f64)> = None;
            for key in &keys {
                let KeyElement::Coordinate { value, .. } = key.element() else {
                    return Err(ctx
                        .internal_error("coordinate search received a non-coordinate axis", span));
                };
                let coordinate = value.get();
                let candidate = match kind {
                    KeyFormKind::Floor if coordinate <= quantity => Some((key, coordinate)),
                    KeyFormKind::Ceil if coordinate >= quantity => Some((key, coordinate)),
                    KeyFormKind::Nearest => Some((key, coordinate)),
                    _ => None,
                };
                let Some((key, coordinate)) = candidate else {
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
                    best = Some((key, coordinate));
                }
            }
            let Some((key, _)) = best else {
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
            Ok(RuntimeValue::Key(key.clone()))
        }
    }
}

/// Evaluate `argmin`/`argmax`: the key of the extremum entry on the reduced
/// axis.
fn eval_extremum_key(
    kind: KeyAggregation,
    indexed: &IndexedValue<RuntimeValue>,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    super::aggregations::extremum_key(kind, indexed)
        .map(RuntimeValue::Key)
        .map_err(|error| {
            let message = error.to_string();
            if error.is_internal_invariant() {
                ctx.internal_error(message, span)
            } else {
                ctx.eval_error(message, span)
            }
        })
}

/// The constant key a qualified label denotes (`Maneuver#Departure`).
fn named_key(
    variant: &graphcal_compiler::resolved_name::ResolvedIndexVariant,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let index = IndexTypeRef::from_resolved(variant.index().clone());
    index_axis_for_ref(&index, ctx)
        .and_then(|axis| {
            KeyValue::for_entry(axis, &IndexEntryKey::named(variant.variant().clone()))
        })
        .map(RuntimeValue::Key)
        .ok_or_else(|| {
            ctx.internal_error(
                format!(
                    "label `{index}#{}` is not an entry of a concrete index",
                    variant.variant()
                ),
                span,
            )
        })
}

fn eval_aggregation_fn(
    kind: ValueAggregation,
    indexed: &IndexedValue<RuntimeValue>,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    super::aggregations::aggregate_indexed_values(kind, indexed).map_err(|error| {
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

fn eval_conversion_fn(
    kind: ConversionFn,
    span: Span,
    args: &[TArg],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match kind {
        ConversionFn::ToFloat => {
            let arg = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let RuntimeValue::Int(i) = arg else {
                return Err(
                    ctx.internal_error("to_float() received non-Int argument", arg_span(&args[0]))
                );
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "explicit Int to float conversion"
            )]
            checked_finite_quantity(i as f64, "to_float()", arg_span(&args[0]), ctx)
        }
        ConversionFn::ToInt => {
            let arg = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            // to_int() on a Fin key exposes its position.
            if let RuntimeValue::Key(key) = &arg {
                let KeyElement::Finite(position) = key.element() else {
                    return Err(
                        ctx.internal_error("to_int() received a non-Fin key", arg_span(&args[0]))
                    );
                };
                return i64::try_from(position).map(RuntimeValue::Int).map_err(|_| {
                    ctx.internal_error(
                        format!("Fin position {position} does not fit Int"),
                        arg_span(&args[0]),
                    )
                });
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
        ConversionFn::Coord => {
            let arg = eval_value(value_arg(&args[0], ctx)?, values, local_values, ctx)?;
            let RuntimeValue::Key(key) = arg else {
                return Err(
                    ctx.internal_error("coord() received a non-key argument", arg_span(&args[0]))
                );
            };
            let KeyElement::Coordinate { value, .. } = key.element() else {
                return Err(
                    ctx.internal_error("coord() received a non-coordinate key", arg_span(&args[0]))
                );
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

/// The contextual literal a function argument consists of, if it is one.
const fn contextual_literal(arg: &TArg) -> Option<&ContextualLiteral> {
    match arg {
        TArg::Contextual(literal) => Some(literal.literal()),
        TArg::Value(_) => None,
    }
}

fn eval_datetime_constructor(
    kind: DatetimeConstructorFn,
    epoch_scale: Option<TimeScale>,
    span: Span,
    args: &[TArg],
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match kind {
        DatetimeConstructorFn::Datetime => {
            let epoch = match args {
                [arg] => {
                    let Some(ContextualLiteral::OffsetDateTime(datetime)) = contextual_literal(arg)
                    else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unparsed offset literal".to_string(),
                            src: src.clone(),
                            span: arg_span(arg).into(),
                        });
                    };
                    super::datetime::datetime_from_offset(*datetime)
                }
                [datetime_arg, timezone_arg] => {
                    let Some(ContextualLiteral::ZonedDateTime(datetime)) =
                        contextual_literal(datetime_arg)
                    else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unresolved zoned literal".to_string(),
                            src: src.clone(),
                            span: arg_span(datetime_arg).into(),
                        });
                    };
                    let Some(ContextualLiteral::TimeZone(time_zone_id)) =
                        contextual_literal(timezone_arg)
                    else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unvalidated timezone argument"
                                .to_string(),
                            src: src.clone(),
                            span: arg_span(timezone_arg).into(),
                        });
                    };
                    if datetime.time_zone() != time_zone_id {
                        return Err(GraphcalError::InternalError {
                            message: "resolved datetime timezone does not match its argument"
                                .to_string(),
                            src: src.clone(),
                            span: arg_span(timezone_arg).into(),
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
            let Some(ContextualLiteral::CivilDateTime(datetime)) = contextual_literal(arg) else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received an unparsed civil literal".to_string(),
                    src: src.clone(),
                    span: arg_span(arg).into(),
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
                        span: arg_span(arg).into(),
                    }
                })?;
            Ok(RuntimeValue::Datetime(epoch))
        }
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
    axes: Vec<IndexAxis>,
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
        (_, RuntimeValue::Indexed(indexed)) => {
            let mut children = indexed
                .values()
                .iter()
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
            axes.push(indexed.axis().clone());
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
    bound_axes: &[IndexAxis],
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
                .try_fold(1_usize, |size, axis| size.checked_mul(axis.len()));
            let Some(child_len) = child_len else {
                return Err(ctx.eval_error("extern result shape cardinality overflowed", span));
            };
            let chunks = values.chunks_exact(child_len);
            if !chunks.remainder().is_empty() || chunks.len() != axis.len() {
                return Err(
                    ctx.internal_error("extern result buffer did not match its bound shape", span)
                );
            }
            let chunks = chunks.collect::<Vec<_>>();
            IndexedValue::try_from_axis(axis.clone(), |key| {
                rebuild_extern_array(remaining, chunks[key.position()], make_leaf, ctx, span)
            })
            .map(RuntimeValue::Indexed)
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
fn eval_extern_fn(
    span: Span,
    ext: &hir::ExternFnRef,
    args: &[TArg],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::function_signature::{ParamKind, ResultKind, ScalarValueKind};

    use crate::host_abi::{
        ValidatedHostArrayValues, ValidatedHostFieldValue, ValidatedHostResult, decode_result,
        encode_bool, encode_int, validate_quantity,
    };
    use crate::host_fns::{HostArray, HostFnValue};

    let Some(registry) = ctx.host_fns() else {
        return Err(ctx.eval_error(
            format!("extern function `{ext}` cannot be evaluated in this context (no host function registry)"),
            span,
        ));
    };
    let key = ext.key();
    let Some(host_fn) = registry.get(&key) else {
        return Err(ctx.eval_error(
            format!(
                "extern function `{}` (plugin \"{}\") is not provided by the host",
                ext.name, ext.plugin
            ),
            span,
        ));
    };
    let Some(function) = ctx.tir.extern_functions().get(&key) else {
        return Err(ctx.internal_error(
            format!("extern function `{ext}` has no resolved signature after dimension checking"),
            span,
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
            span,
        ));
    }

    let mut bound_indexes: std::collections::HashMap<
        graphcal_compiler::function_signature::IndexBinder,
        IndexAxis,
    > = std::collections::HashMap::new();
    let mut arg_values: Vec<HostFnValue> = Vec::with_capacity(args.len());
    for (param, arg) in signature.params().iter().zip(args) {
        let arg_span = arg_span(arg);
        let value = eval_value(value_arg(arg, ctx)?, values, local_values, ctx)?;
        let converted = match (&param.kind, value) {
            (ParamKind::Scalar(ScalarValueKind::Quantity(_)), value) => {
                let value = value
                    .expect_quantity("extern function argument")
                    .map_err(|error| ctx.eval_error(error.to_string(), arg_span))?;
                let value = validate_quantity(value).map_err(|error| {
                    ctx.eval_error(
                        format!(
                            "extern function `{ext}` received an invalid quantity for parameter `{}`: {error}",
                            param.name
                        ),
                        arg_span,
                    )
                })?;
                HostFnValue::F64(value.get())
            }
            (ParamKind::Scalar(ScalarValueKind::Int), RuntimeValue::Int(value)) => {
                let value = encode_int(value).map_err(|error| {
                    ctx.eval_error(
                        format!(
                            "extern function `{ext}` received an invalid Int for parameter `{}`: {error}",
                            param.name
                        ),
                        arg_span,
                    )
                })?;
                HostFnValue::F64(value)
            }
            (ParamKind::Scalar(ScalarValueKind::Bool), RuntimeValue::Bool(value)) => {
                HostFnValue::F64(encode_bool(value))
            }
            (ParamKind::Indexed { element, indexes }, value) => {
                let flattened = flatten_extern_array(&value, element, ctx, arg_span)?;
                if flattened.axes.len() != indexes.len() {
                    return Err(ctx.internal_error(
                        format!(
                            "extern function `{ext}` parameter `{}` received rank {}, expected rank {} after dimension checking",
                            param.name,
                            flattened.axes.len(),
                            indexes.len()
                        ),
                        arg_span,
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
                                arg_span,
                            ));
                        }
                        std::collections::hash_map::Entry::Occupied(_) => {}
                    }
                }
                let shape = flattened.axes.iter().map(IndexAxis::len).collect();
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
                                    arg_span,
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
                                    arg_span,
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };
                let array = HostArray::try_new(shape, encoded_values).map_err(|error| {
                    ctx.internal_error(
                        format!("failed to flatten extern array argument: {error}"),
                        arg_span,
                    )
                })?;
                HostFnValue::Array(array)
            }
            (ParamKind::Scalar(_), _) => {
                return Err(ctx.internal_error(
                    format!(
                        "extern function `{ext}` parameter `{}` received a value of the wrong kind after dimension checking",
                        param.name
                    ),
                    arg_span,
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
            span,
        )
    })?;

    let decoded = decode_result(signature.result(), &result).map_err(|error| {
        ctx.eval_error(
            format!("extern function `{ext}` returned an invalid ABI result: {error}"),
            span,
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
                            span,
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let expected_shape = axes.iter().map(IndexAxis::len).collect::<Vec<_>>();
            if array.shape() != expected_shape {
                return Err(ctx.eval_error(
                    format!(
                        "extern function `{ext}` returned shape {:?}, expected {expected_shape:?}",
                        array.shape()
                    ),
                    span,
                ));
            }
            match array.values() {
                ValidatedHostArrayValues::Quantity { values, .. } => rebuild_extern_array(
                    &axes,
                    values,
                    |value| RuntimeValue::Quantity(*value),
                    ctx,
                    span,
                ),
                ValidatedHostArrayValues::Bool(values) => rebuild_extern_array(
                    &axes,
                    values,
                    |value| RuntimeValue::Bool(*value),
                    ctx,
                    span,
                ),
                ValidatedHostArrayValues::Int(values) => rebuild_extern_array(
                    &axes,
                    values,
                    |value| RuntimeValue::Int(*value),
                    ctx,
                    span,
                ),
            }
        }
        ValidatedHostResult::Struct(fields) => {
            let ResultKind::Struct(result_struct) = signature.result() else {
                return Err(ctx.internal_error(
                    format!("extern function `{ext}` decoded a struct for a non-struct result"),
                    span,
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
                .collect::<Vec<_>>();
            StructValue::try_from_record_shape(
                result_struct.resolved.clone(),
                result_struct.constructor.clone(),
                &result_struct.shape,
                fields,
            )
            .map(RuntimeValue::Struct)
            .map_err(|error| {
                ctx.eval_error(
                    format!("extern function `{ext}` returned a malformed record: {error}"),
                    span,
                )
            })
        }
    }
}

fn eval_builtin_fn(
    span: Span,
    name: ScalarFn,
    args: &[TArg],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let builtin = graphcal_compiler::registry::builtins::scalar_function(name);
    let arg_values: Vec<f64> = args
        .iter()
        .map(|arg| {
            let rv = eval_value(value_arg(arg, ctx)?, values, local_values, ctx)?;
            rv.expect_quantity("function argument")
                .map_err(|e| ctx.eval_error(e.to_string(), arg_span(arg)))
        })
        .collect::<Result<_, _>>()?;
    let result = builtin
        .eval(&arg_values)
        .map_err(|error| ctx.eval_error(format!("builtin function `{name}` {error}"), span))?;
    checked_finite_quantity(
        super::arithmetic::check_finite(result, name.as_str(), ctx, span)?,
        name.as_str(),
        span,
        ctx,
    )
}

fn eval_field_access(
    inner_val: RuntimeValue,
    inner: &TExpr,
    field: &Spanned<graphcal_compiler::syntax::type_name::FieldName>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match inner_val {
        RuntimeValue::Struct(value) => {
            let type_name = value.type_name();
            let constructor = value.constructor();
            let CheckedType::Struct(expected, expected_args) = inner.ty() else {
                return Err(ctx.internal_error(
                    "field access has no retained struct operand type",
                    inner.span(),
                ));
            };
            let expected_runtime = ctx.runtime_struct_type(expected.resolved());
            // A struct value carries exactly its constructor's declared fields,
            // so only the nominal application is compared with the retained
            // type; never resolve a source name or infer a constructor here.
            let same_type = *type_name == expected_runtime;
            let same_arguments = value.generic_args() == expected_args.as_slice();
            let same_application = same_type && same_arguments;
            value
                .field(&field.value)
                .filter(|_| same_application)
                .cloned()
                .ok_or_else(|| {
                    ctx.eval_error(
                        format!("no field `{}` on `{type_name}::{constructor}`; expected `{expected_runtime}`", field.value),
                        field.span,
                    )
                })
        }
        _ => Err(ctx.eval_error("field access on non-struct value", inner.span())),
    }
}

fn eval_constructor_call(
    span: Span,
    application: &graphcal_compiler::tir::texpr::ConstructorApplication,
    fields: &[TFieldInit],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    crate::pipeline_metrics::record(crate::pipeline_metrics::Event::ConstructorFactConsumption);
    let constructor_name = application.constructor.name();
    let owning_type = StructTypeRef::from_resolved(application.definition().clone());
    let mut field_values = Vec::with_capacity(fields.len());
    let mut field_presentations = HashMap::new();
    for field_init in fields {
        let evaluated = eval_texpr_evaluated(
            &field_init.value,
            values,
            presentation_values,
            local_values,
            ctx,
        )?;
        let (val, presentation) = evaluated.into_parts();
        if application.constructor.constrains(&field_init.name)
            && let Some(field_constraints) = ctx.struct_field_constraints()
        {
            let key =
                graphcal_compiler::tir::typed::model::StructFieldConstraintKey::for_application(
                    owning_type.clone(),
                    application.generic_args.clone(),
                    constructor_name.clone(),
                    field_init.name.clone(),
                );
            let constraint = field_constraints.get(&key).ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "required field constraint `{constructor_name}.{}` is missing",
                        field_init.name
                    ),
                    field_init.value.span(),
                )
            })?;
            if let Err(violation) = crate::domain_check::check_domain_constraint(&val, constraint) {
                return Err(ctx.eval_error(
                    format!(
                        "field `{constructor_name}.{}` {}",
                        field_init.name, violation.message
                    ),
                    field_init.value.span(),
                ));
            }
        }
        field_values.push((field_init.name.clone(), val));
        if !presentation.is_none() {
            field_presentations.insert(field_init.name.clone(), presentation);
        }
    }
    Ok(EvaluatedRuntimeValue::new(
        apply_constructor(application, field_values, span, ctx)?,
        PresentationInstance::fields(field_presentations),
    ))
}

/// The concrete axis of `index_ref`, when it has one.
fn index_axis_for_ref(index_ref: &IndexTypeRef, ctx: &EvalContext<'_>) -> Option<IndexAxis> {
    IndexAxis::resolve(ctx.tir, index_ref)
}

fn ensure_index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::resolved_name::ResolvedIndexName,
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
            actual
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

/// One map-literal entry still to place: its key on the current axis, its
/// keys on the remaining axes, and its value.
type MapLiteralEntry<'a> = (
    &'a hir::expr::MapEntryKey,
    &'a [hir::expr::MapEntryKey],
    &'a TExpr,
);

fn eval_map_literal(
    checked_type: &CheckedType,
    map_span: Span,
    entries: &[MapLiteralEntry<'_>],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let first = entries
        .first()
        .ok_or_else(|| ctx.internal_error("empty map literal", map_span))?;
    let (first_key, first_rest, _) = first;
    let arity = first_rest.len().saturating_add(1);
    let CheckedType::Indexed { element, index } = checked_type else {
        return Err(ctx.internal_error("map has no retained indexed type", map_span));
    };
    let idx_name = index.clone();
    let axis = index_axis_for_ref(&idx_name, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("unknown index `{idx_name}`"),
            map_entry_key_span(first_key),
        )
    })?;
    let mut presentations = IndexMap::new();

    if arity == 1 {
        let mut evaluated = IndexMap::new();
        for (key, _, value) in entries {
            let variant = map_entry_variant_for_axis(key, &idx_name, ctx)?;
            let value =
                eval_texpr_evaluated(value, values, presentation_values, local_values, ctx)?;
            evaluated.insert(variant, value);
        }
        let result = IndexedValue::try_from_axis(axis, |key| {
            let variant = key.entry_key();
            let evaluated = evaluated.swap_remove(variant).ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "map literal for index `{idx_name}` is missing entry for variant `{variant}`"
                    ),
                    map_span,
                )
            })?;
            let (value, presentation) = evaluated.into_parts();
            if !presentation.is_none() {
                presentations.insert(variant.clone(), presentation);
            }
            Ok::<_, GraphcalError>(value)
        })?;
        return Ok(EvaluatedRuntimeValue::new(
            RuntimeValue::Indexed(result),
            PresentationInstance::entries(presentations),
        ));
    }

    let outer = IndexedValue::try_from_axis(axis, |key| {
        let variant = key.entry_key();
        let mut sub_entries = Vec::new();
        for (first_entry_key, rest, value) in entries {
            if map_entry_variant_for_axis(first_entry_key, &idx_name, ctx)? != *variant {
                continue;
            }
            let Some((next_key, rest)) = rest.split_first() else {
                return Err(
                    ctx.internal_error("multi-axis map literal entry lost all keys", value.span())
                );
            };
            sub_entries.push((next_key, rest, *value));
        }
        if sub_entries.is_empty() {
            return Err(ctx.internal_error(
                format!(
                    "map literal for index `{idx_name}` is missing entries for variant `{variant}`"
                ),
                map_span,
            ));
        }
        let evaluated = eval_map_literal(
            element,
            map_span,
            &sub_entries,
            values,
            presentation_values,
            local_values,
            ctx,
        )?;
        let (inner, presentation) = evaluated.into_parts();
        if !presentation.is_none() {
            presentations.insert(variant.clone(), presentation);
        }
        Ok::<_, GraphcalError>(inner)
    })?;
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed(outer),
        PresentationInstance::entries(presentations),
    ))
}

fn eval_for_comp_bindings(
    checked_type: &CheckedType,
    bindings: &[hir::expr::ForBinding],
    body: &TExpr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let CheckedType::Indexed { element, index } = checked_type else {
        return Err(ctx.internal_error(
            "comprehension binding has no retained indexed type",
            binding.local.span,
        ));
    };
    let idx_name = index.clone();
    let error_span = binding.local.span;

    let axis = index_axis_for_ref(&idx_name, ctx)
        .ok_or_else(|| ctx.internal_error(format!("unknown index `{idx_name}`"), error_span))?;

    let remaining = &bindings[1..];
    let mut presentations = IndexMap::new();
    let mut inner_locals = local_values.child(Vec::new());
    let entries = IndexedValue::try_from_axis(axis, |key| {
        let variant = key.entry_key();
        let binding_value = RuntimeValue::Key(key.clone());
        inner_locals.bind(
            binding.local.id,
            EvaluatedRuntimeValue::plain(binding_value),
        );
        let evaluated = if remaining.is_empty() {
            eval_texpr_evaluated(body, values, presentation_values, &inner_locals, ctx)?
        } else {
            eval_for_comp_bindings(
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
        if !presentation.is_none() {
            presentations.insert(variant.clone(), presentation);
        }
        Ok::<_, GraphcalError>(value)
    })?;
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed(entries),
        PresentationInstance::entries(presentations),
    ))
}

#[expect(
    clippy::single_match_else,
    reason = "single pattern dispatch keeps borrowed graph references and every index-argument category explicit"
)]
fn eval_index_access(
    span: Span,
    inner: &TExpr,
    args: &[TIndexArg],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let (base_value, base_presentation) = match inner.kind() {
        TExprKind::GraphRef(target) => {
            // This replaces the checkpoint normally performed by
            // `eval_value(inner, ...)` while retaining a reference to the
            // stored value instead of deep-cloning it before traversal.
            ctx.cancellation.checkpoint()?;
            let value = std::borrow::Cow::Borrowed(resolve_graph_ref(
                &target.value,
                target.span,
                values,
                ctx,
            )?);
            let presentation =
                presentation_instance_for(presentation_values, &ctx.resolve(&target.value));
            (value, presentation)
        }
        _ => {
            let evaluated =
                eval_texpr_evaluated(inner, values, presentation_values, local_values, ctx)?;
            let (value, presentation) = evaluated.into_parts();
            (std::borrow::Cow::Owned(value), presentation)
        }
    };
    let mut current = base_value.as_ref();
    let mut selected_keys = Vec::with_capacity(args.len());
    for arg in args {
        let RuntimeValue::Indexed(indexed) = current else {
            return Err(ctx.eval_error("indexing a non-indexed value", span));
        };
        let (entry, entry_key) = match arg {
            TIndexArg::Variant(variant) => {
                ensure_index_ref_matches_resolved(
                    indexed.index(),
                    variant.variant.index(),
                    variant.path_span(),
                    ctx,
                )?;
                let entry_key = IndexEntryKey::named(variant.variant.variant().clone());
                (indexed.get(&entry_key), entry_key)
            }
            TIndexArg::Var(local) => {
                let var_val = local_values
                    .get(local.value)
                    .ok_or_else(|| ctx.eval_error("undefined loop variable", local.span))?;
                let RuntimeValue::Key(key) = var_val.value() else {
                    return Err(ctx.eval_error("value is not a loop variable", local.span));
                };
                (
                    Some(select_by_key(indexed, key, local.span, ctx)?),
                    key.entry_key().clone(),
                )
            }
            TIndexArg::Expr {
                operand: index_expr,
                ..
            } => {
                match eval_value(index_expr, values, local_values, ctx)? {
                    RuntimeValue::Key(key) => (
                        Some(select_by_key(indexed, &key, index_expr.span(), ctx)?),
                        key.entry_key().clone(),
                    ),
                    // A static integer position on a `Fin` axis (`@m[0, 1]`).
                    RuntimeValue::Int(n) => {
                        let position = u64::try_from(n).map_err(|_| {
                            ctx.eval_error(
                                format!("index expression evaluated to negative value: {n}"),
                                index_expr.span(),
                            )
                        })?;
                        let entry_key = IndexEntryKey::position(position);
                        (indexed.get(&entry_key), entry_key)
                    }
                    _ => {
                        return Err(ctx.eval_error(
                            "index expression must evaluate to an integer or an index key",
                            index_expr.span(),
                        ));
                    }
                }
            }
        };
        current = entry
            .ok_or_else(|| ctx.eval_error(format!("index entry `{entry_key}` not found"), span))?;
        selected_keys.push(entry_key);
    }
    let presentation = base_presentation
        .project_indexes(&selected_keys)
        .map_err(|error| ctx.internal_error(error.to_string(), span))?;
    Ok(EvaluatedRuntimeValue::new(
        clone_index_access_result(current),
        presentation,
    ))
}

/// The entry of `indexed` that `key` selects, where the checker proved the
/// key's axis admissible.
fn select_by_key<'v>(
    indexed: &'v IndexedValue<RuntimeValue>,
    key: &KeyValue,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<&'v RuntimeValue, GraphcalError> {
    indexed.get_key(key).ok_or_else(|| {
        ctx.eval_error(
            format!(
                "index argument belongs to `{}`, but value is indexed by `{}`",
                key.index(),
                indexed.index()
            ),
            span,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "scan evaluation is called from expression destructuring"
)]
fn eval_scan(
    source: &TExpr,
    init: &TExpr,
    acc: &hir::LocalDef,
    val: &hir::LocalDef,
    body: &TExpr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let (source_val, source_presentation) =
        eval_texpr_evaluated(source, values, presentation_values, local_values, ctx)?.into_parts();
    let RuntimeValue::Indexed(source_entries) = source_val else {
        return Err(ctx.eval_error("scan source must be an indexed value", source.span()));
    };
    if matches!(source_entries.values().first(), RuntimeValue::Indexed(_)) {
        return Err(ctx.internal_error(
            "multi-axis source reached scan evaluation after rank-one type checking",
            source.span(),
        ));
    }
    let evaluated_init =
        eval_texpr_evaluated(init, values, presentation_values, local_values, ctx)?;
    let (_, initial_presentation) = evaluated_init.clone().into_parts();
    let mut accumulated = evaluated_init;
    let mut presentations = IndexMap::new();
    let mut scan_locals = local_values.child(Vec::new());
    let result_entries = source_entries.try_map_ref(|variant, item| {
        scan_locals.bind(acc.id, accumulated.clone());
        let item_presentation = source_presentation
            .project_indexes_ref(std::slice::from_ref(variant))
            .cloned()
            .map_err(|error| ctx.internal_error(error.to_string(), source.span()))?;
        scan_locals.bind(
            val.id,
            EvaluatedRuntimeValue::new(item.clone(), item_presentation),
        );
        accumulated = eval_texpr_evaluated(body, values, presentation_values, &scan_locals, ctx)?
            .with_default_presentation(&initial_presentation);
        let (value, evidence) = accumulated.clone().into_parts();
        if !evidence.is_none() {
            presentations.insert(variant.clone(), evidence);
        }
        Ok::<_, GraphcalError>(value)
    })?;
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed(result_entries),
        PresentationInstance::entries(presentations),
    ))
}

fn eval_unfold(
    expr: &TExpr,
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let TExprKind::Unfold {
        recurrence,
        init,
        body,
    } = expr.kind()
    else {
        return Err(ctx.internal_error("unfold evaluator received another operation", expr.span()));
    };
    let axis = &recurrence.axis;
    let CheckedType::Indexed { index, .. } = expr.ty() else {
        return Err(ctx.internal_error("unfold has no retained indexed type", expr.span()));
    };
    let index_ref = index.clone();
    let index_axis = index_axis_for_ref(&index_ref, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("missing resolved unfold axis `{}`", axis.value),
            axis.span,
        )
    })?;
    if index_axis.coordinate_data().is_none() {
        return Err(ctx.eval_error(
            format!(
                "unfold requires a coordinate index, but `{index_ref}` is not coordinate-valued"
            ),
            axis.span,
        ));
    }
    let evaluated_init =
        eval_texpr_evaluated(init, values, presentation_values, local_values, ctx)?;
    let mut previous_state = evaluated_init.clone();
    let (init_value, init_presentation) = evaluated_init.into_parts();
    let mut presentations = IndexMap::new();

    let mut unfold_locals = local_values.child(Vec::new());
    let result_entries = IndexedValue::try_from_axis(index_axis.clone(), |key| {
        let variant = key.entry_key();
        let Some(previous_key) = key
            .position()
            .checked_sub(1)
            .and_then(|previous| KeyValue::at(index_axis.clone(), previous))
        else {
            if !init_presentation.is_none() {
                presentations.insert(variant.clone(), init_presentation.clone());
            }
            return Ok(init_value.clone());
        };
        unfold_locals.bind(recurrence.previous_state.id, previous_state.clone());
        unfold_locals.bind(
            recurrence.previous_index.id,
            EvaluatedRuntimeValue::plain(RuntimeValue::Key(previous_key)),
        );
        unfold_locals.bind(
            recurrence.current_index.id,
            EvaluatedRuntimeValue::plain(RuntimeValue::Key(key.clone())),
        );
        previous_state =
            eval_texpr_evaluated(body, values, presentation_values, &unfold_locals, ctx)?
                .with_default_presentation(&init_presentation);
        let (value, evidence) = previous_state.clone().into_parts();
        if !evidence.is_none() {
            presentations.insert(variant.clone(), evidence);
        }
        Ok::<_, GraphcalError>(value)
    })?;
    Ok(EvaluatedRuntimeValue::new(
        RuntimeValue::Indexed(result_entries),
        PresentationInstance::entries(presentations),
    ))
}

fn evaluated_match_field(
    field: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::type_name::FieldName,
    >,
    type_name: &graphcal_compiler::resolved_name::ResolvedStructTypeName,
    scrutinee: &StructValue<RuntimeValue>,
    presentation: &PresentationInstance,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let value = scrutinee.field(&field.value).ok_or_else(|| {
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

fn eval_match(
    span: Span,
    scrutinee: &TExpr,
    arms: &[TMatchArm],
    values: &RuntimeValueMap,
    presentation_values: Option<&PresentationInstanceMap>,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let (scrutinee_val, scrutinee_presentation) =
        eval_texpr_evaluated(scrutinee, values, presentation_values, local_values, ctx)?
            .into_parts();
    match &scrutinee_val {
        RuntimeValue::Key(key) => {
            let KeyElement::Named(variant) = key.element() else {
                return Err(ctx.internal_error("match scrutinee is not a named key", span));
            };
            let matched_arm = arms
                .iter()
                .find(|arm| match &arm.pattern {
                    TMatchPattern::IndexLabel(pat) => {
                        index_ref_matches_resolved(key.index(), pat.variant.index())
                            && pat.variant.variant() == variant
                    }
                    TMatchPattern::Constructor { .. } => false,
                })
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for label `{variant}`"), span)
                })?;
            eval_texpr_evaluated(
                &matched_arm.body,
                values,
                presentation_values,
                local_values,
                ctx,
            )
        }
        RuntimeValue::Struct(scrutinee_struct) => {
            let type_name = scrutinee_struct.type_name();
            let value_constructor = scrutinee_struct.constructor();
            let matched_arm = arms
                .iter()
                .find(|arm| match &arm.pattern {
                    TMatchPattern::Constructor { target, .. } => {
                        *value_constructor == target.constructor
                            && *type_name == target.runtime_type
                    }
                    TMatchPattern::IndexLabel(_) => false,
                })
                .ok_or_else(|| {
                    ctx.eval_error(format!("no match arm for variant `{type_name}`"), span)
                })?;
            let mut arm_locals = local_values.child(Vec::new());
            let TMatchPattern::Constructor { bindings, .. } = &matched_arm.pattern else {
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
                                scrutinee_struct,
                                &scrutinee_presentation,
                                ctx,
                            )?,
                        );
                    }
                    hir::expr::PatternBinding::Wildcard { .. } => {}
                }
            }
            eval_texpr_evaluated(
                &matched_arm.body,
                values,
                presentation_values,
                &arm_locals,
                ctx,
            )
        }
        _ => Err(ctx.eval_error(
            "match scrutinee must be a label or tagged union",
            scrutinee.span(),
        )),
    }
}

fn eval_dag_call(
    target: &Spanned<graphcal_compiler::dag_id::DagId>,
    args: &[TParamBinding],
    output: &Spanned<ResolvedDeclName>,
    caller_values: &RuntimeValueMap,
    caller_presentations: Option<&PresentationInstanceMap>,
    caller_locals: &HirLocalValueMap,
    ctx: &EvalContext<'_>,
) -> Result<EvaluatedRuntimeValue, GraphcalError> {
    let plan = ctx.execution_plan()?;
    let callable = plan.callable(&target.value).ok_or_else(|| {
        ctx.internal_error(
            format!("DAG `{}` has no prepared callable plan", target.value),
            target.span,
        )
    })?;

    let mut frame = crate::execution_frame::ExecutionFrame::new(
        plan,
        callable,
        crate::execution_frame::FailurePolicy::Propagate,
    );
    for binding in args {
        let evaluated = eval_texpr_evaluated(
            &binding.value,
            caller_values,
            caller_presentations,
            caller_locals,
            ctx,
        )?;
        frame.bind_argument(&binding.target, evaluated, ctx.src, binding.value.span())?;
    }
    frame.seed_runtime_imports(|key| {
        imported_runtime_value(key, caller_values, caller_presentations, ctx)
    });

    let evaluated = frame.run(&ctx.cancellation, |entry, frame| {
        let session = ctx.for_declaration(&entry).with_unavailable(frame.errors());
        eval_root_with_presentation(
            entry.body(),
            frame.values(),
            frame.presentations(),
            &session,
        )
    });
    if let Some(calls) = ctx.unfinished_calls {
        calls
            .borrow_mut()
            .extend(frame.unfinished_origins().cloned());
    }
    evaluated?;
    let crate::execution_frame::FrameOutcome {
        values: dag_values,
        presentations: mut dag_presentations,
        errors,
    } = frame.finish();

    check_inline_plan_asserts(
        callable,
        &dag_values,
        target,
        output.span,
        &(**ctx).clone().with_unavailable(&errors),
    )?;

    let output_key = &output.value;
    let output_value = dag_values.get(output_key).cloned().ok_or_else(|| {
        if let Some(reason) = errors.get(output_key) {
            return GraphcalError::EvaluationUnavailable {
                reason: reason.clone(), src: ctx.src.clone(), span: output.span.into(),
            };
        }
        ctx.internal_error(
            format!(
                "dag `{}` has no projected value `{}` after evaluation (should have been caught by dim-check)",
                target.value,
                output.value.as_str()
            ),
            output.span,
        )
    })?;
    let output_presentation = take_presentation_instance(&mut dag_presentations, output_key);
    let presentation =
        super::presentation::resolve_frame(output_presentation, &dag_values, ctx, callable)?;
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

/// The value a prepared runtime import `key` of a called DAG reads from the
/// caller's or the root frame, with its presentation.
///
/// Only explicit prepared runtime imports may consult the caller or root frame.
fn imported_runtime_value(
    key: &ResolvedDeclName,
    caller_values: &RuntimeValueMap,
    caller_presentations: Option<&PresentationInstanceMap>,
    ctx: &EvalContext<'_>,
) -> Option<EvaluatedRuntimeValue> {
    let value = imported_binding_value(key, caller_values, ctx)?;
    let presentation = if key.owner() == ctx.dag_id() {
        caller_presentations.and_then(|instances| instances.get(key))
    } else if key.owner() == ctx.tir.root_dag_id() {
        ctx.root_presentation_instances
            .and_then(|instances| instances.get(key))
    } else {
        None
    };
    let presentation = presentation
        .filter(|presentation| !presentation.is_none())
        .cloned()
        .unwrap_or(PresentationInstance::None);
    Some(EvaluatedRuntimeValue::new(value.clone(), presentation))
}

fn check_inline_plan_asserts(
    callable: &crate::execution_plan::CallablePlan<'_>,
    values: &RuntimeValueMap,
    target: &Spanned<graphcal_compiler::dag_id::DagId>,
    span: Span,
    ctx: &EvalSession<'_>,
) -> Result<(), GraphcalError> {
    callable.execution_dags().iter().try_for_each(|scope| {
        check_inline_dag_asserts(
            scope.dag(),
            values,
            &ctx.with_src(scope.source()),
            target,
            span,
            ctx,
        )
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
    dag_tir: &graphcal_compiler::tir::typed::checked::CheckedDag,
    dag_values: &RuntimeValueMap,
    dag_ctx: &EvalSession<'_>,
    target: &Spanned<graphcal_compiler::dag_id::DagId>,
    call_span: Span,
    ctx: &EvalSession<'_>,
) -> Result<(), GraphcalError> {
    for entry in dag_tir.decls().iter() {
        if !matches!(entry.category(), DeclCategory::Assert) {
            continue;
        }
        let name = &entry.name();
        let key = entry.identity();
        let unit = ctx.tir.declaration_body(&key);
        let Some(body) = unit.and_then(DeclarationBody::assertion) else {
            return Err(ctx.internal_error(
                format!("TIR assertion entry missing for DAG assertion `{name}`"),
                call_span,
            ));
        };
        let ef = unit.and_then(DeclarationBody::expected_fail);
        let context = dag_ctx.for_decl(&key);
        let result = crate::assertion_eval::evaluate_assert_with_expected_fail(
            body.map(|entry| &*entry.body),
            ef,
            &mut |expr| eval_root(&context.executable(expr)?, dag_values, &context),
        );
        match result {
            crate::eval::types::AssertResult::Pass => {}
            crate::eval::types::AssertResult::Fail { message } => {
                return Err(ctx.eval_error(
                    format!(
                        "assertion `{name}` failed in inline call of dag `{}` ({message})",
                        target.value.leaf()
                    ),
                    call_span,
                ));
            }
            crate::eval::types::AssertResult::Blocked { reason } => {
                return Err(GraphcalError::EvaluationUnavailable {
                    reason,
                    src: ctx.src.clone(),
                    span: call_span.into(),
                });
            }
            crate::eval::types::AssertResult::Error { message } => {
                return Err(ctx.eval_error(
                    format!(
                        "assertion `{name}` errored in inline call of dag `{}` ({message})",
                        target.value.leaf()
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
    use super::*;
    use crate::eval::compile_and_eval;

    #[test]
    fn indexed_graph_ref_borrows_collection_before_selecting_entry() {
        reset_cloned_runtime_node_count();
        let direct_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = @source;
";
        compile_and_eval(direct_copy).unwrap();
        assert_eq!(
            take_cloned_runtime_node_count(),
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
            take_cloned_runtime_node_count(),
            16,
            "index traversal must clone only the 16 selected leaves"
        );
    }
}
