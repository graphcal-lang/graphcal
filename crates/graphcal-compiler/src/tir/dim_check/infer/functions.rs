//! Type inference for function calls (built-in functions only).

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::{Expr, ExprKind};
use crate::syntax::dimension::Dimension;
use crate::syntax::names::FnName;

use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::registry::resolve_types::{SpecialFnKind, classify_special_fn};

use super::super::builtins::infer_fn_dim;
use super::super::helpers::{expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Bundles the inference context that is threaded through every dispatch call.
///
/// This avoids repeating 7 arguments in every helper invocation inside `infer_fn_call`.
struct InferCtx<'a> {
    name: &'a crate::syntax::names::Spanned<FnName>,
    args: &'a [Expr],
    declared_types: &'a HashMap<String, DeclaredType>,
    local_types: &'a HashMap<String, InferredType>,
    registry: &'a Registry,
    builtin_fns: &'a HashMap<&'a str, crate::registry::builtins::BuiltinFunction>,
    src: &'a NamedSource<Arc<String>>,
}

impl InferCtx<'_> {
    /// Infer the type of a single argument expression.
    fn infer_arg(&self, arg: &Expr) -> Result<InferredType, GraphcalError> {
        infer_type(
            arg,
            self.declared_types,
            self.local_types,
            self.registry,
            self.builtin_fns,
            self.src,
        )
    }

    /// Aggregation dispatch: if the single argument is `Indexed`, aggregate;
    /// otherwise fall through to builtins (e.g. 2-arg `min`/`max`).
    fn infer_aggregation_fn_call(&self) -> Result<InferredType, GraphcalError> {
        let arg_type = self.infer_arg(&self.args[0])?;
        if let InferredType::Indexed { element, .. } = arg_type {
            return Ok(if self.name.value.as_str() == "count" {
                InferredType::Scalar(Dimension::dimensionless())
            } else {
                *element
            });
        }
        // Not indexed, fall through to builtin (min/max are 2-arg builtins)
        self.infer_builtin_fn_call()
    }

    /// Conversion dispatch: time-scale conversions (`to_utc`, ...) vs type
    /// conversions (`to_float`, `to_int`).
    ///
    /// `SpecialFnKind::Conversion` covers both; we split them here.
    fn infer_conversion_fn_call(&self) -> Result<InferredType, GraphcalError> {
        if let Some(target_scale) =
            crate::registry::time_scale::time_scale_from_conversion_fn(self.name.value.as_str())
        {
            return self.infer_timescale_fn_call(target_scale);
        }
        self.infer_type_conversion_fn_call()
    }

    /// Infer type conversion: `to_float(Int) -> Dimensionless`, `to_int(Dimensionless) -> Int`.
    fn infer_type_conversion_fn_call(&self) -> Result<InferredType, GraphcalError> {
        match self.name.value.as_str() {
            "to_float" => {
                if self.args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new("to_float"),
                        expected: 1,
                        got: self.args.len(),
                        src: self.src.clone(),
                        span: self.name.span.into(),
                    });
                }
                let arg_type = self.infer_arg(&self.args[0])?;
                if !arg_type.is_int_like() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format_inferred_type(&arg_type, self.registry),
                        src: self.src.clone(),
                        span: self.args[0].span.into(),
                        help: "to_float() requires an Int argument".to_string(),
                    });
                }
                Ok(InferredType::Scalar(Dimension::dimensionless()))
            }
            "to_int" => {
                if self.args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new("to_int"),
                        expected: 1,
                        got: self.args.len(),
                        src: self.src.clone(),
                        span: self.name.span.into(),
                    });
                }
                let arg_type = self.infer_arg(&self.args[0])?;
                let arg_dim = expect_scalar(&arg_type, self.registry, self.src, self.args[0].span)?;
                if !arg_dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: self.registry.dimensions.format_dimension(&arg_dim),
                        src: self.src.clone(),
                        span: self.args[0].span.into(),
                        help: "to_int() requires a Dimensionless argument".to_string(),
                    });
                }
                Ok(InferredType::Int)
            }
            _ => {
                // Should not reach here from classify_special_fn, but handle gracefully.
                self.infer_builtin_fn_call()
            }
        }
    }

    /// Infer datetime constructors: `datetime(string)` and `epoch(string, TimeScale)`.
    fn infer_datetime_constructor_call(&self) -> Result<InferredType, GraphcalError> {
        match self.name.value.as_str() {
            "datetime" => self.infer_datetime_call(),
            "epoch" => self.infer_epoch_call(),
            _ => {
                // Should not reach here from classify_special_fn, but handle gracefully.
                self.infer_builtin_fn_call()
            }
        }
    }

    /// `datetime(string_literal)` -> `Datetime(UTC)`
    /// `datetime(string_literal, string_literal)` -> `Datetime(UTC)` (with timezone)
    fn infer_datetime_call(&self) -> Result<InferredType, GraphcalError> {
        if self.args.is_empty() || self.args.len() > 2 {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "datetime() expects 1 or 2 arguments, got {}",
                    self.args.len()
                ),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }
        match &self.args[0].kind {
            ExprKind::StringLiteral(_) => {}
            _ => {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "string literal".to_string(),
                    found: format_inferred_type(&self.infer_arg(&self.args[0])?, self.registry),
                    src: self.src.clone(),
                    span: self.args[0].span.into(),
                    help: "datetime() requires a string literal argument".to_string(),
                });
            }
        }
        if self.args.len() == 2 {
            match &self.args[1].kind {
                ExprKind::StringLiteral(_) => {}
                _ => {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "string literal (IANA timezone)".to_string(),
                        found: format_inferred_type(
                            &self.infer_arg(&self.args[1])?,
                            self.registry,
                        ),
                        src: self.src.clone(),
                        span: self.args[1].span.into(),
                        help: "datetime() second argument must be a timezone string literal (e.g. \"Asia/Tokyo\")".to_string(),
                    });
                }
            }
        }
        Ok(InferredType::Datetime(
            crate::registry::time_scale::TimeScale::UTC,
        ))
    }

    /// `epoch(string_literal, TimeScale)` -> `Datetime(scale)`
    fn infer_epoch_call(&self) -> Result<InferredType, GraphcalError> {
        if self.args.len() != 2 {
            return Err(GraphcalError::WrongArity {
                name: FnName::new("epoch"),
                expected: 2,
                got: self.args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }
        // First arg must be a string literal
        if !matches!(&self.args[0].kind, ExprKind::StringLiteral(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "string literal".to_string(),
                found: format_inferred_type(&self.infer_arg(&self.args[0])?, self.registry),
                src: self.src.clone(),
                span: self.args[0].span.into(),
                help: "epoch() requires a string literal as its first argument".to_string(),
            });
        }
        // Second arg must be a time scale identifier
        let ExprKind::ConstRef(scale_ident) = &self.args[1].kind else {
            return Err(GraphcalError::DimensionMismatch {
                expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)".to_string(),
                found: format_inferred_type(&self.infer_arg(&self.args[1])?, self.registry),
                src: self.src.clone(),
                span: self.args[1].span.into(),
                help: "epoch() requires a time scale identifier as its second argument".to_string(),
            });
        };
        let scale: crate::registry::time_scale::TimeScale = scale_ident
            .value
            .as_str()
            .parse()
            .map_err(|_| GraphcalError::DimensionMismatch {
                expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)".to_string(),
                found: scale_ident.value.to_string(),
                src: self.src.clone(),
                span: self.args[1].span.into(),
                help: format!(
                    "unknown time scale `{}`; expected one of: {}",
                    scale_ident.value.as_str(),
                    crate::registry::time_scale::TimeScale::ALL_NAMES.join(", ")
                ),
            })?;
        Ok(InferredType::Datetime(scale))
    }

    /// Infer time scale conversion: `to_utc`, `to_tai`, etc.
    /// Expects exactly 1 Datetime argument, returns `Datetime(target_scale)`.
    fn infer_timescale_fn_call(
        &self,
        target_scale: crate::registry::time_scale::TimeScale,
    ) -> Result<InferredType, GraphcalError> {
        if self.args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: self.name.value.clone(),
                expected: 1,
                got: self.args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }
        let arg_type = self.infer_arg(&self.args[0])?;
        if !matches!(&arg_type, InferredType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_inferred_type(&arg_type, self.registry),
                src: self.src.clone(),
                span: self.args[0].span.into(),
                help: format!(
                    "{}() requires a Datetime argument",
                    self.name.value.as_str()
                ),
            });
        }
        Ok(InferredType::Datetime(target_scale))
    }

    /// Infer datetime extraction: `year`, `month`, `day`, etc. -> `Int`.
    fn infer_datetime_extract_fn_call(&self) -> Result<InferredType, GraphcalError> {
        if self.args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: self.name.value.clone(),
                expected: 1,
                got: self.args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }
        let arg_type = self.infer_arg(&self.args[0])?;
        if !matches!(&arg_type, InferredType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_inferred_type(&arg_type, self.registry),
                src: self.src.clone(),
                span: self.args[0].span.into(),
                help: format!(
                    "{}() requires a Datetime argument",
                    self.name.value.as_str()
                ),
            });
        }
        Ok(InferredType::Int)
    }

    /// Infer datetime from-numeric constructors: `from_jd`, `from_mjd`, `from_unix` -> `Datetime(UTC)`.
    fn infer_datetime_from_fn_call(&self) -> Result<InferredType, GraphcalError> {
        if self.args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: self.name.value.clone(),
                expected: 1,
                got: self.args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }
        let arg_type = self.infer_arg(&self.args[0])?;
        match &arg_type {
            InferredType::Scalar(dim) if dim.is_dimensionless() => {}
            t if t.is_int_like() => {}
            _ => {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless or Int".to_string(),
                    found: format_inferred_type(&arg_type, self.registry),
                    src: self.src.clone(),
                    span: self.args[0].span.into(),
                    help: format!(
                        "{}() requires a dimensionless numeric argument",
                        self.name.value.as_str()
                    ),
                });
            }
        }
        Ok(InferredType::Datetime(
            crate::registry::time_scale::TimeScale::UTC,
        ))
    }

    /// Infer datetime to-numeric functions: `to_jd`, `to_mjd`, `to_unix` -> `Dimensionless`.
    fn infer_datetime_to_fn_call(&self) -> Result<InferredType, GraphcalError> {
        if self.args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: self.name.value.clone(),
                expected: 1,
                got: self.args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }
        let arg_type = self.infer_arg(&self.args[0])?;
        if !matches!(&arg_type, InferredType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_inferred_type(&arg_type, self.registry),
                src: self.src.clone(),
                span: self.args[0].span.into(),
                help: format!(
                    "{}() requires a Datetime argument",
                    self.name.value.as_str()
                ),
            });
        }
        Ok(InferredType::Scalar(Dimension::dimensionless()))
    }

    /// Infer builtin math functions.
    fn infer_builtin_fn_call(&self) -> Result<InferredType, GraphcalError> {
        if let Some(func) = self.builtin_fns.get(self.name.value.as_str()) {
            let arg_dims: Vec<Dimension> = self
                .args
                .iter()
                .map(|a| {
                    let t = self.infer_arg(a)?;
                    expect_scalar(&t, self.registry, self.src, a.span)
                })
                .collect::<Result<_, _>>()?;
            return infer_fn_dim(&func.dim_sig, &arg_dims, self.args, self.registry, self.src)
                .map(InferredType::Scalar);
        }

        Err(GraphcalError::UnknownFunction {
            name: self.name.value.clone(),
            src: self.src.clone(),
            span: self.name.span.into(),
        })
    }
}

/// Infer the type of a function call (FnCall only; built-in functions).
pub(super) fn infer_fn_call(
    name: &crate::syntax::names::Spanned<FnName>,
    args: &[Expr],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let ctx = InferCtx {
        name,
        args,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    };
    match classify_special_fn(name.value.as_str()) {
        Some(SpecialFnKind::Aggregation) if args.len() == 1 => ctx.infer_aggregation_fn_call(),
        Some(SpecialFnKind::Conversion) => ctx.infer_conversion_fn_call(),
        Some(SpecialFnKind::Constructor) => ctx.infer_datetime_constructor_call(),
        Some(SpecialFnKind::DatetimeExtract) => ctx.infer_datetime_extract_fn_call(),
        Some(SpecialFnKind::DatetimeFrom) => ctx.infer_datetime_from_fn_call(),
        Some(SpecialFnKind::DatetimeTo) => ctx.infer_datetime_to_fn_call(),
        _ => ctx.infer_builtin_fn_call(),
    }
}
