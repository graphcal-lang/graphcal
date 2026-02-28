//! Data types and function classification constants used by the name resolution layer.
//!
//! These types have no dependency on the resolution logic itself, making them
//! suitable for use across all compilation phases.

use std::collections::{HashMap, HashSet};

use graphcal_syntax::ast::{AssertBody, Expr, FigureDecl, FnDecl, LayerDecl, PlotDecl};
use graphcal_syntax::names::{IndexName, VariantName};
use graphcal_syntax::span::Span;

// ---------------------------------------------------------------------------
// Function classification constants
// ---------------------------------------------------------------------------

/// Aggregation functions recognized as special forms (not registered as builtins).
pub const AGGREGATION_FNS: &[&str] = &["sum", "min", "max", "mean", "count"];
pub const CONVERSION_FNS: &[&str] = &[
    "to_float", "to_int", "to_utc", "to_tai", "to_tt", "to_tdb", "to_et", "to_gpst", "to_gst",
    "to_bdt", "to_qzsst",
];
/// Constructor functions that create values from string literals (not registered as builtins).
pub const CONSTRUCTOR_FNS: &[&str] = &["datetime", "epoch"];
/// Functions that construct a Datetime from a numeric value (Julian Date, MJD, Unix).
pub const DATETIME_FROM_FNS: &[&str] = &["from_jd", "from_mjd", "from_unix"];
/// Functions that convert a Datetime to a numeric value (Julian Date, MJD, Unix).
pub const DATETIME_TO_FNS: &[&str] = &["to_jd", "to_mjd", "to_unix"];
/// Datetime component extraction functions.
pub const DATETIME_EXTRACT_FNS: &[&str] = &[
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
    "weekday",
    "day_of_year",
];

/// Classification of special built-in functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialFnKind {
    /// Aggregation functions: `sum`, `min`, `max`, `mean`, `count`.
    Aggregation,
    /// Conversion functions: `to_float`, `to_int`, `to_utc`, etc.
    Conversion,
    /// Constructor functions: `datetime`, `epoch`.
    Constructor,
    /// Datetime extraction functions: `year`, `month`, `day`, etc.
    DatetimeExtract,
    /// Datetime-from-numeric functions: `from_jd`, `from_mjd`, `from_unix`.
    DatetimeFrom,
    /// Datetime-to-numeric functions: `to_jd`, `to_mjd`, `to_unix`.
    DatetimeTo,
}

/// Classify a function name as a special built-in function.
///
/// Returns `None` if the name is not a recognized special function.
#[must_use]
pub fn classify_special_fn(name: &str) -> Option<SpecialFnKind> {
    if AGGREGATION_FNS.contains(&name) {
        Some(SpecialFnKind::Aggregation)
    } else if CONVERSION_FNS.contains(&name) {
        Some(SpecialFnKind::Conversion)
    } else if CONSTRUCTOR_FNS.contains(&name) {
        Some(SpecialFnKind::Constructor)
    } else if DATETIME_EXTRACT_FNS.contains(&name) {
        Some(SpecialFnKind::DatetimeExtract)
    } else if DATETIME_FROM_FNS.contains(&name) {
        Some(SpecialFnKind::DatetimeFrom)
    } else if DATETIME_TO_FNS.contains(&name) {
        Some(SpecialFnKind::DatetimeTo)
    } else {
        None
    }
}

/// Returns `true` if `name` is a built-in aggregation function (`sum`, `min`, etc.).
#[must_use]
pub fn is_aggregation_fn(name: &str) -> bool {
    AGGREGATION_FNS.contains(&name)
}

/// Returns `true` if `name` is a built-in conversion function (`to_float`, `to_int`).
#[must_use]
pub fn is_conversion_fn(name: &str) -> bool {
    CONVERSION_FNS.contains(&name)
}

/// Returns `true` if `name` is a constructor function (`datetime`, `epoch`).
#[must_use]
pub fn is_constructor_fn(name: &str) -> bool {
    CONSTRUCTOR_FNS.contains(&name)
}

/// Returns `true` if `name` is a datetime extraction function (`year`, `month`, etc.).
#[must_use]
pub fn is_datetime_extract_fn(name: &str) -> bool {
    DATETIME_EXTRACT_FNS.contains(&name)
}

/// Returns `true` if `name` is a datetime-from-numeric constructor (`from_jd`, etc.).
#[must_use]
pub fn is_datetime_from_fn(name: &str) -> bool {
    DATETIME_FROM_FNS.contains(&name)
}

/// Returns `true` if `name` is a datetime-to-numeric function (`to_jd`, etc.).
#[must_use]
pub fn is_datetime_to_fn(name: &str) -> bool {
    DATETIME_TO_FNS.contains(&name)
}

/// Returns `true` if `name` is a time scale identifier (`UTC`, `TT`, `TAI`, etc.).
#[must_use]
pub fn is_time_scale_name(name: &str) -> bool {
    crate::time_scale::TimeScale::ALL_NAMES.contains(&name)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A declaration name that may optionally be module-qualified.
///
/// Selective imports produce `Local` names (`x`), while module imports produce
/// `Qualified` names (`module::x`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopedName {
    /// A bare local name: `x`, `G0`, etc.
    Local(String),
    /// A module-qualified name: `module::x`, `constants::G0`, etc.
    Qualified { module: String, member: String },
}

impl ScopedName {
    /// Create a `Local` name.
    #[must_use]
    pub fn local(name: impl Into<String>) -> Self {
        Self::Local(name.into())
    }

    /// Create a `Qualified` name.
    #[must_use]
    pub fn qualified(module: impl Into<String>, member: impl Into<String>) -> Self {
        Self::Qualified {
            module: module.into(),
            member: member.into(),
        }
    }

    /// Returns the member (leaf) part of the name.
    ///
    /// For `Local("x")` this returns `"x"`.
    /// For `Qualified { module: "m", member: "x" }` this also returns `"x"`.
    #[must_use]
    pub fn member(&self) -> &str {
        match self {
            Self::Local(name) => name,
            Self::Qualified { member, .. } => member,
        }
    }

    /// Returns the module part, if qualified.
    #[must_use]
    pub fn module(&self) -> Option<&str> {
        match self {
            Self::Qualified { module, .. } => Some(module),
            Self::Local(_) => None,
        }
    }

    /// Returns whether this is a qualified name.
    #[must_use]
    pub const fn is_qualified(&self) -> bool {
        matches!(self, Self::Qualified { .. })
    }

    /// Qualify a name with a prefix.
    ///
    /// `Local("x").with_prefix("p")` → `Qualified { module: "p", member: "x" }`.
    /// `Qualified { module: "m", member: "x" }.with_prefix("p")` → `Qualified { module: "p", member: "x" }`.
    #[must_use]
    pub fn with_prefix(&self, prefix: &str) -> Self {
        Self::Qualified {
            module: prefix.to_string(),
            member: self.member().to_string(),
        }
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(name) => write!(f, "{name}"),
            Self::Qualified { module, member } => write!(f, "{module}::{member}"),
        }
    }
}

/// Pre-evaluated value bindings imported from already-evaluated dependency files.
///
/// Unlike `ImportedNames` which carries AST expressions, this carries
/// evaluated values. Used in per-file evaluation where each file is
/// compiled and evaluated independently.
#[derive(Debug, Default)]
pub struct ImportedValueNames {
    /// Imported const names (for scope checking only — actual values are in the exec plan).
    pub const_names: Vec<(ScopedName, Span)>,
    /// Imported param names.
    pub param_names: Vec<(ScopedName, Span)>,
    /// Imported node names.
    pub node_names: Vec<(ScopedName, Span)>,
    /// Imported function declarations (still need AST for compilation).
    pub functions: Vec<ResolvedFunctionEntry>,
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
    pub hidden: bool,
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

/// A resolved function declaration.
#[derive(Debug, Clone)]
pub struct ResolvedFunctionEntry {
    pub name: String,
    pub decl: FnDecl,
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
    /// Specific index keys are expected to fail: `#[expected_fail(Index::Variant, ...)]`.
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
    pub runtime_deps: HashMap<String, HashSet<String>>,
    /// For each const, the set of `CONST_REF` references (const deps).
    pub const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations.
    pub functions: Vec<ResolvedFunctionEntry>,
    /// Set of all assert names (for checking `@assert_name` errors).
    pub assert_names: HashSet<String>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub assumes_map: HashMap<String, Vec<String>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Built from `#[expected_fail]` / `#[expected_fail(...)]` attributes.
    pub expected_fail: HashMap<String, ExpectedFail>,
}
