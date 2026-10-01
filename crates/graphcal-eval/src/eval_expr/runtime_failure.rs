//! Why evaluating a checked expression failed at runtime.
//!
//! Every runtime failure the expression kernel reports is a typed
//! [`RuntimeFailure`]. It reaches a diagnostic as
//! [`EvaluationError::Runtime`](graphcal_compiler::semantic_error::evaluation::EvaluationError::Runtime),
//! which keeps the typed failure and renders it only where the diagnostic is
//! displayed.

use graphcal_compiler::builtin::{DatetimeFromNumericFn, ScalarFn};
use graphcal_compiler::hir::expr::ExternFnRef;
use graphcal_compiler::ratio::ExactPowerError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::checked_type::IndexTypeRef;
use graphcal_compiler::semantic::scalar_function::BuiltinEvalError;
use graphcal_compiler::semantic::unit_scale::PositiveFiniteScaleError;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};
use graphcal_compiler::tir::texpr::CoordinateSearch;
use thiserror::Error;

use super::aggregations::AggregationError;
use super::complex::ComplexEvalError;
use super::conversions::ExactIntConversionError;
use super::datetime::DatetimeRangeError;
use super::linear_algebra_error::LinearAlgebraError;
use super::numeric::QuantityValidationError;
use crate::domain_check::DomainViolation;
use crate::host_abi::marshal::{ArgumentError, ResultError};
use crate::host_fns::HostFnError;

/// Why evaluating a checked expression failed at runtime.
#[derive(Debug, Error)]
pub(super) enum RuntimeFailure {
    #[error(transparent)]
    Quantity(#[from] QuantityValidationError),
    #[error(transparent)]
    Integer(#[from] IntegerFailure),
    #[error("division by zero")]
    QuantityDivisionByZero,
    #[error(transparent)]
    Power(#[from] ExactPowerError),
    #[error(transparent)]
    Complex(#[from] ComplexEvalError),
    #[error(transparent)]
    Datetime(#[from] DatetimeRangeError),
    #[error("{}() integer argument {} is too large for exact conversion", .0.as_str(), .1)]
    DatetimeIntegerTooLarge(DatetimeFromNumericFn, i64),
    #[error("to_int() argument {}{}", .0, rounding_help(.0))]
    ToInt(ExactIntConversionError),
    #[error("builtin function `{function}` {error}")]
    ScalarFunction {
        function: ScalarFn,
        error: BuiltinEvalError,
    },
    #[error("{context} {error}")]
    UnitScale {
        context: UnitScaleContext,
        error: PositiveFiniteScaleError,
    },
    #[error(transparent)]
    Aggregation(#[from] AggregationError),
    #[error(transparent)]
    LinearAlgebra(#[from] LinearAlgebraError),
    #[error(transparent)]
    Unbound(#[from] UnboundReference),
    #[error("fin_key: {position} out of bounds for {axis}")]
    FinKeyOutOfBounds { position: i64, axis: IndexTypeRef },
    #[error("{}: no coordinate of `{axis}` is {} the target", search.kind().as_str(), relation(*search))]
    NoCoordinate {
        search: CoordinateSearch,
        axis: IndexTypeRef,
    },
    #[error("unfold requires a coordinate index, but `{0}` is not coordinate-valued")]
    UnfoldWithoutCoordinates(IndexTypeRef),
    #[error("field `{constructor}.{field}` {violation}")]
    FieldConstraint {
        constructor: ConstructorName,
        field: FieldName,
        violation: DomainViolation,
    },
    #[error(transparent)]
    Extern(#[from] ExternFailure),
    #[error(transparent)]
    InlineAssertion(#[from] InlineAssertionFailure),
}

impl From<std::convert::Infallible> for RuntimeFailure {
    fn from(never: std::convert::Infallible) -> Self {
        match never {}
    }
}

/// An `Int` operation without a representable result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(super) enum IntegerFailure {
    #[error("integer division by zero")]
    DivisionByZero,
    #[error("integer modulo by zero")]
    ModuloByZero,
    #[error("integer arithmetic overflow")]
    Overflow,
    #[error("integer negation overflow")]
    NegationOverflow,
    #[error("integer exponent must be non-negative")]
    NegativeExponent,
    #[error("integer exponent too large")]
    ExponentTooLarge,
}

/// The unit-scale computation a non-finite scale came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnitScaleContext {
    DynamicUnit,
    Exponentiation,
    Compound,
}

impl std::fmt::Display for UnitScaleContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DynamicUnit => "dynamic unit scale",
            Self::Exponentiation => "unit scale exponentiation",
            Self::Compound => "compound unit scale",
        })
    }
}

/// A reference whose value is not bound where it is read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(super) enum UnboundReference {
    #[error("undefined constant `{0}`")]
    Constant(ResolvedDeclName),
    #[error("undefined local variable")]
    Local,
    #[error("undefined graph reference `@{0}`")]
    GraphRef(ResolvedDeclName),
}

/// Why a plugin function call failed.
#[derive(Debug, Error)]
pub(super) enum ExternFailure {
    #[error(
        "extern function `{0}` cannot be evaluated in this context (no host function registry)"
    )]
    NoHost(ExternFnRef),
    #[error("extern function `{}` (plugin \"{}\") is not provided by the host", .0.name, .0.plugin)]
    NotProvided(ExternFnRef),
    #[error("{}", .error.describe(.function))]
    Argument {
        function: ExternFnRef,
        error: ArgumentError,
    },
    #[error("extern function `{function}` (plugin \"{}\") failed: {error}", .function.plugin)]
    Host {
        function: ExternFnRef,
        error: HostFnError,
    },
    #[error("{}", .error.describe(.function))]
    Result {
        function: ExternFnRef,
        error: ResultError,
    },
}

/// An assertion of a DAG called inline that did not pass.
#[derive(Debug, Error)]
pub(super) enum InlineAssertionFailure {
    #[error("assertion `{assertion}` failed in inline call of dag `{dag}` ({message})")]
    Failed {
        assertion: graphcal_compiler::syntax::decl_name::DeclName,
        dag: graphcal_compiler::dag_id::DagSegment,
        message: String,
    },
    #[error("assertion `{assertion}` errored in inline call of dag `{dag}` ({message})")]
    Errored {
        assertion: graphcal_compiler::syntax::decl_name::DeclName,
        dag: graphcal_compiler::dag_id::DagSegment,
        message: String,
    },
}

/// The advice an inexact `to_int()` argument gets.
const fn rounding_help(error: &ExactIntConversionError) -> &'static str {
    match error {
        ExactIntConversionError::NonInteger { .. } => {
            "; apply trunc(), floor(), ceil(), or round() explicitly before to_int()"
        }
        ExactIntConversionError::NonFinite { .. } | ExactIntConversionError::OutOfRange { .. } => {
            ""
        }
    }
}

/// How a coordinate search's target relates to the coordinate it selects.
const fn relation(search: CoordinateSearch) -> &'static str {
    match search {
        CoordinateSearch::Floor => "at or below",
        CoordinateSearch::Ceil | CoordinateSearch::Nearest => "at or above",
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::tir::texpr::CoordinateSearch;

    use super::super::conversions::ExactIntConversionError;
    use super::{IntegerFailure, RuntimeFailure, UnitScaleContext, relation, rounding_help};

    #[test]
    fn failures_render_their_established_messages() {
        assert_eq!(
            RuntimeFailure::ToInt(ExactIntConversionError::NonInteger { value: 1.5 }).to_string(),
            "to_int() argument is not integer-valued (got 1.5); apply trunc(), floor(), ceil(), \
             or round() explicitly before to_int()"
        );
        assert_eq!(
            rounding_help(&ExactIntConversionError::NonFinite { value: f64::NAN }),
            ""
        );
        assert_eq!(relation(CoordinateSearch::Floor), "at or below");
        assert_eq!(relation(CoordinateSearch::Ceil), "at or above");
        assert_eq!(
            RuntimeFailure::from(IntegerFailure::ModuloByZero).to_string(),
            "integer modulo by zero"
        );
        assert_eq!(
            UnitScaleContext::Compound.to_string(),
            "compound unit scale"
        );
    }
}
