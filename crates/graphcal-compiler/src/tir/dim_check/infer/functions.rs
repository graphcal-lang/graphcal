//! Type inference for function calls: FnCall and QualifiedFnCall.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::{Expr, ExprKind, GenericArg};
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{FnName, GenericParamName, IndexName};

use crate::registry::error::GraphcalError;
use crate::registry::registry::Registry;
use crate::registry::resolve_types::{SpecialFnKind, classify_special_fn};
use crate::tir::tir::ResolvedFnSig;

use super::super::builtins::infer_fn_dim;
use super::super::helpers::{declared_to_inferred, expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Bundles the inference context that is threaded through every dispatch call.
///
/// This avoids repeating 8 arguments in every helper invocation inside `infer_fn_call`.
struct InferCtx<'a> {
    name: &'a crate::syntax::names::Spanned<FnName>,
    type_args: &'a [GenericArg],
    args: &'a [Expr],
    declared_types: &'a HashMap<String, DeclaredType>,
    local_types: &'a HashMap<String, InferredType>,
    registry: &'a Registry,
    builtin_fns: &'a HashMap<&'a str, crate::registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &'a HashMap<FnName, ResolvedFnSig>,
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
            self.resolved_fn_sigs,
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
        self.infer_builtin_or_user_fn_call()
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
                self.infer_builtin_or_user_fn_call()
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
                self.infer_builtin_or_user_fn_call()
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

    /// Infer builtin math functions or user-defined functions.
    fn infer_builtin_or_user_fn_call(&self) -> Result<InferredType, GraphcalError> {
        // Try builtin first
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

        // Try user-defined function via resolved signatures
        self.infer_user_fn_call()
    }

    /// Infer a user-defined function call with generic parameter inference.
    fn infer_user_fn_call(&self) -> Result<InferredType, GraphcalError> {
        let fn_name_key = FnName::new(self.name.value.as_str());
        let sig = self.resolved_fn_sigs.get(&fn_name_key).ok_or_else(|| {
            GraphcalError::UnknownFunction {
                name: self.name.value.clone(),
                src: self.src.clone(),
                span: self.name.span.into(),
            }
        })?;

        // Arity check
        if self.args.len() != sig.params.len() {
            return Err(GraphcalError::WrongArity {
                name: self.name.value.clone(),
                expected: sig.params.len(),
                got: self.args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }

        // Validate turbofish generic arg count (must match total generic param count if provided)
        if !self.type_args.is_empty() && self.type_args.len() != sig.generic_params_ordered.len() {
            return Err(GraphcalError::WrongGenericArity {
                name: self.name.value.clone(),
                expected: sig.generic_params_ordered.len(),
                got: self.type_args.len(),
                src: self.src.clone(),
                span: self.name.span.into(),
            });
        }

        // Infer arg types
        let arg_types: Vec<InferredType> = self
            .args
            .iter()
            .map(|a| self.infer_arg(a))
            .collect::<Result<_, _>>()?;

        if sig.generic_dim_params.is_empty()
            && sig.generic_index_params.is_empty()
            && sig.generic_nat_params.is_empty()
        {
            // Non-generic: check each param type using resolved signature
            for (i, param) in sig.params.iter().enumerate() {
                let expected =
                    crate::tir::tir::resolved_to_declared_type(&param.resolved_type, self.src)?;
                let expected_inferred = declared_to_inferred(&expected);
                if arg_types[i] != expected_inferred {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(&expected_inferred, self.registry),
                        found: format_inferred_type(&arg_types[i], self.registry),
                        src: self.src.clone(),
                        span: self.args[i].span.into(),
                        help: format!("parameter `{}` expects {expected_inferred:?}", param.name),
                    });
                }
            }
            // Resolve return type
            let ret = crate::tir::tir::resolved_to_declared_type(&sig.return_type, self.src)?;
            Ok(declared_to_inferred(&ret))
        } else {
            // Generic: build substitution maps
            let mut dim_sub: HashMap<GenericParamName, Dimension> = HashMap::new();
            let mut index_sub: HashMap<GenericParamName, IndexName> = HashMap::new();
            let mut nat_sub: HashMap<GenericParamName, u64> = HashMap::new();

            // Pre-populate from turbofish args (if provided)
            if !self.type_args.is_empty() {
                self.populate_subs_from_turbofish(sig, &mut dim_sub, &mut index_sub, &mut nat_sub)?;
            }

            // Unify generic params from argument types
            for (i, param) in sig.params.iter().enumerate() {
                crate::tir::tir::unify_resolved_type(
                    &param.resolved_type,
                    &arg_types[i],
                    &mut dim_sub,
                    &mut index_sub,
                    &mut nat_sub,
                    self.registry,
                    self.src,
                    self.args[i].span,
                )?;
            }
            // Resolve return type with substitution
            let ret_type = crate::tir::tir::substitute_resolved_type(
                &sig.return_type,
                &dim_sub,
                &index_sub,
                &nat_sub,
                self.src,
            )?;
            Ok(ret_type)
        }
    }

    /// Populate substitution maps from explicit turbofish generic arguments.
    fn populate_subs_from_turbofish(
        &self,
        sig: &ResolvedFnSig,
        dim_sub: &mut HashMap<GenericParamName, Dimension>,
        index_sub: &mut HashMap<GenericParamName, IndexName>,
        nat_sub: &mut HashMap<GenericParamName, u64>,
    ) -> Result<(), GraphcalError> {
        use crate::registry::registry::FnGenericConstraint;
        use crate::syntax::ast::TypeExprKind;

        for (i, (arg, param)) in self
            .type_args
            .iter()
            .zip(sig.generic_params_ordered.iter())
            .enumerate()
        {
            match (&param.constraint, arg) {
                (FnGenericConstraint::Nat, GenericArg::Nat(nat_expr)) => {
                    // Evaluate the nat expression to a concrete u64
                    let value = eval_nat_expr_to_u64(nat_expr)?;
                    nat_sub.insert(param.name.clone(), value);
                }
                (FnGenericConstraint::Dim, GenericArg::Type(type_expr)) => {
                    // Resolve the type expression to a dimension
                    let dim = self
                        .registry
                        .dimensions
                        .resolve_type_expr(type_expr)
                        .ok_or_else(|| GraphcalError::GenericArgMismatch {
                            name: self.name.value.clone(),
                            param: param.name.to_string(),
                            expected: "Dim".to_string(),
                            found: format!("{type_expr:?}"),
                            src: self.src.clone(),
                            span: self.type_args[i].span().into(),
                        })?;
                    dim_sub.insert(param.name.clone(), dim);
                }
                (FnGenericConstraint::Index, GenericArg::Type(type_expr)) => {
                    // Extract index name from type expression.
                    // A bare name like `Maneuver` is parsed as DimExpr with a single term.
                    if let TypeExprKind::DimExpr(dim_expr) = &type_expr.kind {
                        if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() {
                            index_sub.insert(
                                param.name.clone(),
                                IndexName::new(&dim_expr.terms[0].term.name.name),
                            );
                        } else {
                            return Err(GraphcalError::GenericArgMismatch {
                                name: self.name.value.clone(),
                                param: param.name.to_string(),
                                expected: "Index (a single name)".to_string(),
                                found: format!("{type_expr:?}"),
                                src: self.src.clone(),
                                span: self.type_args[i].span().into(),
                            });
                        }
                    } else {
                        return Err(GraphcalError::GenericArgMismatch {
                            name: self.name.value.clone(),
                            param: param.name.to_string(),
                            expected: "Index".to_string(),
                            found: format!("{type_expr:?}"),
                            src: self.src.clone(),
                            span: self.type_args[i].span().into(),
                        });
                    }
                }
                (FnGenericConstraint::Nat, GenericArg::Type(_)) => {
                    return Err(GraphcalError::GenericArgMismatch {
                        name: self.name.value.clone(),
                        param: param.name.to_string(),
                        expected: "Nat (integer)".to_string(),
                        found: "type expression".to_string(),
                        src: self.src.clone(),
                        span: self.type_args[i].span().into(),
                    });
                }
                (constraint, GenericArg::Nat(_)) => {
                    return Err(GraphcalError::GenericArgMismatch {
                        name: self.name.value.clone(),
                        param: param.name.to_string(),
                        expected: format!("{constraint:?}"),
                        found: "Nat (integer)".to_string(),
                        src: self.src.clone(),
                        span: self.type_args[i].span().into(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Evaluate a `NatExpr` to a concrete `u64` value.
///
/// Only supports literals for now (turbofish context).
fn eval_nat_expr_to_u64(nat_expr: &crate::syntax::ast::NatExpr) -> Result<u64, GraphcalError> {
    use crate::syntax::ast::NatExpr;
    match nat_expr {
        NatExpr::Literal(v, _) => Ok(*v),
        NatExpr::Var(ident) => Err(GraphcalError::GenericArgMismatch {
            name: FnName::new(""),
            param: String::new(),
            expected: "a concrete integer".to_string(),
            found: format!("variable `{}`", ident.name),
            src: NamedSource::new("", Arc::new(String::new())),
            span: ident.span.into(),
        }),
        NatExpr::Add(lhs, rhs, _) => {
            let l = eval_nat_expr_to_u64(lhs)?;
            let r = eval_nat_expr_to_u64(rhs)?;
            Ok(l + r)
        }
    }
}

/// Infer the type of a function call (FnCall or QualifiedFnCall).
pub(super) fn infer_fn_call(
    name: &crate::syntax::names::Spanned<FnName>,
    type_args: &[GenericArg],
    args: &[Expr],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let ctx = InferCtx {
        name,
        type_args,
        args,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        resolved_fn_sigs,
        src,
    };
    match classify_special_fn(name.value.as_str()) {
        Some(SpecialFnKind::Aggregation) if args.len() == 1 => ctx.infer_aggregation_fn_call(),
        Some(SpecialFnKind::Conversion) => ctx.infer_conversion_fn_call(),
        Some(SpecialFnKind::Constructor) => ctx.infer_datetime_constructor_call(),
        Some(SpecialFnKind::DatetimeExtract) => ctx.infer_datetime_extract_fn_call(),
        Some(SpecialFnKind::DatetimeFrom) => ctx.infer_datetime_from_fn_call(),
        Some(SpecialFnKind::DatetimeTo) => ctx.infer_datetime_to_fn_call(),
        _ => ctx.infer_builtin_or_user_fn_call(),
    }
}
