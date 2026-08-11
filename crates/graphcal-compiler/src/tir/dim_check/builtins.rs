//! Interpreter for [`FunctionSignature`]s during dimension checking.
//!
//! One code path checks every signature-carrying function call — built-ins
//! and extern (plugin) functions alike: bind dimension variables at their
//! bare binding occurrences, check compound monomials by evaluation, and
//! compute the result monomial from the bindings.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::function_signature::{DimMonomial, DimMonomialEvalError, FunctionSignature, ValueKind};
use crate::registry::error::GraphcalError;
use crate::registry::types::SemanticRegistry;
use crate::syntax::dimension::DimVarName;
use crate::syntax::function_name::FnName;
use crate::syntax::span::{Span, Spanned};

/// Check quantity argument dimensions against `sig` and compute the result
/// dimension.
///
/// Arguments are quantity dimensions; callers verify non-quantity parameter kinds
/// ([`ValueKind::Bool`]/[`ValueKind::Int`]) before reaching this walk. All
/// built-in registry signatures are all-quantity, so built-in inference calls
/// this directly.
pub(super) fn infer_fn_dim(
    fn_name: &str,
    sig: &FunctionSignature,
    args: &[Spanned<Dimension>],
    call_span: Span,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    if args.len() != sig.arity() {
        let error_span = args
            .get(sig.arity())
            .or_else(|| args.last())
            .map_or(call_span, |arg| arg.span);
        return Err(GraphcalError::WrongArity {
            name: FnName::expect_valid(fn_name),
            expected: sig.arity(),
            got: args.len(),
            src: src.clone(),
            span: error_span.into(),
        });
    }

    let mut bindings: HashMap<DimVarName, Dimension> = HashMap::new();

    for (param, arg) in sig.params().iter().zip(args) {
        let ValueKind::Quantity(monomial) = &param.kind else {
            return Err(GraphcalError::internal_error(
                format!(
                    "signature for `{fn_name}` has a non-quantity parameter `{}` in the quantity checking path",
                    param.name
                ),
                src,
                DiagnosticAnchor::Source(arg.span),
            ));
        };
        check_quantity_param(
            fn_name,
            sig,
            &param.name,
            monomial,
            &arg.value,
            &mut bindings,
            registry,
            src,
            arg.span,
        )?;
    }

    let ValueKind::Quantity(result) = sig.result() else {
        return Err(GraphcalError::internal_error(
            format!(
                "signature for `{fn_name}` has a non-quantity result in the quantity checking path"
            ),
            src,
            DiagnosticAnchor::Source(call_span),
        ));
    };
    eval_result_monomial(fn_name, result, &bindings, src, call_span)
}

/// Check one quantity argument against its parameter monomial, binding or
/// comparing dimension variables as required. Shared by built-in and extern
/// call checking.
#[expect(clippy::too_many_arguments, reason = "signature-walk context")]
pub(super) fn check_quantity_param(
    fn_name: &str,
    sig: &FunctionSignature,
    param_name: &crate::syntax::function_name::FnParamName,
    monomial: &DimMonomial,
    arg_dim: &Dimension,
    bindings: &mut HashMap<DimVarName, Dimension>,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    arg_span: Span,
) -> Result<(), GraphcalError> {
    if let Some(var) = monomial.as_bare_var() {
        if let Some(bound) = bindings.get(var) {
            if arg_dim != bound {
                let bind_param_name = first_binding_param(sig, var).ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!(
                            "signature for `{fn_name}` lost the parameter that binds dimension variable `{var}`"
                        ),
                        src,
                        DiagnosticAnchor::Source(arg_span),
                    )
                })?;
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
            DimMonomialEvalError::UnboundVar { var } => GraphcalError::internal_error(
                format!(
                    "builtin `{fn_name}` references unbound dim variable `{var}` in its signature"
                ),
                src,
                DiagnosticAnchor::Source(span),
            ),
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
        ValueKind::Quantity(monomial) if monomial.as_bare_var() == Some(var) => {
            Some(p.name.as_str())
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::{BaseDimId, PreludeBaseDimension};
    use crate::registry::types::RegistryBuilder;
    use crate::syntax::function_name::FnParamName;

    #[test]
    fn missing_dimension_binding_parameter_is_an_internal_error() {
        let signature = FunctionSignature::fixed_to_fixed(
            FnParamName::expect_valid("declared"),
            Dimension::dimensionless(),
            Dimension::dimensionless(),
        );
        let registry = RegistryBuilder::new().try_build().unwrap().into_semantic();
        let source = NamedSource::new("test.gcl", Arc::new("f(1.0, 2.0)".to_string()));
        let argument_span = Span::new(7, 3);
        let variable = DimVarName::expect_valid("D");
        let mut bindings = HashMap::from([(
            variable.clone(),
            Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length)),
        )]);
        let argument_dimension = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));

        let error = check_quantity_param(
            "f",
            &signature,
            &FnParamName::expect_valid("value"),
            &DimMonomial::var(variable),
            &argument_dimension,
            &mut bindings,
            &registry,
            &source,
            argument_span,
        )
        .unwrap_err();

        match error {
            GraphcalError::InternalError { message, .. } => assert!(
                message.contains("lost the parameter that binds dimension variable `D`"),
                "{message}"
            ),
            other => panic!("expected internal error, got {other:?}"),
        }
    }

    #[test]
    fn zero_argument_arity_error_uses_the_explicit_call_span() {
        let signature = FunctionSignature::fixed_to_fixed(
            FnParamName::expect_valid("value"),
            Dimension::dimensionless(),
            Dimension::dimensionless(),
        );
        let registry = RegistryBuilder::new().try_build().unwrap().into_semantic();
        let source = NamedSource::new("test.gcl", Arc::new("f()".to_string()));
        let call_span = Span::new(0, 3);

        let error = infer_fn_dim("f", &signature, &[], call_span, &registry, &source).unwrap_err();
        let GraphcalError::WrongArity { span, .. } = error else {
            panic!("expected wrong-arity diagnostic");
        };
        assert_eq!(span.offset(), call_span.offset());
        assert_eq!(span.len(), call_span.len());
    }
}
