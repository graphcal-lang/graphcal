//! Index diagnostics: unknown, missing, and mismatched index variants and bindings.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::hir::expr::LocalId;
use crate::nat::NatOverflowError;
use crate::resolved_name::ResolvedIndexVariant;
use crate::semantic::checked_type::{IndexDisplayName, TypeSpelling};
use crate::semantic::dimension_table::DimensionSpelling;
use crate::semantic::index_def::{CoordinateIndexError, IndexBindingTarget, IndexCardinalityError};
use crate::syntax::ast::{KeyFormKind, NatExpr};
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use crate::syntax::names::NameAtom;
use crate::tir::static_index::StaticIndexError;

/// One coordinate of a map-literal entry: a declared label, or a position of
/// a structural `Fin(N)` axis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapEntryCoordinate {
    Declared(ResolvedIndexVariant),
    Position {
        axis: IndexDisplayName,
        position: u64,
    },
}

impl std::fmt::Display for MapEntryCoordinate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(variant) => variant.fmt(f),
            Self::Position { axis, position } => write!(f, "{axis}.#{position}"),
        }
    }
}

/// Coordinate-constructor arguments whose dimensions disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinateArgumentDimensions {
    Range {
        start: DimensionSpelling,
        end: DimensionSpelling,
        step: DimensionSpelling,
    },
    Linspace {
        start: DimensionSpelling,
        end: DimensionSpelling,
    },
}

impl std::fmt::Display for CoordinateArgumentDimensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Range { start, end, step } => write!(
                f,
                "range start, end, and step have dimensions {start}, {end}, and {step}"
            ),
            Self::Linspace { start, end } => {
                write!(
                    f,
                    "linspace start and end have dimensions {start} and {end}"
                )
            }
        }
    }
}

/// A Nat written where an explicit Index is required.
#[derive(Debug, Clone)]
pub enum FoundNat {
    /// A Nat expression, such as `3` or `N + 1`.
    Expression(NatExpr),
    /// A generic Nat parameter named where an Index parameter is required.
    Parameter(NameAtom),
}

/// Two found Nats are equal when they are the same expression up to source
/// layout, or name the same parameter.
impl PartialEq for FoundNat {
    fn eq(&self, other: &Self) -> bool {
        use crate::syntax::format_equivalent::FormatEquivalent as _;
        match (self, other) {
            (Self::Expression(lhs), Self::Expression(rhs)) => lhs.format_equivalent(rhs),
            (Self::Parameter(lhs), Self::Parameter(rhs)) => lhs == rhs,
            (Self::Expression(_), Self::Parameter(_))
            | (Self::Parameter(_), Self::Expression(_)) => false,
        }
    }
}

impl Eq for FoundNat {}

impl std::fmt::Display for FoundNat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expression(expression) => expression.fmt(f),
            Self::Parameter(name) => name.fmt(f),
        }
    }
}

/// Index diagnostics: unknown, missing, and mismatched index variants and bindings.
#[derive(Debug, Clone, Error)]
pub enum IndexError {
    #[error("unknown index `{name}`")]
    UnknownIndex { name: IndexDisplayName },
    #[error("unknown variant `{variant_name}` in index `{index_name}`")]
    UnknownVariant {
        index_name: IndexDisplayName,
        variant_name: IndexVariantName,
    },
    #[error(
        "missing variant(s) [{}] in map literal for index `{index_name}`",
        format_index_entry_keys(missing)
    )]
    MissingVariants {
        index_name: IndexDisplayName,
        missing: Vec<IndexEntryKey>,
    },
    #[error(
        "extra variant(s) [{}] in map literal for index `{index_name}`",
        format_index_entry_keys(extra)
    )]
    ExtraVariants {
        index_name: IndexDisplayName,
        extra: Vec<IndexEntryKey>,
    },
    #[error("index mismatch: expected `{expected}`, found `{found}`")]
    IndexMismatch {
        expected: IndexDisplayName,
        found: IndexDisplayName,
    },
    #[error("coordinate index `{name}`: {mismatch}")]
    CoordinateIndexDimensionMismatch {
        name: IndexName,
        mismatch: CoordinateArgumentDimensions,
    },
    #[error("coordinate index `{name}`: {error}")]
    CoordinateIndexInvalid {
        name: IndexName,
        error: CoordinateIndexError,
    },
    #[error("expected Index, found Nat `{expression}`")]
    ExpectedIndexFoundNat { expression: FoundNat },
    #[error(
        "index dimension mismatch: `{dep_index}` requires dimension {expected_dim} but `{bound_index}` has dimension {found_dim}"
    )]
    IndexBindingDimensionMismatch {
        dep_index: IndexName,
        expected_dim: DimensionSpelling,
        bound_index: IndexBindingTarget,
        found_dim: DimensionSpelling,
    },
    /// A required typed Static input was not bound at a DAG instantiation boundary.
    #[error("required {kind} `{name}` must be bound at DAG instantiation")]
    RequiredStaticInputNotBound {
        kind: crate::static_interface::StaticInputKind,
        name: NameAtom,
    },
    #[error(
        "key() constructs Fin-axis keys; named-axis keys are written as qualified labels and coordinate keys come from argmax/argmin or the coordinate searches"
    )]
    KeyRequiresFiniteAxis,
    #[error("key() requires a static position; use fin_key() for a runtime-checked position")]
    KeyPositionNotStatic,
    #[error("key() position evaluated to negative value: {position}")]
    NegativeKeyPosition { position: i64 },
    #[error("key() position {position} is out of bounds for {axis}")]
    KeyPositionOutOfBounds {
        position: i64,
        axis: IndexDisplayName,
    },
    #[error("fin_key() requires a Fin(...) axis, got `{axis}`")]
    FinKeyRequiresFiniteAxis { axis: IndexDisplayName },
    #[error(
        "{}() requires a coordinate axis, got `{axis}`",
        function.as_str()
    )]
    CoordinateSearchRequiresCoordinateAxis {
        function: KeyFormKind,
        axis: IndexDisplayName,
    },
    #[error("indexing a non-indexed value")]
    IndexingNonIndexedValue,
    #[error(
        "quantity local cannot index into coordinate index `{index}`; use that coordinate index's loop variable"
    )]
    QuantityLocalIndexesCoordinateIndex { index: IndexDisplayName },
    #[error(
        "`#{}` is not a valid index variable",
        local.index()
    )]
    InvalidIndexVariable { local: LocalId },
    #[error("integer expression cannot index into non-finite-index index `{index}`")]
    IntegerIndexIntoNonFiniteIndex { index: IndexDisplayName },
    #[error(
        "a runtime Int cannot index `{index}` implicitly; write `fin_key({index}, ...)` to make the range check explicit"
    )]
    ImplicitRuntimeIntIndex { index: IndexDisplayName },
    #[error("index expression must be an integer type, got {found}")]
    NonIntegerIndexExpression { found: TypeSpelling },
    #[error("index expression evaluated to negative value: {index}")]
    NegativeIndex { index: i64 },
    #[error("index {index} out of bounds for {axis}")]
    IndexOutOfBounds { index: i64, axis: IndexDisplayName },
    #[error("{error}")]
    StaticIndexOutOfBounds { error: StaticIndexError },
    #[error("map literal key-space cardinality exceeds supported size")]
    MapKeySpaceTooLarge,
    #[error("empty map literal")]
    EmptyMapLiteral,
    #[error("map literal entries have inconsistent key arity: expected {expected}, found {found}")]
    InconsistentMapKeyArity { expected: usize, found: usize },
    #[error(
        "coordinate index `{index}` cannot be used as a map/table literal key; use a `for` comprehension instead"
    )]
    CoordinateIndexMapKey { index: IndexDisplayName },
    #[error("map entry key `{key}` does not match its index category")]
    MapKeyCategoryMismatch { key: IndexEntryKey },
    #[error("position #{position} is outside index `{index}`")]
    MapPositionOutsideIndex {
        position: u64,
        index: IndexDisplayName,
    },
    #[error("duplicate map literal entry")]
    DuplicateMapEntry,
    #[error(
        "non-exhaustive map literal: missing {missing_count} entries; first missing entry is ({})",
        format_map_entry(witness)
    )]
    NonExhaustiveMapLiteral {
        missing_count: std::num::NonZeroUsize,
        witness: Vec<MapEntryCoordinate>,
    },
    #[error(
        "map literal element type must be a value type, not an indexed type; use tuple keys for multi-axis map literals"
    )]
    IndexedMapElement,
    #[error("scan source must be an indexed value")]
    ScanSourceNotIndexed,
    #[error("unfold requires a coordinate index, got `{index}`")]
    UnfoldRequiresCoordinateIndex { index: IndexDisplayName },
    #[error(
        "{}",
        error.describe_finite_index()
    )]
    InvalidFiniteIndexCardinality { error: IndexCardinalityError },
    #[error("{error}")]
    NatOverflow { error: NatOverflowError },
    #[error("Fin cardinality addition overflow")]
    FinCardinalityAdditionOverflow,
    #[error("Fin cardinality multiplication overflow")]
    FinCardinalityMultiplicationOverflow,
    #[error("unresolved finite-index obligation `{index}`")]
    UnresolvedFiniteIndexObligation { index: IndexDisplayName },
    #[error("an indexed type cannot be indexed again; list every axis in one bracket list")]
    NestedIndexedType,
    #[error("map literal entry has no keys")]
    EmptyMapEntry,
}

impl DiagnosticKind for IndexError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownIndex { .. } => "graphcal::I001",
            Self::UnknownVariant { .. } => "graphcal::I002",
            Self::MissingVariants { .. } => "graphcal::I003",
            Self::ExtraVariants { .. } => "graphcal::I004",
            Self::IndexMismatch { .. } => "graphcal::I005",
            Self::CoordinateIndexDimensionMismatch { .. } => "graphcal::I006",
            Self::CoordinateIndexInvalid { .. } => "graphcal::I007",
            Self::ExpectedIndexFoundNat { .. } => "graphcal::I008",
            Self::IndexBindingDimensionMismatch { .. } => "graphcal::I009",
            Self::RequiredStaticInputNotBound { .. } => "graphcal::I010",
            Self::KeyRequiresFiniteAxis => "graphcal::I011",
            Self::KeyPositionNotStatic => "graphcal::I012",
            Self::NegativeKeyPosition { .. } => "graphcal::I013",
            Self::KeyPositionOutOfBounds { .. } => "graphcal::I014",
            Self::FinKeyRequiresFiniteAxis { .. } => "graphcal::I015",
            Self::CoordinateSearchRequiresCoordinateAxis { .. } => "graphcal::I016",
            Self::IndexingNonIndexedValue => "graphcal::I017",
            Self::QuantityLocalIndexesCoordinateIndex { .. } => "graphcal::I018",
            Self::InvalidIndexVariable { .. } => "graphcal::I019",
            Self::IntegerIndexIntoNonFiniteIndex { .. } => "graphcal::I020",
            Self::ImplicitRuntimeIntIndex { .. } => "graphcal::I021",
            Self::NonIntegerIndexExpression { .. } => "graphcal::I022",
            Self::NegativeIndex { .. } => "graphcal::I023",
            Self::IndexOutOfBounds { .. } => "graphcal::I024",
            Self::StaticIndexOutOfBounds { .. } => "graphcal::I025",
            Self::MapKeySpaceTooLarge => "graphcal::I026",
            Self::EmptyMapLiteral => "graphcal::I027",
            Self::InconsistentMapKeyArity { .. } => "graphcal::I028",
            Self::CoordinateIndexMapKey { .. } => "graphcal::I029",
            Self::MapKeyCategoryMismatch { .. } => "graphcal::I030",
            Self::MapPositionOutsideIndex { .. } => "graphcal::I031",
            Self::DuplicateMapEntry => "graphcal::I032",
            Self::NonExhaustiveMapLiteral { .. } => "graphcal::I033",
            Self::IndexedMapElement => "graphcal::I034",
            Self::ScanSourceNotIndexed => "graphcal::I035",
            Self::UnfoldRequiresCoordinateIndex { .. } => "graphcal::I036",
            Self::InvalidFiniteIndexCardinality { .. } => "graphcal::I037",
            Self::NatOverflow { .. } => "graphcal::I038",
            Self::FinCardinalityAdditionOverflow => "graphcal::I039",
            Self::FinCardinalityMultiplicationOverflow => "graphcal::I040",
            Self::UnresolvedFiniteIndexObligation { .. } => "graphcal::I041",
            Self::NestedIndexedType => "graphcal::I042",
            Self::EmptyMapEntry => "graphcal::I043",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::UnknownIndex { .. } => Some("unknown index".to_owned()),
            Self::UnknownVariant { index_name, .. } => {
                Some(format!("not a variant of `{index_name}`"))
            }
            Self::MissingVariants { .. } => Some("incomplete map literal".to_owned()),
            Self::ExtraVariants { .. } => Some("unexpected variants".to_owned()),
            Self::IndexMismatch { .. } => Some("wrong index".to_owned()),
            Self::CoordinateIndexDimensionMismatch { .. }
            | Self::IndexBindingDimensionMismatch { .. } => Some("dimension mismatch".to_owned()),
            Self::CoordinateIndexInvalid { .. } => Some("invalid coordinate index".to_owned()),
            Self::ExpectedIndexFoundNat { .. } => {
                Some("Nat is not implicitly converted to Index".to_owned())
            }
            Self::RequiredStaticInputNotBound { kind, .. } => {
                Some(format!("required {kind} input is not bound"))
            }
            Self::KeyRequiresFiniteAxis
            | Self::KeyPositionNotStatic
            | Self::NegativeKeyPosition { .. }
            | Self::KeyPositionOutOfBounds { .. }
            | Self::FinKeyRequiresFiniteAxis { .. }
            | Self::CoordinateSearchRequiresCoordinateAxis { .. }
            | Self::IndexingNonIndexedValue
            | Self::QuantityLocalIndexesCoordinateIndex { .. }
            | Self::InvalidIndexVariable { .. }
            | Self::IntegerIndexIntoNonFiniteIndex { .. }
            | Self::ImplicitRuntimeIntIndex { .. }
            | Self::NonIntegerIndexExpression { .. }
            | Self::NegativeIndex { .. }
            | Self::IndexOutOfBounds { .. }
            | Self::StaticIndexOutOfBounds { .. }
            | Self::MapKeySpaceTooLarge
            | Self::EmptyMapLiteral
            | Self::InconsistentMapKeyArity { .. }
            | Self::CoordinateIndexMapKey { .. }
            | Self::MapKeyCategoryMismatch { .. }
            | Self::MapPositionOutsideIndex { .. }
            | Self::DuplicateMapEntry
            | Self::NonExhaustiveMapLiteral { .. }
            | Self::IndexedMapElement
            | Self::ScanSourceNotIndexed
            | Self::UnfoldRequiresCoordinateIndex { .. }
            | Self::InvalidFiniteIndexCardinality { .. }
            | Self::NatOverflow { .. }
            | Self::FinCardinalityAdditionOverflow
            | Self::FinCardinalityMultiplicationOverflow
            | Self::UnresolvedFiniteIndexObligation { .. }
            | Self::NestedIndexedType
            | Self::EmptyMapEntry => Some("error here".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UnknownIndex { .. } => Some("declare a named or coordinate index, or write `Fin(N)` explicitly for a structural axis; coordinate constructors are `range(start, end, step: delta)` and `linspace(start, end, points: N)`".to_owned()),
            Self::UnknownVariant { .. }
            | Self::IndexMismatch { .. }
            | Self::KeyRequiresFiniteAxis
            | Self::KeyPositionNotStatic
            | Self::NegativeKeyPosition { .. }
            | Self::KeyPositionOutOfBounds { .. }
            | Self::FinKeyRequiresFiniteAxis { .. }
            | Self::CoordinateSearchRequiresCoordinateAxis { .. }
            | Self::IndexingNonIndexedValue
            | Self::QuantityLocalIndexesCoordinateIndex { .. }
            | Self::InvalidIndexVariable { .. }
            | Self::IntegerIndexIntoNonFiniteIndex { .. }
            | Self::ImplicitRuntimeIntIndex { .. }
            | Self::NonIntegerIndexExpression { .. }
            | Self::NegativeIndex { .. }
            | Self::IndexOutOfBounds { .. }
            | Self::StaticIndexOutOfBounds { .. }
            | Self::MapKeySpaceTooLarge
            | Self::EmptyMapLiteral
            | Self::InconsistentMapKeyArity { .. }
            | Self::CoordinateIndexMapKey { .. }
            | Self::MapKeyCategoryMismatch { .. }
            | Self::MapPositionOutsideIndex { .. }
            | Self::DuplicateMapEntry
            | Self::NonExhaustiveMapLiteral { .. }
            | Self::IndexedMapElement
            | Self::ScanSourceNotIndexed
            | Self::UnfoldRequiresCoordinateIndex { .. }
            | Self::InvalidFiniteIndexCardinality { .. }
            | Self::NatOverflow { .. }
            | Self::FinCardinalityAdditionOverflow
            | Self::FinCardinalityMultiplicationOverflow
            | Self::UnresolvedFiniteIndexObligation { .. }
            | Self::NestedIndexedType
            | Self::EmptyMapEntry => None,
            Self::MissingVariants { .. } => Some("map literals must cover all variants of the index".to_owned()),
            Self::ExtraVariants { .. } => Some("only variants declared in the index are allowed".to_owned()),
            Self::CoordinateIndexDimensionMismatch { .. } => Some("coordinate constructor arguments must have exactly the same dimension".to_owned()),
            Self::CoordinateIndexInvalid { error, .. } => Some(error.help()),
            Self::ExpectedIndexFoundNat { expression, .. } => Some(format!("write `Fin({expression})` for an explicit finite structural index")),
            Self::IndexBindingDimensionMismatch { .. } => Some("coordinate-index bindings must have matching dimensions".to_owned()),
            Self::RequiredStaticInputNotBound { kind, .. } => Some(format!("bind the input with its explicit `{kind}` marker at the include or direct-call site")),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::UnknownIndex { .. }
            | Self::UnknownVariant { .. }
            | Self::MissingVariants { .. }
            | Self::ExtraVariants { .. }
            | Self::IndexMismatch { .. }
            | Self::CoordinateIndexDimensionMismatch { .. }
            | Self::CoordinateIndexInvalid { .. }
            | Self::ExpectedIndexFoundNat { .. }
            | Self::IndexBindingDimensionMismatch { .. }
            | Self::RequiredStaticInputNotBound { .. }
            | Self::KeyRequiresFiniteAxis
            | Self::KeyPositionNotStatic
            | Self::NegativeKeyPosition { .. }
            | Self::KeyPositionOutOfBounds { .. }
            | Self::FinKeyRequiresFiniteAxis { .. }
            | Self::CoordinateSearchRequiresCoordinateAxis { .. }
            | Self::IndexingNonIndexedValue
            | Self::QuantityLocalIndexesCoordinateIndex { .. }
            | Self::InvalidIndexVariable { .. }
            | Self::IntegerIndexIntoNonFiniteIndex { .. }
            | Self::ImplicitRuntimeIntIndex { .. }
            | Self::NonIntegerIndexExpression { .. }
            | Self::NegativeIndex { .. }
            | Self::IndexOutOfBounds { .. }
            | Self::StaticIndexOutOfBounds { .. }
            | Self::MapKeySpaceTooLarge
            | Self::EmptyMapLiteral
            | Self::InconsistentMapKeyArity { .. }
            | Self::CoordinateIndexMapKey { .. }
            | Self::MapKeyCategoryMismatch { .. }
            | Self::MapPositionOutsideIndex { .. }
            | Self::DuplicateMapEntry
            | Self::NonExhaustiveMapLiteral { .. }
            | Self::IndexedMapElement
            | Self::ScanSourceNotIndexed
            | Self::UnfoldRequiresCoordinateIndex { .. }
            | Self::InvalidFiniteIndexCardinality { .. }
            | Self::NatOverflow { .. }
            | Self::FinCardinalityAdditionOverflow
            | Self::FinCardinalityMultiplicationOverflow
            | Self::UnresolvedFiniteIndexObligation { .. }
            | Self::NestedIndexedType
            | Self::EmptyMapEntry => Vec::new(),
        }
    }
}

fn format_index_entry_keys(keys: &[IndexEntryKey]) -> String {
    keys.iter()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_map_entry(coordinates: &[MapEntryCoordinate]) -> String {
    coordinates
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
