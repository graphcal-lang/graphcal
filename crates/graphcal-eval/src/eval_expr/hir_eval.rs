use std::sync::Arc;

use graphcal_compiler::builtin::{AggregationFn, BuiltinFnName};
use graphcal_compiler::hir::{self, ConstRef, FunctionRef};
use graphcal_compiler::ir::resolve::DeclCategory;
use graphcal_compiler::registry::declared_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::registry::types::{IndexDef, IndexKind};
use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;
use graphcal_compiler::tir::typed::{DagTIR, ResolvedConstructorTarget};
use indexmap::IndexMap;
use miette::NamedSource;

use crate::decl_key::RuntimeDeclKey;

use super::builtin_call::{
    DatetimeConstructorFn, DatetimeExtractFn, DatetimeFromFn, DatetimeToFn, EvalBuiltinRule,
    TypeConversionFn, eval_rule_for_builtin,
};
use super::{
    EvalContext, RuntimeValueMap, checked_finite_quantity, checked_unit_scaled_value,
    constructor_fields_for_runtime_struct, find_struct_field_constraint,
    imported_value_source_value, index_ref_matches_resolved, resolve_unit_scale,
    runtime_struct_type_def, topo_order_for_dag_body,
};

pub type HirLocalValueMap<'a> = hir::LocalEnv<'a, RuntimeValue>;

type ResolvedDeclKey = graphcal_compiler::syntax::decl_name::ResolvedDeclName;

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
    // Recursion choke point: evaluation recurses once per tree level
    // (unbounded for left-nested operator chains).
    graphcal_compiler::stack::with_stack_growth(|| {
        eval_hir_expr_inner(expr, values, local_values, ctx)
    })
}

#[expect(clippy::too_many_lines, reason = "exhaustive HIR ExprKind evaluation")]
fn eval_hir_expr_inner(
    expr: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &expr.kind {
        // Error nodes exist only in tolerant lowering for IDE consumers; the
        // batch pipeline rejects them before evaluation.
        hir::ExprKind::Error => {
            Err(ctx.eval_error("unresolved reference reached evaluation", expr.span))
        }
        hir::ExprKind::Number(n) => checked_finite_quantity(*n, "numeric literal", expr.span, ctx),
        hir::ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        hir::ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
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
        hir::ExprKind::UnitLiteral { value, unit } => {
            let scale = resolve_unit_scale(unit, values, ctx)?;
            checked_unit_scaled_value(*value, scale, expr.span, ctx)
        }
        hir::ExprKind::GraphRef(target) => values
            .get(&RuntimeDeclKey::resolved(target.value.clone()))
            .cloned()
            .ok_or_else(|| {
                ctx.eval_error(
                    format!("undefined graph reference `@{}`", target.value),
                    target.span,
                )
            }),
        hir::ExprKind::ConstRef(target) => eval_hir_const_ref(target, values, local_values, ctx),
        hir::ExprKind::LocalRef(local) => local_values
            .get(local.value)
            .cloned()
            .ok_or_else(|| ctx.eval_error("undefined local variable", local.span)),
        hir::ExprKind::BinOp { op, lhs, rhs } => {
            eval_hir_binop(expr.span, *op, lhs, rhs, values, local_values, ctx)
        }
        hir::ExprKind::UnaryOp { op, operand } => {
            eval_hir_unary(expr.span, *op, operand, values, local_values, ctx)
        }
        hir::ExprKind::FnCall { callee, args, .. } => {
            eval_hir_fn_call(expr, callee, args, values, local_values, ctx)
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
                eval_hir_expr(then_branch, values, local_values, ctx)
            } else {
                eval_hir_expr(else_branch, values, local_values, ctx)
            }
        }
        hir::ExprKind::Convert { expr: inner, .. }
        | hir::ExprKind::DisplayTimezone { expr: inner, .. } => {
            eval_hir_expr(inner, values, local_values, ctx)
        }
        hir::ExprKind::FieldAccess { expr: inner, field } => {
            let inner_val = eval_hir_expr(inner, values, local_values, ctx)?;
            eval_hir_field_access(inner_val, inner.span, field, ctx)
        }
        hir::ExprKind::ConstructorCall { callee, fields, .. } => {
            eval_hir_constructor_call(callee, fields, values, local_values, ctx)
        }
        hir::ExprKind::MapLiteral { entries } => {
            eval_hir_map_literal(entries, values, local_values, ctx)
        }
        hir::ExprKind::ForComp { bindings, body } => {
            eval_hir_for_comp(bindings, body, values, local_values, ctx)
        }
        hir::ExprKind::IndexAccess { expr: inner, args } => {
            eval_hir_index_access(expr.span, inner, args, values, local_values, ctx)
        }
        hir::ExprKind::Scan {
            source,
            init,
            acc,
            val,
            body,
        } => eval_hir_scan(source, init, acc, val, body, values, local_values, ctx),
        hir::ExprKind::Unfold {
            recurrence,
            init,
            body,
        } => eval_hir_unfold(recurrence, init, body, values, local_values, ctx),
        hir::ExprKind::Match { scrutinee, arms } => {
            eval_hir_match(expr.span, scrutinee, arms, values, local_values, ctx)
        }
        hir::ExprKind::VariantLiteral(variant) => {
            Ok(RuntimeValue::resolved_label(&variant.variant))
        }
        hir::ExprKind::InlineDagRef {
            target,
            args,
            output,
        } => eval_hir_inline_dag_call(target, args, output, values, local_values, ctx),
    }
}

fn eval_hir_const_ref(
    target: &graphcal_compiler::syntax::span::Spanned<ConstRef>,
    values: &RuntimeValueMap,
    _local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match &target.value {
        ConstRef::Decl(resolved) => values
            .get(&RuntimeDeclKey::resolved(resolved.clone()))
            .cloned()
            .ok_or_else(|| ctx.eval_error(format!("undefined constant `{resolved}`"), target.span)),
        ConstRef::Constructor(constructor) => {
            eval_hir_nullary_constructor(constructor, target.span, ctx)
        }
        ConstRef::Builtin(builtin) => {
            let value = ctx.builtin_consts.get(builtin.as_str()).ok_or_else(|| {
                ctx.eval_error(
                    format!("undefined constant `{}`", builtin.as_str()),
                    target.span,
                )
            })?;
            checked_finite_quantity(*value, "built-in constant", target.span, ctx)
        }
        ConstRef::TimeScale(scale) => Err(ctx.eval_error(
            format!("unexpected time scale `{scale}` outside epoch()"),
            target.span,
        )),
        ConstRef::GenericNatParam(param) => Err(ctx.eval_error(
            format!("generic Nat parameter `{}` is not bound", param.name),
            target.span,
        )),
    }
}

fn eval_hir_nullary_constructor(
    constructor: &graphcal_compiler::syntax::type_name::ResolvedConstructorName,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let target = constructor_target(ctx, constructor)
        .ok_or_else(|| ctx.eval_error(format!("unknown constructor `{constructor}`"), span))?;
    Ok(RuntimeValue::Struct {
        type_name: StructTypeRef::with_display_leaf(
            StructTypeName::from_atom(target.variant.name.atom().clone()),
            target.owning_type.clone(),
        ),
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
                    #[expect(
                        clippy::arithmetic_side_effects,
                        reason = "hifitime exposes Epoch subtraction for datetime differences"
                    )]
                    {
                        return Ok(RuntimeValue::Quantity((*le - *re).to_seconds()));
                    }
                }
                (RuntimeValue::Datetime(_), RuntimeValue::Datetime(_)) => {
                    return Err(ctx.eval_error("cannot add two datetimes", span));
                }
                (RuntimeValue::Datetime(e), RuntimeValue::Quantity(secs)) => {
                    let duration = hifitime::Duration::from_seconds(*secs);
                    #[expect(
                        clippy::arithmetic_side_effects,
                        reason = "hifitime exposes Epoch +/- Duration for datetime arithmetic"
                    )]
                    return match op {
                        BinOp::Add => Ok(RuntimeValue::Datetime(*e + duration)),
                        BinOp::Sub => Ok(RuntimeValue::Datetime(*e - duration)),
                        _ => Err(ctx.eval_error(
                            format!("unsupported operator {op:?} for Datetime and quantity"),
                            span,
                        )),
                    };
                }
                (RuntimeValue::Quantity(secs), RuntimeValue::Datetime(e)) if op == BinOp::Add => {
                    let duration = hifitime::Duration::from_seconds(*secs);
                    #[expect(
                        clippy::arithmetic_side_effects,
                        reason = "hifitime exposes Epoch + Duration for datetime arithmetic"
                    )]
                    {
                        return Ok(RuntimeValue::Datetime(*e + duration));
                    }
                }
                (RuntimeValue::Quantity(_), RuntimeValue::Datetime(_)) => {
                    return Err(ctx.eval_error("cannot subtract a Datetime from a quantity", span));
                }
                _ => {}
            }
            let lv = l
                .expect_quantity("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_quantity("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            super::arithmetic::eval_quantity_binop(op, lv, rv, ctx, span)
                .map(RuntimeValue::Quantity)
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
            super::arithmetic::eval_exact_quantity_power(base, exact, ctx, span)
                .map(RuntimeValue::Quantity)
        }
        (RuntimeValue::Quantity(base), _) => {
            let runtime_exponent = eval_hir_expr(exponent_expr, values, local_values, ctx)?
                .expect_quantity("power exponent")
                .map_err(|error| ctx.internal_error(error.to_string(), span))?;
            super::arithmetic::eval_quantity_binop(op, base, runtime_exponent, ctx, span)
                .map(RuntimeValue::Quantity)
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
                _ => Ok(RuntimeValue::Quantity(
                    -v.expect_quantity("unary negation")
                        .map_err(|e| ctx.eval_error(e.to_string(), span))?,
                )),
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
        EvalBuiltinRule::CollectionAggregation(kind) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Indexed { entries, .. } = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received a non-indexed argument", name.as_str()),
                    args[0].span,
                ));
            };
            eval_hir_aggregation_fn(kind, &entries, expr.span, ctx.src)
        }
        EvalBuiltinRule::LinearAlgebra(function) => {
            expect_hir_builtin_arity(name, args, function.arity(), callee.span, ctx)?;
            let arguments = args
                .iter()
                .map(|argument| eval_hir_expr(argument, values, local_values, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            super::linear_algebra::evaluate(function, arguments).map_err(|error| {
                if error.is_internal_invariant() {
                    ctx.internal_error(error.to_string(), expr.span)
                } else {
                    ctx.eval_error(error.to_string(), expr.span)
                }
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
                RuntimeValue::Quantity(v) => v,
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
            let epoch = match kind {
                DatetimeFromFn::Jd => hifitime::Epoch::from_jde_utc(num),
                DatetimeFromFn::Mjd => hifitime::Epoch::from_mjd_utc(num),
                DatetimeFromFn::Unix => hifitime::Epoch::from_unix_seconds(num),
            };
            Ok(RuntimeValue::Datetime(epoch))
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
            Ok(RuntimeValue::Quantity(result))
        }
        EvalBuiltinRule::RegistryFunction => {
            eval_hir_builtin_fn(expr, name, args, values, local_values, ctx)
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
            Ok(RuntimeValue::Quantity(i as f64))
        }
        TypeConversionFn::ToInt => {
            let arg = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let f = arg
                .expect_quantity("to_int argument")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            super::conversions::checked_f64_to_i64(f)
                .map(RuntimeValue::Int)
                .map_err(|err| ctx.eval_error(err.to_string(), span))
        }
    }
}

fn exact_numeric_datetime_arg(
    value: i64,
    fn_name: &str,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    const MAX_EXACT_F64_INT: u64 = 1_u64 << f64::MANTISSA_DIGITS;
    if value.unsigned_abs() > MAX_EXACT_F64_INT {
        return Err(ctx.eval_error(
            format!("{fn_name}() integer argument {value} is too large for exact conversion"),
            span,
        ));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer magnitude is checked to be exactly representable before casting"
    )]
    {
        Ok(value as f64)
    }
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
                    let hir::ExprKind::OffsetDateTimeLiteral(datetime) = &arg.kind else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unparsed offset literal".to_string(),
                            src: src.clone(),
                            span: arg.span.into(),
                        });
                    };
                    super::datetime::datetime_from_offset(*datetime)
                }
                [datetime_arg, timezone_arg] => {
                    let hir::ExprKind::ZonedDateTimeLiteral(datetime) = &datetime_arg.kind else {
                        return Err(GraphcalError::InternalError {
                            message: "datetime() received an unresolved zoned literal".to_string(),
                            src: src.clone(),
                            span: datetime_arg.span.into(),
                        });
                    };
                    let hir::ExprKind::IanaTimeZoneLiteral(time_zone_id) = &timezone_arg.kind
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
            let hir::ExprKind::CivilDateTimeLiteral(datetime) = &arg.kind else {
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

struct FlattenedExternArray {
    axes: Vec<BoundExternIndex>,
    values: Vec<f64>,
}

fn flatten_extern_array(
    value: &RuntimeValue,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<FlattenedExternArray, GraphcalError> {
    match value {
        RuntimeValue::Quantity(value) => Ok(FlattenedExternArray {
            axes: Vec::new(),
            values: vec![*value],
        }),
        RuntimeValue::Indexed {
            index_name,
            entries,
        } => {
            let children = entries
                .values()
                .map(|value| flatten_extern_array(value, ctx, span))
                .collect::<Result<Vec<_>, _>>()?;
            let (first, rest) = children.split_first().ok_or_else(|| {
                ctx.internal_error("extern array unexpectedly had an empty axis", span)
            })?;
            if rest.iter().any(|child| {
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
            let mut axes = Vec::with_capacity(first.axes.len().saturating_add(1));
            axes.push(BoundExternIndex {
                index_name: index_name.clone(),
                keys: entries.keys().cloned().collect(),
            });
            axes.extend(first.axes.iter().cloned());
            let values = children
                .into_iter()
                .flat_map(|child| child.values)
                .collect();
            Ok(FlattenedExternArray { axes, values })
        }
        RuntimeValue::Bool(_)
        | RuntimeValue::Int(_)
        | RuntimeValue::Label { .. }
        | RuntimeValue::CoordinateLabel { .. }
        | RuntimeValue::Datetime(_)
        | RuntimeValue::Struct { .. } => {
            Err(ctx.eval_error("extern array elements must be quantities", span))
        }
    }
}

fn rebuild_extern_array(
    bound_axes: &[BoundExternIndex],
    values: &[f64],
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<RuntimeValue, GraphcalError> {
    match bound_axes.split_first() {
        None => match values {
            [value] => Ok(RuntimeValue::Quantity(*value)),
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
                    rebuild_extern_array(remaining, values, ctx, span).map(|value| (key, value))
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
    use graphcal_compiler::function_signature::ValueKind;

    use crate::host_fns::{HostArray, HostFnValue};

    let Some(registry) = ctx.host_fns else {
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
    let Some(function) = ctx.tir.extern_functions.get(&key) else {
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
            (ValueKind::Quantity(_), value) => value
                .expect_quantity("extern function argument")
                .map(HostFnValue::F64)
                .map_err(|e| ctx.eval_error(e.to_string(), arg.span))?,
            (ValueKind::Int, RuntimeValue::Int(i)) => {
                HostFnValue::F64(exact_numeric_extern_arg(i, ext, arg.span, ctx)?)
            }
            (ValueKind::Bool, RuntimeValue::Bool(b)) => HostFnValue::F64(if b { 1.0 } else { 0.0 }),
            (ValueKind::Indexed { indexes, .. }, value) => {
                let flattened = flatten_extern_array(&value, ctx, arg.span)?;
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
                let array = HostArray::try_new(shape, flattened.values).map_err(|error| {
                    ctx.internal_error(
                        format!("failed to flatten extern array argument: {error}"),
                        arg.span,
                    )
                })?;
                HostFnValue::Array(array)
            }
            (ValueKind::Int | ValueKind::Bool | ValueKind::Struct(_), _) => {
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

    let abi_f64_result = |result: &HostFnValue| -> Result<f64, GraphcalError> {
        match result {
            HostFnValue::F64(value) => Ok(*value),
            HostFnValue::Array(_) | HostFnValue::Record(_) => Err(ctx.eval_error(
                format!(
                    "extern function `{ext}` declared a single-value result but returned a composite value"
                ),
                expr.span,
            )),
        }
    };

    match signature.result() {
        ValueKind::Quantity(_) => {
            let display = ext.to_string();
            Ok(RuntimeValue::Quantity(super::arithmetic::check_finite(
                abi_f64_result(&result)?,
                &display,
                ctx,
                expr.span,
            )?))
        }
        ValueKind::Int => super::conversions::checked_f64_to_i64(abi_f64_result(&result)?)
            .map(RuntimeValue::Int)
            .map_err(|err| {
                ctx.eval_error(
                    format!("extern function `{ext}` returned a non-Int value: {err}"),
                    expr.span,
                )
            }),
        ValueKind::Bool => {
            let result = abi_f64_result(&result)?;
            #[expect(
                clippy::float_cmp,
                reason = "the Bool host ABI is exactly 0.0/1.0; anything else is a plugin bug"
            )]
            if result == 0.0 {
                Ok(RuntimeValue::Bool(false))
            } else if result == 1.0 {
                Ok(RuntimeValue::Bool(true))
            } else {
                Err(ctx.eval_error(
                    format!(
                        "extern function `{ext}` declared a Bool result but returned {result} (expected 0.0 or 1.0)"
                    ),
                    expr.span,
                ))
            }
        }
        ValueKind::Indexed { indexes, .. } => {
            let HostFnValue::Array(array) = result else {
                return Err(ctx.eval_error(
                    format!(
                        "extern function `{ext}` declared an array result but returned a non-array value"
                    ),
                    expr.span,
                ));
            };
            let axes = indexes
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
            let display = ext.to_string();
            let values = array
                .values()
                .iter()
                .map(|element| super::arithmetic::check_finite(*element, &display, ctx, expr.span))
                .collect::<Result<Vec<_>, _>>()?;
            rebuild_extern_array(&axes, &values, ctx, expr.span)
        }
        ValueKind::Struct(shape) => {
            use graphcal_compiler::function_signature::StructFieldKind;
            use graphcal_compiler::registry::declared_type::StructTypeRef;

            let HostFnValue::Record(buffer) = result else {
                return Err(ctx.eval_error(
                    format!(
                        "extern function `{ext}` declared a struct result but returned an f64 ABI slot"
                    ),
                    expr.span,
                ));
            };
            let Some(result_struct) = &function.result_struct else {
                return Err(ctx.internal_error(
                    format!(
                        "extern function `{ext}` declares a struct result without a resolved record type"
                    ),
                    expr.span,
                ));
            };
            if buffer.len() != shape.fields().len() {
                return Err(ctx.eval_error(
                    format!(
                        "extern function `{ext}` returned {} field slot(s), expected {}",
                        buffer.len(),
                        shape.fields().len()
                    ),
                    expr.span,
                ));
            }
            let display = ext.to_string();
            let fields = shape
                .fields()
                .iter()
                .zip(buffer)
                .map(|(field, slot)| {
                    let value = match &field.kind {
                        StructFieldKind::Quantity(_) => RuntimeValue::Quantity(
                            super::arithmetic::check_finite(slot, &display, ctx, expr.span)?,
                        ),
                        StructFieldKind::Int => super::conversions::checked_f64_to_i64(slot)
                            .map(RuntimeValue::Int)
                            .map_err(|err| {
                                ctx.eval_error(
                                    format!(
                                        "extern function `{ext}` returned a non-Int value for field `{}`: {err}",
                                        field.name
                                    ),
                                    expr.span,
                                )
                            })?,
                        StructFieldKind::Bool => {
                            #[expect(
                                clippy::float_cmp,
                                reason = "the Bool host ABI is exactly 0.0/1.0; anything else is a plugin bug"
                            )]
                            if slot == 0.0 {
                                RuntimeValue::Bool(false)
                            } else if slot == 1.0 {
                                RuntimeValue::Bool(true)
                            } else {
                                return Err(ctx.eval_error(
                                    format!(
                                        "extern function `{ext}` returned {slot} for Bool field `{}` (expected 0.0 or 1.0)",
                                        field.name
                                    ),
                                    expr.span,
                                ));
                            }
                        }
                    };
                    Ok((field.name.clone(), value))
                })
                .collect::<Result<indexmap::IndexMap<_, _>, GraphcalError>>()?;
            Ok(RuntimeValue::Struct {
                type_name: StructTypeRef::from_resolved(result_struct.resolved.clone()),
                fields,
            })
        }
    }
}

/// Convert an Int argument to the single-value f64 host ABI, rejecting magnitudes that
/// are not exactly representable in `f64`.
fn exact_numeric_extern_arg(
    value: i64,
    ext: &hir::ExternFnRef,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<f64, GraphcalError> {
    const MAX_EXACT_F64_INT: u64 = 1_u64 << f64::MANTISSA_DIGITS;
    if value.unsigned_abs() > MAX_EXACT_F64_INT {
        return Err(ctx.eval_error(
            format!(
                "extern function `{ext}` integer argument {value} is too large for exact conversion"
            ),
            span,
        ));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer magnitude is checked to be exactly representable before casting"
    )]
    {
        Ok(value as f64)
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
    let builtin = ctx.builtin_fns.get(name.as_str()).ok_or_else(|| {
        ctx.eval_error(format!("unknown function `{}`", name.as_str()), expr.span)
    })?;
    let arg_values: Vec<f64> = args
        .iter()
        .map(|arg| {
            let rv = eval_hir_expr(arg, values, local_values, ctx)?;
            rv.expect_quantity("function argument")
                .map_err(|e| ctx.eval_error(e.to_string(), arg.span))
        })
        .collect::<Result<_, _>>()?;
    if arg_values.len() != builtin.arity() {
        return Err(ctx.eval_error(
            format!(
                "builtin function `{}` expects {} argument(s) but got {}",
                name.as_str(),
                builtin.arity(),
                arg_values.len()
            ),
            expr.span,
        ));
    }
    let result = (builtin.eval)(&arg_values);
    Ok(RuntimeValue::Quantity(super::arithmetic::check_finite(
        result,
        name.as_str(),
        ctx,
        expr.span,
    )?))
}

fn eval_hir_field_access(
    inner_val: RuntimeValue,
    inner_span: Span,
    field: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::type_name::FieldName,
    >,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match inner_val {
        RuntimeValue::Struct { type_name, fields } => {
            if let Some(type_def) = runtime_struct_type_def(&type_name, ctx) {
                let constructor_fields = constructor_fields_for_runtime_struct(
                    type_def, &type_name,
                )
                .ok_or_else(|| {
                    ctx.eval_error(
                        format!(
                            "constructor `{}` is not a member of struct `{}`",
                            type_name.name(),
                            type_def.name
                        ),
                        inner_span,
                    )
                })?;
                if !constructor_fields
                    .iter()
                    .any(|field_def| field_def.name == field.value)
                {
                    return Err(ctx.eval_error(
                        format!("no field `{}` on struct `{type_name}`", field.value),
                        field.span,
                    ));
                }
            }
            fields.get(field.value.as_str()).cloned().ok_or_else(|| {
                ctx.eval_error(format!("no field `{}` on struct", field.value), field.span)
            })
        }
        _ => Err(ctx.eval_error("field access on non-struct value", inner_span)),
    }
}

fn constructor_target<'a>(
    ctx: &'a EvalContext<'_>,
    constructor: &graphcal_compiler::syntax::type_name::ResolvedConstructorName,
) -> Option<&'a ResolvedConstructorTarget> {
    ctx.current_dag
        .map(|dag| &dag.semantic.constructor_refs)
        .and_then(|refs| refs.constructor_defs.get(constructor))
}

fn eval_hir_constructor_call(
    callee: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::type_name::ResolvedConstructorName,
    >,
    fields: &[hir::expr::FieldInit],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let target = constructor_target(ctx, &callee.value).ok_or_else(|| {
        ctx.eval_error(
            format!("unknown constructor `{}`", callee.value),
            callee.span,
        )
    })?;
    let constructor_name = target.variant.name.clone();
    let owning_type = StructTypeRef::from_resolved(target.owning_type.clone());
    let mut field_map = IndexMap::new();
    for field_init in fields {
        let val = eval_hir_expr(&field_init.value, values, local_values, ctx)?;
        if let Some(field_constraints) = ctx.struct_field_constraints
            && let Some(constraint) = find_struct_field_constraint(
                field_constraints,
                Some(&owning_type),
                &constructor_name,
                &field_init.name.value,
            )
            && let Err(violation) = crate::domain_check::check_domain_constraint(&val, constraint)
        {
            return Err(ctx.eval_error(
                format!(
                    "field `{}.{}` {}",
                    constructor_name, field_init.name.value, violation.message
                ),
                field_init.value.span,
            ));
        }
        field_map.insert(field_init.name.value.clone(), val);
    }
    Ok(RuntimeValue::Struct {
        type_name: StructTypeRef::with_display_leaf(
            StructTypeName::from_atom(target.variant.name.atom().clone()),
            target.owning_type.clone(),
        ),
        fields: field_map,
    })
}

fn index_def_for_ref<'a>(
    index_ref: &IndexTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a IndexDef> {
    if let Some(finite_index) = index_ref.finite_index() {
        return ctx.registry.indexes.get_finite_index(finite_index);
    }
    ctx.current_dag
        .map(|dag| &dag.semantic.collection_refs)
        .and_then(|refs| refs.index_defs.get(index_ref.declared_resolved()?))
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

fn map_entry_index_ref(
    key: &hir::expr::MapEntryKey,
    ctx: &EvalContext<'_>,
) -> Result<IndexTypeRef, GraphcalError> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => {
            Ok(IndexTypeRef::from_resolved(variant.variant.index().clone()))
        }
        hir::expr::MapEntryKey::FinitePosition { size, position } => {
            let finite_index = graphcal_compiler::registry::types::FiniteIndex::try_from_u64(*size)
                .map_err(|err| ctx.eval_error(err.to_string(), position.span))?;
            Ok(IndexTypeRef::from_finite_index(finite_index))
        }
    }
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

fn map_entry_index_def<'a>(
    key: &hir::expr::MapEntryKey,
    index_ref: &IndexTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a IndexDef> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => ctx
            .current_dag
            .map(|dag| &dag.semantic.collection_refs)
            .and_then(|refs| refs.index_defs.get(variant.variant.index())),
        hir::expr::MapEntryKey::FinitePosition { .. } => index_def_for_ref(index_ref, ctx),
    }
}

fn eval_hir_map_literal(
    entries: &[hir::expr::MapEntry],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let first = entries
        .first()
        .ok_or_else(|| ctx.internal_error("empty map literal", Span::new(0, 0)))?;
    let first_key = first.keys.first();
    let arity = first.keys.len();
    let idx_name = map_entry_index_ref(first_key, ctx)?;

    if arity == 1 {
        let idx_def = map_entry_index_def(first_key, &idx_name, ctx).ok_or_else(|| {
            ctx.internal_error(format!("unknown index `{idx_name}`"), Span::new(0, 0))
        })?;
        let mut evaluated = IndexMap::new();
        for entry in entries {
            let key = entry.keys.first();
            let variant = map_entry_variant_for_axis(key, &idx_name, ctx)?;
            let val = eval_hir_expr(&entry.value, values, local_values, ctx)?;
            evaluated.insert(variant, val);
        }
        let mut result = IndexMap::new();
        for variant in idx_def.entry_keys() {
            let val = evaluated.swap_remove(&variant).ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "map literal for index `{idx_name}` is missing entry for variant `{variant}`"
                    ),
                    Span::new(0, 0),
                )
            })?;
            result.insert(variant, val);
        }
        return Ok(RuntimeValue::Indexed {
            index_name: idx_name,
            entries: result,
        });
    }

    let idx_def = map_entry_index_def(first_key, &idx_name, ctx).ok_or_else(|| {
        ctx.internal_error(format!("unknown index `{idx_name}`"), Span::new(0, 0))
    })?;
    let variants = idx_def.entry_keys();
    let mut outer = IndexMap::new();
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
                Span::new(0, 0),
            ));
        }
        let inner = eval_hir_map_literal(&sub_entries, values, local_values, ctx)?;
        outer.insert(variant.clone(), inner);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries: outer,
    })
}

/// Generic Nat parameters are substituted during TIR construction, so a
/// `Param` reaching evaluation is an internal invariant violation.
fn eval_hir_nat_expr(expr: &hir::NatExpr, ctx: &EvalContext<'_>) -> Result<u64, GraphcalError> {
    match expr {
        hir::NatExpr::Literal(n, _) => Ok(*n),
        hir::NatExpr::Param(param) => Err(ctx.internal_error(
            format!(
                "unbound generic Nat parameter `{}` — Nat parameters must be \
                 substituted before evaluation",
                param.value.name
            ),
            param.span,
        )),
        hir::NatExpr::Add(lhs, rhs, span) => {
            let l = eval_hir_nat_expr(lhs, ctx)?;
            let r = eval_hir_nat_expr(rhs, ctx)?;
            l.checked_add(r)
                .ok_or_else(|| ctx.eval_error(format!("nat arithmetic overflow: {l} + {r}"), *span))
        }
        hir::NatExpr::Mul(lhs, rhs, span) => {
            let l = eval_hir_nat_expr(lhs, ctx)?;
            let r = eval_hir_nat_expr(rhs, ctx)?;
            l.checked_mul(r)
                .ok_or_else(|| ctx.eval_error(format!("nat arithmetic overflow: {l} * {r}"), *span))
        }
    }
}

fn eval_hir_for_comp(
    bindings: &[hir::expr::ForBinding],
    body: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let (idx_name, error_span, dynamic_finite_index) = match &binding.index {
        hir::expr::ForBindingIndex::Named(index) => (
            IndexTypeRef::from_resolved(index.value.clone()),
            index.span,
            None,
        ),
        hir::expr::ForBindingIndex::Finite { cardinality, span } => {
            let size = eval_hir_nat_expr(cardinality, ctx)?;
            let finite_index = graphcal_compiler::registry::types::FiniteIndex::try_from_u64(size)
                .map_err(|err| ctx.eval_error(err.to_string(), *span))?;
            (
                IndexTypeRef::from_finite_index(finite_index),
                *span,
                Some(finite_index),
            )
        }
    };

    let dynamic_nat_def;
    let idx_def = if let Some(def) = index_def_for_ref(&idx_name, ctx) {
        def
    } else if let Some(finite_index) = dynamic_finite_index {
        dynamic_nat_def = IndexDef {
            name: finite_index.display_name(),
            kind: IndexKind::Finite {
                cardinality: finite_index.cardinality(),
            },
        };
        &dynamic_nat_def
    } else {
        return Err(ctx.internal_error(format!("unknown index `{idx_name}`"), error_span));
    };

    let remaining = &bindings[1..];
    let variants = idx_def.entry_keys();
    let mut entries = IndexMap::new();
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
                RuntimeValue::CoordinateLabel {
                    index_name: idx_name.clone(),
                    position,
                    value: data.coordinate_value(position),
                }
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
        inner_locals.bind(binding.local.id, binding_value);
        let val = if remaining.is_empty() {
            eval_hir_expr(body, values, &inner_locals, ctx)?
        } else {
            eval_hir_for_comp(remaining, body, values, &inner_locals, ctx)?
        };
        entries.insert(variant.clone(), val);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries,
    })
}

fn eval_hir_index_access(
    span: Span,
    inner: &hir::Expr,
    args: &[hir::expr::IndexArg],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let mut current = eval_hir_expr(inner, values, local_values, ctx)?;
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
                    &index_name,
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
                match var_val {
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
                    RuntimeValue::Struct { type_name, .. } => {
                        IndexEntryKey::named(IndexVariantName::expect_valid(type_name.as_str()))
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
                let RuntimeValue::Int(n) = val else {
                    return Err(ctx.eval_error(
                        "index expression must evaluate to an integer",
                        index_expr.span,
                    ));
                };
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
        };
        current = entries
            .get(&entry_key)
            .cloned()
            .ok_or_else(|| ctx.eval_error(format!("index entry `{entry_key}` not found"), span))?;
    }
    Ok(current)
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
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let source_val = eval_hir_expr(source, values, local_values, ctx)?;
    let RuntimeValue::Indexed {
        index_name,
        entries: source_entries,
    } = source_val
    else {
        return Err(ctx.eval_error("scan source must be an indexed value", source.span));
    };
    let mut acc_val = eval_hir_expr(init, values, local_values, ctx)?;
    let mut result_entries = IndexMap::new();
    let mut scan_locals = local_values.child(Vec::new());
    for (variant, item) in &source_entries {
        scan_locals.bind(acc.id, acc_val);
        scan_locals.bind(val.id, item.clone());
        let body_val = eval_hir_expr(body, values, &scan_locals, ctx)?;
        result_entries.insert(variant.clone(), body_val.clone());
        acc_val = body_val;
    }
    Ok(RuntimeValue::Indexed {
        index_name,
        entries: result_entries,
    })
}

fn eval_hir_unfold(
    recurrence: &hir::expr::UnfoldRecurrence,
    init: &hir::Expr,
    body: &hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let axis = &recurrence.axis;
    let index_ref = IndexTypeRef::from_resolved(axis.value.clone());
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
    let init_value = eval_hir_expr(init, values, local_values, ctx)?;
    let mut previous_state = init_value.clone();
    let mut result_entries = IndexMap::new();
    result_entries.insert(variants[0].clone(), init_value);

    let mut unfold_locals = local_values.child(Vec::new());
    for (position, variant) in variants.iter().enumerate().skip(1) {
        let previous_position = position
            .checked_sub(1)
            .ok_or_else(|| ctx.internal_error("unfold step has no previous position", axis.span))?;
        unfold_locals.bind(recurrence.previous_state.id, previous_state);
        unfold_locals.bind(
            recurrence.previous_index.id,
            RuntimeValue::CoordinateLabel {
                index_name: index_ref.clone(),
                position: previous_position,
                value: coordinate_data.coordinate_value(previous_position),
            },
        );
        unfold_locals.bind(
            recurrence.current_index.id,
            RuntimeValue::CoordinateLabel {
                index_name: index_ref.clone(),
                position,
                value: coordinate_data.coordinate_value(position),
            },
        );
        let body_value = eval_hir_expr(body, values, &unfold_locals, ctx)?;
        result_entries.insert(variant.clone(), body_value.clone());
        previous_state = body_value;
    }
    Ok(RuntimeValue::Indexed {
        index_name: index_ref,
        entries: result_entries,
    })
}

fn runtime_struct_matches_resolved_constructor(
    scrutinee_type: &StructTypeRef,
    target: &ResolvedConstructorTarget,
) -> bool {
    scrutinee_type.name().as_str() == target.variant.name.as_str()
        && scrutinee_type.resolved() == &target.owning_type
}

fn eval_hir_match(
    span: Span,
    scrutinee: &hir::Expr,
    arms: &[hir::expr::MatchArm],
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let scrutinee_val = eval_hir_expr(scrutinee, values, local_values, ctx)?;
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
            eval_hir_expr(&matched_arm.body, values, local_values, ctx)
        }
        RuntimeValue::Struct {
            type_name,
            fields: scrutinee_fields,
        } => {
            let matched_arm = arms
                .iter()
                .find(|arm| match &arm.pattern {
                    hir::expr::MatchPattern::Constructor { constructor, .. } => {
                        constructor_target(ctx, &constructor.value).is_some_and(|target| {
                            runtime_struct_matches_resolved_constructor(type_name, target)
                        })
                    }
                    hir::expr::MatchPattern::IndexLabel { .. } => false,
                })
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
                        let field_val =
                            scrutinee_fields.get(field.value.as_str()).ok_or_else(|| {
                                ctx.eval_error(
                                    format!("no field `{}` on type `{type_name}`", field.value),
                                    field.span,
                                )
                            })?;
                        arm_locals.bind(local.id, field_val.clone());
                    }
                    hir::expr::PatternBinding::Wildcard { .. } => {}
                }
            }
            eval_hir_expr(&matched_arm.body, values, &arm_locals, ctx)
        }
        _ => Err(ctx.eval_error(
            "match scrutinee must be a label or tagged union",
            scrutinee.span,
        )),
    }
}

fn hir_expr_for_dag_body_name<'a>(dag_tir: &'a DagTIR, name: &ScopedName) -> Option<&'a hir::Expr> {
    let key = dag_tir.resolved_decl_key_for_local(name)?;
    dag_tir
        .semantic
        .expressions
        .consts
        .get(&key)
        .or_else(|| dag_tir.semantic.expressions.runtime_expr(&key))
}

fn eval_hir_inline_dag_call(
    target: &graphcal_compiler::syntax::span::Spanned<graphcal_compiler::dag_id::DagId>,
    args: &[hir::expr::ParamBinding],
    output: &graphcal_compiler::syntax::span::Spanned<ResolvedDeclKey>,
    caller_values: &RuntimeValueMap,
    caller_locals: &HirLocalValueMap,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let dag_tir = ctx.tir.dags.get(&target.value).ok_or_else(|| {
        ctx.internal_error(
            format!(
                "dag `{}` has no compiled TIR (should have been caught by dim-check)",
                target.value
            ),
            target.span,
        )
    })?;

    let mut dag_values = RuntimeValueMap::new();
    for binding in args {
        let value = eval_hir_expr(&binding.value, caller_values, caller_locals, ctx)?;
        dag_values.insert(super::dag_decl_runtime_key(&binding.target.value), value);
    }

    seed_inline_dag_imported_values(dag_tir, &mut dag_values, caller_values, ctx);

    let dag_ctx = EvalContext {
        builtin_consts: ctx.builtin_consts,
        builtin_fns: ctx.builtin_fns,
        registry: ctx.registry,
        src: ctx.src,
        tir: ctx.tir,
        current_dag: Some(dag_tir),
        root_values: ctx.root_values,
        struct_field_constraints: ctx.struct_field_constraints,
        host_fns: ctx.host_fns,
    };
    let empty_hir_locals = HirLocalValueMap::root();

    let topo = topo_order_for_dag_body(dag_tir).map_err(|remaining| {
        ctx.internal_error(
            format!(
                "dag body dependency cycle involving: {}",
                remaining
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            output.span,
        )
    })?;
    let categories: std::collections::HashMap<&ScopedName, DeclCategory> = dag_tir
        .source_order
        .iter()
        .map(|(name, cat)| (name, *cat))
        .collect();
    for name in topo {
        // Only value declarations evaluate in the topo order. Asserts are
        // checked after every value is available (the dag-body dependency
        // graph carries no edges for them), and plots/figures/layers have
        // no runtime value in an expression context.
        match categories.get(&name) {
            Some(
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer,
            ) => continue,
            Some(DeclCategory::Const | DeclCategory::Param | DeclCategory::Node) | None => {}
        }
        let local_key = RuntimeDeclKey::for_local_decl(dag_tir, &name);
        if dag_values.contains_key(&local_key) {
            continue;
        }
        if let Some(hir_expr) = hir_expr_for_dag_body_name(dag_tir, &name) {
            let value = eval_hir_expr(hir_expr, &dag_values, &empty_hir_locals, &dag_ctx)?;
            dag_values.insert(local_key, value);
        } else {
            return Err(ctx.internal_error(
                format!("semantic TIR missing HIR expression for DAG body declaration `{name}`"),
                output.span,
            ));
        }
    }

    check_inline_dag_asserts(dag_tir, &dag_values, &dag_ctx, target, output.span, ctx)?;

    let output_key = super::dag_decl_runtime_key(&output.value);
    dag_values.get(&output_key).cloned().ok_or_else(|| {
        ctx.internal_error(
            format!(
                "dag `{}` has no projected value `{}` after evaluation (should have been caught by dim-check)",
                target.value,
                output.value.as_str()
            ),
            output.span,
        )
    })
}

/// Seed an inline dag instantiation's value map with the dag's imported
/// (outer-scope) values: pre-evaluated imports plus values resolvable from
/// the caller's scope. Values already provided by call-site bindings win.
fn seed_inline_dag_imported_values(
    dag_tir: &DagTIR,
    dag_values: &mut RuntimeValueMap,
    caller_values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) {
    let own_names: std::collections::HashSet<&str> = dag_tir
        .consts
        .iter()
        .map(|e| e.name.member())
        .chain(dag_tir.params.iter().map(|e| e.name.member()))
        .chain(dag_tir.nodes.iter().map(|e| e.name.member()))
        .collect();
    let outer_scope_keys: std::collections::HashSet<&ScopedName> = dag_tir
        .imported_values
        .keys()
        .chain(dag_tir.imported_value_sources.keys())
        .collect();
    for scoped in outer_scope_keys {
        let member = scoped.member();
        let visible_key = RuntimeDeclKey::for_visible_name(dag_tir, scoped);
        let unresolved_local_import =
            !dag_tir.semantic.decl_bindings.contains_key(scoped) && own_names.contains(member);
        if unresolved_local_import || dag_values.contains_key(&visible_key) {
            continue;
        }
        let value = dag_tir
            .imported_values
            .get(scoped)
            .map(|(v, _)| v)
            .or_else(|| {
                dag_tir
                    .imported_value_sources
                    .get(scoped)
                    .and_then(|source| imported_value_source_value(source, caller_values, ctx))
            });
        if let Some(value) = value {
            dag_values.insert(visible_key, value.clone());
        }
    }
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
    for (name, cat) in &dag_tir.source_order {
        if !matches!(cat, DeclCategory::Assert) {
            continue;
        }
        let body = dag_tir
            .resolved_decl_key_for_local(name)
            .and_then(|key| dag_tir.semantic.expressions.asserts.get(&key))
            .ok_or_else(|| {
                ctx.internal_error(
                    format!("semantic TIR missing HIR body for DAG assertion `{name}`"),
                    call_span,
                )
            })?;
        let ef = dag_tir.expected_fail.get(name);
        let result = crate::eval::runtime::evaluate_assert_with_expected_fail(
            body,
            ef,
            dag_values,
            &empty_hir_locals,
            dag_ctx,
        );
        match result {
            crate::eval::AssertResult::Pass => {}
            crate::eval::AssertResult::Fail { message } => {
                return Err(ctx.eval_error(
                    format!(
                        "assertion `{name}` failed in inline call of dag `{}` ({message})",
                        target.value.name()
                    ),
                    call_span,
                ));
            }
            crate::eval::AssertResult::Error { message } => {
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
