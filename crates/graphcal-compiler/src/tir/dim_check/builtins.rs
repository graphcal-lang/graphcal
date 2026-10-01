//! Interpreter for [`FunctionSignature`]s during dimension checking.
//!
//! One code path checks every signature-carrying function call — built-ins
//! and extern (plugin) functions alike: bind dimension variables at their
//! bare binding occurrences, check compound monomials by evaluation, and
//! compute the result monomial from the bindings.

use std::collections::HashMap;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::function_signature::{
    DimBinder, DimMonomial, DimMonomialEvalError, FunctionSignature, ParamKind, ResultKind,
    ScalarValueKind,
};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::name::NameError;
use crate::source_id::SourceId;
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
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    if args.len() != sig.arity() {
        let error_span = args
            .get(sig.arity())
            .or_else(|| args.last())
            .map_or(call_span, |arg| arg.span);
        return Err(SemanticError::located(
            src,
            error_span,
            NameError::WrongArity {
                name: crate::semantic_error::name::CalledFunction::Builtin(function),
                expected: sig.arity(),
                got: args.len(),
            },
        ));
    }

    let fn_name = function.as_str();
    let mut walk = SignatureDimWalk::new(fn_name, registry, src);

    for (param, arg) in sig.params().iter().zip(args) {
        let ParamKind::Scalar(ScalarValueKind::Quantity(monomial)) = &param.kind else {
            return Err(SemanticError::internal_error(
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
        return Err(SemanticError::internal_error(
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
pub(super) struct SignatureDimWalk<'a> {
    fn_name: &'a str,
    bindings: HashMap<DimBinder, DimBinding>,
    registry: &'a FormattingRegistry,
    src: SourceId,
}

/// The dimension a variable is bound to, and the parameter whose argument
/// bound it.
struct DimBinding {
    dimension: Dimension,
    parameter: crate::syntax::function_name::FnParamName,
}

impl<'a> SignatureDimWalk<'a> {
    /// Start walking a call of `fn_name` with no dimension variables bound.
    pub(super) fn new(fn_name: &'a str, registry: &'a FormattingRegistry, src: SourceId) -> Self {
        Self {
            fn_name,
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
    ) -> Result<(), SemanticError> {
        if let Some(var) = monomial.as_bare_var() {
            if let Some(bound) = self.bindings.get(var) {
                if *arg_dim != bound.dimension {
                    return Err(SemanticError::located(
                        self.src,
                        arg_span,
                        DimensionError::DimensionMismatch {
                            expected: self.registry.dimensions.format_dimension(&bound.dimension),
                            found: self.registry.dimensions.format_dimension(arg_dim),
                            help: format!(
                                "parameter `{param_name}` must have the same dimension as `{}`",
                                bound.parameter
                            ),
                        },
                    ));
                }
            } else {
                self.bindings.insert(
                    var.clone(),
                    DimBinding {
                        dimension: arg_dim.clone(),
                        parameter: param_name.clone(),
                    },
                );
            }
            return Ok(());
        }

        let expected = eval_monomial(self.fn_name, monomial, &self.bindings, self.src, arg_span)?;
        if *arg_dim != expected {
            return Err(SemanticError::located(
                self.src,
                arg_span,
                DimensionError::DimensionMismatch {
                    expected: self.registry.dimensions.format_dimension(&expected),
                    found: self.registry.dimensions.format_dimension(arg_dim),
                    help: format!(
                        "parameter `{param_name}` requires {}",
                        self.registry.dimensions.format_dimension(&expected),
                    ),
                },
            ));
        }
        Ok(())
    }

    /// Compute the result dimension of the signature from the bound variables.
    pub(super) fn result(
        &self,
        result: &DimMonomial,
        span: Span,
    ) -> Result<Dimension, SemanticError> {
        eval_monomial(self.fn_name, result, &self.bindings, self.src, span)
    }
}

fn eval_monomial(
    fn_name: &str,
    monomial: &DimMonomial,
    bindings: &HashMap<DimBinder, DimBinding>,
    src: SourceId,
    span: Span,
) -> Result<Dimension, SemanticError> {
    monomial
        .eval(|var| bindings.get(var).map(|binding| &binding.dimension))
        .map_err(|err| match err {
            // A missing binding here means the signature is malformed (a compound
            // use or result without a matching bare binding occurrence) — the
            // signature validator rejects that shape, so surface an internal
            // error rather than panicking.
            DimMonomialEvalError::UnboundVar { var } => SemanticError::internal_error(
                format!(
                    "builtin `{fn_name}` references unbound dim variable `{var}` in its signature"
                ),
                src,
                DiagnosticAnchor::Source(span),
            ),
            DimMonomialEvalError::Overflow(_) => {
                SemanticError::located(src, span, DimensionError::DimensionOverflow)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::{BaseDimId, PreludeBaseDimension};
    use crate::semantic_error::SemanticErrorKind;
    use crate::syntax::function_name::FnParamName;

    fn quantity_param(
        signature: &FunctionSignature,
        position: usize,
    ) -> (&FnParamName, &DimMonomial) {
        let param = &signature.params()[position];
        let ParamKind::Scalar(ScalarValueKind::Quantity(monomial)) = &param.kind else {
            panic!("expected a quantity parameter");
        };
        (&param.name, monomial)
    }

    /// A mismatch names the parameter whose argument bound the variable,
    /// whichever kind of parameter it was.
    #[test]
    fn dimension_mismatch_names_the_binding_parameter() {
        let signature = FunctionSignature::same_dim(&["first", "second"]);
        let registry = crate::display::formatting_registry::FormattingRegistry::new(
            std::collections::BTreeMap::new(),
            Vec::new(),
        );
        let source = crate::source_registry::SourceRegistry::new()
            .register("test.gcl", std::sync::Arc::new("f(1.0, 2.0)".to_string()));
        let length = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length));
        let time = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));
        let mut walk = SignatureDimWalk::new("f", &registry, source);
        let (first, monomial) = quantity_param(&signature, 0);
        walk.check_quantity_param(first, monomial, &length, Span::new(2, 3))
            .unwrap();
        let (second, monomial) = quantity_param(&signature, 1);
        let error = walk
            .check_quantity_param(second, monomial, &time, Span::new(7, 3))
            .unwrap_err();
        let SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Dimension(DimensionError::DimensionMismatch { help, .. }),
            ..
        }) = error
        else {
            panic!("expected a dimension mismatch, got {error:?}");
        };
        assert_eq!(
            help,
            "parameter `second` must have the same dimension as `first`"
        );
    }

    #[test]
    fn zero_argument_arity_error_uses_the_explicit_call_span() {
        let signature = FunctionSignature::fixed_to_fixed(
            FnParamName::expect_valid("value"),
            Dimension::dimensionless(),
            Dimension::dimensionless(),
        );
        let registry = crate::display::formatting_registry::FormattingRegistry::new(
            std::collections::BTreeMap::new(),
            Vec::new(),
        );
        let source = crate::source_registry::SourceRegistry::new()
            .register("test.gcl", std::sync::Arc::new("f()".to_string()));
        let call_span = Span::new(0, 3);

        let error = infer_fn_dim(
            crate::builtin::BuiltinFn::Scalar(crate::builtin::ScalarFn::Sqrt),
            &signature,
            &[],
            call_span,
            &registry,
            source,
        )
        .unwrap_err();
        let SemanticError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Name(NameError::WrongArity { .. }),
            primary: span,
            ..
        }) = error
        else {
            panic!("expected wrong-arity diagnostic");
        };
        assert_eq!(span.offset(), call_span.offset());
        assert_eq!(span.len(), call_span.len());
    }
}
