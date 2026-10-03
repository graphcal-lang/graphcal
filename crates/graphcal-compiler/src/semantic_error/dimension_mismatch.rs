//! Typed payloads of [`DimensionError::DimensionMismatch`]: what an operand
//! was expected to be, what it was, and why the rule requires it.
//!
//! [`DimensionError::DimensionMismatch`]: super::dimension::DimensionError::DimensionMismatch

use crate::builtin::{BuiltinFn, ComplexFn, LinearAlgebraFn};
use crate::function_signature::IndexBinder;
use crate::hir::expr::ExternFnRef;
use crate::semantic::checked_type::{IndexTypeRef, Symbolic, TypeSpelling};
use crate::semantic::dimension_table::DimensionSpelling;
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::ast::KeyFormKind;
use crate::syntax::function_name::FnParamName;

/// One side of a dimension mismatch: a checked type, a dimension, or an
/// expectation that names a family of operands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MismatchOperand {
    Type(TypeSpelling),
    Dimension(DimensionSpelling),
    Expected(OperandExpectation),
    /// A contextual string literal outside its declared context.
    ContextualStringLiteral,
    /// A statically known Int exponent.
    IntExponent(i64),
    /// The operand types of a `%` expression.
    ModuloOperands {
        lhs: TypeSpelling,
        rhs: TypeSpelling,
    },
}

impl std::fmt::Display for MismatchOperand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Type(ty) => ty.fmt(f),
            Self::Dimension(dimension) => dimension.fmt(f),
            Self::Expected(expectation) => expectation.fmt(f),
            Self::ContextualStringLiteral => f.write_str("contextual string literal"),
            Self::IntExponent(value) => value.fmt(f),
            Self::ModuloOperands { lhs, rhs } => write!(f, "{lhs} % {rhs}"),
        }
    }
}

impl MismatchOperand {
    /// The label of the operand found where a mismatch was detected.
    #[must_use]
    pub fn found_label(&self) -> String {
        match self {
            Self::Type(ty) => format!("has type {ty}"),
            Self::Dimension(dimension) => format!("has dimension {dimension}"),
            Self::Expected(expectation) => format!("is {expectation}"),
            Self::ContextualStringLiteral => "contextual string literal".to_owned(),
            Self::IntExponent(value) => format!("exponent is {value}"),
            Self::ModuloOperands { lhs, rhs } => format!("operands have types {lhs} % {rhs}"),
        }
    }
}

/// A scalar element kind of an extern parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternScalar {
    Bool,
    Int,
}

impl std::fmt::Display for ExternScalar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bool => "Bool",
            Self::Int => "Int",
        })
    }
}

/// A family of operands a rule expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperandExpectation {
    QuantityType,
    RankIndexedQuantity {
        rank: usize,
    },
    IndexedCollection,
    IndexedQuantityCollection,
    DimensionlessOrInt,
    ComplexQuantity,
    RealOrComplexQuantity,
    Angle,
    DimensionlessOrComplexDimensionless,
    Int,
    FiniteKey,
    Dimensionless,
    CoordinateKey,
    Datetime,
    DatetimeLiteral,
    TimezoneLiteral,
    ScaleFreeDatetimeLiteral,
    NumericOrBooleanExpression,
    Bool,
    ExternIndexedCollection {
        rank: usize,
    },
    ExternIndexedQuantityCollection {
        rank: usize,
    },
    ExternScalarAxes {
        scalar: ExternScalar,
        rank: usize,
    },
    ExternSharedAxis {
        bound: IndexTypeRef<Symbolic>,
        variable: IndexBinder,
    },
    StaticNatPosition,
    StaticNatConstant,
    OrderedQuantity,
    TimeQuantity,
    Time,
    NonNegativeIntExponent,
    NonNegativeExactIntExponent,
    DimensionlessExponent,
    IntOrQuantity,
}

impl std::fmt::Display for OperandExpectation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuantityType => f.write_str("quantity type"),
            Self::RankIndexedQuantity { rank } => write!(f, "rank-{rank} indexed quantity"),
            Self::IndexedCollection => f.write_str("indexed collection"),
            Self::IndexedQuantityCollection => f.write_str("indexed quantity collection"),
            Self::DimensionlessOrInt => f.write_str("Dimensionless or Int"),
            Self::ComplexQuantity => f.write_str("Complex<D>"),
            Self::RealOrComplexQuantity => f.write_str("a real or complex quantity"),
            Self::Angle => f.write_str("Angle"),
            Self::DimensionlessOrComplexDimensionless => {
                f.write_str("Dimensionless or Complex<Dimensionless>")
            }
            Self::Int => f.write_str("Int"),
            Self::FiniteKey => f.write_str("Key<Fin(N)>"),
            Self::Dimensionless => f.write_str("Dimensionless"),
            Self::CoordinateKey => f.write_str("Key<C> for a coordinate axis C"),
            Self::Datetime => f.write_str("Datetime"),
            Self::DatetimeLiteral => f.write_str("datetime literal"),
            Self::TimezoneLiteral => f.write_str("timezone literal"),
            Self::ScaleFreeDatetimeLiteral => f.write_str("scale-free datetime literal"),
            Self::NumericOrBooleanExpression => f.write_str("a numeric or boolean expression"),
            Self::Bool => f.write_str("Bool"),
            Self::ExternIndexedCollection { rank } => {
                write!(f, "a rank-{rank} indexed collection")
            }
            Self::ExternIndexedQuantityCollection { rank } => {
                write!(f, "a rank-{rank} indexed quantity collection")
            }
            Self::ExternScalarAxes { scalar, rank } => {
                write!(f, "{scalar} with exactly {rank} indexed axes")
            }
            Self::ExternSharedAxis { bound, variable } => write!(
                f,
                "an axis over `{bound}` (index variable `{variable}` was bound by an earlier argument)"
            ),
            Self::StaticNatPosition => f.write_str("a static Nat position"),
            Self::StaticNatConstant => f.write_str("a static Nat constant"),
            Self::OrderedQuantity => f.write_str("an ordered real quantity, integer, or datetime"),
            Self::TimeQuantity => f.write_str("Quantity(Time)"),
            Self::Time => f.write_str("Time"),
            Self::NonNegativeIntExponent => f.write_str("non-negative Int exponent"),
            Self::NonNegativeExactIntExponent => f.write_str("non-negative exact Int exponent"),
            Self::DimensionlessExponent => f.write_str("Dimensionless exponent"),
            Self::IntOrQuantity => f.write_str("Int or Quantity"),
        }
    }
}

/// The non-quantity value family an operand that must be a quantity has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonQuantityValue {
    Complex,
    Bool,
    Int,
    Datetime,
    Key,
    Struct,
    Indexed,
}

impl std::fmt::Display for NonQuantityValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Complex => "a Complex value",
            Self::Bool => "a Bool value",
            Self::Int => "an Int value",
            Self::Datetime => "a Datetime value",
            Self::Key => "an index-key value",
            Self::Struct => "a struct",
            Self::Indexed => "an indexed value",
        })
    }
}

/// Why `k + c` Fin-key arithmetic rejected its right operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinKeyArithmeticRule {
    NamedOrCoordinateKey,
    Subtraction,
    IntegerAddend,
    RuntimeOffset,
    NegativeAddend,
}

impl std::fmt::Display for FinKeyArithmeticRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NamedOrCoordinateKey => {
                "named and coordinate keys have no arithmetic; only Fin-axis keys carry the additive fragment `k + c`"
            }
            Self::Subtraction => {
                "key subtraction is fallible at 0 and excluded; restructure additively or use to_int() and fin_key()"
            }
            Self::IntegerAddend => "`k + c` takes an integer constant addend",
            Self::RuntimeOffset => {
                "a runtime offset escapes any static bound; use `to_int(k) + e` and re-enter with fin_key()"
            }
            Self::NegativeAddend => "`k + c` takes a non-negative static constant",
        })
    }
}

/// The rule a dimension mismatch violates, rendered as the diagnostic help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MismatchRule {
    ParameterSameDimension {
        parameter: FnParamName,
        bound_by: FnParamName,
    },
    ParameterDimension {
        parameter: FnParamName,
        dimension: DimensionSpelling,
    },
    ExpectedQuantity(NonQuantityValue),
    LinearAlgebraArgument {
        function: LinearAlgebraFn,
        argument: usize,
        rank: usize,
    },
    IndexedArgument(BuiltinFn),
    QuantityElements(BuiltinFn),
    DimensionlessNumericArgument(BuiltinFn),
    ComplexQuantityArgument {
        function: ComplexFn,
        argument: usize,
    },
    ComplexArgument(ComplexFn),
    RealOrComplexArgument(ComplexFn),
    ComplexComponentsSameDimension,
    PolarPhaseAngle,
    ExpDimensionless,
    ToFloatInt,
    ToIntFiniteKeys,
    ToIntDimensionless,
    CoordExtractsCoordinate,
    CoordCoordinateKeysOnly,
    DatetimeArgument(BuiltinFn),
    DatetimeStringLiteral,
    DatetimeTimezoneLiteral,
    EpochCivilLiteral,
    StringLiteralContext,
    ExternScalarParameter {
        parameter: FnParamName,
        scalar: ExternScalar,
    },
    ExternAxisPerIndexVariable {
        parameter: FnParamName,
        function: ExternFnRef,
    },
    ExternQuantityElements {
        parameter: FnParamName,
        function: ExternFnRef,
    },
    ExternScalarElements {
        parameter: FnParamName,
        function: ExternFnRef,
        scalar: ExternScalar,
    },
    ExternSharedIndexVariable {
        variable: IndexBinder,
        function: ExternFnRef,
    },
    KeyStaticPosition,
    FinKeyIntPosition,
    CoordinateSearchAxisDimension(KeyFormKind),
    TimezoneDisplayDatetime(IanaTimeZoneId),
    ScanBody,
    UnfoldBody,
    FinKeyArithmetic(FinKeyArithmeticRule),
    BooleanOperands,
    EqualitySameType,
    ComplexUnordered,
    ComparisonSameType,
    DatetimeComparisonScales,
    ComparisonSameDimension,
    FinKeyArithmeticKeyFirst,
    ComplexAdditionSameDimension,
    NoImplicitComplexPromotion,
    DatetimeSubtractionScales,
    DatetimeAddition,
    DurationAddSubtract,
    DurationAdd,
    DatetimeFromQuantity,
    AdditionSameDimension,
    ModuloInt,
    NonNegativeExponent,
    ExactIntegerExponent,
    DimensionlessExponent,
    LogicalNot,
    Negation,
    IfConditionBool,
    IfBranchesSameDimension,
    MatchArmsSameType,
    ToleranceSameDimension,
    AbsoluteToleranceDimension,
}

impl std::fmt::Display for MismatchRule {
    #[expect(
        clippy::too_many_lines,
        reason = "one closed rule list renders every dimension-mismatch help"
    )]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParameterSameDimension {
                parameter,
                bound_by,
            } => write!(
                f,
                "parameter `{parameter}` must have the same dimension as `{bound_by}`"
            ),
            Self::ParameterDimension {
                parameter,
                dimension,
            } => write!(f, "parameter `{parameter}` requires {dimension}"),
            Self::ExpectedQuantity(found) => write!(f, "expected a quantity value, not {found}"),
            Self::LinearAlgebraArgument {
                function,
                argument,
                rank,
            } => write!(
                f,
                "{}() requires argument {argument} to be a rank-{rank} indexed quantity",
                function.as_str()
            ),
            Self::IndexedArgument(builtin) => {
                write!(f, "{}() requires an indexed value", builtin.as_str())
            }
            Self::QuantityElements(builtin) => write!(
                f,
                "{}() requires every indexed element to be quantity",
                builtin.as_str()
            ),
            Self::DimensionlessNumericArgument(builtin) => write!(
                f,
                "{}() requires a dimensionless numeric argument",
                builtin.as_str()
            ),
            Self::ComplexQuantityArgument { function, argument } => write!(
                f,
                "{}() requires a quantity in argument {argument}",
                function.as_str()
            ),
            Self::ComplexArgument(function) => {
                write!(f, "{}() requires a complex quantity", function.as_str())
            }
            Self::RealOrComplexArgument(function) => write!(
                f,
                "{}() requires a real or complex quantity",
                function.as_str()
            ),
            Self::ComplexComponentsSameDimension => {
                f.write_str("real and imaginary components must have the same dimension")
            }
            Self::PolarPhaseAngle => f.write_str("polar() phase must be an Angle quantity"),
            Self::ExpDimensionless => {
                f.write_str("exp() requires a dimensionless real or complex argument")
            }
            Self::ToFloatInt => f.write_str("to_float() requires an Int argument"),
            Self::ToIntFiniteKeys => f.write_str(
                "to_int() extracts positions from Fin-axis keys only; named and coordinate keys have no ordinal",
            ),
            Self::ToIntDimensionless => f.write_str("to_int() requires a Dimensionless argument"),
            Self::CoordExtractsCoordinate => {
                f.write_str("coord() extracts the coordinate quantity of a coordinate-axis key")
            }
            Self::CoordCoordinateKeysOnly => f.write_str(
                "coord() applies to coordinate-axis keys only; named keys are opaque and Fin keys expose to_int()",
            ),
            Self::DatetimeArgument(builtin) => {
                write!(f, "{}() requires a Datetime argument", builtin.as_str())
            }
            Self::DatetimeStringLiteral => {
                f.write_str("datetime() requires a contextual datetime string literal")
            }
            Self::DatetimeTimezoneLiteral => {
                f.write_str("datetime() second argument must be an IANA timezone literal")
            }
            Self::EpochCivilLiteral => {
                f.write_str("epoch<S>() requires one civil datetime string literal")
            }
            Self::StringLiteralContext => {
                f.write_str("string literals can only be used in their declared datetime contexts")
            }
            Self::ExternScalarParameter { parameter, scalar } => {
                write!(f, "parameter `{parameter}` requires {scalar}")
            }
            Self::ExternAxisPerIndexVariable {
                parameter,
                function,
            } => write!(
                f,
                "parameter `{parameter}` of `{function}` takes one axis for each declared index variable"
            ),
            Self::ExternQuantityElements {
                parameter,
                function,
            } => write!(
                f,
                "parameter `{parameter}` of `{function}` requires quantity elements"
            ),
            Self::ExternScalarElements {
                parameter,
                function,
                scalar,
            } => write!(
                f,
                "parameter `{parameter}` of `{function}` requires {scalar} elements"
            ),
            Self::ExternSharedIndexVariable { variable, function } => write!(
                f,
                "axes sharing index variable `{variable}` of `{function}` must use the same typed index"
            ),
            Self::KeyStaticPosition => f.write_str("key(Fin(N), position) takes an integer position"),
            Self::FinKeyIntPosition => {
                f.write_str("fin_key(Fin(N), position) takes an Int position, checked at runtime")
            }
            Self::CoordinateSearchAxisDimension(kind) => {
                write!(f, "{}() takes a quantity in the axis dimension", kind.as_str())
            }
            Self::TimezoneDisplayDatetime(timezone) => write!(
                f,
                "timezone display `-> \"{timezone}\"` requires a Datetime expression"
            ),
            Self::ScanBody => f.write_str("scan body must return the same type as the accumulator"),
            Self::UnfoldBody => {
                f.write_str("unfold body must return the same type as the previous state")
            }
            Self::FinKeyArithmetic(rule) => rule.fmt(f),
            Self::BooleanOperands => f.write_str("boolean operators require Bool operands"),
            Self::EqualitySameType => f.write_str("equality operands must have the same type"),
            Self::ComplexUnordered => f.write_str(
                "complex quantities are unordered; compare re(), im(), abs(), or phase() explicitly",
            ),
            Self::ComparisonSameType => f.write_str("comparison operands must have the same type"),
            Self::DatetimeComparisonScales => {
                f.write_str("cannot compare datetimes with different time scales")
            }
            Self::ComparisonSameDimension => {
                f.write_str("comparison operands must have the same dimension")
            }
            Self::FinKeyArithmeticKeyFirst => f.write_str(
                "Fin-key arithmetic is written key-first: `k + c` with a static Nat constant",
            ),
            Self::ComplexAdditionSameDimension => f.write_str(
                "complex operands of addition and subtraction must have the same dimension",
            ),
            Self::NoImplicitComplexPromotion => f.write_str(
                "addition and subtraction do not implicitly promote real quantities; use to_complex()",
            ),
            Self::DatetimeSubtractionScales => {
                f.write_str("cannot subtract datetimes with different time scales")
            }
            Self::DatetimeAddition => f.write_str("cannot add two datetimes; did you mean to subtract?"),
            Self::DurationAddSubtract => {
                f.write_str("can only add/subtract a Time duration to/from a Datetime")
            }
            Self::DurationAdd => f.write_str("can only add a Time duration to a Datetime"),
            Self::DatetimeFromQuantity => f.write_str("cannot subtract a Datetime from a quantity"),
            Self::AdditionSameDimension => {
                f.write_str("operands of addition and subtraction must have the same dimension")
            }
            Self::ModuloInt => f.write_str("modulo operator requires Int operands"),
            Self::NonNegativeExponent => {
                f.write_str("integer power requires a non-negative exact integer exponent")
            }
            Self::ExactIntegerExponent => {
                f.write_str("integer power requires an exact integer exponent such as `2`")
            }
            Self::DimensionlessExponent => f.write_str("the exponent of a power must be dimensionless"),
            Self::LogicalNot => f.write_str("logical NOT requires a Bool operand"),
            Self::Negation => f.write_str("negation requires a numeric quantity or Int operand"),
            Self::IfConditionBool => f.write_str("if/else condition must be Bool"),
            Self::IfBranchesSameDimension => {
                f.write_str("both branches of if/else must have the same dimension")
            }
            Self::MatchArmsSameType => f.write_str("all match arms must return the same type"),
            Self::ToleranceSameDimension => f.write_str(
                "actual and expected in tolerance assertion must have the same dimension",
            ),
            Self::AbsoluteToleranceDimension => {
                f.write_str("absolute tolerance must have the same dimension as actual/expected")
            }
        }
    }
}
