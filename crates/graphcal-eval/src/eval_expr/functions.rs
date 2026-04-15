use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::syntax::ast::{Expr, ExprKind};
use graphcal_compiler::syntax::names::VariantName;

use crate::error::GraphcalError;
use crate::resolve_types::{
    AggregationFn, ConstructorFn, DatetimeExtractFn, DatetimeFromFn, DatetimeToFn,
    SpecialFnKind, TypeConversionFn, classify_special_fn,
};
use crate::runtime_value::RuntimeValue;

use super::EvalContext;
use super::arithmetic::check_finite;
use super::eval_expr;

/// Evaluate a built-in function call expression (`FnCall`).
pub(super) fn eval_fn_call(
    expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let fn_ctx = FnDispatch {
        expr,
        name,
        args,
        values,
        local_values,
        ctx,
    };
    match classify_special_fn(name.value.as_str()) {
        Some(SpecialFnKind::Aggregation(kind)) if args.len() == 1 => {
            fn_ctx.dispatch_aggregation(kind)
        }
        Some(SpecialFnKind::TypeConversion(kind)) => {
            eval_conversion_fn(kind, expr, name, args, values, local_values, ctx)
        }
        Some(SpecialFnKind::TimeScaleConversion) => {
            #[expect(
                clippy::expect_used,
                reason = "TimeScaleConversion variant guarantees a valid time scale name"
            )]
            let scale =
                crate::time_scale::time_scale_from_conversion_fn(name.value.as_str())
                    .expect("TimeScaleConversion variant guarantees a valid time scale name");
            fn_ctx.dispatch_timescale(scale)
        }
        Some(SpecialFnKind::Constructor(kind)) => {
            eval_datetime_constructor(kind, expr, name, args, ctx.src)
        }
        Some(SpecialFnKind::DatetimeExtract(kind)) => {
            fn_ctx.dispatch_datetime_extract(kind)
        }
        Some(SpecialFnKind::DatetimeFrom(kind)) => fn_ctx.dispatch_datetime_from(kind),
        Some(SpecialFnKind::DatetimeTo(kind)) => fn_ctx.dispatch_datetime_to(kind),
        _ => fn_ctx.dispatch_timescale_or_builtin(),
    }
}

/// Bundles the function dispatch context.
///
/// This avoids repeating arguments in every helper invocation inside `eval_fn_call`.
struct FnDispatch<'a, 'ctx> {
    expr: &'a Expr,
    name: &'a graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &'a [Expr],
    values: &'a HashMap<String, RuntimeValue>,
    local_values: &'a HashMap<String, RuntimeValue>,
    ctx: &'a EvalContext<'ctx>,
}

impl FnDispatch<'_, '_> {
    /// Aggregation dispatch: if the single argument is `Indexed`, aggregate; otherwise fall
    /// through to builtins (e.g. 2-arg `min`/`max`).
    fn dispatch_aggregation(&self, kind: AggregationFn) -> Result<RuntimeValue, GraphcalError> {
        let arg_val = eval_expr(&self.args[0], self.values, self.local_values, self.ctx)?;
        if let RuntimeValue::Indexed { entries, .. } = arg_val {
            return eval_aggregation_fn(kind, &entries, self.expr, self.ctx.src);
        }
        // Not indexed, fall through to builtin (min/max are 2-arg builtins)
        self.dispatch_builtin()
    }

    /// Fallback: check for a time-scale conversion, then builtins.
    fn dispatch_timescale_or_builtin(&self) -> Result<RuntimeValue, GraphcalError> {
        crate::time_scale::time_scale_from_conversion_fn(self.name.value.as_str()).map_or_else(
            || self.dispatch_builtin(),
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

    fn dispatch_builtin(&self) -> Result<RuntimeValue, GraphcalError> {
        eval_builtin_fn(
            self.expr,
            self.name,
            self.args,
            self.values,
            self.local_values,
            self.ctx,
        )
    }

    fn dispatch_datetime_extract(
        &self,
        kind: DatetimeExtractFn,
    ) -> Result<RuntimeValue, GraphcalError> {
        eval_datetime_extract_fn(
            kind, self.expr, self.name, self.args, self.values, self.local_values, self.ctx,
        )
    }

    fn dispatch_datetime_from(
        &self,
        kind: DatetimeFromFn,
    ) -> Result<RuntimeValue, GraphcalError> {
        eval_datetime_from_fn(
            kind, self.expr, self.name, self.args, self.values, self.local_values, self.ctx,
        )
    }

    fn dispatch_datetime_to(
        &self,
        kind: DatetimeToFn,
    ) -> Result<RuntimeValue, GraphcalError> {
        eval_datetime_to_fn(
            kind, self.expr, self.name, self.args, self.values, self.local_values, self.ctx,
        )
    }
}

/// Evaluate an aggregation function over indexed entries.
fn eval_aggregation_fn(
    kind: AggregationFn,
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
    Ok(match kind {
        AggregationFn::Sum => {
            let total = entries.values().try_fold(0.0_f64, |acc, v| {
                Ok(acc + v.expect_scalar("sum element").map_err(&type_err)?)
            })?;
            RuntimeValue::Scalar(total)
        }
        AggregationFn::Min => {
            let min = entries.values().try_fold(f64::INFINITY, |acc, v| {
                Ok(acc.min(v.expect_scalar("min element").map_err(&type_err)?))
            })?;
            RuntimeValue::Scalar(min)
        }
        AggregationFn::Max => {
            let max = entries.values().try_fold(f64::NEG_INFINITY, |acc, v| {
                Ok(acc.max(v.expect_scalar("max element").map_err(&type_err)?))
            })?;
            RuntimeValue::Scalar(max)
        }
        AggregationFn::Mean => {
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
        AggregationFn::Count => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "indexed collection length fits in f64"
            )]
            let n = entries.len() as f64;
            RuntimeValue::Scalar(n)
        }
    })
}

/// Evaluate a type conversion function (`to_float`, `to_int`).
fn eval_conversion_fn(
    kind: TypeConversionFn,
    expr: &Expr,
    _name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    match kind {
        TypeConversionFn::ToFloat => {
            let arg = eval_expr(&args[0], values, local_values, ctx)?;
            let RuntimeValue::Int(i) = arg else {
                return Err(
                    ctx.internal_error("to_float() received non-Int argument", args[0].span)
                );
            };
            #[expect(
                clippy::cast_precision_loss,
                reason = "explicit conversion from Int to float"
            )]
            Ok(RuntimeValue::Scalar(i as f64))
        }
        TypeConversionFn::ToInt => {
            let arg = eval_expr(&args[0], values, local_values, ctx)?;
            let f = arg
                .expect_scalar("to_int argument")
                .map_err(|e| ctx.eval_error(e.to_string(), expr.span))?;
            if !f.is_finite() {
                return Err(ctx.eval_error(
                    format!("to_int() requires a finite value, got {f}"),
                    expr.span,
                ));
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
                return Err(ctx.eval_error(
                    format!(
                        "to_int() argument {f} is outside the representable integer range ({}..={})",
                        i64::MIN,
                        i64::MAX,
                    ),
                    expr.span,
                ));
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "range-checked truncating conversion from float to Int"
            )]
            Ok(RuntimeValue::Int(f as i64))
        }
    }
}

/// Evaluate a datetime constructor (`datetime`, `epoch`).
fn eval_datetime_constructor(
    kind: ConstructorFn,
    _expr: &Expr,
    _name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match kind {
        ConstructorFn::Datetime => {
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
        ConstructorFn::Epoch => {
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
        return Err(ctx.internal_error(
            format!("{}() received non-Datetime argument", name.value),
            args[0].span,
        ));
    };
    let converted = epoch.to_time_scale(target_scale.to_hifitime());
    Ok(RuntimeValue::Datetime(converted))
}

/// Evaluate a datetime extraction function (`year`, `month`, `day`, etc.).
fn eval_datetime_extract_fn(
    kind: DatetimeExtractFn,
    _expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(&args[0], values, local_values, ctx)?;
    let RuntimeValue::Datetime(epoch) = arg_val else {
        return Err(ctx.internal_error(
            format!("{}() received non-Datetime argument", name.value),
            args[0].span,
        ));
    };
    // Decompose into Gregorian components in UTC
    let (year, month, day, hour, minute, second, _nanos) = epoch.to_gregorian_utc();
    let result: i64 = match kind {
        DatetimeExtractFn::Year => i64::from(year),
        DatetimeExtractFn::Month => i64::from(month),
        DatetimeExtractFn::Day => i64::from(day),
        DatetimeExtractFn::Hour => i64::from(hour),
        DatetimeExtractFn::Minute => i64::from(minute),
        DatetimeExtractFn::Second => i64::from(second),
        DatetimeExtractFn::Weekday => i64::from(u8::from(epoch.weekday_utc())),
        DatetimeExtractFn::DayOfYear => {
            let start_of_year = hifitime::Epoch::from_gregorian_utc_at_midnight(year, 1, 1);
            let diff = epoch - start_of_year;
            #[expect(clippy::cast_possible_truncation, reason = "day-of-year fits in i64")]
            let doy = diff.to_seconds().div_euclid(86400.0) as i64 + 1;
            doy
        }
    };
    Ok(RuntimeValue::Int(result))
}

/// Evaluate a datetime-from-numeric constructor (`from_jd`, `from_mjd`, `from_unix`).
fn eval_datetime_from_fn(
    kind: DatetimeFromFn,
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
            return Err(ctx.internal_error(
                format!("{}() received non-numeric argument", name.value),
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

/// Evaluate a datetime-to-numeric function (`to_jd`, `to_mjd`, `to_unix`).
fn eval_datetime_to_fn(
    kind: DatetimeToFn,
    _expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(&args[0], values, local_values, ctx)?;
    let RuntimeValue::Datetime(epoch) = arg_val else {
        return Err(ctx.internal_error(
            format!("{}() received non-Datetime argument", name.value),
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

/// Evaluate a builtin numeric function.
fn eval_builtin_fn(
    expr: &Expr,
    name: &graphcal_compiler::syntax::names::Spanned<graphcal_compiler::syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let builtin = ctx
        .builtin_fns
        .get(name.value.as_str())
        .ok_or_else(|| ctx.eval_error(format!("unknown function `{}`", name.value), name.span))?;

    let arg_values: Vec<f64> = args
        .iter()
        .map(|a| {
            let rv = eval_expr(a, values, local_values, ctx)?;
            rv.expect_scalar("function argument")
                .map_err(|e| ctx.eval_error(e.to_string(), a.span))
        })
        .collect::<Result<_, _>>()?;
    if arg_values.len() != builtin.arity() {
        return Err(ctx.eval_error(
            format!(
                "builtin function `{}` expects {} argument(s) but got {}",
                name.value,
                builtin.arity(),
                arg_values.len(),
            ),
            expr.span,
        ));
    }
    let result = (builtin.eval)(&arg_values);
    Ok(RuntimeValue::Scalar(check_finite(
        result,
        name.value.as_str(),
        ctx.src,
        expr.span,
    )?))
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
