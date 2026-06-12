use std::collections::BTreeMap;

use indexmap::IndexMap;
use miette::Diagnostic;
use thiserror::Error;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::EncodingChannel;
use graphcal_compiler::registry::declared_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::syntax::dimension::{BaseDimId, Dimension, Rational};
use graphcal_compiler::syntax::names::{
    DeclName, FieldName, IndexName, IndexVariantName, ScopedName, StructTypeName,
};
use graphcal_compiler::syntax::span::Span;

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclType {
    Const,
    Param,
    Node,
}

/// Display unit metadata: the unit name(s) and scale factor for pretty-printing.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayUnit {
    /// Human-readable unit string (e.g., "km", "m/s^2", "km/hour")
    pub label: String,
    /// Scale factor from SI to this display unit: `display_value = si_value / scale`
    pub scale: f64,
}

/// A runtime value: either a scalar with dimension and display info, a bool, an integer, or a struct.
#[derive(Debug, Clone)]
pub enum Value {
    Scalar {
        /// The value in base SI units.
        si_value: f64,
        /// The dimension of this value.
        dimension: Dimension,
        /// Optional display unit for pretty-printing.
        display_unit: Option<DisplayUnit>,
    },
    Bool(bool),
    Int(i64),
    /// A label value from a named index (e.g., `Maneuver.Departure`).
    Label {
        /// The index identity (e.g., `Maneuver`), including a canonical owner when available.
        index_name: IndexTypeRef,
        /// The variant name (e.g., `Departure`).
        variant: IndexVariantName,
    },
    Struct {
        /// The concrete type/constructor display leaf plus canonical owning struct identity when available.
        type_name: StructTypeRef,
        /// Fields in definition order.
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values.
    Indexed {
        /// The index type identity, including a canonical owner when available.
        index_name: IndexTypeRef,
        /// Entries in declaration order, keyed by semantic variant leaves.
        entries: IndexMap<IndexVariantName, Self>,
        /// Optional display labels for entry keys (for example, range-index step values).
        ///
        /// These are presentation strings only. Semantic consumers must continue to
        /// use `entries` keys rather than parsing these labels.
        entry_display_names: Option<IndexMap<IndexVariantName, String>>,
    },
    /// A datetime instant.
    Datetime {
        /// The hifitime epoch (internal representation).
        epoch: hifitime::Epoch,
        /// The time scale for display purposes.
        time_scale: graphcal_compiler::registry::time_scale::TimeScale,
        /// Optional IANA timezone for display (e.g. `"America/New_York"`).
        display_tz: Option<String>,
    },
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Scalar {
                    si_value: l_si,
                    dimension: l_dim,
                    display_unit: l_unit,
                },
                Self::Scalar {
                    si_value: r_si,
                    dimension: r_dim,
                    display_unit: r_unit,
                },
            ) => l_si == r_si && l_dim == r_dim && l_unit == r_unit,
            (Self::Bool(l), Self::Bool(r)) => l == r,
            (Self::Int(l), Self::Int(r)) => l == r,
            (
                Self::Label {
                    index_name: l_index,
                    variant: l_variant,
                },
                Self::Label {
                    index_name: r_index,
                    variant: r_variant,
                },
            ) => l_index.matches_ref(r_index) && l_variant == r_variant,
            (
                Self::Struct {
                    type_name: l_type,
                    fields: l_fields,
                },
                Self::Struct {
                    type_name: r_type,
                    fields: r_fields,
                },
            ) => {
                struct_value_type_refs_equal(l_type, r_type)
                    && value_field_maps_equal(l_fields, r_fields)
            }
            (
                Self::Indexed {
                    index_name: l_index,
                    entries: l_entries,
                    ..
                },
                Self::Indexed {
                    index_name: r_index,
                    entries: r_entries,
                    ..
                },
            ) => l_index.matches_ref(r_index) && value_entry_maps_equal(l_entries, r_entries),
            (
                Self::Datetime {
                    epoch: l_epoch,
                    time_scale: l_scale,
                    display_tz: l_tz,
                },
                Self::Datetime {
                    epoch: r_epoch,
                    time_scale: r_scale,
                    display_tz: r_tz,
                },
            ) => l_epoch == r_epoch && l_scale == r_scale && l_tz == r_tz,
            _ => false,
        }
    }
}

fn struct_value_type_refs_equal(lhs: &StructTypeRef, rhs: &StructTypeRef) -> bool {
    lhs.matches_ref(rhs)
}

fn value_field_maps_equal(
    lhs: &IndexMap<FieldName, Value>,
    rhs: &IndexMap<FieldName, Value>,
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .all(|(field, value)| rhs.get(field).is_some_and(|rhs_value| value == rhs_value))
}

fn value_entry_maps_equal(
    lhs: &IndexMap<IndexVariantName, Value>,
    rhs: &IndexMap<IndexVariantName, Value>,
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .all(|(variant, value)| rhs.get(variant).is_some_and(|rhs_value| value == rhs_value))
}

/// Error returned when a [`Value`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, Error)]
#[error("expected Scalar value, got {actual}")]
pub struct ValueError {
    /// A short description of the actual variant (e.g. "Bool", "Int", "struct `Foo`").
    pub actual: String,
}

impl Value {
    /// Construct a label value after resolving the index leaf into an owner.
    #[must_use]
    pub fn label_with_owner(
        owner: DagId,
        index_name: IndexName,
        variant: IndexVariantName,
    ) -> Self {
        Self::Label {
            index_name: IndexTypeRef::with_owner(owner, index_name),
            variant,
        }
    }

    /// Construct a struct value after resolving the struct leaf into an owner.
    #[must_use]
    pub fn struct_with_owner(
        owner: DagId,
        type_name: StructTypeName,
        fields: IndexMap<FieldName, Self>,
    ) -> Self {
        Self::Struct {
            type_name: StructTypeRef::with_owner(owner, type_name),
            fields,
        }
    }

    /// Construct an indexed value after resolving the index leaf into an owner.
    #[must_use]
    pub fn indexed_with_owner(
        owner: DagId,
        index_name: IndexName,
        entries: IndexMap<IndexVariantName, Self>,
    ) -> Self {
        Self::Indexed {
            index_name: IndexTypeRef::with_owner(owner, index_name),
            entries,
            entry_display_names: None,
        }
    }

    /// Display label for an indexed entry key.
    ///
    /// This is an I/O helper: it renders optional range-index labels and falls
    /// back to the semantic variant leaf. Core logic should use `entries` keys.
    #[must_use]
    pub fn indexed_entry_display_name(&self, variant: &IndexVariantName) -> String {
        match self {
            Self::Indexed {
                entry_display_names: Some(display_names),
                ..
            } => display_names
                .get(variant)
                .cloned()
                .unwrap_or_else(|| variant.as_str().to_string()),
            _ => variant.as_str().to_string(),
        }
    }

    /// A short description of this value's variant for error messages.
    fn variant_description(&self) -> String {
        match self {
            Self::Scalar { .. } => "Scalar".to_string(),
            Self::Bool(_) => "Bool".to_string(),
            Self::Int(_) => "Int".to_string(),
            Self::Label {
                index_name,
                variant,
            } => variant.qualified_by(&index_name.display_name()).to_string(),
            Self::Struct { type_name, .. } => format!("struct `{type_name}`"),
            Self::Indexed { index_name, .. } => format!("indexed `{index_name}[...]`"),
            Self::Datetime { .. } => "Datetime".to_string(),
        }
    }

    /// Get the SI value.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Scalar`.
    pub fn si_value(&self) -> Result<f64, ValueError> {
        match self {
            Self::Scalar { si_value, .. } => Ok(*si_value),
            other => Err(ValueError {
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the dimension.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Scalar`.
    pub fn dimension(&self) -> Result<Dimension, ValueError> {
        match self {
            Self::Scalar { dimension, .. } => Ok(dimension.clone()),
            other => Err(ValueError {
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the value formatted for display: in display units if available, otherwise SI.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Scalar`.
    pub fn display_value(&self) -> Result<f64, ValueError> {
        match self {
            Self::Scalar {
                si_value,
                display_unit,
                ..
            } => Ok(scalar_display_value(*si_value, display_unit.as_ref())),
            other => Err(ValueError {
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the unit label for display, or `None` for dimensionless values.
    ///
    /// Returns the explicit display unit label if set (e.g., "km", "km/hour"),
    /// otherwise falls back to a label built from registered base-unit symbols
    /// (e.g., "m/s", "kg").
    #[must_use]
    pub fn display_label(&self, symbols: &BTreeMap<BaseDimId, String>) -> Option<String> {
        match self {
            Self::Scalar {
                display_unit,
                dimension,
                ..
            } => display_unit.as_ref().map_or_else(
                || default_unit_label(dimension, symbols),
                |du| Some(du.label.clone()),
            ),
            Self::Bool(_)
            | Self::Int(_)
            | Self::Label { .. }
            | Self::Struct { .. }
            | Self::Indexed { .. }
            | Self::Datetime { .. } => None,
        }
    }

    /// Format this value as a flat display string (no name prefix, no recursion).
    ///
    /// If `symbols` is provided, scalar values include their unit label in brackets
    /// (e.g., `"42.5 [km/hour]"`). Without `symbols`, only the numeric value is shown.
    ///
    /// Composite values (`Struct`, `Indexed`) are shown as their variant name or
    /// a placeholder string, not recursively expanded.
    #[must_use]
    pub fn format_display(&self, symbols: Option<&BTreeMap<BaseDimId, String>>) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::Label {
                index_name,
                variant,
            } => variant.qualified_by(&index_name.display_name()).to_string(),
            Self::Struct { type_name, .. } => type_name.as_str().to_string(),
            Self::Datetime {
                epoch, display_tz, ..
            } => format_epoch_with_tz(epoch, display_tz.as_deref()),
            Self::Scalar {
                si_value,
                display_unit,
                ..
            } => {
                let formatted = graphcal_compiler::registry::format::format_number(
                    scalar_display_value(*si_value, display_unit.as_ref()),
                );
                match symbols.and_then(|s| self.display_label(s)) {
                    Some(label) => format!("{formatted} [{label}]"),
                    None => formatted,
                }
            }
            Self::Indexed { .. } => "[...]".to_string(),
        }
    }

    /// Format a `Datetime` value for display.
    ///
    /// If `display_tz` is set, formats the instant in that IANA timezone
    /// (e.g. `"2024-11-05T10:00:00+09:00[Asia/Tokyo]"`).
    /// Otherwise, falls back to the hifitime `Epoch` display (e.g. `"2024-11-05T12:00:00 UTC"`).
    ///
    /// Returns `None` if this is not a `Datetime` value.
    #[must_use]
    pub fn format_datetime(&self) -> Option<String> {
        let Self::Datetime {
            epoch, display_tz, ..
        } = self
        else {
            return None;
        };
        Some(format_epoch_with_tz(epoch, display_tz.as_deref()))
    }
}

/// Compute the displayed scalar value from its SI value and optional display unit.
///
/// Returns `si_value` directly when no display unit is set; otherwise scales it
/// by the unit's conversion factor (`display_value = si_value / scale`).
#[must_use]
pub fn scalar_display_value(si_value: f64, display_unit: Option<&DisplayUnit>) -> f64 {
    display_unit.map_or(si_value, |du| si_value / du.scale)
}

/// Format a scalar's default unit label from its dimension and registered base-unit symbols.
///
/// This is intentionally kept in the runtime display layer rather than on
/// `Dimension`: dimensions describe physical semantics; unit labels are a
/// presentation concern derived from registry metadata.
#[must_use]
fn default_unit_label(
    dimension: &Dimension,
    symbols: &BTreeMap<BaseDimId, String>,
) -> Option<String> {
    if dimension.is_dimensionless() {
        return None;
    }

    let mut result = String::new();
    let mut first = true;

    for (id, &exp) in dimension.iter() {
        if exp.num() <= 0 {
            continue;
        }
        if !first {
            result.push('*');
        }
        first = false;
        push_unit_factor(&mut result, id, exp, symbols);
    }

    for (id, &exp) in dimension.iter() {
        if exp.num() >= 0 {
            continue;
        }
        if first {
            push_unit_factor(&mut result, id, exp, symbols);
            first = false;
        } else {
            result.push('/');
            push_unit_factor(&mut result, id, -exp, symbols);
        }
    }

    Some(result)
}

fn push_unit_factor(
    result: &mut String,
    id: &BaseDimId,
    exp: Rational,
    symbols: &BTreeMap<BaseDimId, String>,
) {
    let symbol = symbols
        .get(id)
        .map_or_else(|| id.fallback_symbol(), String::clone);
    result.push_str(&symbol);
    if exp != Rational::ONE {
        result.push('^');
        result.push_str(&exp.to_string());
    }
}

/// Format an `hifitime::Epoch` with an optional IANA timezone.
///
/// If `tz` is `Some`, converts to that timezone via jiff and formats as
/// `"2024-11-05T10:00:00+09:00[Asia/Tokyo]"`.
/// Otherwise, falls back to hifitime's `Display` (e.g. `"2024-11-05T12:00:00 UTC"`).
#[must_use]
pub fn format_epoch_with_tz(epoch: &hifitime::Epoch, tz: Option<&str>) -> String {
    if let Some(tz_name) = tz
        && let Ok(formatted) = format_epoch_in_timezone(epoch, tz_name)
    {
        return formatted;
    }
    format!("{epoch}")
}

/// Convert a `hifitime::Epoch` to a jiff `Zoned` datetime in the given timezone
/// and format it as an ISO 8601 string.
fn format_epoch_in_timezone(
    epoch: &hifitime::Epoch,
    tz_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let ts = epoch_to_jiff_timestamp(epoch)?;
    let zdt = ts.in_tz(tz_name)?;
    Ok(zdt.strftime("%Y-%m-%dT%H:%M:%S%:z[%Q]").to_string())
}

/// Format a `hifitime::Epoch` as an RFC 3339 / ISO 8601 string in UTC
/// (e.g. `"2026-01-01T00:00:00Z"`), for machine consumers such as
/// Vega-Lite temporal data.
///
/// Falls back to the hifitime `Display` form for epochs outside jiff's
/// representable range (beyond year ±9999).
#[must_use]
pub fn epoch_to_rfc3339(epoch: &hifitime::Epoch) -> String {
    epoch_to_jiff_timestamp(epoch).map_or_else(|_| format!("{epoch}"), |ts| ts.to_string())
}

/// Convert a `hifitime::Epoch` to a `jiff::Timestamp`.
fn epoch_to_jiff_timestamp(epoch: &hifitime::Epoch) -> Result<jiff::Timestamp, jiff::Error> {
    let unix_secs = epoch.to_unix_seconds();
    let secs = unix_secs.floor();
    let nanos = (unix_secs - secs) * 1e9;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "unix seconds fit in i64 for any reasonable date; nanos < 1e9 fits in i32"
    )]
    jiff::Timestamp::new(secs as i64, nanos as i32)
}

/// A runtime error associated with a specific node or param evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeError {
    /// The expression evaluation failed directly (e.g., division by zero).
    EvalFailed {
        /// Human-readable error message.
        message: String,
    },
    /// Could not evaluate because one or more dependencies failed.
    DependencyFailed {
        /// Names of the dependencies that failed.
        failed_deps: Vec<DeclName>,
    },
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EvalFailed { message } => write!(f, "{message}"),
            Self::DependencyFailed { failed_deps } => {
                let names: Vec<&str> = failed_deps.iter().map(DeclName::as_str).collect();
                write!(f, "dependency failed: {}", names.join(", "))
            }
        }
    }
}

/// A plot declaration that could not be evaluated, with the reason.
///
/// Plot evaluation is per-plot best-effort: one failing plot does not stop
/// the others, but the failure must be reported, never silently dropped
/// (#842).
#[derive(Debug, Clone, PartialEq)]
pub struct PlotError {
    /// The plot declaration name.
    pub name: ScopedName,
    /// Human-readable reason the plot was not rendered.
    pub message: String,
}

/// The result of evaluating an assertion.
#[derive(Debug, Clone, PartialEq)]
pub enum AssertResult {
    /// The assertion passed (body evaluated to `true`).
    Pass,
    /// The assertion failed (body evaluated to `false`).
    Fail {
        /// Human-readable failure message.
        message: String,
    },
    /// The assertion could not be evaluated (e.g., a dependency failed).
    Error {
        /// Human-readable error message.
        message: String,
    },
}

/// The result of evaluating a `.gcl` file.
///
/// Entries are keyed by [`ScopedName`]: a top-level declaration is a bare
/// local name, while a declaration instantiated through `include ... as
/// alias` keeps its alias-qualified path (`alias.decl`). Output boundaries
/// (text, JSON, LSP) render the full path so multiple instantiations of the
/// same dag never collapse onto one key (#813).
#[derive(Debug)]
pub struct EvalResult {
    /// Const values in source order. Const *values* are compile-time, but a
    /// const's display unit (e.g. a dynamic conversion target) resolves at
    /// runtime and can fail per-node.
    pub consts: Vec<(ScopedName, Result<Value, NodeError>)>,
    /// Param values in source order (may contain per-node errors).
    pub params: Vec<(ScopedName, Result<Value, NodeError>)>,
    /// Node values in source order (may contain per-node errors).
    pub nodes: Vec<(ScopedName, Result<Value, NodeError>)>,
    /// All values in source order with their declaration type.
    pub all: Vec<(ScopedName, Result<Value, NodeError>, DeclType)>,
    /// Assertion results in source order: (name, result, span).
    pub assertions: Vec<(ScopedName, AssertResult, Span)>,
    /// Evaluated plot specifications in source order.
    pub plots: Vec<PlotSpec>,
    /// Plots that failed to evaluate, with their reasons (#842).
    pub plot_errors: Vec<PlotError>,
    /// Evaluated figure specifications in source order.
    pub figures: Vec<FigureSpec>,
    /// Evaluated layer specifications in source order.
    pub layers: Vec<LayerSpec>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: std::collections::HashMap<ScopedName, Vec<ScopedName>>,
    /// Base dimension symbols for display (e.g., `BaseDimId::Prelude("Length") → "m"`).
    pub base_dim_symbols:
        std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    /// Domain constraints for params/nodes, for programmatic access (sweeping/sampling).
    pub domain_constraints: std::collections::HashMap<
        ScopedName,
        graphcal_compiler::tir::typed::ResolvedDomainConstraint,
    >,
}

impl EvalResult {
    /// Returns `true` if any param/node evaluation failed or any assertion failed.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.params.iter().any(|(_, r)| r.is_err())
            || self.nodes.iter().any(|(_, r)| r.is_err())
            || self.assertions.iter().any(|(_, r, _)| {
                matches!(r, AssertResult::Fail { .. } | AssertResult::Error { .. })
            })
    }
}

/// A mark-level property (style applied to the mark in a plot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkProperty {
    StrokeWidth,
    Opacity,
    Size,
    Color,
    Filled,
    Interpolate,
}

impl MarkProperty {
    /// Parse a mark property from its source-level name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "stroke_width" => Some(Self::StrokeWidth),
            "opacity" => Some(Self::Opacity),
            "size" => Some(Self::Size),
            "color" => Some(Self::Color),
            "filled" => Some(Self::Filled),
            "interpolate" => Some(Self::Interpolate),
            _ => None,
        }
    }

    /// The Vega-Lite camelCase property name.
    #[must_use]
    pub const fn vega_name(&self) -> &'static str {
        match self {
            Self::StrokeWidth => "strokeWidth",
            Self::Opacity => "opacity",
            Self::Size => "size",
            Self::Color => "color",
            Self::Filled => "filled",
            Self::Interpolate => "interpolate",
        }
    }
}

/// A plot-level property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlotProperty {
    Title,
    Width,
    Height,
    XLabel,
    YLabel,
}

impl PlotProperty {
    /// Parse a plot property from its source-level name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "title" => Some(Self::Title),
            "width" => Some(Self::Width),
            "height" => Some(Self::Height),
            "x_label" => Some(Self::XLabel),
            "y_label" => Some(Self::YLabel),
            _ => None,
        }
    }
}

/// A figure/layer-level property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompositionProperty {
    Title,
    Width,
    Height,
}

impl CompositionProperty {
    /// Parse a composition property from its source-level name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "title" => Some(Self::Title),
            "width" => Some(Self::Width),
            "height" => Some(Self::Height),
            _ => None,
        }
    }
}

/// A single evaluated plot specification.
#[derive(Debug, Clone)]
pub struct PlotSpec {
    /// The plot declaration name.
    pub name: ScopedName,
    /// The mark type (point, line, bar, area, rect, tick).
    pub mark_type: graphcal_compiler::desugar::desugared_ast::MarkType,
    /// Evaluated encoding channels (x, y, color, etc.) with their data.
    pub encodings: Vec<(EncodingChannel, PlotFieldValue)>,
    /// Axis metadata per encoding channel.
    /// Used by the CLI to auto-generate axis titles like "Velocity (km/s)".
    pub encoding_meta: Vec<(EncodingChannel, AxisMeta)>,
    /// Evaluated mark properties (`stroke_width`, `opacity`, etc.).
    pub mark_properties: Vec<(MarkProperty, PlotFieldValue)>,
    /// Evaluated plot-level properties (title, width, height, etc.).
    pub properties: Vec<(PlotProperty, PlotFieldValue)>,
    /// Whether this plot is `pub` (visible in standalone output).
    /// Non-`pub` plots are only usable in figure composition.
    pub is_pub: bool,
}

/// A single evaluated figure specification.
#[derive(Debug, Clone)]
pub struct FigureSpec {
    /// The figure declaration name.
    pub name: ScopedName,
    /// The plot names referenced by this figure.
    pub plot_names: Vec<ScopedName>,
    /// Additional evaluated properties (e.g., title).
    pub properties: Vec<(CompositionProperty, PlotFieldValue)>,
}

/// A single evaluated layer specification.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    /// The layer declaration name.
    pub name: ScopedName,
    /// The plot names to overlay in this layer.
    pub plot_names: Vec<ScopedName>,
    /// Additional evaluated properties (e.g., title, width, height).
    pub properties: Vec<(CompositionProperty, PlotFieldValue)>,
}

/// Axis metadata for auto-generating axis titles from dimension/unit info.
#[derive(Debug, Clone, Default)]
pub struct AxisMeta {
    /// The dimension name (e.g., "Velocity", "Length * Time^-1").
    pub dimension_label: Option<String>,
    /// The display unit label (e.g., "km/s", "m").
    pub unit_label: Option<String>,
}

/// A resolved value for a plot field.
#[derive(Debug, Clone)]
pub enum PlotFieldValue {
    /// A list of f64 values (from evaluated numeric expressions/for-comprehensions).
    Numbers(Vec<f64>),
    /// A list of string labels (from evaluated label expressions/for-comprehensions).
    Labels(Vec<String>),
    /// A list of datetime instants as RFC 3339 / ISO 8601 strings,
    /// rendered with Vega-Lite temporal encoding (#846).
    Datetimes(Vec<String>),
    /// A single string value (e.g., title).
    String(String),
    /// A single numeric value.
    Number(f64),
    /// A single datetime instant as an RFC 3339 / ISO 8601 string (#846).
    Datetime(String),
}

/// Top-level compile error that wraps both parse and eval errors.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(#[from] graphcal_compiler::syntax::parser::ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(#[from] graphcal_compiler::registry::error::GraphcalError),
}

impl CompileError {
    /// Return the `NamedSource` embedded in this error, if any.
    ///
    /// Forwards to the inner
    /// [`ParseError::named_source`](graphcal_compiler::syntax::parser::ParseError::named_source)
    /// or
    /// [`GraphcalError::named_source`](graphcal_compiler::registry::error::GraphcalError::named_source).
    /// When present, the returned
    /// `NamedSource` pairs the file's name with the exact source text whose
    /// byte offsets the error's labels index into — so diagnostic emitters
    /// can build a line index over the right text without having to look it
    /// up by name.
    ///
    /// `ParseError` always carries a source; `GraphcalError` may return
    /// `None` for a few variants representing source-less errors (e.g.
    /// `FileNotFound`, `CircularImport`).
    #[must_use]
    pub const fn named_source(&self) -> Option<&miette::NamedSource<std::sync::Arc<String>>> {
        match self {
            Self::Parse(e) => Some(e.named_source()),
            Self::Eval(e) => e.named_source(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dim_id(name: &str) -> BaseDimId {
        BaseDimId::Prelude(name.to_string())
    }

    fn scalar(dimension: Dimension, display_unit: Option<DisplayUnit>) -> Value {
        Value::Scalar {
            si_value: 1.0,
            dimension,
            display_unit,
        }
    }

    fn symbols() -> BTreeMap<BaseDimId, String> {
        BTreeMap::from([
            (dim_id("Length"), "m".to_string()),
            (dim_id("Time"), "s".to_string()),
        ])
    }

    #[test]
    fn display_label_falls_back_to_default_unit_symbols() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = scalar(velocity, None);

        assert_eq!(value.display_label(&symbols()), Some("m/s".to_string()));
    }

    #[test]
    fn display_label_prefers_explicit_display_unit() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = scalar(
            velocity,
            Some(DisplayUnit {
                label: "km/hour".to_string(),
                scale: 1000.0 / 3600.0,
            }),
        );

        assert_eq!(value.display_label(&symbols()), Some("km/hour".to_string()));
    }

    #[test]
    fn display_label_omits_dimensionless_default_unit() {
        let value = scalar(Dimension::dimensionless(), None);

        assert_eq!(value.display_label(&symbols()), None);
    }
}
