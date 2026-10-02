//! Inference of built-in function calls: arity, dispatch, and the linear-algebra and complex families.

use crate::hir::expr::{Expr, FunctionRef};
use crate::outcome::Outcome;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::semantic_error::name::NameError;
use crate::source_id::SourceId;

use crate::builtin::{
    AggregationFn, BuiltinArity, BuiltinFn, DatetimeFn, ScalarFn, ValueAggregation,
};
use crate::dimension::{Dimension, Rational};
use crate::semantic_error::SemanticError;
use crate::syntax::span::Span;

use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::tir::dim_check::builtins::infer_fn_dim;
use crate::tir::dim_check::helpers::{expect_quantity, format_checked_type};
use crate::tir::dim_check::infer::linear_algebra::{
    LinearAlgebraTypeError, infer_linear_algebra_type,
};

use super::context::Infer;

/// Check a built-in call's argument count against its static entry.
///
/// This is the only arity check for built-in calls: `infer_hir_fn_call` runs
/// it once, before inferring any argument, and the family rules rely on it.
fn check_builtin_arity(
    function: BuiltinFn,
    got: usize,
    span: Span,
    src: SourceId,
) -> Result<(), SemanticError> {
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
                EvaluationError::Failed {
                    message: format!("{function}() expects {arity} arguments, got {got}"),
                },
            ))
        }
        BuiltinArity::Exact(_) | BuiltinArity::OptionalTrailing { .. } => Ok(()),
    }
}

impl Infer<'_> {
    fn infer_hir_linear_algebra_call(
        &self,
        function: crate::builtin::LinearAlgebraFn,
        callee_span: Span,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let argument_types = args
            .iter()
            .map(|arg| self.infer_arg(arg))
            .collect::<Result<Vec<_>, _>>()?;

        infer_linear_algebra_type(function, &argument_types, |index| {
        crate::tir::dim_check::infer::concrete_cardinality_for_inferred(index, self.env.tir)
    })
    .map_err(|error| match error {
        LinearAlgebraTypeError::ExpectedIndexedQuantity { argument, rank } => {
            SemanticError::located(self.env.src, args[argument].span, DimensionError::DimensionMismatch { expected: format!("rank-{rank} indexed quantity"), found: format_checked_type(&argument_types[argument], self.env.registry), help: format!(
                    "{}() requires argument {} to be a rank-{rank} indexed quantity",
                    function.as_str(),
                    argument.saturating_add(1)
                ) })
        }
        LinearAlgebraTypeError::AxisMismatch {
            argument,
            expected,
            found,
        } => SemanticError::located(self.env.src, args[argument].span, DimensionError::LinearAlgebraShapeMismatch { function, expected: expected.to_string(), found: found.to_string(), help: "linear-algebra contractions match axes by typed identity; use the same declared index (or the same Fin(N) structural index) at both contracted positions"
                .to_string() }),
        LinearAlgebraTypeError::CardinalityMismatch {
            argument,
            expected,
            found,
        } => SemanticError::located(self.env.src, args[argument].span, DimensionError::LinearAlgebraShapeMismatch { function, expected: format!("an axis with exactly {expected} entries"), found: found.map_or_else(
                || "an axis whose cardinality is not concrete".to_string(),
                |cardinality| format!("an axis with {cardinality} entries"),
            ), help: format!("{}() is defined only for three-component vectors", function.as_str()) }),
        LinearAlgebraTypeError::ConcreteCardinalityRequired { argument } => {
            SemanticError::located(self.env.src, args[argument].span, DimensionError::LinearAlgebraShapeMismatch { function, expected: "an axis with a concrete cardinality".to_string(), found: "an axis whose cardinality is still generic".to_string(), help: format!(
                    "{}() needs a concrete matrix size because its result dimension depends on that size",
                    function.as_str()
                ) })
        }
        LinearAlgebraTypeError::DimensionOverflow => SemanticError::located(self.env.src, callee_span, DimensionError::DimensionOverflow),
    }).map_err(Outcome::Failed)
    }

    pub(super) fn infer_hir_fn_call(
        &self,
        callee: &crate::syntax::span::Spanned<FunctionRef>,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let builtin = match &callee.value {
            FunctionRef::Builtin(builtin) => builtin.function(),
            FunctionRef::Epoch { .. } => BuiltinFn::EPOCH,
            FunctionRef::External(ext) => {
                return self.infer_extern_fn_call(ext, callee.span, args);
            }
        };
        // The single arity check for every built-in family, driven by its static
        // entry and run before any argument is inferred. Family rules below may
        // rely on the accepted argument count.
        check_builtin_arity(builtin, args.len(), callee.span, self.env.src)?;
        match builtin {
            BuiltinFn::Complex(function) => self.infer_hir_complex_call(function, args),
            BuiltinFn::Aggregation(kind) => {
                let arg_type = self.infer_arg(&args[0])?;
                let CheckedType::Indexed { element, index } = &arg_type else {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: "indexed collection".to_string(),
                            found: format_checked_type(&arg_type, self.env.registry),
                            help: format!("{}() requires an indexed value", builtin.as_str()),
                        },
                    )
                    .into());
                };
                let rank = arg_type.indexed_rank();
                if rank > 1 {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::MultiAxisAggregation {
                            function: kind,
                            rank,
                        },
                    )
                    .into());
                }
                if kind == AggregationFn::Value(ValueAggregation::Count) {
                    return Ok(CheckedType::Int);
                }
                if matches!(kind, AggregationFn::Key(_)) {
                    // The extremum's identity: a key of the reduced axis. The
                    // element-type requirement below still applies, so check it
                    // before returning.
                    if element.quantity_dimension().is_none() {
                        return Err(SemanticError::located(
                            self.env.src,
                            args[0].span,
                            DimensionError::DimensionMismatch {
                                expected: "indexed quantity collection".to_string(),
                                found: format_checked_type(element, self.env.registry),
                                help: format!(
                                    "{}() requires every indexed element to be quantity",
                                    builtin.as_str()
                                ),
                            },
                        )
                        .into());
                    }
                    return Ok(CheckedType::Key(index.clone()));
                }
                let Some(dimension) = element.quantity_dimension().cloned() else {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: "indexed quantity collection".to_string(),
                            found: format_checked_type(element, self.env.registry),
                            help: format!(
                                "{}() requires every indexed element to be quantity",
                                builtin.as_str()
                            ),
                        },
                    )
                    .into());
                };
                if kind != AggregationFn::Value(ValueAggregation::Product)
                    || dimension.is_dimensionless()
                {
                    return Ok(CheckedType::Quantity(dimension));
                }
                let cardinality = crate::tir::dim_check::infer::concrete_cardinality_for_inferred(
                    index,
                    self.env.tir,
                )
                .ok_or_else(|| {
                    SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::AggregationCardinalityUnknown { function: kind },
                    )
                })?;
                i32::try_from(cardinality)
                    .ok()
                    .and_then(|exponent| Rational::integer(exponent).ok())
                    .and_then(|exponent| dimension.pow(exponent).ok())
                    .map(CheckedType::Quantity)
                    .ok_or_else(|| {
                        SemanticError::located(
                            self.env.src,
                            args[0].span,
                            DimensionError::DimensionOverflow,
                        )
                    })
                    .map_err(Outcome::Failed)
            }
            BuiltinFn::LinearAlgebra(function) => {
                self.infer_hir_linear_algebra_call(function, callee.span, args)
            }
            BuiltinFn::Conversion(kind) => self.infer_hir_type_conversion(kind, args),
            BuiltinFn::Datetime(DatetimeFn::ScaleConversion(conversion)) => {
                self.infer_hir_timescale_conversion(builtin, conversion.target(), args)
            }
            BuiltinFn::Datetime(DatetimeFn::Constructor(kind)) => {
                self.infer_hir_datetime_constructor(kind, callee.span, args)
            }
            BuiltinFn::Datetime(DatetimeFn::Field(_)) => {
                self.infer_hir_datetime_unary(builtin, args, CheckedType::Int)
            }
            BuiltinFn::Datetime(DatetimeFn::FromNumeric(_)) => {
                let arg_type = self.infer_arg(&args[0])?;
                match &arg_type {
                    t if t
                        .quantity_dimension()
                        .is_some_and(Dimension::is_dimensionless) => {}
                    CheckedType::Int => {}
                    _ => {
                        return Err(SemanticError::located(
                            self.env.src,
                            args[0].span,
                            DimensionError::DimensionMismatch {
                                expected: "Dimensionless or Int".to_string(),
                                found: format_checked_type(&arg_type, self.env.registry),
                                help: format!(
                                    "{}() requires a dimensionless numeric argument",
                                    builtin.as_str()
                                ),
                            },
                        )
                        .into());
                    }
                }
                Ok(CheckedType::Datetime(
                    crate::semantic::time_scale::TimeScale::UTC,
                ))
            }
            BuiltinFn::Datetime(DatetimeFn::ToNumeric(_)) => self.infer_hir_datetime_unary(
                builtin,
                args,
                CheckedType::Quantity(Dimension::dimensionless()),
            ),
            BuiltinFn::Scalar(function) => self.infer_hir_builtin_fn(function, callee.span, args),
        }
    }

    fn infer_hir_complex_call(
        &self,
        function: crate::builtin::ComplexFn,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        use crate::tir::dim_check::infer::complex::ComplexTypeError;

        let inferred = args
            .iter()
            .map(|arg| self.infer_arg(arg))
            .collect::<Result<Vec<_>, _>>()?;
        crate::tir::dim_check::infer::complex::infer(function, &inferred)
            .map_err(|error| match error {
                ComplexTypeError::ExpectedQuantity { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: "quantity type".to_string(),
                        found: format_checked_type(&inferred[argument], self.env.registry),
                        help: format!(
                            "{}() requires a quantity in argument {}",
                            function.as_str(),
                            argument.saturating_add(1)
                        ),
                    },
                ),
                ComplexTypeError::ExpectedComplex { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: "Complex<D>".to_string(),
                        found: format_checked_type(&inferred[argument], self.env.registry),
                        help: format!("{}() requires a complex quantity", function.as_str()),
                    },
                ),
                ComplexTypeError::ExpectedQuantityOrComplex { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: "a real or complex quantity".to_string(),
                        found: format_checked_type(&inferred[argument], self.env.registry),
                        help: format!(
                            "{}() requires a real or complex quantity",
                            function.as_str()
                        ),
                    },
                ),
                ComplexTypeError::DimensionMismatch { left, right } => SemanticError::located(
                    self.env.src,
                    args[right].span,
                    DimensionError::DimensionMismatch {
                        expected: format_checked_type(&inferred[left], self.env.registry),
                        found: format_checked_type(&inferred[right], self.env.registry),
                        help: "real and imaginary components must have the same dimension"
                            .to_string(),
                    },
                ),
                ComplexTypeError::ExpectedAngle { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: "Angle".to_string(),
                        found: format_checked_type(&inferred[argument], self.env.registry),
                        help: "polar() phase must be an Angle quantity".to_string(),
                    },
                ),
                ComplexTypeError::ExpectedDimensionless { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: "Dimensionless or Complex<Dimensionless>".to_string(),
                        found: format_checked_type(&inferred[argument], self.env.registry),
                        help: "exp() requires a dimensionless real or complex argument".to_string(),
                    },
                ),
            })
            .map_err(Outcome::Failed)
    }

    fn infer_hir_builtin_fn(
        &self,
        name: ScalarFn,
        callee_span: Span,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let func = crate::semantic::scalar_function::scalar_function(name);
        let dimension_args = args
            .iter()
            .map(|arg| {
                let inferred = self.infer_arg(arg)?;
                let dimension =
                    expect_quantity(&inferred, self.env.registry, self.env.src, arg.span)?;
                Ok(crate::syntax::span::Spanned::new(dimension, arg.span))
            })
            .collect::<Result<Vec<_>, Outcome<SemanticError>>>()?;
        infer_fn_dim(
            name.into(),
            func.quantity_signature(),
            &dimension_args,
            callee_span,
            self.env.registry,
            self.env.src,
        )
        .map(CheckedType::Quantity)
        .map_err(Outcome::Failed)
    }
}
