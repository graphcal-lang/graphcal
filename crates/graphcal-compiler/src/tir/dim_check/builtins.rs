//! Interpreter for [`FunctionSignature`]s during dimension checking.
//!
//! One code path checks every signature-carrying function call — built-ins
//! and extern (plugin) functions alike: bind dimension variables at their
//! bare binding occurrences, check compound monomials by evaluation, and
//! compute the result monomial from the bindings.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::function_signature::{DimMonomial, DimMonomialEvalError, FunctionSignature, ValueKind};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::DimVarName;
use crate::syntax::function_name::FnName;
use crate::syntax::span::Span;

/// Check scalar argument dimensions against `sig` and compute the result
/// dimension.
///
/// Arguments are scalar dimensions; callers verify non-scalar parameter kinds
/// ([`ValueKind::Bool`]/[`ValueKind::Int`]) before reaching this walk. All
/// built-in registry signatures are all-scalar, so built-in inference calls
/// this directly.
pub(super) fn infer_fn_dim_from_spans(
    fn_name: &str,
    sig: &FunctionSignature,
    arg_dims: &[Dimension],
    arg_spans: &[Span],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    if arg_dims.len() != sig.arity() {
        return Err(GraphcalError::WrongArity {
            name: FnName::expect_valid(fn_name),
            expected: sig.arity(),
            got: arg_dims.len(),
            src: src.clone(),
            span: arg_spans
                .get(sig.arity())
                .or_else(|| arg_spans.last())
                .copied()
                .unwrap_or_else(|| Span::new(0, 0))
                .into(),
        });
    }

    let mut bindings: HashMap<DimVarName, Dimension> = HashMap::new();

    for (i, param) in sig.params().iter().enumerate() {
        let Some(arg_dim) = arg_dims.get(i) else {
            return Err(GraphcalError::InternalError {
                message: format!("dimension signature for `{fn_name}` saw missing argument {i}"),
                src: src.clone(),
                span: arg_spans
                    .first()
                    .copied()
                    .unwrap_or_else(|| Span::new(0, 0))
                    .into(),
            });
        };
        let arg_span = arg_spans.get(i).copied().unwrap_or_else(|| {
            arg_spans
                .first()
                .copied()
                .unwrap_or_else(|| Span::new(0, 0))
        });
        let ValueKind::Scalar(monomial) = &param.kind else {
            return Err(GraphcalError::InternalError {
                message: format!(
                    "signature for `{fn_name}` has a non-scalar parameter `{}` in the scalar checking path",
                    param.name
                ),
                src: src.clone(),
                span: arg_span.into(),
            });
        };
        check_scalar_param(
            fn_name,
            sig,
            &param.name,
            monomial,
            arg_dim,
            &mut bindings,
            registry,
            src,
            arg_span,
        )?;
    }

    let result_span = arg_spans
        .first()
        .copied()
        .unwrap_or_else(|| Span::new(0, 0));
    let ValueKind::Scalar(result) = sig.result() else {
        return Err(GraphcalError::InternalError {
            message: format!(
                "signature for `{fn_name}` has a non-scalar result in the scalar checking path"
            ),
            src: src.clone(),
            span: result_span.into(),
        });
    };
    eval_result_monomial(fn_name, result, &bindings, src, result_span)
}

/// Check one scalar argument against its parameter monomial, binding or
/// comparing dimension variables as required. Shared by built-in and extern
/// call checking.
#[expect(clippy::too_many_arguments, reason = "signature-walk context")]
pub(super) fn check_scalar_param(
    fn_name: &str,
    sig: &FunctionSignature,
    param_name: &crate::syntax::function_name::FnParamName,
    monomial: &DimMonomial,
    arg_dim: &Dimension,
    bindings: &mut HashMap<DimVarName, Dimension>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    arg_span: Span,
) -> Result<(), GraphcalError> {
    if let Some(var) = monomial.as_bare_var() {
        if let Some(bound) = bindings.get(var) {
            if arg_dim != bound {
                let bind_param_name = first_binding_param(sig, var).map_or("?", |name| name);
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(bound),
                    found: registry.dimensions.format_dimension(arg_dim),
                    help: format!(
                        "parameter `{param_name}` must have the same dimension as `{bind_param_name}`",
                    ),
                    src: src.clone(),
                    span: arg_span.into(),
                });
            }
        } else {
            bindings.insert(var.clone(), arg_dim.clone());
        }
        return Ok(());
    }

    let expected = eval_monomial(fn_name, monomial, bindings, src, arg_span)?;
    if *arg_dim != expected {
        return Err(GraphcalError::DimensionMismatch {
            expected: registry.dimensions.format_dimension(&expected),
            found: registry.dimensions.format_dimension(arg_dim),
            help: format!(
                "parameter `{param_name}` requires {}",
                registry.dimensions.format_dimension(&expected),
            ),
            src: src.clone(),
            span: arg_span.into(),
        });
    }
    Ok(())
}

/// Compute the result dimension of a signature from the bound variables.
/// Shared by built-in and extern call checking.
pub(super) fn eval_result_monomial(
    fn_name: &str,
    result: &DimMonomial,
    bindings: &HashMap<DimVarName, Dimension>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    eval_monomial(fn_name, result, bindings, src, span)
}

fn eval_monomial(
    fn_name: &str,
    monomial: &DimMonomial,
    bindings: &HashMap<DimVarName, Dimension>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    monomial
        .eval(|var| bindings.get(var))
        .map_err(|err| match err {
            // A missing binding here means the signature is malformed (a compound
            // use or result without a matching bare binding occurrence) — the
            // signature validator rejects that shape, so surface an internal
            // error rather than panicking.
            DimMonomialEvalError::UnboundVar { var } => GraphcalError::InternalError {
                message: format!(
                    "builtin `{fn_name}` references unbound dim variable `{var}` in its signature"
                ),
                src: src.clone(),
                span: span.into(),
            },
            DimMonomialEvalError::Overflow(_) => GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: span.into(),
            },
        })
}

/// Find the display name of the first parameter that binds `var` as a bare
/// variable, for "must have the same dimension as `x`" diagnostics.
fn first_binding_param<'a>(sig: &'a FunctionSignature, var: &DimVarName) -> Option<&'a str> {
    sig.params().iter().find_map(|p| match &p.kind {
        ValueKind::Scalar(monomial) if monomial.as_bare_var() == Some(var) => Some(p.name.as_str()),
        _ => None,
    })
}
