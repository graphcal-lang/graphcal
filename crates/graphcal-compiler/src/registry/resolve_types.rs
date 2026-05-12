//! Data types and function classification constants used by the name resolution layer.
//!
//! These types have no dependency on the resolution logic itself, making them
//! suitable for use across all compilation phases.

use std::collections::{HashMap, HashSet};

use crate::desugar::resolved_ast::{AssertBody, Expr, FigureDecl, LayerDecl, PlotDecl};
use crate::syntax::names::{DeclName, IndexName, ScopedName, VariantName};
use crate::syntax::span::Span;

// ---------------------------------------------------------------------------
// Function classification enums
// ---------------------------------------------------------------------------
//
// Each special-function category has its own enum whose variants are the
// canonical source of truth for the function names in that category.
// The `classify_special_fn` function maps a string to one of these.

/// Aggregation functions: operate on indexed collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationFn {
    Sum,
    Min,
    Max,
    Mean,
    Count,
}

impl AggregationFn {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "sum" => Some(Self::Sum),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "mean" => Some(Self::Mean),
            "count" => Some(Self::Count),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Min => "min",
            Self::Max => "max",
            Self::Mean => "mean",
            Self::Count => "count",
        }
    }
}

/// Type conversion functions: `to_float`, `to_int`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConversionFn {
    ToFloat,
    ToInt,
}

impl TypeConversionFn {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "to_float" => Some(Self::ToFloat),
            "to_int" => Some(Self::ToInt),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToFloat => "to_float",
            Self::ToInt => "to_int",
        }
    }
}

/// Datetime constructor functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructorFn {
    Datetime,
    Epoch,
}

impl ConstructorFn {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "datetime" => Some(Self::Datetime),
            "epoch" => Some(Self::Epoch),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Datetime => "datetime",
            Self::Epoch => "epoch",
        }
    }
}

/// Datetime extraction functions: extract a component from a `Datetime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatetimeExtractFn {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Weekday,
    DayOfYear,
}

impl DatetimeExtractFn {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "day" => Some(Self::Day),
            "hour" => Some(Self::Hour),
            "minute" => Some(Self::Minute),
            "second" => Some(Self::Second),
            "weekday" => Some(Self::Weekday),
            "day_of_year" => Some(Self::DayOfYear),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
            Self::Weekday => "weekday",
            Self::DayOfYear => "day_of_year",
        }
    }
}

/// Datetime-from-numeric constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatetimeFromFn {
    FromJd,
    FromMjd,
    FromUnix,
}

impl DatetimeFromFn {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "from_jd" => Some(Self::FromJd),
            "from_mjd" => Some(Self::FromMjd),
            "from_unix" => Some(Self::FromUnix),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FromJd => "from_jd",
            Self::FromMjd => "from_mjd",
            Self::FromUnix => "from_unix",
        }
    }
}

/// Datetime-to-numeric functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatetimeToFn {
    ToJd,
    ToMjd,
    ToUnix,
}

impl DatetimeToFn {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "to_jd" => Some(Self::ToJd),
            "to_mjd" => Some(Self::ToMjd),
            "to_unix" => Some(Self::ToUnix),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToJd => "to_jd",
            Self::ToMjd => "to_mjd",
            Self::ToUnix => "to_unix",
        }
    }
}

/// Classification of special built-in functions.
///
/// Each variant carries a sub-enum identifying the specific function, so
/// downstream handlers can match on typed variants instead of raw strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialFnKind {
    /// Aggregation functions: `sum`, `min`, `max`, `mean`, `count`.
    Aggregation(AggregationFn),
    /// Type conversion functions: `to_float`, `to_int`.
    TypeConversion(TypeConversionFn),
    /// Time-scale conversion functions: `to_utc`, `to_tai`, etc.
    /// These are further resolved by [`crate::registry::time_scale::time_scale_from_conversion_fn`].
    TimeScaleConversion,
    /// Constructor functions: `datetime`, `epoch`.
    Constructor(ConstructorFn),
    /// Datetime extraction functions: `year`, `month`, `day`, etc.
    DatetimeExtract(DatetimeExtractFn),
    /// Datetime-from-numeric functions: `from_jd`, `from_mjd`, `from_unix`.
    DatetimeFrom(DatetimeFromFn),
    /// Datetime-to-numeric functions: `to_jd`, `to_mjd`, `to_unix`.
    DatetimeTo(DatetimeToFn),
}

/// Classify a function name as a special built-in function.
///
/// Returns `None` if the name is not a recognized special function.
#[must_use]
pub fn classify_special_fn(name: &str) -> Option<SpecialFnKind> {
    if let Some(f) = AggregationFn::parse(name) {
        return Some(SpecialFnKind::Aggregation(f));
    }
    if let Some(f) = TypeConversionFn::parse(name) {
        return Some(SpecialFnKind::TypeConversion(f));
    }
    if crate::registry::time_scale::time_scale_from_conversion_fn(name).is_some() {
        return Some(SpecialFnKind::TimeScaleConversion);
    }
    if let Some(f) = ConstructorFn::parse(name) {
        return Some(SpecialFnKind::Constructor(f));
    }
    if let Some(f) = DatetimeExtractFn::parse(name) {
        return Some(SpecialFnKind::DatetimeExtract(f));
    }
    if let Some(f) = DatetimeFromFn::parse(name) {
        return Some(SpecialFnKind::DatetimeFrom(f));
    }
    if let Some(f) = DatetimeToFn::parse(name) {
        return Some(SpecialFnKind::DatetimeTo(f));
    }
    None
}

/// Returns `true` if `name` is a built-in aggregation function (`sum`, `min`, etc.).
#[must_use]
pub fn is_aggregation_fn(name: &str) -> bool {
    AggregationFn::parse(name).is_some()
}

/// Returns `true` if `name` is a time scale identifier (`UTC`, `TT`, `TAI`, etc.).
#[must_use]
pub fn is_time_scale_name(name: &str) -> bool {
    crate::registry::time_scale::TimeScale::ALL_NAMES.contains(&name)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Pre-evaluated value bindings imported from already-evaluated dependency files.
///
/// Unlike `ImportedNames` which carries AST expressions, this carries
/// evaluated values. Used in per-file evaluation where each file is
/// compiled and evaluated independently.
#[derive(Debug, Default, Clone)]
pub struct ImportedValueNames {
    /// Imported const names (for scope checking only — actual values are in the exec plan).
    pub const_names: Vec<(ScopedName, Span)>,
    /// Imported param names.
    pub param_names: Vec<(ScopedName, Span)>,
    /// Imported node names.
    pub node_names: Vec<(ScopedName, Span)>,
    /// Imported assert names (for `#[assumes]` validation).
    pub assert_names: Vec<(String, Span)>,
}

/// The kind of a declaration (used for source-order tracking).
#[derive(Debug, Clone, Copy)]
pub enum DeclCategory {
    Const,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
}

impl std::fmt::Display for DeclCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Const => write!(f, "const"),
            Self::Param => write!(f, "param"),
            Self::Node => write!(f, "node"),
            Self::Assert => write!(f, "assert"),
            Self::Plot => write!(f, "plot"),
            Self::Figure => write!(f, "figure"),
            Self::Layer => write!(f, "layer"),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry types for resolved declarations
// ---------------------------------------------------------------------------

/// A resolved const declaration (before type annotation is added).
#[derive(Debug)]
pub struct ResolvedConstEntry {
    pub name: String,
    pub expr: Expr,
    pub span: Span,
}

/// A resolved param declaration (before type annotation is added).
#[derive(Debug)]
pub struct ResolvedParamEntry {
    pub name: String,
    pub default_expr: Option<Expr>,
    pub span: Span,
}

/// A resolved node declaration (before type annotation is added).
#[derive(Debug)]
pub struct ResolvedNodeEntry {
    pub name: String,
    pub expr: Expr,
    pub span: Span,
}

/// A resolved assert declaration.
#[derive(Debug)]
pub struct ResolvedAssertEntry {
    pub name: String,
    pub body: AssertBody,
    pub span: Span,
}

/// A resolved plot declaration.
#[derive(Debug)]
pub struct ResolvedPlotEntry {
    pub name: String,
    pub decl: PlotDecl,
    pub span: Span,
}

/// A resolved figure declaration.
#[derive(Debug)]
pub struct ResolvedFigureEntry {
    pub name: String,
    pub decl: FigureDecl,
    pub span: Span,
}

/// A resolved layer declaration.
#[derive(Debug)]
pub struct ResolvedLayerEntry {
    pub name: String,
    pub decl: LayerDecl,
    pub span: Span,
}

/// A single expected-fail key: a list of `(IndexName, VariantName)` pairs.
///
/// - Length 1 for single-index assertions: `[("Mode", "Boost")]`
/// - Length >1 for multi-index assertions: `[("Mode", "Boost"), ("Phase", "Launch")]`
pub type ExpectedFailKey = Vec<(IndexName, VariantName)>;

/// Describes how an assertion is expected to fail.
#[derive(Debug, Clone)]
pub enum ExpectedFail {
    /// The entire assertion is expected to fail: `#[expected_fail]`.
    All,
    /// Specific index keys are expected to fail: `#[expected_fail(Index.Variant, ...)]`.
    Variants(Vec<ExpectedFailKey>),
}

/// The result of name resolution: declarations separated by category with dependency info.
#[derive(Debug)]
pub struct ResolvedFile {
    /// Const declarations in source order.
    pub consts: Vec<ResolvedConstEntry>,
    /// Param declarations in source order.
    pub params: Vec<ResolvedParamEntry>,
    /// Node declarations in source order.
    pub nodes: Vec<ResolvedNodeEntry>,
    /// Assert declarations in source order.
    pub asserts: Vec<ResolvedAssertEntry>,
    /// Plot declarations in source order.
    pub plots: Vec<ResolvedPlotEntry>,
    /// Figure declarations in source order.
    pub figures: Vec<ResolvedFigureEntry>,
    /// Layer declarations in source order.
    pub layers: Vec<ResolvedLayerEntry>,
    /// For each node/param, the set of `@`-references (graph deps).
    /// Keys are bare locals (the file's own decls); values may be qualified
    /// when the ref targets an imported module member.
    pub runtime_deps:
        HashMap<crate::syntax::names::ScopedName, HashSet<crate::syntax::names::ScopedName>>,
    /// For each const, the set of `CONST_REF` references (const deps).
    /// Keys are bare locals (the file's own decls); values may be qualified
    /// when the ref targets an imported module member.
    pub const_deps:
        HashMap<crate::syntax::names::ScopedName, HashSet<crate::syntax::names::ScopedName>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(DeclName, DeclCategory)>,
    /// Set of all assert names (for checking `@assert_name` errors).
    pub assert_names: HashSet<DeclName>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub assumes_map: HashMap<String, Vec<String>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Built from `#[expected_fail]` / `#[expected_fail(...)]` attributes.
    pub expected_fail: HashMap<String, ExpectedFail>,
    /// Names of all declarations marked `pub` in this file (values + type-system).
    pub pub_names: HashSet<DeclName>,
}
