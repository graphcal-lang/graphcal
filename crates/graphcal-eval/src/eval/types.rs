use indexmap::IndexMap;
use miette::Diagnostic;
use thiserror::Error;

use graphcal_compiler::syntax::ast::EncodingChannel;
use graphcal_compiler::syntax::dimension::Dimension;
use graphcal_compiler::syntax::names::{
    DeclName, FieldName, IndexName, StructTypeName, VariantName,
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
#[derive(Debug, Clone, PartialEq)]
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
        /// The index name (e.g., `Maneuver`).
        index_name: IndexName,
        /// The variant name (e.g., `Departure`).
        variant: VariantName,
    },
    Struct {
        /// The concrete type name (e.g., `Impulsive`, `TransferResult`).
        type_name: StructTypeName,
        /// Fields in definition order.
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values.
    Indexed {
        /// The index type name.
        index_name: IndexName,
        /// Entries in declaration order.
        entries: IndexMap<VariantName, Self>,
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

/// Error returned when a [`Value`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, Error)]
#[error("expected Scalar value, got {actual}")]
pub struct ValueError {
    /// A short description of the actual variant (e.g. "Bool", "Int", "struct `Foo`").
    pub actual: String,
}

impl Value {
    /// A short description of this value's variant for error messages.
    fn variant_description(&self) -> String {
        match self {
            Self::Scalar { .. } => "Scalar".to_string(),
            Self::Bool(_) => "Bool".to_string(),
            Self::Int(_) => "Int".to_string(),
            Self::Label {
                index_name,
                variant,
            } => format!("{index_name}::{variant}"),
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
            } => Ok(display_unit
                .as_ref()
                .map_or(*si_value, |du| *si_value / du.scale)),
            other => Err(ValueError {
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the unit label for display, or `None` for dimensionless values.
    ///
    /// Returns the explicit display unit label if set (e.g., "km", "km/hour"),
    /// otherwise falls back to the SI unit string (e.g., "m/s", "kg").
    #[must_use]
    pub fn display_label(
        &self,
        symbols: &std::collections::BTreeMap<
            graphcal_compiler::syntax::dimension::BaseDimId,
            String,
        >,
    ) -> Option<String> {
        match self {
            Self::Scalar {
                display_unit,
                dimension,
                ..
            } => display_unit.as_ref().map_or_else(
                || dimension.si_unit_string(symbols),
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
    pub fn format_display(
        &self,
        symbols: Option<
            &std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
        >,
    ) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::Label {
                index_name,
                variant,
            } => format!("{index_name}::{variant}"),
            Self::Struct { type_name, .. } => type_name.as_str().to_string(),
            Self::Datetime { .. } =>
            {
                #[expect(
                    clippy::expect_used,
                    reason = "format_datetime always returns Some for Datetime variant"
                )]
                self.format_datetime().expect("self is Datetime")
            }
            Self::Scalar { .. } => {
                #[expect(
                    clippy::expect_used,
                    reason = "display_value always returns Ok for Scalar variant"
                )]
                let formatted = graphcal_compiler::registry::format::format_number(
                    self.display_value().expect("self is Scalar"),
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
#[derive(Debug)]
pub struct EvalResult {
    /// Const values in source order (consts are compile-time and never fail at runtime).
    pub consts: Vec<(DeclName, Value)>,
    /// Param values in source order (may contain per-node errors).
    pub params: Vec<(DeclName, Result<Value, NodeError>)>,
    /// Node values in source order (may contain per-node errors).
    pub nodes: Vec<(DeclName, Result<Value, NodeError>)>,
    /// All values in source order with their declaration type.
    pub all: Vec<(DeclName, Result<Value, NodeError>, DeclType)>,
    /// Assertion results in source order: (name, result, span).
    pub assertions: Vec<(DeclName, AssertResult, Span)>,
    /// Evaluated plot specifications in source order.
    pub plots: Vec<PlotSpec>,
    /// Evaluated figure specifications in source order.
    pub figures: Vec<FigureSpec>,
    /// Evaluated layer specifications in source order.
    pub layers: Vec<LayerSpec>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: std::collections::HashMap<DeclName, Vec<DeclName>>,
    /// Base dimension symbols for display (e.g., `BaseDimId::Prelude("Length") → "m"`).
    pub base_dim_symbols:
        std::collections::BTreeMap<graphcal_compiler::syntax::dimension::BaseDimId, String>,
    /// Domain constraints for params/nodes, for programmatic access (sweeping/sampling).
    pub domain_constraints: std::collections::HashMap<
        DeclName,
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
    pub name: DeclName,
    /// The mark type (point, line, bar, area, rect, tick).
    pub mark_type: graphcal_compiler::syntax::ast::MarkType,
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
    pub name: DeclName,
    /// The plot names referenced by this figure.
    pub plot_names: Vec<DeclName>,
    /// Additional evaluated properties (e.g., title).
    pub properties: Vec<(CompositionProperty, PlotFieldValue)>,
}

/// A single evaluated layer specification.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    /// The layer declaration name.
    pub name: DeclName,
    /// The plot names to overlay in this layer.
    pub plot_names: Vec<DeclName>,
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
    /// A single string value (e.g., title).
    String(String),
    /// A single numeric value.
    Number(f64),
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
