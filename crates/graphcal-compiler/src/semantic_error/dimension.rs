//! Dimension and unit diagnostics: mismatched dimensions, units, shapes, and time scales.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::builtin::{AggregationFn, LinearAlgebraFn};
use crate::datetime_literal::CivilDateTimeLiteral;
use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::hir::const_expr::ConstExprError;
use crate::semantic::unit_scale::PositiveFiniteScaleError;

/// The unit scale whose value failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitScaleSite {
    /// A unit definition's own scale.
    Definition,
    /// A compound unit's product of scales.
    Compound,
}

impl std::fmt::Display for UnitScaleSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Definition => "unit scale",
            Self::Compound => "compound unit scale",
        })
    }
}
use super::dimension_mismatch::{MismatchOperand, MismatchRule};
use crate::semantic::checked_type::{IndexTypeRef, Symbolic, TypeSpelling};
use crate::semantic::dimension_table::DimensionSpelling;
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::dimension::{DimName, UnitName, UnitRef};
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;

/// What a plot property value was instead of the type the property expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlotPropertyValue {
    /// A non-literal expression where only a string literal is valid.
    NotStringLiteral,
    /// A string literal where a number or Bool is expected.
    StringLiteral,
    Type(TypeSpelling),
}

impl std::fmt::Display for PlotPropertyValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStringLiteral => f.write_str("not a string literal"),
            Self::StringLiteral => f.write_str("a string literal"),
            Self::Type(ty) => ty.fmt(f),
        }
    }
}

/// The index axes one plot encoding channel ranges over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotChannelAxes {
    pub channel: crate::syntax::ast::EncodingChannel,
    pub axes: Vec<IndexTypeRef<Symbolic>>,
}

impl std::fmt::Display for PlotChannelAxes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` ranges over ", self.channel)?;
        if self.axes.is_empty() {
            return f.write_str("no index");
        }
        for (position, index) in self.axes.iter().enumerate() {
            if position > 0 {
                f.write_str(" × ")?;
            }
            match index.declared_resolved() {
                Some(declared) => declared.fmt(f)?,
                None => index.display_name().fmt(f)?,
            }
        }
        Ok(())
    }
}

fn format_channel_axes(channels: &[PlotChannelAxes]) -> String {
    channels
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where two indexed operands with different axes met.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeContext {
    ToleranceAssertion,
}

impl std::fmt::Display for ShapeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ToleranceAssertion => "tolerance assertion",
        })
    }
}

/// How a linear-algebra argument's axis fails the operation's shape rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearAlgebraAxisMismatch {
    /// Contracted axes must have the same typed identity.
    Identity {
        expected: Box<IndexTypeRef<Symbolic>>,
        found: Box<IndexTypeRef<Symbolic>>,
    },
    /// The operation needs an axis of exactly `expected` entries.
    Cardinality {
        expected: usize,
        found: Option<usize>,
    },
    /// The operation needs a concrete axis cardinality.
    ConcreteCardinalityRequired,
}

impl LinearAlgebraAxisMismatch {
    fn expected(&self) -> String {
        match self {
            Self::Identity { expected, .. } => expected.to_string(),
            Self::Cardinality { expected, .. } => {
                format!("an axis with exactly {expected} entries")
            }
            Self::ConcreteCardinalityRequired => "an axis with a concrete cardinality".to_owned(),
        }
    }

    fn found(&self) -> String {
        match self {
            Self::Identity { found, .. } => found.to_string(),
            Self::Cardinality { found: None, .. } => {
                "an axis whose cardinality is not concrete".to_owned()
            }
            Self::Cardinality {
                found: Some(cardinality),
                ..
            } => format!("an axis with {cardinality} entries"),
            Self::ConcreteCardinalityRequired => {
                "an axis whose cardinality is still generic".to_owned()
            }
        }
    }

    fn help(&self, function: LinearAlgebraFn) -> String {
        match self {
            Self::Identity { .. } => "linear-algebra contractions match axes by typed identity; use the same declared index (or the same Fin(N) structural index) at both contracted positions".to_owned(),
            Self::Cardinality { .. } => format!("{}() is defined only for three-component vectors", function.as_str()),
            Self::ConcreteCardinalityRequired => format!(
                "{}() needs a concrete matrix size because its result dimension depends on that size",
                function.as_str()
            ),
        }
    }
}

/// Why a `base unit` declaration cannot be the canonical unit of its
/// dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseUnitRejection {
    /// The dimension already has a canonical base unit.
    CanonicalUnitTaken { existing: UnitName },
    /// The dimension is not a bare base dimension.
    NotBaseDimension,
}

/// Why a datetime literal is invalid.
#[derive(Debug, Clone)]
pub enum DatetimeLiteralError {
    Offset(crate::datetime_literal::ParseOffsetDateTimeLiteralError),
    Civil(crate::datetime_literal::ParseCivilDateTimeLiteralError),
    Zoned(Box<crate::datetime_literal::ResolveZonedDateTimeLiteralError>),
    Epoch(std::sync::Arc<crate::datetime_literal::ResolveEpochLiteralError>),
}

/// Parser errors carry foreign error values without structural equality, so
/// two reasons are equal when they are the same kind of failure and render
/// the same text.
impl PartialEq for DatetimeLiteralError {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
            && self.to_string() == other.to_string()
    }
}

impl Eq for DatetimeLiteralError {}

impl std::fmt::Display for DatetimeLiteralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offset(error) => error.fmt(f),
            Self::Civil(error) => error.fmt(f),
            Self::Zoned(error) => error.fmt(f),
            Self::Epoch(error) => error.fmt(f),
        }
    }
}

/// Dimension and unit diagnostics: mismatched dimensions, units, shapes, and time scales.
#[derive(Debug, Clone, Error)]
pub enum DimensionError {
    #[error("property `{property}` expects {expected}")]
    PlotPropertyTypeMismatch {
        property: &'static str,
        expected: &'static str,
        found: PlotPropertyValue,
    },
    #[error(
        "property `{property}` must be dimensionless, but this value has dimension {dimension}"
    )]
    PlotPropertyDimensioned {
        property: &'static str,
        dimension: DimensionSpelling,
    },
    #[error("encoding channel `{channel}` cannot plot values of type {found}")]
    PlotEncodingTypeMismatch {
        channel: crate::syntax::ast::EncodingChannel,
        found: TypeSpelling,
    },
    #[error("plot encoding channels range over incompatible index axes")]
    PlotEncodingAxisMismatch { channels: Vec<PlotChannelAxes> },
    #[error("graph reference `@{name}` not allowed in const unit scale")]
    GraphRefInConstUnit { name: ScopedName },
    #[error("non-const unit `{name}` not allowed in const expression")]
    NonConstUnitInConst { name: UnitRef },
    #[error("dimension exponent overflow")]
    DimensionOverflow,
    #[error("dimension mismatch: expected {expected}, found {found}")]
    DimensionMismatch {
        expected: Box<MismatchOperand>,
        found: Box<MismatchOperand>,
        help: Box<MismatchRule>,
    },
    #[error("mismatched index axes in {context}: {lhs} vs {rhs}")]
    IndexedShapeMismatch {
        context: ShapeContext,
        lhs: TypeSpelling,
        rhs: TypeSpelling,
    },
    #[error(
        "incompatible indexed shape for `{function}()`: expected {}, found {}",
        mismatch.expected(),
        mismatch.found()
    )]
    LinearAlgebraShapeMismatch {
        function: LinearAlgebraFn,
        mismatch: LinearAlgebraAxisMismatch,
    },
    #[error("comparison operators require unindexed operands, found {found}")]
    IndexedComparisonOperand { found: TypeSpelling },
    #[error("`{function}()` does not accept a rank-{rank} indexed value")]
    MultiAxisAggregation {
        function: AggregationFn,
        rank: usize,
    },
    #[error("`scan()` requires a rank-one source, found rank-{rank}")]
    MultiAxisScanSource { rank: usize },
    #[error(
        "`{function}()` cannot determine the result dimension without a concrete axis cardinality"
    )]
    AggregationCardinalityUnknown { function: AggregationFn },
    #[error("materialized indexed value exceeds the eager limit of {maximum} scalar values")]
    MaterializedShapeTooLarge { maximum: usize },
    #[error("type annotation mismatch: declared {declared}, inferred {inferred}")]
    DimensionMismatchInAnnotation {
        declared: TypeSpelling,
        inferred: TypeSpelling,
    },
    #[error("unit `{name}` is declared as {declared}, but its definition uses {definition}")]
    UnitDefinitionDimensionMismatch {
        name: UnitName,
        declared: DimensionSpelling,
        definition: DimensionSpelling,
    },
    #[error("dynamic unit `{name}` requires a scalar Dimensionless scale, but found {found}")]
    DynamicUnitScaleTypeMismatch { name: UnitRef, found: TypeSpelling },
    #[error("unknown unit `{name}`")]
    UnknownUnit { name: UnitRef },
    #[error("unknown dimension `{name}`")]
    UnknownDimension { name: NamePath },
    #[error("cyclic dimension dependency involving `{name}`")]
    CyclicDimension { name: DimName },
    #[error("cyclic unit dependency involving `{name}`")]
    CyclicUnit { name: UnitName },
    #[error("a dimensioned base requires a statically exact rational exponent")]
    RuntimeExponentForDimensionedBase,
    #[error("float syntax cannot be the exponent of a dimensioned base")]
    FloatPowerExponent {
        /// Exact source replacement when the decimal value fits the dimension
        /// rational model. Also carried as structured LSP diagnostic data.
        replacement: Option<String>,
    },
    #[error("conversion target dimension {target} does not match expression dimension {expr_dim}")]
    ConversionDimensionMismatch {
        target: DimensionSpelling,
        expr_dim: DimensionSpelling,
    },
    #[error("`->` cannot be applied to an expression that already has a display target")]
    NestedConversion,
    #[error("`->` has no effect in this position")]
    IneffectiveConversion,
    #[error("cannot declare `{name}` as the base unit of dimension `{dim}`")]
    InvalidBaseUnitDeclaration {
        name: UnitName,
        dim: DimensionSpelling,
        rejection: BaseUnitRejection,
    },
    #[error("user-defined units on dimension `{dim}` are not supported")]
    AffineProneUnitDefinition { dim: DimensionSpelling },
    // --- Domain constraint errors ---
    #[error("unknown timezone `{timezone}`")]
    InvalidTimezone {
        timezone: String,
        tzdb_version: &'static str,
    },
    #[error("invalid datetime literal: {reason}")]
    InvalidDatetimeLiteral {
        expectation: crate::datetime_literal::DatetimeLiteralExpectation,
        reason: DatetimeLiteralError,
    },
    #[error("epoch requires exactly one static time-scale argument, got {got}")]
    EpochTimeScaleArgumentCount { got: usize },
    #[error("epoch's static time-scale argument must be a bare name")]
    InvalidEpochTimeScaleArgument,
    #[error("unsupported epoch time scale `{name}`")]
    UnsupportedEpochTimeScale { name: NameAtom },
    #[error("local civil datetime `{datetime}` does not exist in timezone `{time_zone}`")]
    NonexistentCivilDateTime {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
        time_zone_span: Span,
    },
    #[error("local civil datetime `{datetime}` occurs twice in timezone `{time_zone}`")]
    RepeatedCivilDateTime {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
        time_zone_span: Span,
    },
    /// A unit-scale or coordinate constant expression that is not static.
    #[error("{error}")]
    InvalidConstantExpression { error: ConstExprError },
    #[error("{site} {error}")]
    InvalidUnitScale {
        site: UnitScaleSite,
        error: PositiveFiniteScaleError,
    },
    #[error("unit `{name}` has a dynamic scale and cannot be used here")]
    DynamicUnitScaleNotAllowed { name: UnitRef },
    #[error("expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)")]
    ExpectedTimeScale,
    #[error("unknown time scale `{name}`; expected one of: {expected}")]
    UnknownTimeScale {
        name: NameAtom,
        expected: &'static str,
    },
    #[error("type `Datetime` expects 0 or 1 type argument(s), got {got}")]
    WrongDatetimeArgCount { got: usize },
}

impl DiagnosticKind for DimensionError {
    fn code(&self) -> &'static str {
        match self {
            Self::PlotPropertyTypeMismatch { .. } => "graphcal::D015",
            Self::PlotPropertyDimensioned { .. } => "graphcal::D016",
            Self::PlotEncodingTypeMismatch { .. } => "graphcal::D033",
            Self::PlotEncodingAxisMismatch { .. } => "graphcal::D034",
            Self::GraphRefInConstUnit { .. } => "graphcal::D017",
            Self::NonConstUnitInConst { .. } => "graphcal::D018",
            Self::DimensionOverflow => "graphcal::D010",
            Self::DimensionMismatch { .. } => "graphcal::D001",
            Self::IndexedShapeMismatch { .. } => "graphcal::D011",
            Self::LinearAlgebraShapeMismatch { .. } => "graphcal::D022",
            Self::IndexedComparisonOperand { .. } => "graphcal::D019",
            Self::MultiAxisAggregation { .. } => "graphcal::D021",
            Self::MultiAxisScanSource { .. } => "graphcal::D026",
            Self::AggregationCardinalityUnknown { .. } => "graphcal::D027",
            Self::MaterializedShapeTooLarge { .. } => "graphcal::D035",
            Self::DimensionMismatchInAnnotation { .. } => "graphcal::D002",
            Self::UnitDefinitionDimensionMismatch { .. } => "graphcal::D031",
            Self::DynamicUnitScaleTypeMismatch { .. } => "graphcal::D032",
            Self::UnknownUnit { .. } => "graphcal::D003",
            Self::UnknownDimension { .. } => "graphcal::D004",
            Self::CyclicDimension { .. } => "graphcal::D008",
            Self::CyclicUnit { .. } => "graphcal::D009",
            Self::RuntimeExponentForDimensionedBase => "graphcal::D005",
            Self::FloatPowerExponent { .. } => "graphcal::D020",
            Self::ConversionDimensionMismatch { .. } => "graphcal::D006",
            Self::NestedConversion => "graphcal::D012",
            Self::IneffectiveConversion => "graphcal::D013",
            Self::InvalidBaseUnitDeclaration { .. } => "graphcal::D036",
            Self::AffineProneUnitDefinition { .. } => "graphcal::D014",
            Self::InvalidTimezone { .. } => "graphcal::D007",
            Self::InvalidDatetimeLiteral { .. } => "graphcal::D028",
            Self::EpochTimeScaleArgumentCount { .. } => "graphcal::D023",
            Self::InvalidEpochTimeScaleArgument { .. } => "graphcal::D029",
            Self::UnsupportedEpochTimeScale { .. } => "graphcal::D030",
            Self::NonexistentCivilDateTime { .. } => "graphcal::D024",
            Self::RepeatedCivilDateTime { .. } => "graphcal::D025",
            Self::InvalidConstantExpression { .. } => "graphcal::D037",
            Self::InvalidUnitScale { .. } => "graphcal::D038",
            Self::DynamicUnitScaleNotAllowed { .. } => "graphcal::D039",
            Self::ExpectedTimeScale => "graphcal::D040",
            Self::UnknownTimeScale { .. } => "graphcal::D041",
            Self::WrongDatetimeArgCount { .. } => "graphcal::D042",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::PlotPropertyTypeMismatch { found, .. } => Some(format!("this is {found}")),
            Self::PlotPropertyDimensioned { .. } => Some("dimensioned value".to_owned()),
            Self::PlotEncodingTypeMismatch { .. } => Some("not a plottable value".to_owned()),
            Self::PlotEncodingAxisMismatch { .. } => {
                Some("this channel cannot align with the shared plot rows".to_owned())
            }
            Self::GraphRefInConstUnit { .. } => {
                Some("@ reference not allowed in a const unit".to_owned())
            }
            Self::NonConstUnitInConst { .. } => Some("unit is not const".to_owned()),
            Self::DimensionOverflow => Some("overflow here".to_owned()),
            Self::DimensionMismatch { found, .. } => Some(format!("has dimension {found}")),
            Self::IndexedShapeMismatch { rhs, .. } => Some(format!("has type {rhs}")),
            Self::LinearAlgebraShapeMismatch { mismatch, .. } => {
                Some(format!("found {}", mismatch.found()))
            }
            Self::IndexedComparisonOperand { found, .. } => {
                Some(format!("indexed operand has type {found}"))
            }
            Self::MultiAxisAggregation { rank, .. } => Some(format!("rank-{rank} input")),
            Self::MultiAxisScanSource { rank, .. } => Some(format!("rank-{rank} source")),
            Self::AggregationCardinalityUnknown { .. } => {
                Some("axis cardinality is abstract here".to_owned())
            }
            Self::MaterializedShapeTooLarge { .. } => {
                Some("this indexed expression would materialize too many values".to_owned())
            }
            Self::DimensionMismatchInAnnotation { declared, .. } => {
                Some(format!("declared as {declared}"))
            }
            Self::UnitDefinitionDimensionMismatch { definition, .. } => {
                Some(format!("this unit expression has dimension {definition}"))
            }
            Self::DynamicUnitScaleTypeMismatch { found, .. } => {
                Some(format!("this scale expression has type {found}"))
            }
            Self::UnknownUnit { .. } => Some("unknown unit".to_owned()),
            Self::UnknownDimension { .. } => Some("unknown dimension".to_owned()),
            Self::CyclicDimension { .. } | Self::CyclicUnit { .. } => {
                Some("involved in cycle".to_owned())
            }
            Self::RuntimeExponentForDimensionedBase => {
                Some("runtime exponent cannot determine the result dimension".to_owned())
            }
            Self::FloatPowerExponent { .. } => {
                Some("exact rational syntax is required here".to_owned())
            }
            Self::ConversionDimensionMismatch { .. } => {
                Some("target unit has different dimension".to_owned())
            }
            Self::NestedConversion => {
                Some("the operand of this conversion is itself a conversion".to_owned())
            }
            Self::IneffectiveConversion => {
                Some("this conversion's display target is discarded".to_owned())
            }
            Self::InvalidBaseUnitDeclaration { dim, rejection, .. } => Some(match rejection {
                BaseUnitRejection::CanonicalUnitTaken { existing } => {
                    format!("`{dim}` already has canonical base unit `{existing}`")
                }
                BaseUnitRejection::NotBaseDimension => format!("`{dim}` is not a base dimension"),
            }),
            Self::AffineProneUnitDefinition { .. } => {
                Some("unit defined on an affine-prone dimension".to_owned())
            }
            Self::InvalidTimezone { .. } => Some("not a recognized IANA timezone".to_owned()),
            Self::InvalidDatetimeLiteral { .. } => {
                Some("does not satisfy this constructor's datetime literal contract".to_owned())
            }
            Self::EpochTimeScaleArgumentCount { .. } => {
                Some("expected exactly one time scale here".to_owned())
            }
            Self::InvalidEpochTimeScaleArgument { .. } => {
                Some("expected a supported bare time-scale name".to_owned())
            }
            Self::UnsupportedEpochTimeScale { .. } => Some("not a supported time scale".to_owned()),
            Self::NonexistentCivilDateTime { .. } => Some("this local time is skipped".to_owned()),
            Self::RepeatedCivilDateTime { .. } => Some("this local time is repeated".to_owned()),
            Self::InvalidConstantExpression { .. }
            | Self::InvalidUnitScale { .. }
            | Self::DynamicUnitScaleNotAllowed { .. }
            | Self::ExpectedTimeScale
            | Self::UnknownTimeScale { .. }
            | Self::WrongDatetimeArgCount { .. } => Some("error here".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::PlotPropertyTypeMismatch { .. }
            | Self::InvalidConstantExpression { .. }
            | Self::InvalidUnitScale { .. }
            | Self::DynamicUnitScaleNotAllowed { .. }
            | Self::ExpectedTimeScale
            | Self::UnknownTimeScale { .. }
            | Self::WrongDatetimeArgCount { .. } => None,
            Self::PlotPropertyDimensioned { .. } => Some("plot properties are raw rendering quantities (pixels, ratios); write a plain number instead of a dimensioned value".to_owned()),
            Self::PlotEncodingTypeMismatch { .. } => Some("plot quantities, Int, Bool, Datetime, index keys, or a contextual string literal; project algebraic or Complex values to a plottable field first".to_owned()),
            Self::PlotEncodingAxisMismatch { channels, .. } => Some(format!("{}; every channel must range over a subset of one channel's axes", format_channel_axes(channels))),
            Self::GraphRefInConstUnit { .. } => Some("`const unit` scales are compile-time constants; use plain `unit` for runtime-dependent units".to_owned()),
            Self::NonConstUnitInConst { .. } => Some("`const node` bodies and `const unit` definitions can only use prelude units, `base unit`, or `const unit` declarations; use `node` or plain `unit` for runtime-unit calculations".to_owned()),
            Self::DimensionOverflow => Some("dimension exponents are stored as `i32`; reduce the magnitude of the exponent".to_owned()),
            Self::DimensionMismatch { help, .. } => Some(help.to_string()),
            Self::LinearAlgebraShapeMismatch { function, mismatch } => Some(mismatch.help(*function)),
            Self::FloatPowerExponent { replacement } => Some(replacement.as_ref().map_or_else(
                || "write the exponent as an exact integer or parenthesized rational".to_owned(),
                |replacement| format!("replace the float exponent with `{replacement}`"),
            )),
            Self::InvalidBaseUnitDeclaration { name, rejection, .. } => Some(match rejection {
                BaseUnitRejection::CanonicalUnitTaken { .. } => "a dimension can have only one canonical base unit; if another multiplicative unit is valid for this dimension, define its scale explicitly with `const unit` or `unit`".to_owned(),
                BaseUnitRejection::NotBaseDimension => format!("`base unit` can only define the canonical unit of a bare base dimension; define `{name}` with an explicit `const unit` or `unit` scale instead"),
            }),
            Self::IndexedShapeMismatch { .. } => Some("element-wise operands must be indexed by the same axes in the same order; an unindexed operand broadcasts to every key".to_owned()),
            Self::IndexedComparisonOperand { .. } => Some("comparison operators do not broadcast; use an explicit `for` comprehension and compare individual indexed elements in its body".to_owned()),
            Self::MultiAxisAggregation { .. } => Some("reduce one axis at a time with an explicit `for` comprehension; total- and partial-axis aggregation are not yet defined".to_owned()),
            Self::MultiAxisScanSource { .. } => Some("scan does not choose an axis implicitly; use an explicit `for` comprehension to select each rank-one series before scanning it".to_owned()),
            Self::AggregationCardinalityUnknown { .. } => Some("apply product() where the index is concrete, or reduce dimensionless values whose result does not depend on cardinality".to_owned()),
            Self::MaterializedShapeTooLarge { maximum, .. } => Some(format!("reduce one or more axis cardinalities so their product is at most {maximum}")),
            Self::DimensionMismatchInAnnotation { .. } => Some("the declared type must match the inferred dimension of the expression".to_owned()),
            Self::UnitDefinitionDimensionMismatch { .. } => Some("the unit expression on the right-hand side must have exactly the declared dimension".to_owned()),
            Self::DynamicUnitScaleTypeMismatch { .. } => Some("use an unindexed Dimensionless quantity; Bool, Int, structures, indexed values, and dimensioned quantities are not valid unit scales".to_owned()),
            Self::UnknownUnit { .. } => Some("a bare unit name must be declared in this file, selectively imported, or part of the prelude; units of a module imported with an alias are referenced as `alias::unit`".to_owned()),
            Self::UnknownDimension { .. } => Some("dimension must be declared or part of the prelude".to_owned()),
            Self::CyclicDimension { .. } => Some("derived dimensions cannot form dependency cycles".to_owned()),
            Self::CyclicUnit { .. } => Some("units cannot form dependency cycles".to_owned()),
            Self::RuntimeExponentForDimensionedBase => Some("use an exact integer such as `2` or a parenthesized rational such as `(3/2)`".to_owned()),
            Self::ConversionDimensionMismatch { .. } => Some("the `->` conversion operator can only change units within the same dimension".to_owned()),
            Self::NestedConversion => Some("an expression carries at most one `->` target; remove the inner conversion — only the outermost target takes effect".to_owned()),
            Self::IneffectiveConversion => Some("a conversion only affects how a declaration's final value is displayed; move it to the top level of the declaration (or a selected `if`/`match` branch, constructor field, map entry, for-comprehension body, or scan/unfold init), or remove it".to_owned()),
            Self::AffineProneUnitDefinition { .. } => Some("common units of this dimension (e.g. \u{b0}C, \u{b0}F for Temperature) are affine scales with an offset; a purely multiplicative `unit` definition would display silently wrong values. Keep values in the base unit, or model the offset explicitly in your expressions".to_string()),
            Self::InvalidTimezone { tzdb_version, .. } => Some(format!("use a name present in Graphcal's bundled IANA tzdb {tzdb_version}, such as \"UTC\", \"America/New_York\", or \"Asia/Tokyo\"")),
            Self::InvalidDatetimeLiteral { expectation, .. } => Some(format!("{expectation}")),
            Self::EpochTimeScaleArgumentCount { .. } => Some("write a supported scale in angle brackets, for example `epoch<TT>(\"2024-11-05T12:00:00\")`".to_owned()),
            Self::InvalidEpochTimeScaleArgument | Self::UnsupportedEpochTimeScale { .. } => Some(format!(
                "use one of {}",
                crate::semantic::time_scale::TimeScale::expected_names()
            )),
            Self::NonexistentCivilDateTime { after, before, .. } => Some(format!("the timezone offset jumps from {before} to {after} across this gap; choose an existing local time or use one-argument `datetime` with an explicit offset")),
            Self::RepeatedCivilDateTime { after, before, .. } => Some(format!("the repeated time can use offset {before} or {after}; use one-argument `datetime` with an explicit offset to select an instant")),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::PlotPropertyTypeMismatch { .. }
            | Self::PlotPropertyDimensioned { .. }
            | Self::PlotEncodingTypeMismatch { .. }
            | Self::PlotEncodingAxisMismatch { .. }
            | Self::GraphRefInConstUnit { .. }
            | Self::NonConstUnitInConst { .. }
            | Self::DimensionOverflow
            | Self::DimensionMismatch { .. }
            | Self::IndexedShapeMismatch { .. }
            | Self::LinearAlgebraShapeMismatch { .. }
            | Self::IndexedComparisonOperand { .. }
            | Self::MultiAxisAggregation { .. }
            | Self::MultiAxisScanSource { .. }
            | Self::AggregationCardinalityUnknown { .. }
            | Self::MaterializedShapeTooLarge { .. }
            | Self::DimensionMismatchInAnnotation { .. }
            | Self::UnitDefinitionDimensionMismatch { .. }
            | Self::DynamicUnitScaleTypeMismatch { .. }
            | Self::UnknownUnit { .. }
            | Self::UnknownDimension { .. }
            | Self::CyclicDimension { .. }
            | Self::CyclicUnit { .. }
            | Self::RuntimeExponentForDimensionedBase
            | Self::FloatPowerExponent { .. }
            | Self::ConversionDimensionMismatch { .. }
            | Self::NestedConversion
            | Self::IneffectiveConversion
            | Self::InvalidBaseUnitDeclaration { .. }
            | Self::AffineProneUnitDefinition { .. }
            | Self::InvalidTimezone { .. }
            | Self::InvalidDatetimeLiteral { .. }
            | Self::EpochTimeScaleArgumentCount { .. }
            | Self::InvalidEpochTimeScaleArgument { .. }
            | Self::UnsupportedEpochTimeScale { .. }
            | Self::InvalidConstantExpression { .. }
            | Self::InvalidUnitScale { .. }
            | Self::DynamicUnitScaleNotAllowed { .. }
            | Self::ExpectedTimeScale
            | Self::UnknownTimeScale { .. }
            | Self::WrongDatetimeArgCount { .. } => Vec::new(),
            Self::NonexistentCivilDateTime { time_zone_span, .. } => vec![SecondaryLabel {
                span: *time_zone_span,
                text: "gap occurs in this timezone".to_owned(),
            }],
            Self::RepeatedCivilDateTime { time_zone_span, .. } => vec![SecondaryLabel {
                span: *time_zone_span,
                text: "fold occurs in this timezone".to_owned(),
            }],
        }
    }
}
