use std::sync::Arc;

use graphcal_compiler::builtin::{
    AggregationFn, BuiltinFnName, ConstructorFn, DatetimeExtractFn, DatetimeFromFn, DatetimeToFn,
    SpecialFnKind, TypeConversionFn,
};
use graphcal_compiler::hir::{self, ConstRef, FunctionRef};
use graphcal_compiler::ir::resolve::DeclCategory;
use graphcal_compiler::registry::declared_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{IndexDef, IndexKind};
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;
use graphcal_compiler::tir::typed::{DagTIR, ResolvedConstructorTarget};
use indexmap::IndexMap;
use miette::NamedSource;

use crate::decl_key::RuntimeDeclKey;

use super::{
    EvalContext, RuntimeValueMap, checked_finite_scalar, checked_unit_scaled_value,
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
        hir::ExprKind::Number(n) => checked_finite_scalar(*n, "numeric literal", expr.span, ctx),
        hir::ExprKind::Integer(n) => Ok(RuntimeValue::Int(*n)),
        hir::ExprKind::Bool(b) => Ok(RuntimeValue::Bool(*b)),
        hir::ExprKind::StringLiteral(_) => {
            Err(ctx.eval_error("unexpected string literal in evaluation context", expr.span))
        }
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
            init,
            prev,
            curr,
            body,
        } => eval_hir_unfold(expr.span, init, prev, curr, body, values, ctx),
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
            checked_finite_scalar(*value, "built-in constant", target.span, ctx)
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
                        return Ok(RuntimeValue::Scalar((*le - *re).to_seconds()));
                    }
                }
                (RuntimeValue::Datetime(_), RuntimeValue::Datetime(_)) => {
                    return Err(ctx.eval_error("cannot add two datetimes", span));
                }
                (RuntimeValue::Datetime(e), RuntimeValue::Scalar(secs)) => {
                    let duration = hifitime::Duration::from_seconds(*secs);
                    #[expect(
                        clippy::arithmetic_side_effects,
                        reason = "hifitime exposes Epoch +/- Duration for datetime arithmetic"
                    )]
                    return match op {
                        BinOp::Add => Ok(RuntimeValue::Datetime(*e + duration)),
                        BinOp::Sub => Ok(RuntimeValue::Datetime(*e - duration)),
                        _ => Err(ctx.eval_error(
                            format!("unsupported operator {op:?} for Datetime and scalar"),
                            span,
                        )),
                    };
                }
                (RuntimeValue::Scalar(secs), RuntimeValue::Datetime(e)) if op == BinOp::Add => {
                    let duration = hifitime::Duration::from_seconds(*secs);
                    #[expect(
                        clippy::arithmetic_side_effects,
                        reason = "hifitime exposes Epoch + Duration for datetime arithmetic"
                    )]
                    {
                        return Ok(RuntimeValue::Datetime(*e + duration));
                    }
                }
                (RuntimeValue::Scalar(_), RuntimeValue::Datetime(_)) => {
                    return Err(ctx.eval_error("cannot subtract a Datetime from a scalar", span));
                }
                _ => {}
            }
            let lv = l
                .expect_scalar("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            let rv = r
                .expect_scalar("binary operand")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            super::arithmetic::eval_scalar_binop(op, lv, rv, ctx, span).map(RuntimeValue::Scalar)
        }
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
                _ => Ok(RuntimeValue::Scalar(
                    -v.expect_scalar("unary negation")
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
    let FunctionRef::Builtin(name) = callee.value;
    match name.special_kind() {
        Some(SpecialFnKind::Aggregation(kind)) if args.len() == 1 => {
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            if let RuntimeValue::Indexed { entries, .. } = arg_val {
                return eval_hir_aggregation_fn(kind, &entries, expr.span, ctx.src);
            }
            eval_hir_builtin_fn(expr, name, args, values, local_values, ctx)
        }
        Some(SpecialFnKind::TypeConversion(kind)) => {
            eval_hir_conversion_fn(kind, expr.span, args, values, local_values, ctx)
        }
        Some(SpecialFnKind::TimeScaleConversion(scale)) => {
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
        Some(SpecialFnKind::Constructor(kind)) => {
            eval_hir_datetime_constructor(kind, expr.span, args, ctx.src)
        }
        Some(SpecialFnKind::DatetimeExtract(kind)) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    args[0].span,
                ));
            };
            let (year, month, day, hour, minute, second, _) = epoch.to_gregorian_utc();
            let result = match kind {
                DatetimeExtractFn::Year => i64::from(year),
                DatetimeExtractFn::Month => i64::from(month),
                DatetimeExtractFn::Day => i64::from(day),
                DatetimeExtractFn::Hour => i64::from(hour),
                DatetimeExtractFn::Minute => i64::from(minute),
                DatetimeExtractFn::Second => i64::from(second),
                DatetimeExtractFn::Weekday => i64::from(u8::from(epoch.weekday_utc())),
                DatetimeExtractFn::DayOfYear => {
                    let start = hifitime::Epoch::from_gregorian_utc_at_midnight(year, 1, 1);
                    #[expect(
                        clippy::arithmetic_side_effects,
                        reason = "hifitime exposes Epoch subtraction for elapsed-day calculation"
                    )]
                    let doy = (epoch - start).to_seconds().div_euclid(86400.0);
                    if !doy.is_finite() || !(0.0..366.0).contains(&doy) {
                        return Err(ctx.eval_error(
                            format!(
                                "day_of_year() input is outside the current Gregorian year: {doy}"
                            ),
                            args[0].span,
                        ));
                    }
                    #[expect(clippy::cast_possible_truncation, reason = "bounds-checked above")]
                    let day_index = doy as i64;
                    day_index.checked_add(1).ok_or_else(|| {
                        ctx.eval_error("day_of_year() overflowed i64", args[0].span)
                    })?
                }
            };
            Ok(RuntimeValue::Int(result))
        }
        Some(SpecialFnKind::DatetimeFrom(kind)) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let num = match arg_val {
                RuntimeValue::Scalar(v) => v,
                #[expect(clippy::cast_precision_loss, reason = "Julian/Unix values fit f64")]
                RuntimeValue::Int(v) => v as f64,
                _ => {
                    return Err(ctx.internal_error(
                        format!("{}() received non-numeric argument", name.as_str()),
                        args[0].span,
                    ));
                }
            };
            let epoch = match kind {
                DatetimeFromFn::FromJd => hifitime::Epoch::from_jde_utc(num),
                DatetimeFromFn::FromMjd => hifitime::Epoch::from_mjd_utc(num),
                DatetimeFromFn::FromUnix => hifitime::Epoch::from_unix_seconds(num),
            };
            Ok(RuntimeValue::Datetime(epoch))
        }
        Some(SpecialFnKind::DatetimeTo(kind)) => {
            expect_hir_builtin_arity(name, args, 1, callee.span, ctx)?;
            let arg_val = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Datetime(epoch) = arg_val else {
                return Err(ctx.internal_error(
                    format!("{}() received non-Datetime argument", name.as_str()),
                    args[0].span,
                ));
            };
            let result = match kind {
                DatetimeToFn::ToJd => epoch.to_jde_utc_days(),
                DatetimeToFn::ToMjd => epoch.to_mjd_utc_days(),
                DatetimeToFn::ToUnix => epoch.to_unix_seconds(),
            };
            Ok(RuntimeValue::Scalar(result))
        }
        None | Some(SpecialFnKind::Aggregation(_)) => {
            eval_hir_builtin_fn(expr, name, args, values, local_values, ctx)
        }
    }
}

fn eval_hir_aggregation_fn(
    kind: AggregationFn,
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    super::aggregations::aggregate_indexed_scalars(kind, entries).map_err(|err| {
        GraphcalError::EvalError {
            message: err.to_string(),
            src: src.clone(),
            span: span.into(),
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
            Ok(RuntimeValue::Scalar(i as f64))
        }
        TypeConversionFn::ToInt => {
            let arg = eval_hir_expr(&args[0], values, local_values, ctx)?;
            let f = arg
                .expect_scalar("to_int argument")
                .map_err(|e| ctx.eval_error(e.to_string(), span))?;
            super::conversions::checked_f64_to_i64(f)
                .map(RuntimeValue::Int)
                .map_err(|err| ctx.eval_error(err.to_string(), span))
        }
    }
}

fn eval_hir_datetime_constructor(
    kind: ConstructorFn,
    span: Span,
    args: &[hir::Expr],
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match kind {
        ConstructorFn::Datetime => {
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
            let hir::ExprKind::StringLiteral(s) = &args[0].kind else {
                return Err(GraphcalError::InternalError {
                    message: "datetime() received non-string argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            let epoch = if args.len() == 2 {
                let hir::ExprKind::StringLiteral(tz_name) = &args[1].kind else {
                    return Err(GraphcalError::InternalError {
                        message: "datetime() received non-string timezone argument".to_string(),
                        src: src.clone(),
                        span: args[1].span.into(),
                    });
                };
                super::functions::datetime_with_timezone(s, tz_name).map_err(|e| {
                    GraphcalError::EvalError {
                        message: format!("invalid datetime with timezone: {e}"),
                        src: src.clone(),
                        span: args[0].span.into(),
                    }
                })?
            } else {
                hifitime::Epoch::from_gregorian_str(s).map_err(|e| GraphcalError::EvalError {
                    message: format!("invalid datetime string: {e}"),
                    src: src.clone(),
                    span: args[0].span.into(),
                })?
            };
            Ok(RuntimeValue::Datetime(epoch))
        }
        ConstructorFn::Epoch => {
            if args.len() != 2 {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "epoch() received {} argument(s) after dim-check accepted arity 2",
                        args.len()
                    ),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let hir::ExprKind::StringLiteral(s) = &args[0].kind else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received non-string first argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            let hir::ExprKind::ConstRef(scale_ref) = &args[1].kind else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received non-time-scale second argument".to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            };
            let ConstRef::TimeScale(scale) = &scale_ref.value else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received non-time-scale second argument".to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            };
            let with_scale = format!("{s} {scale}");
            let epoch = hifitime::Epoch::from_gregorian_str(&with_scale).map_err(|e| {
                GraphcalError::EvalError {
                    message: format!("invalid epoch string: {e}"),
                    src: src.clone(),
                    span: span.into(),
                }
            })?;
            Ok(RuntimeValue::Datetime(epoch))
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
    let builtin = ctx.builtin_fns.get(name.as_str()).ok_or_else(|| {
        ctx.eval_error(format!("unknown function `{}`", name.as_str()), expr.span)
    })?;
    let arg_values: Vec<f64> = args
        .iter()
        .map(|arg| {
            let rv = eval_hir_expr(arg, values, local_values, ctx)?;
            rv.expect_scalar("function argument")
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
    Ok(RuntimeValue::Scalar(super::arithmetic::check_finite(
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
    if let Some(nat_range) = index_ref.nat_range() {
        return ctx.registry.indexes.get_nat_range(nat_range);
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
        hir::expr::MapEntryKey::NatRangeVariant { size, variant } => {
            let nat_range = graphcal_compiler::registry::types::NatRangeIndex::try_from_u64(*size)
                .map_err(|err| ctx.eval_error(err.to_string(), variant.span))?;
            Ok(IndexTypeRef::from_nat_range(nat_range))
        }
    }
}

fn map_entry_variant_for_axis(
    key: &hir::expr::MapEntryKey,
    axis: &IndexTypeRef,
    ctx: &EvalContext<'_>,
) -> Result<IndexVariantName, GraphcalError> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => {
            ensure_index_ref_matches_resolved(
                axis,
                variant.variant.index(),
                variant.variant_span,
                ctx,
            )?;
            Ok(variant.variant.variant().clone())
        }
        hir::expr::MapEntryKey::NatRangeVariant { variant, .. } => Ok(variant.value.clone()),
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
        hir::expr::MapEntryKey::NatRangeVariant { .. } => index_def_for_ref(index_ref, ctx),
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
        let mut result = IndexMap::new();
        for entry in entries {
            let key = entry.keys.first();
            let variant = map_entry_variant_for_axis(key, &idx_name, ctx)?;
            let val = eval_hir_expr(&entry.value, values, local_values, ctx)?;
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
    let variants = idx_def.variants();
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
    let (idx_name, error_span, dynamic_nat_size) = match &binding.index {
        hir::expr::ForBindingIndex::Named(index) => (
            IndexTypeRef::from_resolved(index.value.clone()),
            index.span,
            None,
        ),
        hir::expr::ForBindingIndex::Range { arg, span } => {
            let size = eval_hir_nat_expr(arg, ctx)?;
            if size == 0 {
                return Err(ctx.eval_error(
                    "range(0) is not allowed; indexes must contain at least one element",
                    *span,
                ));
            }
            let nat_range = graphcal_compiler::registry::types::NatRangeIndex::try_from_u64(size)
                .map_err(|err| ctx.eval_error(err.to_string(), *span))?;
            (
                IndexTypeRef::from_nat_range(nat_range),
                *span,
                Some(nat_range),
            )
        }
    };

    let dynamic_nat_def;
    let idx_def = if let Some(def) = index_def_for_ref(&idx_name, ctx) {
        def
    } else if let Some(nat_range) = dynamic_nat_size {
        dynamic_nat_def = IndexDef {
            name: nat_range.display_name(),
            kind: IndexKind::NatRange {
                size: nat_range.size(),
            },
        };
        &dynamic_nat_def
    } else {
        return Err(ctx.internal_error(format!("unknown index `{idx_name}`"), error_span));
    };

    let remaining = &bindings[1..];
    let variants = idx_def.variants();
    let mut entries = IndexMap::new();
    let mut inner_locals = local_values.child(Vec::new());
    for (step_index, variant) in variants.iter().enumerate() {
        let binding_value = match &idx_def.kind {
            IndexKind::Named { .. } | IndexKind::RequiredNamed => RuntimeValue::Label {
                index_name: idx_name.clone(),
                variant: variant.clone(),
            },
            IndexKind::Range(data) => RuntimeValue::RangeLabel {
                step_index,
                value: data.step_value(step_index),
            },
            IndexKind::RequiredRange { .. } => {
                return Err(ctx.internal_error("RequiredRange should have been bound", error_span));
            }
            IndexKind::NatRange { .. } => {
                RuntimeValue::Int(i64::try_from(step_index).map_err(|_| {
                    ctx.internal_error(
                        format!("nat range step {step_index} too large for i64"),
                        error_span,
                    )
                })?)
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
        let variant_name = match arg {
            hir::expr::IndexArg::Variant(variant) => {
                ensure_index_ref_matches_resolved(
                    &index_name,
                    variant.variant.index(),
                    variant.path_span(),
                    ctx,
                )?;
                variant.variant.variant().clone()
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
                        variant.clone()
                    }
                    RuntimeValue::Struct { type_name, .. } => {
                        IndexVariantName::expect_valid(type_name.as_str())
                    }
                    RuntimeValue::RangeLabel { step_index, .. } => {
                        IndexVariantName::range_step(step_index)
                    }
                    RuntimeValue::Int(n) => IndexVariantName::range_step(n),
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
                IndexVariantName::range_step(n)
            }
        };
        current = entries
            .get(variant_name.as_str())
            .cloned()
            .ok_or_else(|| ctx.eval_error(format!("variant `{variant_name}` not found"), span))?;
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
    span: Span,
    init: &hir::Expr,
    prev: &hir::LocalDef,
    curr: &hir::LocalDef,
    body: &hir::Expr,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let unfold_ctx = ctx.unfold_context.as_ref().ok_or_else(|| {
        ctx.eval_error(
            "unfold expression requires evaluation context with self_name and declared_types",
            span,
        )
    })?;
    let self_name = unfold_ctx.self_name;
    let declared = unfold_ctx
        .declared_types
        .get(&ScopedName::local(self_name))
        .ok_or_else(|| {
            ctx.eval_error(
                format!("no declared type for node `{self_name}`"),
                Span::new(0, 0),
            )
        })?;
    let index_ref = match declared {
        graphcal_compiler::registry::declared_type::DeclaredType::Indexed { index, .. } => {
            index.clone()
        }
        _ => {
            return Err(ctx.eval_error(
                format!("node `{self_name}` must have an indexed type for time scan"),
                Span::new(0, 0),
            ));
        }
    };
    let idx_def = index_def_for_ref(&index_ref, ctx)
        .ok_or_else(|| ctx.eval_error(format!("unknown index `{index_ref}`"), Span::new(0, 0)))?;
    let step_count = idx_def.step_count();
    let variants = idx_def.variants();
    let range_data = idx_def.range_data().ok_or_else(|| {
        ctx.eval_error(
            format!("unfold requires a range index, but `{index_ref}` is not a range"),
            Span::new(0, 0),
        )
    })?;
    let empty_locals = HirLocalValueMap::root();
    let init_val = eval_hir_expr(init, values, &empty_locals, ctx)?;
    let mut result_entries = IndexMap::new();
    result_entries.insert(variants[0].clone(), init_val);

    let self_scoped = ScopedName::local(self_name);
    let self_key = RuntimeDeclKey::for_local_decl(
        ctx.current_dag.unwrap_or_else(|| ctx.tir.root()),
        &self_scoped,
    );
    let mut overlay_values = values.clone();
    overlay_values.insert(
        self_key.clone(),
        RuntimeValue::Indexed {
            index_name: index_ref.clone(),
            entries: IndexMap::new(),
        },
    );
    let mut scan_locals = HirLocalValueMap::root();
    #[expect(
        clippy::needless_range_loop,
        reason = "step index addresses both variants and accumulated values"
    )]
    for i in 1..step_count {
        if let Some(RuntimeValue::Indexed { entries, .. }) = overlay_values.get_mut(&self_key) {
            *entries = std::mem::take(&mut result_entries);
        }
        let previous_step_index = i.saturating_sub(1);
        scan_locals.bind(
            prev.id,
            RuntimeValue::RangeLabel {
                step_index: previous_step_index,
                value: range_data.step_value(previous_step_index),
            },
        );
        scan_locals.bind(
            curr.id,
            RuntimeValue::RangeLabel {
                step_index: i,
                value: range_data.step_value(i),
            },
        );
        let body_val = eval_hir_expr(body, &overlay_values, &scan_locals, ctx)?;
        if let Some(RuntimeValue::Indexed { entries, .. }) = overlay_values.get_mut(&self_key) {
            result_entries = std::mem::take(entries);
        }
        result_entries.insert(variants[i].clone(), body_val);
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
        unfold_context: None,
        tir: ctx.tir,
        current_dag: Some(dag_tir),
        root_values: ctx.root_values,
        struct_field_constraints: ctx.struct_field_constraints,
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
                "dag `{}` has no node `{}` after evaluation (should have been caught by dim-check)",
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
