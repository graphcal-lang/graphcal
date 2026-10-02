//! Inference of built-in function calls: arity, dispatch, and the linear-algebra and complex families.

use crate::hir::expr::{Expr, FunctionRef};
use crate::outcome::Outcome;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension::LinearAlgebraAxisMismatch;
use crate::semantic_error::dimension_mismatch::{
    MismatchOperand, MismatchRule, OperandExpectation,
};

use crate::builtin::{AggregationFn, BuiltinFn, DatetimeFn, ScalarFn, ValueAggregation};
use crate::dimension::{Dimension, Rational};
use crate::semantic_error::SemanticError;
use crate::syntax::span::Span;

use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::tir::dim_check::builtins::{ArityChecked, infer_fn_dim};
use crate::tir::dim_check::helpers::expect_quantity;
use crate::tir::dim_check::infer::linear_algebra::{
    LinearAlgebraTypeError, infer_linear_algebra_type,
};

use super::context::Infer;

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
                SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::RankIndexedQuantity { rank },
                        )),
                        found: Box::new(MismatchOperand::Type(
                            argument_types[argument].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::LinearAlgebraArgument {
                            function,
                            argument: argument.saturating_add(1),
                            rank,
                        }),
                    },
                )
            }
            LinearAlgebraTypeError::AxisMismatch {
                argument,
                expected,
                found,
            } => SemanticError::located(
                self.env.src,
                args[argument].span,
                DimensionError::LinearAlgebraShapeMismatch {
                    function,
                    mismatch: LinearAlgebraAxisMismatch::Identity {
                        expected: Box::new(expected),
                        found: Box::new(found),
                    },
                },
            ),
            LinearAlgebraTypeError::CardinalityMismatch {
                argument,
                expected,
                found,
            } => SemanticError::located(
                self.env.src,
                args[argument].span,
                DimensionError::LinearAlgebraShapeMismatch {
                    function,
                    mismatch: LinearAlgebraAxisMismatch::Cardinality { expected, found },
                },
            ),
            LinearAlgebraTypeError::ConcreteCardinalityRequired { argument } => {
                SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::LinearAlgebraShapeMismatch {
                        function,
                        mismatch: LinearAlgebraAxisMismatch::ConcreteCardinalityRequired,
                    },
                )
            }
            LinearAlgebraTypeError::DimensionOverflow => {
                SemanticError::located(self.env.src, callee_span, DimensionError::DimensionOverflow)
            }
        })
        .map_err(Outcome::Failed)
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
        let checked =
            ArityChecked::check(builtin, args.iter().collect(), callee.span, self.env.src)?;
        match builtin {
            BuiltinFn::Complex(function) => self.infer_hir_complex_call(function, args),
            BuiltinFn::Aggregation(kind) => {
                let arg_type = self.infer_arg(&args[0])?;
                let CheckedType::Indexed { element, index } = &arg_type else {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::IndexedCollection,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::IndexedArgument(builtin)),
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
                                expected: Box::new(MismatchOperand::Expected(
                                    OperandExpectation::IndexedQuantityCollection,
                                )),
                                found: Box::new(MismatchOperand::Type(
                                    element.spelling(&self.env.registry.dimensions),
                                )),
                                help: Box::new(MismatchRule::QuantityElements(builtin)),
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
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::IndexedQuantityCollection,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                element.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::QuantityElements(builtin)),
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
                                expected: Box::new(MismatchOperand::Expected(
                                    OperandExpectation::DimensionlessOrInt,
                                )),
                                found: Box::new(MismatchOperand::Type(
                                    arg_type.spelling(&self.env.registry.dimensions),
                                )),
                                help: Box::new(MismatchRule::DimensionlessNumericArgument(builtin)),
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
            BuiltinFn::Scalar(function) => {
                self.infer_hir_builtin_fn(function, callee.span, checked)
            }
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
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::QuantityType,
                        )),
                        found: Box::new(MismatchOperand::Type(
                            inferred[argument].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::ComplexQuantityArgument {
                            function,
                            argument: argument.saturating_add(1),
                        }),
                    },
                ),
                ComplexTypeError::ExpectedComplex { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::ComplexQuantity,
                        )),
                        found: Box::new(MismatchOperand::Type(
                            inferred[argument].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::ComplexArgument(function)),
                    },
                ),
                ComplexTypeError::ExpectedQuantityOrComplex { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::RealOrComplexQuantity,
                        )),
                        found: Box::new(MismatchOperand::Type(
                            inferred[argument].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::RealOrComplexArgument(function)),
                    },
                ),
                ComplexTypeError::DimensionMismatch { left, right } => SemanticError::located(
                    self.env.src,
                    args[right].span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Type(
                            inferred[left].spelling(&self.env.registry.dimensions),
                        )),
                        found: Box::new(MismatchOperand::Type(
                            inferred[right].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::ComplexComponentsSameDimension),
                    },
                ),
                ComplexTypeError::ExpectedAngle { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(OperandExpectation::Angle)),
                        found: Box::new(MismatchOperand::Type(
                            inferred[argument].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::PolarPhaseAngle),
                    },
                ),
                ComplexTypeError::ExpectedDimensionless { argument } => SemanticError::located(
                    self.env.src,
                    args[argument].span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::DimensionlessOrComplexDimensionless,
                        )),
                        found: Box::new(MismatchOperand::Type(
                            inferred[argument].spelling(&self.env.registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::ExpDimensionless),
                    },
                ),
            })
            .map_err(Outcome::Failed)
    }

    fn infer_hir_builtin_fn(
        &self,
        name: ScalarFn,
        callee_span: Span,
        args: ArityChecked<&Expr>,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let func = crate::semantic::scalar_function::scalar_function(name);
        let dimension_args = args.try_map(|arg| {
            let inferred = self.infer_arg(arg)?;
            let dimension = expect_quantity(&inferred, self.env.registry, self.env.src, arg.span)?;
            Ok::<_, Outcome<SemanticError>>(crate::syntax::span::Spanned::new(dimension, arg.span))
        })?;
        infer_fn_dim(
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
