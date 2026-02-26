use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_syntax::ast::{Expr, ExprKind, FnBody};
use graphcal_syntax::names::VariantName;

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;
use crate::resolve_types::{SpecialFnKind, classify_special_fn};
use crate::runtime_value::RuntimeValue;

use super::arithmetic::check_finite;
use super::eval_expr;

/// Evaluate a function call expression (`FnCall` or `QualifiedFnCall`).
pub(super) fn eval_fn_call(
    expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let ctx = EvalCtx {
        expr,
        name,
        args,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    };
    match classify_special_fn(name.value.as_str()) {
        Some(SpecialFnKind::Aggregation) if args.len() == 1 => ctx.dispatch_aggregation(),
        Some(SpecialFnKind::Conversion) => ctx.dispatch_conversion(),
        Some(SpecialFnKind::Constructor) => eval_datetime_constructor(expr, name, args, src),
        Some(SpecialFnKind::DatetimeExtract) => ctx.dispatch_with_eval(eval_datetime_extract_fn),
        Some(SpecialFnKind::DatetimeFrom) => ctx.dispatch_with_eval(eval_datetime_from_fn),
        Some(SpecialFnKind::DatetimeTo) => ctx.dispatch_with_eval(eval_datetime_to_fn),
        _ => ctx.dispatch_timescale_or_builtin(),
    }
}

/// Function pointer type for helpers that take the standard evaluation context.
type EvalHelperFn = fn(
    &Expr,
    &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    &[Expr],
    &HashMap<String, RuntimeValue>,
    &HashMap<String, RuntimeValue>,
    &HashMap<&str, f64>,
    &HashMap<&str, BuiltinFunction>,
    &Registry,
    &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError>;

/// Bundles the evaluation context that is threaded through every dispatch call.
///
/// This avoids repeating 9+ arguments in every helper invocation inside `eval_fn_call`.
struct EvalCtx<'a> {
    expr: &'a Expr,
    name: &'a graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &'a [Expr],
    values: &'a HashMap<String, RuntimeValue>,
    local_values: &'a HashMap<String, RuntimeValue>,
    builtin_consts: &'a HashMap<&'a str, f64>,
    builtin_fns: &'a HashMap<&'a str, BuiltinFunction>,
    registry: &'a Registry,
    src: &'a NamedSource<Arc<String>>,
}

impl EvalCtx<'_> {
    /// Aggregation dispatch: if the single argument is `Indexed`, aggregate; otherwise fall
    /// through to builtins (e.g. 2-arg `min`/`max`).
    fn dispatch_aggregation(&self) -> Result<RuntimeValue, GraphcalError> {
        let arg_val = eval_expr(
            &self.args[0],
            self.values,
            self.local_values,
            self.builtin_consts,
            self.builtin_fns,
            self.registry,
            self.src,
        )?;
        if let RuntimeValue::Indexed { entries, .. } = arg_val {
            return eval_aggregation_fn(self.name, &entries, self.expr, self.src);
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
                    self.builtin_consts,
                    self.builtin_fns,
                    self.registry,
                    self.src,
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
            self.builtin_consts,
            self.builtin_fns,
            self.registry,
            self.src,
        )
    }

    fn dispatch_builtin_or_user(&self) -> Result<RuntimeValue, GraphcalError> {
        eval_builtin_or_user_fn(
            self.expr,
            self.name,
            self.args,
            self.values,
            self.local_values,
            self.builtin_consts,
            self.builtin_fns,
            self.registry,
            self.src,
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
            self.builtin_consts,
            self.builtin_fns,
            self.registry,
            self.src,
        )
    }
}

/// Evaluate an aggregation function (`sum`, `min`, `max`, `mean`, `count`) over indexed entries.
fn eval_aggregation_fn(
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    entries: &IndexMap<VariantName, RuntimeValue>,
    expr: &Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let type_err = |msg: String| GraphcalError::EvalError {
        message: msg,
        src: src.clone(),
        span: expr.span.into(),
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
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
fn eval_conversion_fn(
    expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    match name.value.as_str() {
        "to_float" => {
            let arg = eval_expr(
                &args[0],
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            let RuntimeValue::Int(i) = arg else {
                return Err(GraphcalError::InternalError {
                    message: "to_float() received non-Int argument".to_string(),
                    src: src.clone(),
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
            let arg = eval_expr(
                &args[0],
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            let f =
                arg.expect_scalar("to_int argument")
                    .map_err(|msg| GraphcalError::EvalError {
                        message: msg,
                        src: src.clone(),
                        span: expr.span.into(),
                    })?;
            if !f.is_finite() {
                return Err(GraphcalError::EvalError {
                    message: format!("to_int() requires a finite value, got {f}"),
                    src: src.clone(),
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
                    src: src.clone(),
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
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
}

/// Evaluate a datetime constructor (`datetime`, `epoch`).
fn eval_datetime_constructor(
    expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
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
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
fn eval_timescale_fn(
    _expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    target_scale: crate::time_scale::TimeScale,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg = eval_expr(
        &args[0],
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;
    let RuntimeValue::Datetime(epoch) = arg else {
        return Err(GraphcalError::InternalError {
            message: format!("{}() received non-Datetime argument", name.value),
            src: src.clone(),
            span: args[0].span.into(),
        });
    };
    let converted = epoch.to_time_scale(target_scale.to_hifitime());
    Ok(RuntimeValue::Datetime(converted))
}

/// Evaluate a datetime extraction function (`year`, `month`, `day`, etc.).
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
fn eval_datetime_extract_fn(
    _expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(
        &args[0],
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;
    let RuntimeValue::Datetime(epoch) = arg_val else {
        return Err(GraphcalError::InternalError {
            message: format!("{}() received non-Datetime argument", name.value),
            src: src.clone(),
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
                src: src.clone(),
                span: name.span.into(),
            });
        }
    };
    Ok(RuntimeValue::Int(result))
}

/// Evaluate a datetime-from-numeric constructor (`from_jd`, `from_mjd`, `from_unix`).
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
fn eval_datetime_from_fn(
    _expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(
        &args[0],
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;
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
                src: src.clone(),
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
                src: src.clone(),
                span: name.span.into(),
            });
        }
    };
    Ok(RuntimeValue::Datetime(epoch))
}

/// Evaluate a datetime-to-numeric function (`to_jd`, `to_mjd`, `to_unix`).
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
fn eval_datetime_to_fn(
    _expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let arg_val = eval_expr(
        &args[0],
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;
    let RuntimeValue::Datetime(epoch) = arg_val else {
        return Err(GraphcalError::InternalError {
            message: format!("{}() received non-Datetime argument", name.value),
            src: src.clone(),
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
                src: src.clone(),
                span: name.span.into(),
            });
        }
    };
    Ok(RuntimeValue::Scalar(result))
}

/// Evaluate a builtin numeric function or a user-defined function.
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
fn eval_builtin_or_user_fn(
    expr: &Expr,
    name: &graphcal_syntax::names::Spanned<graphcal_syntax::names::FnName>,
    args: &[Expr],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    // Try builtin first
    if let Some(builtin) = builtin_fns.get(name.value.as_str()) {
        let arg_values: Vec<f64> = args
            .iter()
            .map(|a| {
                let rv = eval_expr(
                    a,
                    values,
                    local_values,
                    builtin_consts,
                    builtin_fns,
                    registry,
                    src,
                )?;
                rv.expect_scalar("function argument")
                    .map_err(|msg| GraphcalError::EvalError {
                        message: msg,
                        src: src.clone(),
                        span: a.span.into(),
                    })
            })
            .collect::<Result<_, _>>()?;
        let result = (builtin.eval)(&arg_values);
        return Ok(RuntimeValue::Scalar(check_finite(
            result,
            name.value.as_str(),
            src,
            expr.span,
        )?));
    }

    // Try user-defined function
    let fn_def = registry
        .functions
        .get_function(name.value.as_str())
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("unknown function `{}`", name.value),
            src: src.clone(),
            span: name.span.into(),
        })?;

    // Evaluate arguments
    let arg_values: Vec<RuntimeValue> = args
        .iter()
        .map(|a| {
            eval_expr(
                a,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
        })
        .collect::<Result<_, _>>()?;

    // Build fn_locals from param names + arg values
    let mut fn_locals: HashMap<String, RuntimeValue> = HashMap::new();
    for (param, val) in fn_def.params.iter().zip(arg_values) {
        fn_locals.insert(param.name.clone(), val);
    }

    // Evaluate body: pass `values` for ConstRef access (user consts like GM_EARTH).
    // Purity is enforced by the resolver's @ prohibition -- no GraphRef nodes
    // exist in function bodies, so passing values is safe.
    let body = fn_def.body.clone();
    match &body {
        FnBody::Short(expr) => eval_expr(
            expr,
            values,
            &fn_locals,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        ),
        FnBody::Block { stmts, expr } => {
            let mut block_locals = fn_locals;
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
                expr,
                values,
                &block_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )
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
