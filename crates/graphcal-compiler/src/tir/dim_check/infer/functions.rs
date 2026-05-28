//! Type inference for function calls (built-in functions only).

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{Expr, ExprKind};
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{FnName, ScopedName};
use crate::syntax::span::Spanned;

use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{
    AggregationFn, ConstructorFn, SpecialFnKind, TypeConversionFn, classify_special_fn,
};
use crate::registry::types::Registry;

use super::super::builtins::infer_fn_dim;
use super::super::helpers::{expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Bundles the inference context that is threaded through every dispatch call.
///
/// This avoids repeating 7 arguments in every helper invocation inside `infer_fn_call`.
struct InferCtx<'a> {
    name: &'a crate::syntax::span::Spanned<FnName>,
    args: &'a [Expr],
    declared_types: &'a HashMap<ScopedName, DeclaredType>,
    local_types: &'a HashMap<String, InferredType>,
    tir: &'a crate::tir::typed::TIR,
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
            self.tir,
            self.registry,
            self.builtin_fns,
            self.src,
        )
    }

    /// Aggregation dispatch: if the single argument is `Indexed`, aggregate;
    /// otherwise fall through to builtins (e.g. 2-arg `min`/`max`).
    fn infer_aggregation_fn_call(
        &self,
        kind: AggregationFn,
    ) -> Result<InferredType, GraphcalError> {
        let arg_type = self.infer_arg(&self.args[0])?;
        if let InferredType::Indexed { element, .. } = arg_type {
            return Ok(match kind {
                AggregationFn::Count => InferredType::Scalar(Dimension::dimensionless()),
                AggregationFn::Sum
                | AggregationFn::Min
                | AggregationFn::Max
                | AggregationFn::Mean => *element,
            });
        }
        // Not indexed, fall through to builtin (min/max are 2-arg builtins)
        self.infer_builtin_fn_call()
    }

    /// Infer type conversion: `to_float(Int) -> Dimensionless`, `to_int(Dimensionless) -> Int`.
    fn infer_type_conversion_fn_call_typed(
        &self,
        kind: TypeConversionFn,
    ) -> Result<InferredType, GraphcalError> {
        match kind {
            TypeConversionFn::ToFloat => {
                if self.args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new(TypeConversionFn::ToFloat.as_str()),
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
                        help: "to_float() requires an Int argument".to_string(),
                        src: self.src.clone(),
                        span: self.args[0].span.into(),
                    });
                }
                Ok(InferredType::Scalar(Dimension::dimensionless()))
            }
            TypeConversionFn::ToInt => {
                if self.args.len() != 1 {
                    return Err(GraphcalError::WrongArity {
                        name: FnName::new(TypeConversionFn::ToInt.as_str()),
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
                        help: "to_int() requires a Dimensionless argument".to_string(),
                        src: self.src.clone(),
                        span: self.args[0].span.into(),
                    });
                }
                Ok(InferredType::Int)
            }
        }
    }

    /// Infer datetime constructors: `datetime(string)` and `epoch(string, TimeScale)`.
    fn infer_datetime_constructor_call_typed(
        &self,
        kind: ConstructorFn,
    ) -> Result<InferredType, GraphcalError> {
        match kind {
            ConstructorFn::Datetime => self.infer_datetime_call(),
            ConstructorFn::Epoch => self.infer_epoch_call(),
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
                    help: "datetime() requires a string literal argument".to_string(),
                    src: self.src.clone(),
                    span: self.args[0].span.into(),
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
                                   help: "datetime() second argument must be a timezone string literal (e.g. \"Asia/Tokyo\")".to_string(),
                                   src: self.src.clone(),
                                   span: self.args[1].span.into(),
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
                help: "epoch() requires a string literal as its first argument".to_string(),
                src: self.src.clone(),
                span: self.args[0].span.into(),
            });
        }
        // Second arg must be a time scale identifier
        let ExprKind::ConstRef(scale_ident) = &self.args[1].kind else {
            return Err(GraphcalError::DimensionMismatch {
                expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)".to_string(),
                found: format_inferred_type(&self.infer_arg(&self.args[1])?, self.registry),
                help: "epoch() requires a time scale identifier as its second argument".to_string(),
                src: self.src.clone(),
                span: self.args[1].span.into(),
            });
        };
        // Time scale identifiers are always bare; a qualified `mod.UTC`
        // is necessarily not a valid time scale and is rejected here.
        let scale: crate::registry::time_scale::TimeScale = scale_ident
            .value
            .member()
            .parse()
            .map_err(|_| GraphcalError::DimensionMismatch {
                expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)".to_string(),
                found: scale_ident.value.to_string(),
                help: format!(
                    "unknown time scale `{}`; expected one of: {}",
                    scale_ident.value,
                    crate::registry::time_scale::TimeScale::ALL_NAMES.join(", ")
                ),
                src: self.src.clone(),
                span: self.args[1].span.into(),
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
                help: format!(
                    "{}() requires a Datetime argument",
                    self.name.value.as_str()
                ),
                src: self.src.clone(),
                span: self.args[0].span.into(),
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
                help: format!(
                    "{}() requires a Datetime argument",
                    self.name.value.as_str()
                ),
                src: self.src.clone(),
                span: self.args[0].span.into(),
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
                    help: format!(
                        "{}() requires a dimensionless numeric argument",
                        self.name.value.as_str()
                    ),
                    src: self.src.clone(),
                    span: self.args[0].span.into(),
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
                help: format!(
                    "{}() requires a Datetime argument",
                    self.name.value.as_str()
                ),
                src: self.src.clone(),
                span: self.args[0].span.into(),
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
            return infer_fn_dim(
                self.name.value.as_str(),
                &func.dim_sig,
                &arg_dims,
                self.args,
                self.registry,
                self.src,
            )
            .map(InferredType::Scalar);
        }

        Err(GraphcalError::UnknownFunction {
            name: self.name.value.to_string(),
            src: self.src.clone(),
            span: self.name.span.into(),
        })
    }
}

/// Infer the type of a function call (FnCall only; built-in functions).
pub(super) fn infer_fn_call(
    callee: &crate::syntax::ast::IdentPath,
    args: &[Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(segment) = callee.as_bare() else {
        return Err(GraphcalError::UnknownFunction {
            name: callee.display_path(),
            src: src.clone(),
            span: callee.span().into(),
        });
    };
    let name = Spanned::new(FnName::new(&segment.name), segment.span);
    let ctx = InferCtx {
        name: &name,
        args,
        declared_types,
        local_types,
        tir,
        registry,
        builtin_fns,
        src,
    };
    match classify_special_fn(name.value.as_str()) {
        Some(SpecialFnKind::Aggregation(kind)) if args.len() == 1 => {
            ctx.infer_aggregation_fn_call(kind)
        }
        Some(SpecialFnKind::TypeConversion(kind)) => ctx.infer_type_conversion_fn_call_typed(kind),
        Some(SpecialFnKind::TimeScaleConversion(target_scale)) => {
            ctx.infer_timescale_fn_call(target_scale)
        }
        Some(SpecialFnKind::Constructor(kind)) => ctx.infer_datetime_constructor_call_typed(kind),
        Some(SpecialFnKind::DatetimeExtract(_)) => ctx.infer_datetime_extract_fn_call(),
        Some(SpecialFnKind::DatetimeFrom(_)) => ctx.infer_datetime_from_fn_call(),
        Some(SpecialFnKind::DatetimeTo(_)) => ctx.infer_datetime_to_fn_call(),
        _ => ctx.infer_builtin_fn_call(),
    }
}
