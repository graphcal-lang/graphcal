//! Type inference for function calls: FnCall and QualifiedFnCall.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{Expr, ExprKind};
use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{FnName, GenericParamName};

use crate::tir::ResolvedFnSig;
use graphcal_ir::resolve::is_aggregation_fn;
use graphcal_registry::error::GraphcalError;
use graphcal_registry::registry::Registry;

use super::super::builtins::infer_fn_dim;
use super::super::helpers::{declared_to_inferred, expect_scalar, format_inferred_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Infer the type of a function call (FnCall or QualifiedFnCall).
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive handling of all function call kinds"
)]
pub(super) fn infer_fn_call(
    name: &graphcal_syntax::names::Spanned<FnName>,
    args: &[Expr],
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, graphcal_registry::builtins::BuiltinFunction>,
    resolved_fn_sigs: &HashMap<FnName, ResolvedFnSig>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Aggregation functions over indexed values: sum, min, max, mean, count
    if is_aggregation_fn(name.value.as_str()) && args.len() == 1 {
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        if let InferredType::Indexed { element, .. } = arg_type {
            return Ok(if name.value.as_str() == "count" {
                InferredType::Scalar(Dimension::dimensionless())
            } else {
                *element
            });
        }
        // If not indexed, fall through to builtins (min/max are 2-arg builtins too)
    }

    // Conversion builtins: to_float(Int) -> Dimensionless, to_int(Dimensionless) -> Int
    if name.value.as_str() == "to_float" {
        if args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: FnName::new("to_float"),
                expected: 1,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        if arg_type != InferredType::Int {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Int".to_string(),
                found: format_inferred_type(&arg_type, registry),
                src: src.clone(),
                span: args[0].span.into(),
                help: "to_float() requires an Int argument".to_string(),
            });
        }
        return Ok(InferredType::Scalar(Dimension::dimensionless()));
    }
    if name.value.as_str() == "to_int" {
        if args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: FnName::new("to_int"),
                expected: 1,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        let arg_dim = expect_scalar(&arg_type, registry, src, args[0].span)?;
        if !arg_dim.is_dimensionless() {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Dimensionless".to_string(),
                found: registry.dimensions.format_dimension(&arg_dim),
                src: src.clone(),
                span: args[0].span.into(),
                help: "to_int() requires a Dimensionless argument".to_string(),
            });
        }
        return Ok(InferredType::Int);
    }

    // datetime(string_literal) -> Datetime(UTC)
    // datetime(string_literal, string_literal) -> Datetime(UTC)  (with timezone)
    if name.value.as_str() == "datetime" {
        if args.is_empty() || args.len() > 2 {
            return Err(GraphcalError::EvalError {
                message: format!("datetime() expects 1 or 2 arguments, got {}", args.len()),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        match &args[0].kind {
            ExprKind::StringLiteral(_) => {}
            _ => {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "string literal".to_string(),
                    found: format_inferred_type(
                        &infer_type(
                            &args[0],
                            declared_types,
                            local_types,
                            registry,
                            builtin_fns,
                            resolved_fn_sigs,
                            src,
                        )?,
                        registry,
                    ),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "datetime() requires a string literal argument".to_string(),
                });
            }
        }
        if args.len() == 2 {
            match &args[1].kind {
                ExprKind::StringLiteral(_) => {}
                _ => {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "string literal (IANA timezone)".to_string(),
                        found: format_inferred_type(
                            &infer_type(
                                &args[1],
                                declared_types,
                                local_types,
                                registry,
                                builtin_fns,
                                resolved_fn_sigs,
                                src,
                            )?,
                            registry,
                        ),
                        src: src.clone(),
                        span: args[1].span.into(),
                        help: "datetime() second argument must be a timezone string literal (e.g. \"Asia/Tokyo\")".to_string(),
                    });
                }
            }
        }
        return Ok(InferredType::Datetime(
            graphcal_registry::time_scale::TimeScale::UTC,
        ));
    }

    // epoch(string_literal, TimeScale) -> Datetime(scale)
    if name.value.as_str() == "epoch" {
        if args.len() != 2 {
            return Err(GraphcalError::WrongArity {
                name: FnName::new("epoch"),
                expected: 2,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        // First arg must be a string literal
        if !matches!(&args[0].kind, ExprKind::StringLiteral(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "string literal".to_string(),
                found: format_inferred_type(
                    &infer_type(
                        &args[0],
                        declared_types,
                        local_types,
                        registry,
                        builtin_fns,
                        resolved_fn_sigs,
                        src,
                    )?,
                    registry,
                ),
                src: src.clone(),
                span: args[0].span.into(),
                help: "epoch() requires a string literal as its first argument".to_string(),
            });
        }
        // Second arg must be a time scale identifier
        let ExprKind::ConstRef(scale_ident) = &args[1].kind else {
            return Err(GraphcalError::DimensionMismatch {
                expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)".to_string(),
                found: format_inferred_type(
                    &infer_type(
                        &args[1],
                        declared_types,
                        local_types,
                        registry,
                        builtin_fns,
                        resolved_fn_sigs,
                        src,
                    )?,
                    registry,
                ),
                src: src.clone(),
                span: args[1].span.into(),
                help: "epoch() requires a time scale identifier as its second argument".to_string(),
            });
        };
        let scale: graphcal_registry::time_scale::TimeScale = scale_ident
            .value
            .as_str()
            .parse()
            .map_err(|_| GraphcalError::DimensionMismatch {
                expected: "time scale (UTC, TAI, TT, TDB, ET, GPST, GST, BDT, QZSST)".to_string(),
                found: scale_ident.value.to_string(),
                src: src.clone(),
                span: args[1].span.into(),
                help: format!(
                    "unknown time scale `{}`; expected one of: {}",
                    scale_ident.value.as_str(),
                    graphcal_registry::time_scale::TimeScale::ALL_NAMES.join(", ")
                ),
            })?;
        return Ok(InferredType::Datetime(scale));
    }

    // Time scale conversion: to_utc, to_tai, to_tt, to_tdb, to_et, to_gpst, to_gst, to_bdt, to_qzsst
    if let Some(target_scale) =
        graphcal_registry::time_scale::time_scale_from_conversion_fn(name.value.as_str())
    {
        if args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: name.value.clone(),
                expected: 1,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        if !matches!(&arg_type, InferredType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_inferred_type(&arg_type, registry),
                src: src.clone(),
                span: args[0].span.into(),
                help: format!("{}() requires a Datetime argument", name.value.as_str()),
            });
        }
        return Ok(InferredType::Datetime(target_scale));
    }

    // Datetime extraction functions: year, month, day, etc. -> Int
    if graphcal_ir::resolve::DATETIME_EXTRACT_FNS.contains(&name.value.as_str()) {
        if args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: name.value.clone(),
                expected: 1,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        if !matches!(&arg_type, InferredType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_inferred_type(&arg_type, registry),
                src: src.clone(),
                span: args[0].span.into(),
                help: format!("{}() requires a Datetime argument", name.value.as_str()),
            });
        }
        return Ok(InferredType::Int);
    }

    // Datetime from-numeric constructors: from_jd, from_mjd, from_unix -> Datetime(UTC)
    if graphcal_ir::resolve::DATETIME_FROM_FNS.contains(&name.value.as_str()) {
        if args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: name.value.clone(),
                expected: 1,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        match &arg_type {
            InferredType::Scalar(dim) if dim.is_dimensionless() => {}
            InferredType::Int => {}
            _ => {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless or Int".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: format!(
                        "{}() requires a dimensionless numeric argument",
                        name.value.as_str()
                    ),
                });
            }
        }
        return Ok(InferredType::Datetime(
            graphcal_registry::time_scale::TimeScale::UTC,
        ));
    }

    // Datetime to-numeric functions: to_jd, to_mjd, to_unix -> Dimensionless
    if graphcal_ir::resolve::DATETIME_TO_FNS.contains(&name.value.as_str()) {
        if args.len() != 1 {
            return Err(GraphcalError::WrongArity {
                name: name.value.clone(),
                expected: 1,
                got: args.len(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        let arg_type = infer_type(
            &args[0],
            declared_types,
            local_types,
            registry,
            builtin_fns,
            resolved_fn_sigs,
            src,
        )?;
        if !matches!(&arg_type, InferredType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_inferred_type(&arg_type, registry),
                src: src.clone(),
                span: args[0].span.into(),
                help: format!("{}() requires a Datetime argument", name.value.as_str()),
            });
        }
        return Ok(InferredType::Scalar(Dimension::dimensionless()));
    }

    // Try builtin first
    if let Some(func) = builtin_fns.get(name.value.as_str()) {
        let arg_dims: Vec<Dimension> = args
            .iter()
            .map(|a| {
                let t = infer_type(
                    a,
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    resolved_fn_sigs,
                    src,
                )?;
                expect_scalar(&t, registry, src, a.span)
            })
            .collect::<Result<_, _>>()?;
        return infer_fn_dim(&func.dim_sig, &arg_dims, args, registry, src)
            .map(InferredType::Scalar);
    }

    // Try user-defined function via resolved signatures
    let fn_name_key = FnName::new(name.value.as_str());
    let sig = resolved_fn_sigs
        .get(&fn_name_key)
        .ok_or_else(|| GraphcalError::UnknownFunction {
            name: name.value.clone(),
            src: src.clone(),
            span: name.span.into(),
        })?;

    // Arity check
    if args.len() != sig.params.len() {
        return Err(GraphcalError::WrongArity {
            name: name.value.clone(),
            expected: sig.params.len(),
            got: args.len(),
            src: src.clone(),
            span: name.span.into(),
        });
    }

    // Infer arg types
    let arg_types: Vec<InferredType> = args
        .iter()
        .map(|a| {
            infer_type(
                a,
                declared_types,
                local_types,
                registry,
                builtin_fns,
                resolved_fn_sigs,
                src,
            )
        })
        .collect::<Result<_, _>>()?;

    if sig.generic_dim_params.is_empty() && sig.generic_index_params.is_empty() {
        // Non-generic: check each param type using resolved signature
        for (i, param) in sig.params.iter().enumerate() {
            let expected = crate::tir::resolved_to_declared_type(&param.resolved_type, src)?;
            let expected_inferred = declared_to_inferred(&expected);
            if arg_types[i] != expected_inferred {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(&expected_inferred, registry),
                    found: format_inferred_type(&arg_types[i], registry),
                    src: src.clone(),
                    span: args[i].span.into(),
                    help: format!("parameter `{}` expects {expected_inferred:?}", param.name),
                });
            }
        }
        // Resolve return type
        let ret = crate::tir::resolved_to_declared_type(&sig.return_type, src)?;
        Ok(declared_to_inferred(&ret))
    } else {
        // Generic: unify generic params from arg types
        let mut dim_sub: HashMap<GenericParamName, Dimension> = HashMap::new();
        let mut index_sub: HashMap<GenericParamName, graphcal_syntax::names::IndexName> =
            HashMap::new();
        for (i, param) in sig.params.iter().enumerate() {
            crate::tir::unify_resolved_type(
                &param.resolved_type,
                &arg_types[i],
                &mut dim_sub,
                &mut index_sub,
                registry,
                src,
                args[i].span,
            )?;
        }
        // Resolve return type with substitution
        let ret_type =
            crate::tir::substitute_resolved_type(&sig.return_type, &dim_sub, &index_sub, src)?;
        Ok(ret_type)
    }
}
