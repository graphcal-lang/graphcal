use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::syntax::ast::{Expr, ExprKind, FnBody};
use graphcal_compiler::syntax::names::VariantName;

use crate::error::GraphcalError;
use crate::resolve_types::{SpecialFnKind, classify_special_fn};
use crate::runtime_value::RuntimeValue;

use super::EvalContext;
use super::arithmetic::check_finite;
use super::eval_expr;

/// Evaluate a function call expression (`FnCall` or `QualifiedFnCall`).
pub(super) fn eval_fn_call(
    expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    type_args: &[graphcal_compiler::syntax::ast::GenericArg],
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let fn_ctx = FnDispatch {
        expr,
        name,
        type_args,
        args,
        values,
        local_values,
        ctx,
    };
    match classify_special_fn(name.value.as_str()) {
        Some(SpecialFnKind::Aggregation) if args.len() == 1 => fn_ctx.dispatch_aggregation(),
        Some(SpecialFnKind::Conversion) => fn_ctx.dispatch_conversion(),
        Some(SpecialFnKind::Constructor) => eval_datetime_constructor(expr, name, args, ctx.src),
        Some(SpecialFnKind::DatetimeExtract) => fn_ctx.dispatch_with_eval(eval_datetime_extract_fn),
        Some(SpecialFnKind::DatetimeFrom) => fn_ctx.dispatch_with_eval(eval_datetime_from_fn),
        Some(SpecialFnKind::DatetimeTo) => fn_ctx.dispatch_with_eval(eval_datetime_to_fn),
        _ => fn_ctx.dispatch_timescale_or_builtin(),
    }
}

/// Function pointer type for helpers that take the standard evaluation context.
type EvalHelperFn = fn(
    &Expr,
    &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    &[Expr],
    &HashMap<String, RuntimeValue>,
    &HashMap<String, RuntimeValue>,
    &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError>;

/// Bundles the function dispatch context.
///
/// This avoids repeating arguments in every helper invocation inside `eval_fn_call`.
struct FnDispatch<'a, 'ctx> {
    expr: &'a Expr,
    name: &'a graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    type_args: &'a [graphcal_compiler::syntax::ast::GenericArg],
    args: &'a [Expr],
    values: &'a HashMap<String, RuntimeValue>,
    local_values: &'a HashMap<String, RuntimeValue>,
    ctx: &'a EvalContext<'ctx>,
}

impl FnDispatch<'_, '_> {
    /// Aggregation dispatch: if the single argument is `Indexed`, aggregate; otherwise fall
    /// through to builtins (e.g. 2-arg `min`/`max`).
    fn dispatch_aggregation(&self) -> Result<RuntimeValue, GraphcalError> {
        let arg_val = eval_expr(&self.args[0], self.values, self.local_values, self.ctx)?;
        if let RuntimeValue::Indexed { entries, .. } = arg_val {
            return eval_aggregation_fn(self.name, &entries, self.expr, self.ctx.src);
        }
        // Not indexed, fall through to builtin (min/max are 2-arg builtins)
        self.dispatch_builtin_or_user()
    }

    /// Conversion dispatch: time-scale conversions (`to_utc`, …) vs type conversions
    /// (`to_float`, `to_int`).
    fn dispatch_conversion(&self) -> Result<RuntimeValue, GraphcalError> {
        crate::time_scale::time_scale_from_conversion_fn(self.name.value.as_str()).map_or_else(
            || {
                eval_conversion_fn(
                    self.expr,
                    self.name,
                    self.args,
                    self.values,
                    self.local_values,
                    self.ctx,
                )
            },
            |scale| self.dispatch_timescale(scale),
        )
    }

    /// Fallback: check for a time-scale conversion, then builtins / user-defined functions.
    fn dispatch_timescale_or_builtin(&self) -> Result<RuntimeValue, GraphcalError> {
        crate::time_scale::time_scale_from_conversion_fn(self.name.value.as_str()).map_or_else(
            || self.dispatch_builtin_or_user(),
            |scale| self.dispatch_timescale(scale),
        )
    }

    fn dispatch_timescale(
        &self,
        target_scale: crate::time_scale::TimeScale,
    ) -> Result<RuntimeValue, GraphcalError> {
        eval_timescale_fn(
            self.expr,
            self.name,
            self.args,
            target_scale,
            self.values,
            self.local_values,
            self.ctx,
        )
    }

    fn dispatch_builtin_or_user(&self) -> Result<RuntimeValue, GraphcalError> {
        eval_builtin_or_user_fn(
            self.expr,
            self.name,
            self.type_args,
            self.args,
            self.values,
            self.local_values,
            self.ctx,
        )
    }

    /// Dispatch to a helper that takes the standard eval-context arguments.
    fn dispatch_with_eval(&self, f: EvalHelperFn) -> Result<RuntimeValue, GraphcalError> {
        f(
            self.expr,
            self.name,
            self.args,
            self.values,
            self.local_values,
            self.ctx,
        )
    }
}

/// Evaluate an aggregation function (`sum`, `min`, `max`, `mean`, `count`) over indexed entries.
fn eval_aggregation_fn(
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    entries: &IndexMap<VariantName, RuntimeValue>,
    expr: &Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let type_err = |e: graphcal_compiler::registry::runtime_value::RuntimeValueError| {
        GraphcalError::EvalError {
            message: e.to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }
    };
    Ok(match name.value.as_str() {
        "sum" => {
            let total = entries.values().try_fold(0.0_f64, |acc, v| {
                Ok(acc + v.expect_scalar("sum element").map_err(&type_err)?)
            })?;
            RuntimeValue::Scalar(total)
        }
        "min" => {
            let min = entries.values().try_fold(f64::INFINITY, |acc, v| {
                Ok(acc.min(v.expect_scalar("min element").map_err(&type_err)?))
            })?;
            RuntimeValue::Scalar(min)
        }
        "max" => {
            let max = entries.values().try_fold(f64::NEG_INFINITY, |acc, v| {
                Ok(acc.max(v.expect_scalar("max element").map_err(&type_err)?))
            })?;
            RuntimeValue::Scalar(max)
        }
        "mean" => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "indexed collection length fits in f64"
            )]
            let n = entries.len() as f64;
            let total = entries.values().try_fold(0.0_f64, |acc, v| {
                Ok(acc + v.expect_scalar("mean element").map_err(&type_err)?)
            })?;
            RuntimeValue::Scalar(total / n)
        }
        "count" => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "indexed collection length fits in f64"
            )]
            let n = entries.len() as f64;
            RuntimeValue::Scalar(n)
        }
        _ => {
            return Err(GraphcalError::InternalError {
                message: format!("unexpected aggregate function `{}`", name.value),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
    })
}

/// Evaluate a type conversion function (`to_float`, `to_int`).
fn eval_conversion_fn(
    expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match name.value.as_str() {
        "to_float" => {
            let arg = eval_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Int(i) = arg else {
                return Err(GraphcalError::InternalError {
                    message: "to_float() received non-Int argument".to_string(),
                    src: ctx.src.clone(),
                    span: args[0].span.into(),
                });
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "explicit conversion from Int to float"
            )]
            Ok(RuntimeValue::Scalar(i as f64))
        }
        "to_int" => {
            let arg = eval_expr(&args[0], values, local_values, ctx)?;
            let f = arg
                .expect_scalar("to_int argument")
                .map_err(|e| GraphcalError::EvalError {
                    message: e.to_string(),
                    src: ctx.src.clone(),
                    span: expr.span.into(),
                })?;
            if !f.is_finite() {
                return Err(GraphcalError::EvalError {
                    message: format!("to_int() requires a finite value, got {f}"),
                    src: ctx.src.clone(),
                    span: expr.span.into(),
                });
            }
            // i64 range: -9_223_372_036_854_775_808 ..= 9_223_372_036_854_775_807
            // The casts below round to the nearest f64: i64::MIN rounds exactly,
            // i64::MAX rounds up to 9.223372036854776e18. This makes the check
            // slightly conservative (rejects a few borderline values), which is
            // the safe direction for engineering use.
            #[expect(
                clippy::cast_precision_loss,
                reason = "intentional: boundary rounds to safe side, rejecting borderline values"
            )]
            if f < (i64::MIN as f64) || f > (i64::MAX as f64) {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "to_int() argument {f} is outside the representable integer range ({}..={})",
                        i64::MIN,
                        i64::MAX,
                    ),
                    src: ctx.src.clone(),
                    span: expr.span.into(),
                });
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "range-checked truncating conversion from float to Int"
            )]
            Ok(RuntimeValue::Int(f as i64))
        }
        _ => Err(GraphcalError::InternalError {
            message: format!("unexpected conversion function `{}`", name.value),
            src: ctx.src.clone(),
            span: expr.span.into(),
        }),
    }
}

/// Evaluate a datetime constructor (`datetime`, `epoch`).
fn eval_datetime_constructor(
    expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match name.value.as_str() {
        "datetime" => {
            let ExprKind::StringLiteral(s) = &args[0].kind else {
                return Err(GraphcalError::InternalError {
                    message: "datetime() received non-string argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            let epoch = if args.len() == 2 {
                // Two-arg form: datetime("2024-11-05T10:00", "Asia/Tokyo")
                let ExprKind::StringLiteral(tz_name) = &args[1].kind else {
                    return Err(GraphcalError::InternalError {
                        message: "datetime() received non-string timezone argument".to_string(),
                        src: src.clone(),
                        span: args[1].span.into(),
                    });
                };
                datetime_with_timezone(s, tz_name).map_err(|e| GraphcalError::EvalError {
                    message: format!("invalid datetime with timezone: {e}"),
                    src: src.clone(),
                    span: args[0].span.into(),
                })?
            } else {
                // One-arg form: datetime("2024-11-05T12:00:00Z")
                hifitime::Epoch::from_gregorian_str(s).map_err(|e| GraphcalError::EvalError {
                    message: format!("invalid datetime string: {e}"),
                    src: src.clone(),
                    span: args[0].span.into(),
                })?
            };
            Ok(RuntimeValue::Datetime(epoch))
        }
        "epoch" => {
            let ExprKind::StringLiteral(s) = &args[0].kind else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received non-string first argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            let ExprKind::ConstRef(scale_ident) = &args[1].kind else {
                return Err(GraphcalError::InternalError {
                    message: "epoch() received non-identifier second argument".to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            };
            // Append the time scale suffix for hifitime's parser
            let with_scale = format!("{s} {}", scale_ident.value);
            let epoch = hifitime::Epoch::from_gregorian_str(&with_scale).map_err(|e| {
                GraphcalError::EvalError {
                    message: format!("invalid epoch string: {e}"),
                    src: src.clone(),
                    span: args[0].span.into(),
                }
            })?;
            Ok(RuntimeValue::Datetime(epoch))
        }
        _ => Err(GraphcalError::InternalError {
            message: format!("unexpected constructor function `{}`", name.value),
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
}

/// Evaluate a time scale conversion function (`to_utc`, `to_tai`, `to_tt`, etc.).
fn eval_timescale_fn(
    _expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    target_scale: crate::time_scale::TimeScale,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg = eval_expr(&args[0], values, local_values, ctx)?;
    let RuntimeValue::Datetime(epoch) = arg else {
        return Err(GraphcalError::InternalError {
            message: format!("{}() received non-Datetime argument", name.value),
            src: ctx.src.clone(),
            span: args[0].span.into(),
        });
    };
    let converted = epoch.to_time_scale(target_scale.to_hifitime());
    Ok(RuntimeValue::Datetime(converted))
}

/// Evaluate a datetime extraction function (`year`, `month`, `day`, etc.).
fn eval_datetime_extract_fn(
    _expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(&args[0], values, local_values, ctx)?;
    let RuntimeValue::Datetime(epoch) = arg_val else {
        return Err(GraphcalError::InternalError {
            message: format!("{}() received non-Datetime argument", name.value),
            src: ctx.src.clone(),
            span: args[0].span.into(),
        });
    };
    // Decompose into Gregorian components in UTC
    let (year, month, day, hour, minute, second, _nanos) = epoch.to_gregorian_utc();
    let result: i64 = match name.value.as_str() {
        "year" => i64::from(year),
        "month" => i64::from(month),
        "day" => i64::from(day),
        "hour" => i64::from(hour),
        "minute" => i64::from(minute),
        "second" => i64::from(second),
        "weekday" => i64::from(u8::from(epoch.weekday_utc())),
        "day_of_year" => {
            let start_of_year = hifitime::Epoch::from_gregorian_utc_at_midnight(year, 1, 1);
            let diff = epoch - start_of_year;
            #[expect(clippy::cast_possible_truncation, reason = "day-of-year fits in i64")]
            let doy = diff.to_seconds().div_euclid(86400.0) as i64 + 1;
            doy
        }
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("unknown extraction function `{}`", name.value),
                src: ctx.src.clone(),
                span: name.span.into(),
            });
        }
    };
    Ok(RuntimeValue::Int(result))
}

/// Evaluate a datetime-from-numeric constructor (`from_jd`, `from_mjd`, `from_unix`).
fn eval_datetime_from_fn(
    _expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(&args[0], values, local_values, ctx)?;
    let num = match arg_val {
        RuntimeValue::Scalar(v) => v,
        #[expect(
            clippy::cast_precision_loss,
            reason = "Julian/Unix values are small enough for f64"
        )]
        RuntimeValue::Int(v) => v as f64,
        _ => {
            return Err(GraphcalError::InternalError {
                message: format!("{}() received non-numeric argument", name.value),
                src: ctx.src.clone(),
                span: args[0].span.into(),
            });
        }
    };
    let epoch = match name.value.as_str() {
        "from_jd" => hifitime::Epoch::from_jde_utc(num),
        "from_mjd" => hifitime::Epoch::from_mjd_utc(num),
        "from_unix" => hifitime::Epoch::from_unix_seconds(num),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("unknown from-datetime function `{}`", name.value),
                src: ctx.src.clone(),
                span: name.span.into(),
            });
        }
    };
    Ok(RuntimeValue::Datetime(epoch))
}

/// Evaluate a datetime-to-numeric function (`to_jd`, `to_mjd`, `to_unix`).
fn eval_datetime_to_fn(
    _expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(&args[0], values, local_values, ctx)?;
    let RuntimeValue::Datetime(epoch) = arg_val else {
        return Err(GraphcalError::InternalError {
            message: format!("{}() received non-Datetime argument", name.value),
            src: ctx.src.clone(),
            span: args[0].span.into(),
        });
    };
    let result = match name.value.as_str() {
        "to_jd" => epoch.to_jde_utc_days(),
        "to_mjd" => epoch.to_mjd_utc_days(),
        "to_unix" => epoch.to_unix_seconds(),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("unknown to-datetime function `{}`", name.value),
                src: ctx.src.clone(),
                span: name.span.into(),
            });
        }
    };
    Ok(RuntimeValue::Scalar(result))
}

/// Evaluate a builtin numeric function or a user-defined function.
fn eval_builtin_or_user_fn(
    expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    type_args: &[graphcal_compiler::syntax::ast::GenericArg],
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    // Try builtin first
    if let Some(builtin) = ctx.builtin_fns.get(name.value.as_str()) {
        let arg_values: Vec<f64> = args
            .iter()
            .map(|a| {
                let rv = eval_expr(a, values, local_values, ctx)?;
                rv.expect_scalar("function argument")
                    .map_err(|e| GraphcalError::EvalError {
                        message: e.to_string(),
                        src: ctx.src.clone(),
                        span: a.span.into(),
                    })
            })
            .collect::<Result<_, _>>()?;
        let result = (builtin.eval)(&arg_values);
        return Ok(RuntimeValue::Scalar(check_finite(
            result,
            name.value.as_str(),
            ctx.src,
            expr.span,
        )?));
    }

    // Try user-defined function
    let fn_def = ctx
        .registry
        .functions
        .get_function(name.value.as_str())
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("unknown function `{}`", name.value),
            src: ctx.src.clone(),
            span: name.span.into(),
        })?;

    // Evaluate arguments
    let arg_values: Vec<RuntimeValue> = args
        .iter()
        .map(|a| eval_expr(a, values, local_values, ctx))
        .collect::<Result<_, _>>()?;

    // Build fn_locals from param names + arg values
    let mut fn_locals: HashMap<String, RuntimeValue> = HashMap::new();
    for (param, val) in fn_def.params.iter().zip(arg_values) {
        fn_locals.insert(param.name.clone(), val);
    }

    // Resolve nat generic params from turbofish type args and/or argument shapes.
    // First try turbofish (explicit), then fall back to argument-shape extraction.
    for (i, gp) in fn_def.generic_params.iter().enumerate() {
        if gp.constraint == graphcal_compiler::registry::registry::FnGenericConstraint::Nat {
            let nat_name = gp.name.as_str();

            // Try turbofish first
            let size = if i < type_args.len() {
                extract_nat_from_generic_arg(&type_args[i])
            } else {
                None
            };
            // Fall back to argument shape extraction
            let size = size.or_else(|| extract_nat_param_from_args(nat_name, fn_def, &fn_locals));

            if let Some(size) = size {
                let nat_int = RuntimeValue::Int(i64::try_from(size).unwrap_or(0));
                fn_locals.insert(format!("__nat_param_{nat_name}"), nat_int.clone());
                fn_locals.insert(nat_name.to_string(), nat_int);
            }
        }
    }

    // Evaluate body: pass `values` for ConstRef access (user consts like GM_EARTH).
    // Purity is enforced by the resolver's @ prohibition -- no GraphRef nodes
    // exist in function bodies, so passing values is safe.
    let body = fn_def.body.clone();
    match &body {
        FnBody::Short(expr) => eval_expr(expr, values, &fn_locals, ctx),
        FnBody::Block { stmts, expr } => {
            let mut block_locals = fn_locals;
            for binding in stmts {
                let val = eval_expr(&binding.value, values, &block_locals, ctx)?;
                block_locals.insert(binding.name.name.clone(), val);
            }
            eval_expr(expr, values, &block_locals, ctx)
        }
    }
}

/// Extract a nat value from a turbofish `GenericArg`, if it is a `Nat` variant.
fn extract_nat_from_generic_arg(arg: &graphcal_compiler::syntax::ast::GenericArg) -> Option<u64> {
    use graphcal_compiler::syntax::ast::GenericArg;
    match arg {
        GenericArg::Nat(nat_expr) => eval_nat_expr(nat_expr),
        GenericArg::Type(_) => None,
    }
}

/// Evaluate a `NatExpr` to a concrete `u64` at runtime (literals and addition only).
fn eval_nat_expr(nat_expr: &graphcal_compiler::syntax::ast::NatExpr) -> Option<u64> {
    use graphcal_compiler::syntax::ast::NatExpr;
    match nat_expr {
        NatExpr::Literal(v, _) => Some(*v),
        NatExpr::Add(lhs, rhs, _) => {
            let l = eval_nat_expr(lhs)?;
            let r = eval_nat_expr(rhs)?;
            Some(l + r)
        }
        NatExpr::Mul(lhs, rhs, _) => {
            let l = eval_nat_expr(lhs)?;
            let r = eval_nat_expr(rhs)?;
            Some(l * r)
        }
        NatExpr::Var(_) => None,
    }
}

/// Extract the value of a nat param by inspecting the function's argument values.
///
/// Walks the parameter type annotations looking for the nat param name in index position.
/// When found, extracts the corresponding nat range size from the actual argument value.
fn extract_nat_param_from_args(
    nat_name: &str,
    fn_def: &graphcal_compiler::registry::registry::FnDef,
    fn_locals: &HashMap<String, RuntimeValue>,
) -> Option<u64> {
    for param_def in &fn_def.params {
        if let Some(size) = extract_nat_from_type_and_value(
            nat_name,
            &param_def.type_expr,
            fn_locals.get(&param_def.name)?,
        ) {
            return Some(size);
        }
    }
    None
}

/// Recursively extract a nat param value from a type annotation and matching runtime value.
fn extract_nat_from_type_and_value(
    nat_name: &str,
    type_expr: &graphcal_compiler::syntax::ast::TypeExpr,
    value: &RuntimeValue,
) -> Option<u64> {
    use graphcal_compiler::syntax::ast::{IndexExpr, TypeExprKind};

    if let TypeExprKind::Indexed { base, indexes } = &type_expr.kind {
        // Peel from outermost: first index is outermost
        let mut current = value;
        for idx in indexes {
            let RuntimeValue::Indexed {
                index_name,
                entries,
            } = current
            else {
                return None;
            };
            match idx {
                IndexExpr::Name(ident) if ident.name == nat_name => {
                    // This index position has the nat param we're looking for.
                    // Extract the size from the actual index name.
                    return graphcal_compiler::registry::registry::parse_nat_range_index_name(
                        index_name.as_str(),
                    );
                }
                IndexExpr::NatExpr(nat_expr) => {
                    // Compound nat expression (e.g., N + 1): try to solve for the param.
                    if nat_expr_contains_var(nat_expr, nat_name) {
                        let actual_size =
                            graphcal_compiler::registry::registry::parse_nat_range_index_name(
                                index_name.as_str(),
                            )?;
                        return solve_nat_expr_for_var(nat_expr, nat_name, actual_size);
                    }
                }
                _ => {}
            }
            // Move to the first element to descend
            current = entries.values().next()?;
        }
        // Also check the base type recursively
        return extract_nat_from_type_and_value(nat_name, base, current);
    }
    None
}

/// Check if a `NatExpr` references a given variable.
fn nat_expr_contains_var(expr: &graphcal_compiler::syntax::ast::NatExpr, var_name: &str) -> bool {
    use graphcal_compiler::syntax::ast::NatExpr;
    match expr {
        NatExpr::Literal(_, _) => false,
        NatExpr::Var(ident) => ident.name == var_name,
        NatExpr::Add(lhs, rhs, _) | NatExpr::Mul(lhs, rhs, _) => {
            nat_expr_contains_var(lhs, var_name) || nat_expr_contains_var(rhs, var_name)
        }
    }
}

/// Solve a `NatExpr` for a single variable, given the target value.
///
/// Computes the constant part and the coefficient of the target variable,
/// then solves `coefficient * var + constant_sum = target`.
fn solve_nat_expr_for_var(
    expr: &graphcal_compiler::syntax::ast::NatExpr,
    var_name: &str,
    target: u64,
) -> Option<u64> {
    let (constant_sum, var_coeff) = nat_expr_linear_parts(expr, var_name);
    if var_coeff == 0 {
        return None;
    }
    let remainder = target.checked_sub(constant_sum)?;
    if remainder % var_coeff != 0 {
        return None;
    }
    Some(remainder / var_coeff)
}

/// Decompose a `NatExpr` into `(constant_sum, coefficient_of_var)` for a given variable.
fn nat_expr_linear_parts(
    expr: &graphcal_compiler::syntax::ast::NatExpr,
    var_name: &str,
) -> (u64, u64) {
    use graphcal_compiler::syntax::ast::NatExpr;
    match expr {
        NatExpr::Literal(n, _) => (*n, 0),
        NatExpr::Var(ident) => {
            if ident.name == var_name {
                (0, 1)
            } else {
                // Other variables are treated as constants (they should already be resolved)
                (0, 0)
            }
        }
        NatExpr::Add(lhs, rhs, _) => {
            let (lc, lv) = nat_expr_linear_parts(lhs, var_name);
            let (rc, rv) = nat_expr_linear_parts(rhs, var_name);
            (lc + rc, lv + rv)
        }
        NatExpr::Mul(lhs, rhs, _) => {
            let (lc, lv) = nat_expr_linear_parts(lhs, var_name);
            let (rc, rv) = nat_expr_linear_parts(rhs, var_name);
            // For linear solving: (lc * rc) is the constant-constant product,
            // (lc * rv + rc * lv) is the effective coefficient of the variable.
            // Cross-term lv * rv is the quadratic coefficient — we ignore it
            // (solve_nat_expr_for_var only handles linear equations).
            (lc * rc, lc * rv + rc * lv)
        }
    }
}

/// Parse a civil datetime string in a given IANA timezone and return a UTC `hifitime::Epoch`.
///
/// Uses jiff to resolve the civil time to a UTC instant, then converts to hifitime.
fn datetime_with_timezone(
    datetime_str: &str,
    tz_name: &str,
) -> Result<hifitime::Epoch, Box<dyn std::error::Error>> {
    let civil_dt: jiff::civil::DateTime = datetime_str.parse()?;
    let tz = jiff::tz::TimeZone::get(tz_name)?;
    let zdt = tz.to_zoned(civil_dt)?;
    let ts = zdt.timestamp();
    #[expect(
        clippy::cast_precision_loss,
        reason = "unix seconds for reasonable dates fit within f64 mantissa precision"
    )]
    let epoch = hifitime::Epoch::from_unix_seconds(ts.as_second() as f64)
        + hifitime::Duration::from_nanoseconds(f64::from(ts.subsec_nanosecond()));
    Ok(epoch)
}
