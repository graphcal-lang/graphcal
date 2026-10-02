//! Inference of the type-conversion, time-scale, and datetime built-in calls.

use crate::builtin::{BuiltinFn, ConversionFn, DatetimeConstructorFn};
use crate::dimension::Dimension;
use crate::hir::expr::{Expr, ExprKind};
use crate::outcome::Outcome;
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::{
    MismatchOperand, MismatchRule, OperandExpectation,
};
use crate::semantic_error::index::IndexError;

use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::tir::dim_check::helpers::expect_quantity;

use super::context::Infer;

impl Infer<'_> {
    pub(super) fn infer_hir_type_conversion(
        &self,
        kind: ConversionFn,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let arg_type = self.infer_arg(&args[0])?;
        match kind {
            ConversionFn::ToFloat => {
                if arg_type != CheckedType::Int {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(OperandExpectation::Int)),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::ToFloatInt),
                        },
                    )
                    .into());
                }
                Ok(CheckedType::Quantity(Dimension::dimensionless()))
            }
            ConversionFn::ToInt => {
                // A `Fin`-axis key exposes its position: the position is the
                // key's semantic content. Named and coordinate keys stay opaque.
                if let CheckedType::Key(index) = &arg_type {
                    if index.finite_index_form().is_some() {
                        return Ok(CheckedType::Int);
                    }
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::FiniteKey,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::ToIntFiniteKeys),
                        },
                    )
                    .into());
                }
                let dim =
                    expect_quantity(&arg_type, self.env.registry, self.env.src, args[0].span)?;
                if !dim.is_dimensionless() {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::Dimensionless,
                            )),
                            found: Box::new(MismatchOperand::Dimension(
                                self.env.registry.dimensions.dimension_spelling(&dim),
                            )),
                            help: Box::new(MismatchRule::ToIntDimensionless),
                        },
                    )
                    .into());
                }
                Ok(CheckedType::Int)
            }
            ConversionFn::Coord => {
                let CheckedType::Key(index) = &arg_type else {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::CoordinateKey,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::CoordExtractsCoordinate),
                        },
                    )
                    .into());
                };
                if index.finite_index_form().is_some() {
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::CoordinateKey,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::CoordCoordinateKeysOnly),
                        },
                    )
                    .into());
                }
                let index_def =
                    crate::tir::dim_check::infer::index_def_for_inferred(index, self.env.tir)
                        .ok_or_else(|| {
                            SemanticError::located(
                                self.env.src,
                                args[0].span,
                                IndexError::UnknownIndex {
                                    name: index.display_name(),
                                },
                            )
                        })?;
                index_def
                    .coordinate_dimension()
                    .map_or_else(
                        || {
                            Err(SemanticError::located(
                                self.env.src,
                                args[0].span,
                                DimensionError::DimensionMismatch {
                                    expected: Box::new(MismatchOperand::Expected(
                                        OperandExpectation::CoordinateKey,
                                    )),
                                    found: Box::new(MismatchOperand::Type(
                                        arg_type.spelling(&self.env.registry.dimensions),
                                    )),
                                    help: Box::new(MismatchRule::CoordCoordinateKeysOnly),
                                },
                            ))
                        },
                        |dimension| Ok(CheckedType::Quantity(dimension.clone())),
                    )
                    .map_err(Outcome::Failed)
            }
        }
    }

    pub(super) fn infer_hir_timescale_conversion(
        &self,
        name: BuiltinFn,
        scale: crate::semantic::time_scale::TimeScale,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let arg_type = self.infer_arg(&args[0])?;
        if !matches!(arg_type, CheckedType::Datetime(_)) {
            return Err(SemanticError::located(
                self.env.src,
                args[0].span,
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Expected(OperandExpectation::Datetime)),
                    found: Box::new(MismatchOperand::Type(
                        arg_type.spelling(&self.env.registry.dimensions),
                    )),
                    help: Box::new(MismatchRule::DatetimeArgument(name)),
                },
            )
            .into());
        }
        Ok(CheckedType::Datetime(scale))
    }

    pub(super) fn infer_hir_datetime_constructor(
        &self,
        kind: DatetimeConstructorFn,
        span: crate::syntax::span::Span,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        match kind {
            DatetimeConstructorFn::Datetime => {
                let first_is_valid = match args.len() {
                    1 => matches!(args[0].kind(), ExprKind::OffsetDateTimeLiteral(_)),
                    2 => matches!(args[0].kind(), ExprKind::ZonedDateTimeLiteral(_)),
                    _ => false,
                };
                if !first_is_valid {
                    let found = self.infer_arg(&args[0])?;
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::DatetimeLiteral,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                found.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::DatetimeStringLiteral),
                        },
                    )
                    .into());
                }
                if args.len() == 2 && !matches!(args[1].kind(), ExprKind::IanaTimeZoneLiteral(_)) {
                    let found = self.infer_arg(&args[1])?;
                    return Err(SemanticError::located(
                        self.env.src,
                        args[1].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::TimezoneLiteral,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                found.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::DatetimeTimezoneLiteral),
                        },
                    )
                    .into());
                }
                let resolved_timezone_matches_argument = match args {
                    [datetime, time_zone] => match (datetime.kind(), time_zone.kind()) {
                        (
                            ExprKind::ZonedDateTimeLiteral(datetime),
                            ExprKind::IanaTimeZoneLiteral(time_zone),
                        ) => datetime.time_zone() == time_zone,
                        _ => true,
                    },
                    _ => true,
                };
                if !resolved_timezone_matches_argument {
                    return Err(SemanticError::internal_error(
                        "resolved datetime timezone does not match its source argument".to_string(),
                        self.env.src,
                        crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
                    )
                    .into());
                }
                self.record_contextual_args(args)?;
                Ok(CheckedType::Datetime(
                    crate::semantic::time_scale::TimeScale::UTC,
                ))
            }
            DatetimeConstructorFn::Epoch => {
                let ExprKind::EpochLiteral(literal) = args[0].kind() else {
                    let found = self.infer_arg(&args[0])?;
                    return Err(SemanticError::located(
                        self.env.src,
                        args[0].span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::ScaleFreeDatetimeLiteral,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                found.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::EpochCivilLiteral),
                        },
                    )
                    .into());
                };
                self.record_contextual_args(args)?;
                Ok(CheckedType::Datetime(literal.scale()))
            }
        }
    }

    /// Record the contextual literal arguments a datetime constructor accepted.
    fn record_contextual_args(&self, args: &[Expr]) -> Result<(), SemanticError> {
        args.iter().try_for_each(|arg| {
            self.control
                .observations()
                .record_contextual(arg, self.env.src)
        })
    }

    pub(super) fn infer_hir_datetime_unary(
        &self,
        name: BuiltinFn,
        args: &[Expr],
        result: CheckedType<Symbolic>,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let arg_type = self.infer_arg(&args[0])?;
        if !matches!(arg_type, CheckedType::Datetime(_)) {
            return Err(SemanticError::located(
                self.env.src,
                args[0].span,
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Expected(OperandExpectation::Datetime)),
                    found: Box::new(MismatchOperand::Type(
                        arg_type.spelling(&self.env.registry.dimensions),
                    )),
                    help: Box::new(MismatchRule::DatetimeArgument(name)),
                },
            )
            .into());
        }
        Ok(result)
    }
}
