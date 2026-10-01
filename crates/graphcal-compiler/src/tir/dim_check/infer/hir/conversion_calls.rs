//! Inference of the type-conversion, time-scale, and datetime built-in calls.

use crate::builtin::{BuiltinFn, ConversionFn, DatetimeConstructorFn};
use crate::dimension::Dimension;
use crate::graphcal_error::GraphcalError;
use crate::hir::expr::{Expr, ExprKind};
use crate::outcome::Outcome;

use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::tir::dim_check::helpers::{expect_quantity, format_checked_type};

use super::context::Infer;

impl Infer<'_> {
    pub(super) fn infer_hir_type_conversion(
        &self,
        kind: ConversionFn,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let arg_type = self.infer_arg(&args[0])?;
        match kind {
            ConversionFn::ToFloat => {
                if arg_type != CheckedType::Int {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format_checked_type(&arg_type, self.env.registry),
                        help: "to_float() requires an Int argument".to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
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
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Key<Fin(N)>".to_string(),
                        found: format_checked_type(&arg_type, self.env.registry),
                        help: "to_int() extracts positions from Fin-axis keys only; \
                           named and coordinate keys have no ordinal"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
                    .into());
                }
                let dim =
                    expect_quantity(&arg_type, self.env.registry, self.env.src, args[0].span)?;
                if !dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: self.env.registry.dimensions.format_dimension(&dim),
                        help: "to_int() requires a Dimensionless argument".to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
                    .into());
                }
                Ok(CheckedType::Int)
            }
            ConversionFn::Coord => {
                let CheckedType::Key(index) = &arg_type else {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Key<C> for a coordinate axis C".to_string(),
                        found: format_checked_type(&arg_type, self.env.registry),
                        help: "coord() extracts the coordinate quantity of a \
                           coordinate-axis key"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
                    .into());
                };
                if index.finite_index_form().is_some() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Key<C> for a coordinate axis C".to_string(),
                        found: format_checked_type(&arg_type, self.env.registry),
                        help: "coord() applies to coordinate-axis keys only; named \
                           keys are opaque and Fin keys expose to_int()"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
                    .into());
                }
                let index_def =
                    crate::tir::dim_check::infer::index_def_for_inferred(index, self.env.tir)
                        .ok_or_else(|| GraphcalError::UnknownIndex {
                            name: index.display_name(),
                            src: self.env.src.clone(),
                            span: args[0].span.into(),
                        })?;
                index_def
                    .coordinate_dimension()
                    .map_or_else(
                        || {
                            Err(GraphcalError::DimensionMismatch {
                                expected: "Key<C> for a coordinate axis C".to_string(),
                                found: format_checked_type(&arg_type, self.env.registry),
                                help: "coord() applies to coordinate-axis keys only; named \
                               keys are opaque and Fin keys expose to_int()"
                                    .to_string(),
                                src: self.env.src.clone(),
                                span: args[0].span.into(),
                            })
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
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let arg_type = self.infer_arg(&args[0])?;
        if !matches!(arg_type, CheckedType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_checked_type(&arg_type, self.env.registry),
                help: format!("{}() requires a Datetime argument", name.as_str()),
                src: self.env.src.clone(),
                span: args[0].span.into(),
            }
            .into());
        }
        Ok(CheckedType::Datetime(scale))
    }

    pub(super) fn infer_hir_datetime_constructor(
        &self,
        kind: DatetimeConstructorFn,
        epoch_scale: Option<crate::semantic::time_scale::TimeScale>,
        span: crate::syntax::span::Span,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        match kind {
            DatetimeConstructorFn::Datetime => {
                let first_is_valid = match args.len() {
                    1 => matches!(args[0].kind(), ExprKind::OffsetDateTimeLiteral(_)),
                    2 => matches!(args[0].kind(), ExprKind::ZonedDateTimeLiteral(_)),
                    _ => false,
                };
                if !first_is_valid {
                    let found = self.infer_arg(&args[0])?;
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "datetime literal".to_string(),
                        found: format_checked_type(&found, self.env.registry),
                        help: "datetime() requires a contextual datetime string literal"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
                    .into());
                }
                if args.len() == 2 && !matches!(args[1].kind(), ExprKind::IanaTimeZoneLiteral(_)) {
                    let found = self.infer_arg(&args[1])?;
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "timezone literal".to_string(),
                        found: format_checked_type(&found, self.env.registry),
                        help: "datetime() second argument must be an IANA timezone literal"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: args[1].span.into(),
                    }
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
                    return Err(GraphcalError::InternalError {
                        message: "resolved datetime timezone does not match its source argument"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: span.into(),
                    }
                    .into());
                }
                self.record_contextual_args(args)?;
                Ok(CheckedType::Datetime(
                    crate::semantic::time_scale::TimeScale::UTC,
                ))
            }
            DatetimeConstructorFn::Epoch => {
                if !matches!(args[0].kind(), ExprKind::CivilDateTimeLiteral(_)) {
                    let found = self.infer_arg(&args[0])?;
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "scale-free datetime literal".to_string(),
                        found: format_checked_type(&found, self.env.registry),
                        help: "epoch<S>() requires one civil datetime string literal".to_string(),
                        src: self.env.src.clone(),
                        span: args[0].span.into(),
                    }
                    .into());
                }
                self.record_contextual_args(args)?;
                epoch_scale
                    .map(CheckedType::Datetime)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: "epoch call reached type inference without a static time scale"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: span.into(),
                    })
                    .map_err(Outcome::Failed)
            }
        }
    }

    /// Record the contextual literal arguments a datetime constructor accepted.
    fn record_contextual_args(&self, args: &[Expr]) -> Result<(), GraphcalError> {
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
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let arg_type = self.infer_arg(&args[0])?;
        if !matches!(arg_type, CheckedType::Datetime(_)) {
            return Err(GraphcalError::DimensionMismatch {
                expected: "Datetime".to_string(),
                found: format_checked_type(&arg_type, self.env.registry),
                help: format!("{}() requires a Datetime argument", name.as_str()),
                src: self.env.src.clone(),
                span: args[0].span.into(),
            }
            .into());
        }
        Ok(result)
    }
}
