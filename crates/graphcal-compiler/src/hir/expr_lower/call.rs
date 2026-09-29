//! Function-call application and constructor-context datetime literals.

use crate::builtin::{BuiltinApplication, BuiltinFn, ComplexFn, DatetimeConstructorFn, DatetimeFn};
use crate::datetime_literal::{
    CivilDateTimeLiteral, DatetimeLiteralExpectation, OffsetDateTimeLiteral,
    ResolveZonedDateTimeLiteralError, ZonedDateTimeLiteral,
};
use crate::desugar::desugared_ast as ast;
use crate::registry::time_scale::TimeScale;
use crate::registry::time_zone::IanaTimeZoneId;
use crate::syntax::span::{Span, Spanned};

use super::error::ExprLowerError;
use super::lowerer::ExprLowerer;
use super::tolerant::Tolerant;
use crate::hir::expr::{Expr, ExprKind, FunctionRef, UnappliedFunctionRef};

/// Arity that HIR lowering validates before lowering the arguments.
///
/// Scalar kernels, and the real overloads `abs`/`exp` that share their early
/// check, are validated here. Every other built-in reports its arity from its
/// type rule, after argument inference.
pub(super) const fn lowering_arity(function: BuiltinFn) -> Option<usize> {
    match function {
        BuiltinFn::Scalar(function) => Some(function.arity()),
        BuiltinFn::Complex(function @ (ComplexFn::Absolute | ComplexFn::Exponential)) => {
            Some(function.arity())
        }
        BuiltinFn::Complex(_)
        | BuiltinFn::Aggregation(_)
        | BuiltinFn::LinearAlgebra(_)
        | BuiltinFn::Datetime(_)
        | BuiltinFn::Conversion(_) => None,
    }
}

/// `datetime(...)`, whose string arguments lower to datetime and timezone literals.
pub(super) const DATETIME: BuiltinFn =
    BuiltinFn::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Datetime));

impl ExprLowerer<'_> {
    pub(super) fn lower_function_application(
        function_ref: UnappliedFunctionRef,
        generic_args: &[ast::GenericArg],
        path: String,
        callee_span: Span,
    ) -> Result<FunctionRef, ExprLowerError> {
        let applied = match function_ref {
            UnappliedFunctionRef::Builtin(name) => match name.application() {
                BuiltinApplication::Epoch => {
                    return Self::lower_epoch_function_ref(generic_args, callee_span);
                }
                BuiltinApplication::ScaleFree(builtin) => FunctionRef::Builtin(builtin),
            },
            UnappliedFunctionRef::External(extern_ref) => FunctionRef::External(extern_ref),
        };
        match generic_args {
            [] => Ok(applied),
            _ => Err(ExprLowerError::UnsupportedFunctionGenericArgs {
                path,
                span: generic_args
                    .first()
                    .map_or(callee_span, ast::GenericArg::span),
            }),
        }
    }

    pub(super) fn lower_epoch_function_ref(
        generic_args: &[ast::GenericArg],
        callee_span: Span,
    ) -> Result<FunctionRef, ExprLowerError> {
        let [arg] = generic_args else {
            return Err(ExprLowerError::EpochTimeScaleArgumentCount {
                got: generic_args.len(),
                span: generic_args
                    .first()
                    .map_or(callee_span, ast::GenericArg::span),
            });
        };
        let ast::GenericArg::Ambiguous(ast::AmbiguousGenericArg::Name(ident)) = arg else {
            return Err(ExprLowerError::InvalidEpochTimeScaleArgument { span: arg.span() });
        };
        let scale = ident.name.as_str().parse::<TimeScale>().map_err(|_| {
            ExprLowerError::UnsupportedEpochTimeScale {
                name: ident.name.atom().clone(),
                span: ident.span,
            }
        })?;
        Ok(FunctionRef::Epoch {
            scale: Spanned::new(scale, ident.span),
        })
    }

    /// Lower constructor-context strings into parsed semantic datetime and
    /// timezone literals. Ordinary strings remain syntax-only HIR leaves.
    pub(super) fn lower_function_args(
        &mut self,
        function_ref: &FunctionRef,
        args: &[ast::Expr],
    ) -> Result<Vec<Expr<Tolerant>>, ExprLowerError> {
        match (function_ref, args) {
            (FunctionRef::Builtin(builtin), [datetime, time_zone])
                if builtin.function() == DATETIME =>
            {
                self.lower_zoned_datetime_args(datetime, time_zone)
            }
            _ => args
                .iter()
                .enumerate()
                .map(
                    |(index, arg)| match (function_ref, index, args.len(), &arg.kind) {
                        (
                            FunctionRef::Builtin(builtin),
                            0,
                            1,
                            ast::ExprKind::StringLiteral(source),
                        ) if builtin.function() == DATETIME => {
                            Self::lower_offset_datetime_literal(source, arg.span)
                        }
                        (
                            FunctionRef::Epoch { scale },
                            0,
                            1,
                            ast::ExprKind::StringLiteral(source),
                        ) => Self::lower_civil_datetime_literal(
                            source,
                            DatetimeLiteralExpectation::Epoch(scale.value),
                            arg.span,
                        ),
                        _ => Ok(self.lower_expr(arg)),
                    },
                )
                .collect(),
        }
    }

    pub(super) fn lower_zoned_datetime_args(
        &mut self,
        datetime_arg: &ast::Expr,
        time_zone_arg: &ast::Expr,
    ) -> Result<Vec<Expr<Tolerant>>, ExprLowerError> {
        let datetime = match &datetime_arg.kind {
            ast::ExprKind::StringLiteral(source) => Self::lower_civil_datetime_literal(
                source,
                DatetimeLiteralExpectation::ZonedCivilDateTime,
                datetime_arg.span,
            )?,
            _ => self.lower_expr(datetime_arg),
        };
        let time_zone = match &time_zone_arg.kind {
            ast::ExprKind::StringLiteral(source) => self
                .lower_iana_time_zone_id(source, time_zone_arg.span)
                .map(|time_zone| {
                    Expr::new(ExprKind::IanaTimeZoneLiteral(time_zone), time_zone_arg.span)
                })?,
            _ => self.lower_expr(time_zone_arg),
        };

        let resolution_inputs = match (datetime.kind(), time_zone.kind()) {
            (
                ExprKind::CivilDateTimeLiteral(datetime),
                ExprKind::IanaTimeZoneLiteral(time_zone),
            ) => Some((*datetime, time_zone.clone())),
            _ => None,
        };
        let datetime = match resolution_inputs {
            Some((datetime, time_zone_id)) => {
                ZonedDateTimeLiteral::resolve(datetime, time_zone_id, self.ctx.time_zones)
                    .map(|resolved| {
                        Expr::new(ExprKind::ZonedDateTimeLiteral(resolved), datetime_arg.span)
                    })
                    .map_err(|error| {
                        Self::lower_zoned_datetime_error(
                            error,
                            datetime_arg.span,
                            time_zone_arg.span,
                        )
                    })?
            }
            None => datetime,
        };
        Ok(vec![datetime, time_zone])
    }

    pub(super) fn lower_zoned_datetime_error(
        error: ResolveZonedDateTimeLiteralError,
        datetime_span: Span,
        time_zone_span: Span,
    ) -> ExprLowerError {
        match error {
            ResolveZonedDateTimeLiteralError::Nonexistent {
                datetime,
                time_zone,
                before,
                after,
            } => ExprLowerError::NonexistentCivilDateTime {
                datetime,
                time_zone,
                before,
                after,
                datetime_span,
                time_zone_span,
            },
            ResolveZonedDateTimeLiteralError::Repeated {
                datetime,
                time_zone,
                before,
                after,
            } => ExprLowerError::RepeatedCivilDateTime {
                datetime,
                time_zone,
                before,
                after,
                datetime_span,
                time_zone_span,
            },
            ResolveZonedDateTimeLiteralError::TimeZoneRegistryInvariant { time_zone, source } => {
                ExprLowerError::TimeZoneRegistryInvariant {
                    time_zone,
                    reason: source.to_string(),
                    span: time_zone_span,
                }
            }
            error @ ResolveZonedDateTimeLiteralError::OutOfRange { .. } => {
                ExprLowerError::InvalidDatetimeLiteral {
                    expectation: DatetimeLiteralExpectation::ZonedCivilDateTime,
                    reason: error.to_string(),
                    span: datetime_span,
                }
            }
        }
    }

    pub(super) fn lower_offset_datetime_literal(
        source: &str,
        span: Span,
    ) -> Result<Expr<Tolerant>, ExprLowerError> {
        OffsetDateTimeLiteral::parse(source)
            .map(|literal| Expr::new(ExprKind::OffsetDateTimeLiteral(literal), span))
            .map_err(|error| ExprLowerError::InvalidDatetimeLiteral {
                expectation: DatetimeLiteralExpectation::OffsetDateTime,
                reason: error.to_string(),
                span,
            })
    }

    pub(super) fn lower_civil_datetime_literal(
        source: &str,
        expectation: DatetimeLiteralExpectation,
        span: Span,
    ) -> Result<Expr<Tolerant>, ExprLowerError> {
        CivilDateTimeLiteral::parse(source)
            .map(|literal| Expr::new(ExprKind::CivilDateTimeLiteral(literal), span))
            .map_err(|error| ExprLowerError::InvalidDatetimeLiteral {
                expectation,
                reason: error.to_string(),
                span,
            })
    }

    pub(super) fn lower_iana_time_zone_id(
        &self,
        timezone: &str,
        span: Span,
    ) -> Result<IanaTimeZoneId, ExprLowerError> {
        self.ctx
            .time_zones
            .parse_iana_id(timezone)
            .map_err(|_| ExprLowerError::InvalidTimezone {
                timezone: timezone.to_string(),
                tzdb_version: self.ctx.time_zones.version().unwrap_or("unknown"),
                span,
            })
    }

    /// Validate a built-in call's argument count when lowering owns the check
    /// (see [`lowering_arity`]). Other built-ins and externs defer shape
    /// checks to their typed rules.
    pub(super) const fn check_function_arity(
        function_ref: &FunctionRef,
        got: usize,
        span: Span,
    ) -> Result<(), ExprLowerError> {
        let Some(builtin) = function_ref.builtin() else {
            return Ok(());
        };
        let Some(expected) = lowering_arity(builtin) else {
            return Ok(());
        };
        if got != expected {
            return Err(ExprLowerError::WrongArity {
                name: builtin,
                expected,
                got,
                span,
            });
        }
        Ok(())
    }
}
