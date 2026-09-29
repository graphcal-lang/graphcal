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
use crate::function_signature::{
    DimBinder, DimMonomial, DimMonomialEvalError, FunctionSignature, ParamKind, ResultKind,
    ScalarValueKind, StructResult,
};
use crate::registry::error::GraphcalError;
use crate::registry::types::FormattingRegistry;
use crate::syntax::span::{Span, Spanned};

/// Check quantity argument dimensions against `sig` and compute the result
/// dimension.
///
/// Arguments are quantity dimensions; callers verify non-quantity scalar kinds
/// before reaching this walk. All
/// built-in registry signatures are all-quantity, so built-in inference calls
/// this directly.
pub(super) fn infer_fn_dim(
    function: crate::builtin::BuiltinFn,
    sig: &FunctionSignature,
    args: &[Spanned<Dimension>],
    call_span: Span,
    registry: &FormattingRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    if args.len() != sig.arity() {
        let error_span = args
            .get(sig.arity())
            .or_else(|| args.last())
            .map_or(call_span, |arg| arg.span);
        return Err(GraphcalError::WrongArity {
            name: crate::registry::error::CalledFunction::Builtin(function),
            expected: sig.arity(),
            got: args.len(),
            src: src.clone(),
            span: error_span.into(),
        });
    }

    let fn_name = function.as_str();
    let mut walk = SignatureDimWalk::new(fn_name, sig, registry, src);

    for (param, arg) in sig.params().iter().zip(args) {
        let ParamKind::Scalar(ScalarValueKind::Quantity(monomial)) = &param.kind else {
            return Err(GraphcalError::internal_error(
                format!(
                    "signature for `{fn_name}` has a non-quantity parameter `{}` in the quantity checking path",
                    param.name
                ),
                src,
                DiagnosticAnchor::Source(arg.span),
            ));
        };
        walk.check_quantity_param(&param.name, monomial, &arg.value, arg.span)?;
    }

    let ResultKind::Value(ParamKind::Scalar(ScalarValueKind::Quantity(result))) = sig.result()
    else {
        return Err(GraphcalError::internal_error(
            format!(
                "signature for `{fn_name}` has a non-quantity result in the quantity checking path"
            ),
            src,
            DiagnosticAnchor::Source(call_span),
        ));
    };
    walk.result(result, call_span)
}

/// Dimension-variable bindings accumulated while checking one call against
/// its signature. Shared by built-in and extern call checking.
pub(super) struct SignatureDimWalk<'a, S: StructResult = crate::function_signature::StructShape> {
    fn_name: &'a str,
    sig: &'a FunctionSignature<S>,
    bindings: HashMap<DimBinder, Dimension>,
    registry: &'a FormattingRegistry,
    src: &'a NamedSource<Arc<String>>,
}

impl<'a, S: StructResult> SignatureDimWalk<'a, S> {
    /// Start walking `sig` with no dimension variables bound.
    pub(super) fn new(
        fn_name: &'a str,
        sig: &'a FunctionSignature<S>,
        registry: &'a FormattingRegistry,
        src: &'a NamedSource<Arc<String>>,
    ) -> Self {
        Self {
            fn_name,
            sig,
            bindings: HashMap::new(),
            registry,
            src,
        }
    }

    /// Check one quantity argument against its parameter monomial, binding or
    /// comparing dimension variables as required.
    pub(super) fn check_quantity_param(
        &mut self,
        param_name: &crate::syntax::function_name::FnParamName,
        monomial: &DimMonomial,
        arg_dim: &Dimension,
        arg_span: Span,
    ) -> Result<(), GraphcalError> {
        if let Some(var) = monomial.as_bare_var() {
            if let Some(bound) = self.bindings.get(var) {
                if arg_dim != bound {
                    let bind_param_name = first_binding_param(self.sig, var).ok_or_else(|| {
                        GraphcalError::internal_error(
                            format!(
                                "signature for `{}` lost the parameter that binds dimension variable `{var}`",
                                self.fn_name
                            ),
                            self.src,
                            DiagnosticAnchor::Source(arg_span),
                        )
                    })?;
                    return Err(GraphcalError::DimensionMismatch {
                        expected: self.registry.dimensions.format_dimension(bound),
                        found: self.registry.dimensions.format_dimension(arg_dim),
                        help: format!(
                            "parameter `{param_name}` must have the same dimension as `{bind_param_name}`",
                        ),
                        src: self.src.clone(),
                        span: arg_span.into(),
                    });
                }
            } else {
                self.bindings.insert(var.clone(), arg_dim.clone());
            }
            return Ok(());
        }

        let expected = eval_monomial(self.fn_name, monomial, &self.bindings, self.src, arg_span)?;
        if *arg_dim != expected {
            return Err(GraphcalError::DimensionMismatch {
                expected: self.registry.dimensions.format_dimension(&expected),
                found: self.registry.dimensions.format_dimension(arg_dim),
                help: format!(
                    "parameter `{param_name}` requires {}",
                    self.registry.dimensions.format_dimension(&expected),
                ),
                src: self.src.clone(),
                span: arg_span.into(),
            });
        }
        Ok(())
    }

    /// Compute the result dimension of the signature from the bound variables.
    pub(super) fn result(
        &self,
        result: &DimMonomial,
        span: Span,
    ) -> Result<Dimension, GraphcalError> {
        eval_monomial(self.fn_name, result, &self.bindings, self.src, span)
    }
}

fn eval_monomial(
    fn_name: &str,
    monomial: &DimMonomial,
    bindings: &HashMap<DimBinder, Dimension>,
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
fn first_binding_param<'a, S: StructResult>(
    sig: &'a FunctionSignature<S>,
    var: &DimBinder,
) -> Option<&'a str> {
    sig.params().iter().find_map(|p| match &p.kind {
        ParamKind::Scalar(ScalarValueKind::Quantity(monomial))
            if monomial.as_bare_var() == Some(var) =>
        {
            Some(p.name.as_str())
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::{BaseDimId, PreludeBaseDimension};
    use crate::syntax::function_name::FnParamName;

    #[test]
    fn missing_dimension_binding_parameter_is_an_internal_error() {
        let signature = FunctionSignature::fixed_to_fixed(
            FnParamName::expect_valid("declared"),
            Dimension::dimensionless(),
            Dimension::dimensionless(),
        );
        let registry = crate::registry::types::FormattingRegistry::new(
            std::collections::BTreeMap::new(),
            Vec::new(),
        );
        let source = NamedSource::new("test.gcl", Arc::new("f(1.0, 2.0)".to_string()));
        let argument_span = Span::new(7, 3);
        // A binder from a different signature: `signature` has no parameter
        // binding it.
        let foreign = FunctionSignature::passthrough("x");
        let ParamKind::Scalar(ScalarValueKind::Quantity(foreign_monomial)) =
            &foreign.params()[0].kind
        else {
            panic!("passthrough takes a quantity");
        };
        let variable = foreign_monomial.as_bare_var().unwrap().clone();
        let mut walk = SignatureDimWalk::new("f", &signature, &registry, &source);
        walk.bindings = HashMap::from([(
            variable.clone(),
            Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length)),
        )]);
        let argument_dimension = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));

        let error = walk
            .check_quantity_param(
                &FnParamName::expect_valid("value"),
                &DimMonomial::var(variable),
                &argument_dimension,
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
        let registry = crate::registry::types::FormattingRegistry::new(
            std::collections::BTreeMap::new(),
            Vec::new(),
        );
        let source = NamedSource::new("test.gcl", Arc::new("f()".to_string()));
        let call_span = Span::new(0, 3);

        let error = infer_fn_dim(
            crate::builtin::BuiltinFn::Scalar(crate::builtin::ScalarFn::Sqrt),
            &signature,
            &[],
            call_span,
            &registry,
            &source,
        )
        .unwrap_err();
        let GraphcalError::WrongArity { span, .. } = error else {
            panic!("expected wrong-arity diagnostic");
        };
        assert_eq!(span.offset(), call_span.offset());
        assert_eq!(span.len(), call_span.len());
    }
}
