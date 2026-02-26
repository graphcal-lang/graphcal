use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{Expr, ExprKind, FnBody};

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;
use crate::resolve::is_aggregation_fn;
use crate::runtime_value::RuntimeValue;

use super::arithmetic::check_finite;
use super::eval_expr;

/// Evaluate a function call expression (`FnCall` or `QualifiedFnCall`).
#[expect(
    clippy::too_many_lines,
    reason = "single function handling all call variants"
)]
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
    // Aggregation functions over indexed values: sum, min, max, mean, count
    if is_aggregation_fn(name.value.as_str()) && args.len() == 1 {
        let arg_val = eval_expr(
            &args[0],
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
        if let RuntimeValue::Indexed { entries, .. } = arg_val {
            let type_err = |msg: String| GraphcalError::EvalError {
                message: msg,
                src: src.clone(),
                span: expr.span.into(),
            };
            return Ok(match name.value.as_str() {
                "sum" => {
                    let mut total = 0.0_f64;
                    for v in entries.values() {
                        total += v.expect_scalar("sum element").map_err(&type_err)?;
                    }
                    RuntimeValue::Scalar(total)
                }
                "min" => {
                    let mut min = f64::INFINITY;
                    for v in entries.values() {
                        min = min.min(v.expect_scalar("min element").map_err(&type_err)?);
                    }
                    RuntimeValue::Scalar(min)
                }
                "max" => {
                    let mut max = f64::NEG_INFINITY;
                    for v in entries.values() {
                        max = max.max(v.expect_scalar("max element").map_err(&type_err)?);
                    }
                    RuntimeValue::Scalar(max)
                }
                "mean" => {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "indexed collection length fits in f64"
                    )]
                    let n = entries.len() as f64;
                    let mut total = 0.0_f64;
                    for v in entries.values() {
                        total += v.expect_scalar("mean element").map_err(&type_err)?;
                    }
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
            });
        }
        // If not indexed, fall through to builtins (min/max are 2-arg builtins)
    }

    // Conversion builtins: to_float and to_int
    if name.value.as_str() == "to_float" {
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
        return Ok(RuntimeValue::Scalar(i as f64));
    }
    if name.value.as_str() == "to_int" {
        let arg = eval_expr(
            &args[0],
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
        let f = arg
            .expect_scalar("to_int argument")
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
        return Ok(RuntimeValue::Int(f as i64));
    }

    // datetime(string_literal) -> Datetime(UTC)
    // datetime(string_literal, timezone_string) -> Datetime(UTC)
    if name.value.as_str() == "datetime" {
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
        return Ok(RuntimeValue::Datetime(epoch));
    }

    // epoch(string_literal, TimeScale) -> Datetime in specified scale
    if name.value.as_str() == "epoch" {
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
        return Ok(RuntimeValue::Datetime(epoch));
    }

    // Time scale conversion: to_utc, to_tai, to_tt, etc.
    if let Some(target_scale) =
        crate::time_scale::time_scale_from_conversion_fn(name.value.as_str())
    {
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
        return Ok(RuntimeValue::Datetime(converted));
    }

    // Datetime extraction functions: year, month, day, etc.
    if crate::resolve::DATETIME_EXTRACT_FNS.contains(&name.value.as_str()) {
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
        return Ok(RuntimeValue::Int(result));
    }

    // Datetime from-numeric constructors: from_jd, from_mjd, from_unix
    if crate::resolve::DATETIME_FROM_FNS.contains(&name.value.as_str()) {
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
        return Ok(RuntimeValue::Datetime(epoch));
    }

    // Datetime to-numeric functions: to_jd, to_mjd, to_unix
    if crate::resolve::DATETIME_TO_FNS.contains(&name.value.as_str()) {
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
        return Ok(RuntimeValue::Scalar(result));
    }

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
