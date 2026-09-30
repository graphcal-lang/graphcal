//! Typed parse errors.
//!
//! A [`ParseError`] is a [`ParseErrorKind`] located at a [`Span`] of the one
//! source the parser read. It carries no file name or source text: the shell
//! that owns the source attaches it with [`ParseError::located`] and renders
//! the resulting [`Diagnostic`] through
//! [`RenderableDiagnostic`](crate::diagnostic_render::RenderableDiagnostic).
//! Payloads are typed; [`DiagnosticKind`] is the only place they become text.

use std::fmt;
use std::num::{ParseFloatError, ParseIntError};

use crate::diagnostic::{Diagnostic, DiagnosticKind, SecondaryLabel};
use crate::source_id::SourceId;
use crate::syntax::ast::{DomainBoundKind, NatExpr};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;
use crate::syntax::token::SourceIdentifier;

use super::expected::{Expected, Found};
use super::nesting_limit::MAX_NESTING_DEPTH;

/// A parse failure located in the parsed source.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// What went wrong.
    pub kind: ParseErrorKind,
    /// Primary location of the failure.
    pub span: Span,
}

impl ParseError {
    /// Locate `kind` at `span`.
    #[must_use]
    pub const fn new(kind: ParseErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// Attach the identity of the parsed source.
    #[must_use]
    pub fn located(self, src: SourceId) -> Diagnostic<ParseErrorKind> {
        Diagnostic::new(src, self.span, self.kind)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for ParseError {}

/// Why a parse failed.
#[derive(Debug, Clone)]
pub enum ParseErrorKind {
    /// The next token does not continue the production being parsed.
    UnexpectedToken { expected: Expected, found: Found },
    /// A slice label of a table names a different axis than the table's
    /// slice axis at that position.
    SliceAxisMismatch { expected: NamePath, found: NamePath },
    /// The source ended inside a production.
    UnexpectedEof { expected: Expected },
    /// A numeric literal is malformed or out of range.
    InvalidNumber { reason: InvalidNumberReason },
    /// A table row's value count differs from its header.
    TableRowLengthMismatch { expected: u64, got: u64 },
    /// A domain constraint key other than `min` / `max`.
    InvalidDomainBoundKey { key: SourceIdentifier },
    /// A domain constraint appears twice.
    DuplicateDomainBound { bound: DomainBoundKind },
    /// A name is bound twice in one DAG binding list.
    DuplicateDagBinding { name: NameAtom, first: Span },
    /// A character the lexer does not recognize.
    UnknownToken,
    /// A multi-decl slot tuple has the wrong number of entries.
    MultiDeclTupleArity {
        slot_count: usize,
        tuple_count: usize,
    },
    /// A multi-decl header row has the wrong number of cells.
    MultiDeclHeaderArity {
        slot_count: usize,
        header_count: usize,
    },
    /// A multi-decl data row has the wrong number of values.
    MultiDeclRowArity {
        expected_count: usize,
        got: usize,
        row_label: IndexEntryKey,
    },
    /// A multi-decl without a shared axis.
    MultiDeclNoSharedAxis,
    /// A multi-decl shape the parser does not support yet.
    MultiDeclUnsupportedShape { shape: UnsupportedMultiDeclShape },
    /// An inline DAG call without `::<out>`.
    InlineDagCallMissingProjection,
    /// Nesting beyond the parser's depth limit.
    TooDeeplyNested,
    /// A `^0` exponent.
    ZeroExponent,
    /// `-` inside a Nat expression.
    NatSubtractionUnsupported,
    /// A Nat literal where an index is required.
    ExpectedIndexFoundNat {
        /// Source spelling of the literal.
        expression: String,
    },
    /// The removed `range(N)` structural index constructor.
    ObsoleteStructuralRange { cardinality: NatExpr },
    /// A plot, mark, encode, figure, or layer field appears twice.
    DuplicatePlotField {
        field: SourceIdentifier,
        context: PlotFieldContext,
    },
    /// A plot without encoding channels.
    MissingPlotEncoding,
    /// A figure or layer without plots.
    EmptyCompositionPlots { kind: CompositionKind },
}

/// Why a numeric literal was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidNumberReason {
    /// The float literal failed to parse.
    Float(ParseFloatError),
    /// The integer literal failed to parse.
    Integer(ParseIntError),
    /// The float literal is infinite.
    NonFiniteFloat,
    /// An integer literal followed by a unit.
    IntegerWithUnits {
        /// Source spelling of the literal, without `_` separators.
        literal: String,
    },
    /// A Nat literal that is not a non-negative integer.
    NatLiteral,
    /// An index-position literal that is not a non-negative integer.
    IndexPosition,
    /// An attribute `#N` position that is not a non-negative integer.
    AttributePosition,
    /// A table `Fin(N)` cardinality that is not a non-negative integer.
    TableFinCardinality,
    /// A table value count beyond `u64`.
    TableValueCount,
    /// A table column position beyond `u64`.
    TableColumnPosition,
    /// A finite table row position beyond `u64`.
    FiniteRowPosition,
    /// A `#N` slice label that is not a non-negative integer.
    SliceLabel,
    /// A `#N` slice label outside its `Fin(cardinality)` axis.
    SliceIndexOutOfRange { value: u64, cardinality: u64 },
    /// An exact power exponent with a zero denominator.
    PowerExponentZeroDenominator,
    /// An exact power exponent beyond `i64`.
    PowerExponentOverflow,
    /// A dimension exponent beyond `i32`.
    DimensionExponentOverflow,
    /// A unit exponent beyond `i32`.
    UnitExponentOverflow,
    /// An exponent outside the representable range.
    ExponentOutOfRange,
    /// A fractional exponent with a zero denominator.
    ExponentZeroDenominator,
    /// A unit numerator other than `1`.
    UnitNumeratorNotOne,
    /// A literal that must be an integer.
    ExpectedInteger,
}

impl fmt::Display for InvalidNumberReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Float(error) => error.fmt(f),
            Self::Integer(error) => error.fmt(f),
            Self::NonFiniteFloat => f.write_str("floating-point literal must be finite"),
            Self::IntegerWithUnits { literal } => write!(
                f,
                "integer literal cannot have units; write `{literal}.0` instead"
            ),
            Self::NatLiteral => f.write_str("expected a non-negative integer in a Nat expression"),
            Self::IndexPosition => f.write_str("expected non-negative integer in index position"),
            Self::AttributePosition => {
                f.write_str("expected non-negative integer after `#` in attribute argument")
            }
            Self::TableFinCardinality => {
                f.write_str("table Fin cardinality must be a non-negative integer literal")
            }
            Self::TableValueCount => f.write_str("table value count does not fit in u64"),
            Self::TableColumnPosition => f.write_str("table column position does not fit in u64"),
            Self::FiniteRowPosition => f.write_str("finite table row position does not fit u64"),
            Self::SliceLabel => f.write_str("expected non-negative integer in slice label"),
            Self::SliceIndexOutOfRange { value, cardinality } => write!(
                f,
                "slice index #{value} out of range for Fin({cardinality})"
            ),
            Self::PowerExponentZeroDenominator => {
                f.write_str("power exponent denominator must be non-zero")
            }
            Self::PowerExponentOverflow => f.write_str("exact power exponent overflows `i64`"),
            Self::DimensionExponentOverflow => f.write_str("dimension exponent overflows `i32`"),
            Self::UnitExponentOverflow => f.write_str("unit exponent overflows `i32`"),
            Self::ExponentOutOfRange => f.write_str("exponent is out of range"),
            Self::ExponentZeroDenominator => {
                f.write_str("exponent denominator must be a non-zero integer")
            }
            Self::UnitNumeratorNotOne => {
                f.write_str("only `1` can appear as a unit numerator (e.g. `1/min`)")
            }
            Self::ExpectedInteger => f.write_str("expected integer"),
        }
    }
}

/// A multi-decl shape the parser does not support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedMultiDeclShape {
    /// More than one slot declares an extra axis.
    MultipleExtraAxisSlots,
    /// Several shared axes but no `[slice]` section.
    MissingSliceSection,
    /// A slice label names another axis than the shared axis it labels.
    SliceLabelAxis {
        label_axis: NamePath,
        shared_axis: NamePath,
    },
    /// A 1-D slot's header cell is a variant label instead of `_`.
    UnderscoreHeaderRequired { slot: DeclName },
    /// An extra-axis slot has no variant cells in the header row.
    MissingVariantCells { slot: DeclName },
}

impl fmt::Display for UnsupportedMultiDeclShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleExtraAxisSlots => f.write_str(
                "multi-decl with more than one extra-axis slot is not yet supported (v3)",
            ),
            Self::MissingSliceSection => f.write_str(
                "multi-decl with multiple shared axes requires at least one `[slice]` section",
            ),
            Self::SliceLabelAxis {
                label_axis,
                shared_axis,
            } => write!(
                f,
                "slice label qualifies axis `{label_axis}`, but the shared axis at this position is `{shared_axis}`"
            ),
            Self::UnderscoreHeaderRequired { slot } => {
                write!(f, "header cell for 1-D slot `{slot}` must be `_`")
            }
            Self::MissingVariantCells { slot } => write!(
                f,
                "slot `{slot}` is declared with an extra axis but has zero variant cells in the header row"
            ),
        }
    }
}

/// The block in which a duplicate plot field appeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotFieldContext {
    PlotDeclaration,
    MarkProperties,
    EncodeBlock,
    Composition(CompositionKind),
}

impl fmt::Display for PlotFieldContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlotDeclaration => f.write_str("plot declaration"),
            Self::MarkProperties => f.write_str("mark properties"),
            Self::EncodeBlock => f.write_str("encode block"),
            Self::Composition(kind) => kind.fmt(f),
        }
    }
}

/// A declaration that composes plots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionKind {
    Figure,
    Layer,
}

impl fmt::Display for CompositionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Figure => "figure",
            Self::Layer => "layer",
        })
    }
}

/// `"y"` / `"ies"` or `""` / `"s"` for English plurals of `count`.
const fn plural(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedToken { found, .. } => write!(f, "unexpected token `{found}`"),
            Self::SliceAxisMismatch { found, .. } => write!(f, "unexpected token `{found}`"),
            Self::UnexpectedEof { .. } => f.write_str("unexpected end of file"),
            Self::InvalidNumber { .. } => f.write_str("invalid number literal"),
            Self::TableRowLengthMismatch { expected, got } => write!(
                f,
                "table row has {got} value(s), but the header has {expected} column(s)"
            ),
            Self::InvalidDomainBoundKey { key } => {
                write!(f, "unknown domain constraint key `{key}`")
            }
            Self::DuplicateDomainBound { bound } => {
                write!(f, "duplicate domain constraint `{bound}`")
            }
            Self::DuplicateDagBinding { name, .. } => write!(f, "duplicate DAG binding `{name}`"),
            Self::UnknownToken => f.write_str("stray character in source"),
            Self::MultiDeclTupleArity {
                slot_count,
                tuple_count,
            } => write!(
                f,
                "multi-decl slot tuple has {tuple_count} entr{}, but the multi-decl declares {slot_count} slot{}",
                plural(*tuple_count, "y", "ies"),
                plural(*slot_count, "", "s"),
            ),
            Self::MultiDeclHeaderArity {
                slot_count,
                header_count,
            } => write!(
                f,
                "multi-decl header row has {header_count} cell{}, but the multi-decl declares {slot_count} slot{}",
                plural(*header_count, "", "s"),
                plural(*slot_count, "", "s"),
            ),
            Self::MultiDeclRowArity {
                expected_count,
                got,
                row_label,
            } => write!(
                f,
                "multi-decl row `{row_label}` has {got} value(s), but the header row declares {expected_count} value column{}",
                plural(*expected_count, "", "s"),
            ),
            Self::MultiDeclNoSharedAxis => {
                f.write_str("multi-decl requires at least one shared axis")
            }
            Self::MultiDeclUnsupportedShape { shape } => shape.fmt(f),
            Self::InlineDagCallMissingProjection => {
                f.write_str("inline DAG call requires `::<out>` projection")
            }
            Self::TooDeeplyNested => f.write_str("syntax nesting is too deep"),
            Self::ZeroExponent => f.write_str("`^0` exponent has no effect"),
            Self::NatSubtractionUnsupported => f.write_str("Nat subtraction is not supported"),
            Self::ExpectedIndexFoundNat { expression } => {
                write!(f, "expected Index, found Nat `{expression}`")
            }
            Self::ObsoleteStructuralRange { .. } => {
                f.write_str("`range(N)` is no longer a structural index constructor")
            }
            Self::DuplicatePlotField { field, context } => {
                write!(f, "duplicate `{field}` in {context}")
            }
            Self::MissingPlotEncoding => f.write_str("plot declaration has no encoding channels"),
            Self::EmptyCompositionPlots { kind } => write!(f, "{kind} declaration has no plots"),
        }
    }
}

impl DiagnosticKind for ParseErrorKind {
    fn code(&self) -> &'static str {
        match self {
            Self::UnexpectedToken { .. } | Self::SliceAxisMismatch { .. } => "graphcal::P001",
            Self::UnexpectedEof { .. } => "graphcal::P002",
            Self::InvalidNumber { .. } => "graphcal::P003",
            Self::TableRowLengthMismatch { .. } => "graphcal::P004",
            Self::InvalidDomainBoundKey { .. } => "graphcal::P005",
            Self::UnknownToken => "graphcal::P006",
            Self::MultiDeclTupleArity { .. } => "graphcal::P007",
            Self::MultiDeclHeaderArity { .. } => "graphcal::P008",
            Self::MultiDeclRowArity { .. } => "graphcal::P009",
            Self::MultiDeclNoSharedAxis => "graphcal::P011",
            Self::MultiDeclUnsupportedShape { .. } => "graphcal::P012",
            Self::InlineDagCallMissingProjection => "graphcal::P014",
            Self::TooDeeplyNested => "graphcal::P015",
            Self::ZeroExponent => "graphcal::P016",
            Self::DuplicatePlotField { .. } => "graphcal::P018",
            Self::MissingPlotEncoding => "graphcal::P019",
            Self::EmptyCompositionPlots { .. } => "graphcal::P020",
            Self::DuplicateDomainBound { .. } => "graphcal::P021",
            Self::NatSubtractionUnsupported => "graphcal::P022",
            Self::ExpectedIndexFoundNat { .. } => "graphcal::P023",
            Self::ObsoleteStructuralRange { .. } => "graphcal::P024",
            Self::DuplicateDagBinding { .. } => "graphcal::P025",
        }
    }

    fn primary_label(&self) -> Option<String> {
        Some(match self {
            Self::UnexpectedToken { .. }
            | Self::SliceAxisMismatch { .. }
            | Self::UnexpectedEof { .. }
            | Self::MultiDeclUnsupportedShape { .. } => "here".to_string(),
            Self::InvalidNumber { reason } => reason.to_string(),
            Self::TableRowLengthMismatch { got, .. } => format!("this row has {got} value(s)"),
            Self::MultiDeclRowArity { got, .. } => format!("this row has {got} value(s)"),
            Self::InvalidDomainBoundKey { .. } => "unknown key".to_string(),
            Self::DuplicateDomainBound { .. } => "duplicate bound here".to_string(),
            Self::DuplicateDagBinding { .. } => "duplicate binding".to_string(),
            Self::UnknownToken => "stray character".to_string(),
            Self::MultiDeclTupleArity { .. } => "slot tuple here".to_string(),
            Self::MultiDeclHeaderArity { .. } => "header row here".to_string(),
            Self::MultiDeclNoSharedAxis => "missing shared axis".to_string(),
            Self::InlineDagCallMissingProjection => {
                "expected `::<out>` projection here".to_string()
            }
            Self::TooDeeplyNested => "nesting exceeds the limit here".to_string(),
            Self::ZeroExponent => "exponent must be a non-zero integer".to_string(),
            Self::NatSubtractionUnsupported => {
                "`-` is not part of the Nat polynomial algebra".to_string()
            }
            Self::ExpectedIndexFoundNat { .. } => {
                "Nat is not implicitly converted to Index".to_string()
            }
            Self::ObsoleteStructuralRange { .. } => "use `Fin(...)` here".to_string(),
            Self::DuplicatePlotField { .. } => "duplicate field here".to_string(),
            Self::MissingPlotEncoding => {
                "this plot has an empty or missing `encode:` block".to_string()
            }
            Self::EmptyCompositionPlots { kind } => {
                format!("this {kind} has an empty or missing `plots:` list")
            }
        })
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UnexpectedToken { expected, .. } | Self::UnexpectedEof { expected } => {
                Some(format!("expected {expected}"))
            }
            Self::SliceAxisMismatch { expected, .. } => {
                Some(format!("expected slice axis `{expected}`"))
            }
            Self::InvalidNumber { .. } | Self::TableRowLengthMismatch { .. } => None,
            Self::InvalidDomainBoundKey { .. } => {
                Some("valid domain constraint keys are `min` and `max`".to_string())
            }
            Self::DuplicateDomainBound { .. } => {
                Some("each domain constraint may appear at most once".to_string())
            }
            Self::DuplicateDagBinding { .. } => {
                Some("each name may appear at most once in a DAG binding list".to_string())
            }
            Self::UnknownToken => Some(
                "remove or replace this character; it is not part of the graphcal grammar"
                    .to_string(),
            ),
            Self::MultiDeclTupleArity { .. } => Some(
                "the slot tuple in `table[..., (…)]` must contain exactly one entry per declared slot"
                    .to_string(),
            ),
            Self::MultiDeclHeaderArity { .. } => Some(
                "the header row (`: _, _, …;`) must have exactly one cell per slot".to_string(),
            ),
            Self::MultiDeclRowArity { .. } => {
                Some("each row must have exactly one value per header column".to_string())
            }
            Self::MultiDeclNoSharedAxis => {
                Some("declare the row axis in `table[SharedAxis, (…)]`".to_string())
            }
            Self::MultiDeclUnsupportedShape { .. } => Some(
                "this multi-decl shape is scheduled for a later version; see issue #481 for the incremental plan"
                    .to_string(),
            ),
            Self::InlineDagCallMissingProjection => Some(
                "add `::<output_name>` after the call; an instantiated DAG without a projection is not a graph value"
                    .to_string(),
            ),
            Self::TooDeeplyNested => Some(format!(
                "the parser limits nesting to {MAX_NESTING_DEPTH} levels; simplify the nested syntax"
            )),
            Self::ZeroExponent => Some(
                "a zero power erases its term; remove the term (or the exponent) instead of raising to zero"
                    .to_string(),
            ),
            Self::NatSubtractionUnsupported => Some(
                "express the larger size additively instead, for example use `D[Fin(N + 1)]` for the input and `D[Fin(N)]` for the smaller output"
                    .to_string(),
            ),
            Self::ExpectedIndexFoundNat { expression } => Some(format!(
                "write `Fin({expression})` for an explicit finite structural index"
            )),
            Self::ObsoleteStructuralRange { cardinality } => Some(format!(
                "write `Fin({cardinality})`; `range` is reserved for coordinate indexes with an explicit `step:`"
            )),
            Self::DuplicatePlotField { .. } => Some(
                "each field may appear at most once; remove or rename the duplicate".to_string(),
            ),
            Self::MissingPlotEncoding => Some(
                "add an `encode:` block with at least one channel, e.g. `encode: { x: ..., y: ... }`"
                    .to_string(),
            ),
            Self::EmptyCompositionPlots { .. } => {
                Some("add a non-empty `plots:` list, e.g. `plots: [my_plot]`".to_string())
            }
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::DuplicateDagBinding { first, .. } => vec![SecondaryLabel {
                span: *first,
                text: "first bound here".to_string(),
            }],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reason renders exactly the text the former string payloads held.
    #[test]
    fn every_invalid_number_reason_renders_its_legacy_text() {
        let cases = [
            (
                InvalidNumberReason::Float("x".parse::<f64>().expect_err("not a float")),
                "invalid float literal",
            ),
            (
                InvalidNumberReason::Integer("x".parse::<i64>().expect_err("not an integer")),
                "invalid digit found in string",
            ),
            (
                InvalidNumberReason::NonFiniteFloat,
                "floating-point literal must be finite",
            ),
            (
                InvalidNumberReason::IntegerWithUnits {
                    literal: "3".to_string(),
                },
                "integer literal cannot have units; write `3.0` instead",
            ),
            (
                InvalidNumberReason::NatLiteral,
                "expected a non-negative integer in a Nat expression",
            ),
            (
                InvalidNumberReason::IndexPosition,
                "expected non-negative integer in index position",
            ),
            (
                InvalidNumberReason::AttributePosition,
                "expected non-negative integer after `#` in attribute argument",
            ),
            (
                InvalidNumberReason::TableFinCardinality,
                "table Fin cardinality must be a non-negative integer literal",
            ),
            (
                InvalidNumberReason::TableValueCount,
                "table value count does not fit in u64",
            ),
            (
                InvalidNumberReason::TableColumnPosition,
                "table column position does not fit in u64",
            ),
            (
                InvalidNumberReason::FiniteRowPosition,
                "finite table row position does not fit u64",
            ),
            (
                InvalidNumberReason::SliceLabel,
                "expected non-negative integer in slice label",
            ),
            (
                InvalidNumberReason::SliceIndexOutOfRange {
                    value: 4,
                    cardinality: 3,
                },
                "slice index #4 out of range for Fin(3)",
            ),
            (
                InvalidNumberReason::PowerExponentZeroDenominator,
                "power exponent denominator must be non-zero",
            ),
            (
                InvalidNumberReason::PowerExponentOverflow,
                "exact power exponent overflows `i64`",
            ),
            (
                InvalidNumberReason::DimensionExponentOverflow,
                "dimension exponent overflows `i32`",
            ),
            (
                InvalidNumberReason::UnitExponentOverflow,
                "unit exponent overflows `i32`",
            ),
            (
                InvalidNumberReason::ExponentOutOfRange,
                "exponent is out of range",
            ),
            (
                InvalidNumberReason::ExponentZeroDenominator,
                "exponent denominator must be a non-zero integer",
            ),
            (
                InvalidNumberReason::UnitNumeratorNotOne,
                "only `1` can appear as a unit numerator (e.g. `1/min`)",
            ),
            (InvalidNumberReason::ExpectedInteger, "expected integer"),
        ];
        for (reason, text) in cases {
            assert_eq!(reason.to_string(), text, "{reason:?}");
        }
    }

    #[test]
    fn every_unsupported_shape_renders_its_legacy_text() {
        let slot = DeclName::try_new("s").expect("valid declaration name");
        let axis = |name: &str| NamePath::local(NameAtom::parse(name).expect("valid name atom"));
        let cases = [
            (
                UnsupportedMultiDeclShape::MultipleExtraAxisSlots,
                "multi-decl with more than one extra-axis slot is not yet supported (v3)"
                    .to_string(),
            ),
            (
                UnsupportedMultiDeclShape::MissingSliceSection,
                "multi-decl with multiple shared axes requires at least one `[slice]` section"
                    .to_string(),
            ),
            (
                UnsupportedMultiDeclShape::SliceLabelAxis {
                    label_axis: axis("A"),
                    shared_axis: axis("B"),
                },
                "slice label qualifies axis `A`, but the shared axis at this position is `B`"
                    .to_string(),
            ),
            (
                UnsupportedMultiDeclShape::UnderscoreHeaderRequired { slot: slot.clone() },
                "header cell for 1-D slot `s` must be `_`".to_string(),
            ),
            (
                UnsupportedMultiDeclShape::MissingVariantCells { slot },
                "slot `s` is declared with an extra axis but has zero variant cells in the header row"
                    .to_string(),
            ),
        ];
        for (shape, text) in cases {
            assert_eq!(shape.to_string(), text, "{shape:?}");
        }
    }

    #[test]
    fn plot_field_contexts_render_their_block_names() {
        let cases = [
            (PlotFieldContext::PlotDeclaration, "plot declaration"),
            (PlotFieldContext::MarkProperties, "mark properties"),
            (PlotFieldContext::EncodeBlock, "encode block"),
            (
                PlotFieldContext::Composition(CompositionKind::Figure),
                "figure",
            ),
            (
                PlotFieldContext::Composition(CompositionKind::Layer),
                "layer",
            ),
        ];
        for (context, text) in cases {
            assert_eq!(context.to_string(), text);
        }
    }

    #[test]
    fn multi_decl_arity_messages_pluralize_their_counts() {
        let tuple = |slot_count, tuple_count| {
            ParseErrorKind::MultiDeclTupleArity {
                slot_count,
                tuple_count,
            }
            .to_string()
        };
        assert_eq!(
            tuple(2, 1),
            "multi-decl slot tuple has 1 entry, but the multi-decl declares 2 slots"
        );
        assert_eq!(
            tuple(1, 2),
            "multi-decl slot tuple has 2 entries, but the multi-decl declares 1 slot"
        );
        let header = ParseErrorKind::MultiDeclHeaderArity {
            slot_count: 1,
            header_count: 1,
        };
        assert_eq!(
            header.to_string(),
            "multi-decl header row has 1 cell, but the multi-decl declares 1 slot"
        );
    }

    #[test]
    fn located_errors_keep_their_kind_and_span() {
        let mut registry = crate::source_registry::SourceRegistry::new();
        let src = registry.register("main.gcl", std::sync::Arc::new(String::new()));
        let diagnostic =
            ParseError::new(ParseErrorKind::ZeroExponent, Span::new(2, 3)).located(src);
        assert_eq!(diagnostic.src, src);
        assert_eq!(diagnostic.primary, Span::new(2, 3));
        assert!(matches!(diagnostic.kind, ParseErrorKind::ZeroExponent));
    }
}
