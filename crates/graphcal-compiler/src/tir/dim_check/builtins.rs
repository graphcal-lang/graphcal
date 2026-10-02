//! Interpreter for [`FunctionSignature`]s during dimension checking.
//!
//! One code path checks every signature-carrying function call — built-ins
//! and extern (plugin) functions alike: bind dimension variables at their
//! bare binding occurrences, check compound monomials by evaluation, and
//! compute the result monomial from the bindings.

use crate::semantic_error::dimension_mismatch::{MismatchOperand, MismatchRule};
use std::collections::HashMap;

use crate::builtin::{BuiltinArity, BuiltinFn};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::function_signature::{DimBinder, DimMonomial, DimMonomialEvalError, QuantitySignature};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::name::NameError;
use crate::source_id::SourceId;
use crate::syntax::span::{Span, Spanned};

/// The arguments of a built-in call, whose count [`ArityChecked::check`]
/// matched against the function's static entry.
///
/// This is the only arity check of a built-in call: it runs once, before any
/// argument is inferred, and every later reading of the arguments relies on
/// the count it accepted.
#[derive(Debug)]
pub(super) struct ArityChecked<T> {
    function: BuiltinFn,
    args: Vec<T>,
}

impl<T> ArityChecked<T> {
    /// The arguments `args` of a call of `function`, whose callee spans
    /// `span`.
    ///
    /// # Errors
    ///
    /// Returns a wrong-arity diagnostic when `function`'s entry does not
    /// accept as many arguments.
    pub(super) fn check(
        function: BuiltinFn,
        args: Vec<T>,
        span: Span,
        src: SourceId,
    ) -> Result<Self, SemanticError> {
        let got = args.len();
        match function.entry().arity() {
            BuiltinArity::Exact(expected) if got != expected => Err(SemanticError::located(
                src,
                span,
                NameError::WrongArity {
                    name: crate::semantic_error::name::CalledFunction::Builtin(function),
                    expected,
                    got,
                },
            )),
            arity @ BuiltinArity::OptionalTrailing { .. } if !arity.accepts(got) => {
                Err(SemanticError::located(
                    src,
                    span,
                    NameError::WrongOptionalArity {
                        function,
                        arity,
                        got,
                    },
                ))
            }
            BuiltinArity::Exact(_) | BuiltinArity::OptionalTrailing { .. } => {
                Ok(Self { function, args })
            }
        }
    }

    /// The called function.
    pub(super) const fn function(&self) -> BuiltinFn {
        self.function
    }

    /// The arguments, each read by `read`; the count is kept.
    pub(super) fn try_map<U, E>(
        self,
        read: impl FnMut(T) -> Result<U, E>,
    ) -> Result<ArityChecked<U>, E> {
        Ok(ArityChecked {
            function: self.function,
            args: self.args.into_iter().map(read).collect::<Result<_, _>>()?,
        })
    }
}

/// Check quantity argument dimensions against `sig`, the quantity signature
/// of the called scalar function, and compute the result dimension.
///
/// Arguments are quantity dimensions; callers verify non-quantity scalar kinds
/// before reaching this walk. A scalar function's signature has the arity of
/// its static entry, which [`ArityChecked`] matched.
pub(super) fn infer_fn_dim(
    sig: &QuantitySignature,
    args: &ArityChecked<Spanned<Dimension>>,
    call_span: Span,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    let fn_name = args.function().as_str();
    let mut walk = SignatureDimWalk::new(fn_name, registry, src);

    for (param, arg) in sig.params().iter().zip(&args.args) {
        walk.check_quantity_param(param.name(), param.monomial(), &arg.value, arg.span)?;
    }
    walk.result(sig.result(), call_span)
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
                            expected: Box::new(MismatchOperand::Dimension(
                                self.registry
                                    .dimensions
                                    .dimension_spelling(&bound.dimension),
                            )),
                            found: Box::new(MismatchOperand::Dimension(
                                self.registry.dimensions.dimension_spelling(arg_dim),
                            )),
                            help: Box::new(MismatchRule::ParameterSameDimension {
                                parameter: param_name.clone(),
                                bound_by: bound.parameter.clone(),
                            }),
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
                    expected: Box::new(MismatchOperand::Dimension(
                        self.registry.dimensions.dimension_spelling(&expected),
                    )),
                    found: Box::new(MismatchOperand::Dimension(
                        self.registry.dimensions.dimension_spelling(arg_dim),
                    )),
                    help: Box::new(MismatchRule::ParameterDimension {
                        parameter: param_name.clone(),
                        dimension: self.registry.dimensions.dimension_spelling(&expected),
                    }),
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
    use crate::function_signature::{FunctionSignature, ParamKind, ScalarValueKind};
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
            help.to_string(),
            "parameter `second` must have the same dimension as `first`"
        );
    }

    #[test]
    fn calls_are_checked_against_their_entry_arity_once() {
        let source = crate::source_registry::SourceRegistry::new()
            .register("test.gcl", std::sync::Arc::new("f()".to_string()));
        let sqrt = BuiltinFn::Scalar(crate::builtin::ScalarFn::Sqrt);
        let call_span = Span::new(0, 3);
        let error = ArityChecked::<()>::check(sqrt, Vec::new(), call_span, source).unwrap_err();
        let SemanticError::Located(crate::diagnostic::Diagnostic {
            kind:
                SemanticErrorKind::Name(NameError::WrongArity {
                    expected: 1,
                    got: 0,
                    ..
                }),
            primary: span,
            ..
        }) = error
        else {
            panic!("expected wrong-arity diagnostic");
        };
        assert_eq!(span.offset(), call_span.offset());
        let checked = ArityChecked::check(sqrt, vec![2], call_span, source).unwrap();
        let mapped = checked.try_map(|arg| Ok::<_, ()>(arg * 2)).unwrap();
        assert_eq!(mapped.function(), sqrt);
        assert_eq!(mapped.args, [4]);
    }
}
