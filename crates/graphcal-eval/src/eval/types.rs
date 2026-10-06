use std::collections::BTreeMap;

use indexmap::IndexMap;
use thiserror::Error;

use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::desugar::desugared_ast::EncodingChannel;
use graphcal_compiler::dimension::{BaseDimId, Dimension, Rational};
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::ratio::ExponentStyle;
use graphcal_compiler::semantic::checked_type::{CheckedGenericArg, IndexTypeRef, StructTypeRef};
use graphcal_compiler::semantic::time_zone::{IanaTimeZoneId, TimeZoneRegistry};
use graphcal_compiler::semantic::unit_scale::PositiveFiniteScale;
use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};

use crate::runtime_value::{KeyElement, KeyValue};

/// Display unit metadata: the unit name(s) and validated scale factor for pretty-printing.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayUnit {
    /// Human-readable unit string (e.g., "km", "m/s^2", "km/h")
    pub label: String,
    /// Scale factor from SI to this display unit.
    pub scale: PositiveFiniteScale,
}

impl DisplayUnit {
    /// Construct display metadata from an already validated scale.
    #[must_use]
    pub fn new(label: impl Into<String>, scale: PositiveFiniteScale) -> Self {
        Self {
            label: label.into(),
            scale,
        }
    }
}

/// Failure to project a finite SI quantity into a requested display unit.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DisplayProjectionError {
    /// Dividing by a tiny valid scale overflowed binary64.
    #[error("display conversion to `{unit}` produced a non-finite value")]
    NonFiniteResult { unit: String },
    /// A nonzero SI value became zero in the requested unit.
    #[error("display conversion to `{unit}` underflowed to zero")]
    Underflow { unit: String },
}

/// Error returned by a display accessor or projection.
#[cfg(test)]
#[derive(Debug, Error)]
pub(crate) enum DisplayValueError {
    /// The accessor was called on the wrong public value variant.
    #[error(transparent)]
    Value(#[from] ValueError),
    /// Numeric projection could not preserve display invariants.
    #[error(transparent)]
    Projection(#[from] DisplayProjectionError),
}

/// Failure to project an exact hifitime epoch into jiff's timestamp range.
#[derive(Debug, Error)]
pub(crate) enum EpochProjectionError {
    /// Whole Unix seconds exceeded jiff's signed-second input representation.
    #[error("datetime Unix seconds are outside the i64 range")]
    SecondsOutOfRange,
    /// Jiff rejected an otherwise exact seconds/nanoseconds pair.
    #[error(transparent)]
    Jiff(#[from] jiff::Error),
}

/// A user-facing evaluated runtime value.
///
/// Equality is structural over every field, presentation included: two values
/// are equal only when they carry the same semantic identity *and* the same
/// display metadata. Registry data needed only to render a value (such as the
/// time-zone database) lives in [`RenderContext`], never in the value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Quantity {
        /// The finite value in base SI units.
        si_value: FiniteQuantity,
        /// The dimension of this value.
        dimension: Dimension,
        /// Optional display unit for pretty-printing.
        display_unit: Option<DisplayUnit>,
    },
    Complex {
        /// Cartesian components in base SI units.
        si_value: ComplexValue,
        /// Shared physical dimension of both components.
        dimension: Dimension,
        /// Optional display unit applied to both components.
        display_unit: Option<DisplayUnit>,
    },
    Bool(bool),
    Int(i64),
    /// A `Key<I>` value: one entry of a concrete index axis. It renders as
    /// its label, `Fin` position, or coordinate (see [`KeyRendering`]).
    Key(KeyValue),
    Struct {
        /// Canonical owner-qualified nominal type identity.
        type_name: StructTypeRef,
        /// Constructor member identity within `type_name`; also the rendered value leaf.
        constructor: ConstructorName,
        /// Concrete generic arguments of the nominal type (part of value identity).
        generic_args: Vec<CheckedGenericArg>,
        /// Fields in definition order.
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection keyed by named labels or typed positions.
    Indexed {
        /// The index type identity, including a canonical owner when available.
        index_name: IndexTypeRef,
        /// Entries in declaration order.
        entries: IndexMap<IndexEntryKey, Self>,
        /// Optional display labels for entry keys (for example, coordinate values).
        ///
        /// These are presentation strings only. Semantic consumers must continue to
        /// use `entries` keys rather than parsing these labels.
        entry_display_names: Option<IndexMap<IndexEntryKey, String>>,
    },
    /// A datetime instant.
    Datetime {
        /// The hifitime epoch (internal representation).
        epoch: hifitime::Epoch,
        /// The time scale for display purposes.
        time_scale: graphcal_compiler::semantic::time_scale::TimeScale,
        /// Optional IANA timezone for display (e.g. `"America/New_York"`).
        display_tz: Option<IanaTimeZoneId>,
    },
}

/// Error returned when a [`Value`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, Error)]
#[error("expected {expected} value, got {actual}")]
pub struct ValueError {
    /// Description of the variant accepted by the accessor.
    expected: &'static str,
    /// A short description of the actual variant (e.g. "Bool", "Int", "struct `Foo`").
    actual: String,
}

impl Value {
    /// Construct a non-generic struct value whose type is named directly, for
    /// tests.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn struct_with_owner(
        owner: graphcal_compiler::dag_id::DagId,
        type_name: graphcal_compiler::syntax::type_name::StructTypeName,
        constructor: ConstructorName,
        fields: IndexMap<FieldName, Self>,
    ) -> Self {
        Self::Struct {
            type_name: StructTypeRef::with_owner(owner, type_name),
            constructor,
            generic_args: Vec::new(),
            fields,
        }
    }

    /// Construct an indexed value whose index is named directly, for tests.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn indexed_with_owner(
        owner: graphcal_compiler::dag_id::DagId,
        index_name: graphcal_compiler::syntax::index_name::IndexName,
        entries: IndexMap<IndexEntryKey, Self>,
    ) -> Self {
        Self::Indexed {
            index_name: IndexTypeRef::with_owner(owner, index_name),
            entries,
            entry_display_names: None,
        }
    }

    /// Display label for an indexed entry key.
    ///
    /// This is an I/O helper: it renders optional coordinate-index labels and falls
    /// back to the typed key's boundary rendering. Core logic uses `entries` keys.
    #[must_use]
    pub fn indexed_entry_display_name(&self, key: &IndexEntryKey) -> String {
        match self {
            Self::Indexed {
                entry_display_names: Some(display_names),
                ..
            } => display_names
                .get(key)
                .cloned()
                .unwrap_or_else(|| key.to_string()),
            _ => key.to_string(),
        }
    }

    /// A short description of this value's variant for error messages.
    fn variant_description(&self) -> String {
        match self {
            Self::Quantity { .. } => "Quantity".to_string(),
            Self::Complex { .. } => "Complex".to_string(),
            Self::Bool(_) => "Bool".to_string(),
            Self::Int(_) => "Int".to_string(),
            Self::Key(key) => format!("key of `{}`", key.index()),
            Self::Struct { constructor, .. } => format!("struct `{constructor}`"),
            Self::Indexed { index_name, .. } => format!("indexed `{index_name}[...]`"),
            Self::Datetime { .. } => "Datetime".to_string(),
        }
    }

    /// Get the SI value.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Quantity`.
    pub fn si_value(&self) -> Result<FiniteQuantity, ValueError> {
        match self {
            Self::Quantity { si_value, .. } => Ok(*si_value),
            other => Err(ValueError {
                expected: "Quantity",
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the dimension of a real or complex quantity.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Quantity` or `Complex` value.
    pub fn dimension(&self) -> Result<Dimension, ValueError> {
        match self {
            Self::Quantity { dimension, .. } | Self::Complex { dimension, .. } => {
                Ok(dimension.clone())
            }
            other => Err(ValueError {
                expected: "Quantity or Complex",
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the value formatted for display: in display units if available, otherwise SI.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Quantity`.
    #[cfg(test)]
    pub(crate) fn display_value(&self) -> Result<f64, DisplayValueError> {
        match self {
            Self::Quantity {
                si_value,
                display_unit,
                ..
            } => quantity_display_value(*si_value, display_unit.as_ref()).map_err(Into::into),
            other => Err(ValueError {
                expected: "Quantity",
                actual: other.variant_description(),
            }
            .into()),
        }
    }

    /// Get the unit label for display, or `None` for dimensionless values.
    ///
    /// Returns the explicit display unit label if set (e.g., "km", "km/h"),
    /// otherwise uses the context's base-unit symbols (e.g., "m/s", "kg"). If
    /// any referenced base dimension has no registered symbol, no default
    /// label is shown.
    #[must_use]
    pub fn display_label(&self, render: &RenderContext) -> Option<String> {
        match self {
            Self::Quantity {
                display_unit,
                dimension,
                ..
            }
            | Self::Complex {
                display_unit,
                dimension,
                ..
            } => display_unit.as_ref().map_or_else(
                || default_unit_label(dimension, &render.base_dim_symbols),
                |du| Some(du.label.clone()),
            ),
            Self::Key(key) => match KeyRendering::of(key) {
                KeyRendering::Coordinate(quantity) => quantity.display_label(render),
                KeyRendering::Label { .. } | KeyRendering::Position(_) => None,
            },
            Self::Bool(_)
            | Self::Int(_)
            | Self::Struct { .. }
            | Self::Indexed { .. }
            | Self::Datetime { .. } => None,
        }
    }

    /// Format this value as a flat display string (no name prefix, no recursion).
    ///
    /// With [`UnitLabel::Inline`], quantity values include their unit label in
    /// brackets (e.g., `"42.5 [km/h]"`); with [`UnitLabel::Omitted`], only the
    /// numeric value is shown.
    ///
    /// Composite values (`Struct`, `Indexed`) are shown as their variant name or
    /// a placeholder string, not recursively expanded.
    ///
    /// # Errors
    ///
    /// Returns the display-unit projection error of a quantity leaf.
    pub fn format_display(
        &self,
        render: &RenderContext,
        unit_label: UnitLabel,
    ) -> Result<String, DisplayProjectionError> {
        let labelled = |formatted: String| match unit_label {
            UnitLabel::Inline => match self.display_label(render) {
                Some(label) => format!("{formatted} [{label}]"),
                None => formatted,
            },
            UnitLabel::Omitted => formatted,
        };
        Ok(match self {
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::Key(key) => match KeyRendering::of(key) {
                KeyRendering::Label {
                    index_name,
                    variant,
                } => format!("{index_name}#{variant}"),
                KeyRendering::Position(position) => position.to_string(),
                KeyRendering::Coordinate(quantity) => {
                    quantity.format_display(render, unit_label)?
                }
            },
            Self::Struct { constructor, .. } => constructor.as_str().to_string(),
            Self::Datetime {
                epoch, display_tz, ..
            } => render.format_datetime(epoch, display_tz.as_ref()),
            Self::Quantity {
                si_value,
                display_unit,
                ..
            } => labelled(graphcal_compiler::display::number::format_number(
                quantity_display_value(*si_value, display_unit.as_ref())?,
            )),
            Self::Complex {
                si_value,
                display_unit,
                ..
            } => {
                let re = graphcal_compiler::display::number::format_number(quantity_display_value(
                    si_value.real_part(),
                    display_unit.as_ref(),
                )?);
                let displayed_im =
                    quantity_display_value(si_value.imaginary_part(), display_unit.as_ref())?;
                let sign = if displayed_im.is_sign_negative() {
                    "-"
                } else {
                    "+"
                };
                let im = graphcal_compiler::display::number::format_number(displayed_im.abs());
                labelled(format!("{re} {sign} {im}i"))
            }
            Self::Indexed { .. } => "[...]".to_string(),
        })
    }
}

/// How a key renders at output boundaries: as its label, its `Fin` position,
/// or its coordinate quantity.
///
/// Every output boundary renders a key through this view, so a coordinate key
/// renders exactly as the quantity it denotes and a `Fin` key exactly as an
/// integer.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyRendering<'a> {
    /// A named-index key, rendered `Index#Variant`.
    Label {
        index_name: &'a IndexTypeRef,
        variant: &'a IndexVariantName,
    },
    /// A `Fin` key, rendered as its integer position.
    Position(usize),
    /// A coordinate key, rendered as its coordinate: a [`Value::Quantity`] in
    /// the axis's dimension and display unit.
    Coordinate(Box<Value>),
}

impl<'a> KeyRendering<'a> {
    /// The rendering view of `key`.
    #[must_use]
    pub fn of(key: &'a KeyValue) -> Self {
        match key.element() {
            KeyElement::Named(variant) => Self::Label {
                index_name: key.index(),
                variant,
            },
            KeyElement::Finite(position) => Self::Position(position),
            KeyElement::Coordinate { value, data } => Self::Coordinate(Box::new(Value::Quantity {
                si_value: value,
                dimension: data.dimension().clone(),
                display_unit: data
                    .display()
                    .label
                    .as_ref()
                    .map(|label| DisplayUnit::new(label.clone(), data.display().scale)),
            })),
        }
    }
}

/// Whether [`Value::format_display`] appends a quantity's unit label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitLabel {
    /// Append the unit label in brackets (e.g., `"42.5 [km/h]"`).
    Inline,
    /// Show only the number; the caller renders the unit elsewhere (e.g., a
    /// table caption shared by every cell).
    Omitted,
}

/// Registry metadata a display boundary needs to render public [`Value`]s.
///
/// Values carry only their semantic content and per-declaration presentation
/// (display units, display time zone); program-wide rendering data lives here
/// so it is neither cloned into every value nor part of value equality.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// Base dimension symbols for default unit labels (e.g., prelude `Length` -> `m`).
    base_dim_symbols: BTreeMap<BaseDimId, String>,
    /// Time-zone database used to render datetimes in their display zone.
    time_zones: TimeZoneRegistry,
}

impl RenderContext {
    /// Bundle the program's base-unit symbols with the time-zone database.
    #[must_use]
    pub const fn new(
        base_dim_symbols: BTreeMap<BaseDimId, String>,
        time_zones: TimeZoneRegistry,
    ) -> Self {
        Self {
            base_dim_symbols,
            time_zones,
        }
    }

    /// Format a datetime instant, in its display time zone when one is set.
    ///
    /// With a display zone, formats the instant in that IANA zone (e.g.
    /// `"2024-11-05T10:00:00+09:00[Asia/Tokyo]"`). Otherwise, or if the instant
    /// cannot be represented in that zone, falls back to the hifitime `Epoch`
    /// display (e.g. `"2024-11-05T12:00:00 UTC"`).
    #[must_use]
    pub fn format_datetime(
        &self,
        epoch: &hifitime::Epoch,
        display_tz: Option<&IanaTimeZoneId>,
    ) -> String {
        if let Some(time_zone_id) = display_tz
            && let Ok(formatted) = format_epoch_in_timezone(epoch, time_zone_id, &self.time_zones)
        {
            return formatted;
        }
        format!("{epoch}")
    }
}

/// Compute the displayed quantity value from its SI value and optional display unit.
///
/// Returns `si_value` directly when no display unit is set; otherwise scales it
/// by the unit's conversion factor (`display_value = si_value / scale`).
///
/// # Errors
///
/// Rejects an overflowing conversion or a nonzero value that underflows to
/// zero in the requested unit.
pub fn quantity_display_value(
    si_value: FiniteQuantity,
    display_unit: Option<&DisplayUnit>,
) -> Result<f64, DisplayProjectionError> {
    let si_value = si_value.get();
    let Some(display_unit) = display_unit else {
        return Ok(si_value);
    };
    let displayed = si_value / display_unit.scale.get();
    if !displayed.is_finite() {
        return Err(DisplayProjectionError::NonFiniteResult {
            unit: display_unit.label.clone(),
        });
    }
    if si_value != 0.0 && displayed == 0.0 {
        return Err(DisplayProjectionError::Underflow {
            unit: display_unit.label.clone(),
        });
    }
    Ok(displayed)
}

pub(super) fn validate_display_projection(value: &Value) -> Result<(), DisplayProjectionError> {
    match value {
        Value::Quantity {
            si_value,
            display_unit,
            ..
        } => quantity_display_value(*si_value, display_unit.as_ref()).map(|_| ()),
        Value::Complex {
            si_value,
            display_unit,
            ..
        } => {
            quantity_display_value(si_value.real_part(), display_unit.as_ref())?;
            quantity_display_value(si_value.imaginary_part(), display_unit.as_ref()).map(|_| ())
        }
        Value::Struct { fields, .. } => fields.values().try_for_each(validate_display_projection),
        Value::Indexed { entries, .. } => {
            entries.values().try_for_each(validate_display_projection)
        }
        Value::Key(key) => match KeyRendering::of(key) {
            KeyRendering::Coordinate(quantity) => validate_display_projection(&quantity),
            KeyRendering::Label { .. } | KeyRendering::Position(_) => Ok(()),
        },
        Value::Bool(_) | Value::Int(_) | Value::Datetime { .. } => Ok(()),
    }
}

/// Format a quantity's default unit label from its dimension and registered base-unit symbols.
///
/// This is intentionally kept in the runtime display layer rather than on
/// `Dimension`: dimensions describe physical semantics; unit labels are a
/// presentation concern derived from registry metadata.
#[must_use]
pub fn default_unit_label(
    dimension: &Dimension,
    symbols: &BTreeMap<BaseDimId, String>,
) -> Option<String> {
    if dimension.is_dimensionless() {
        return None;
    }

    let mut result = String::new();
    let mut first = true;

    for (id, &exp) in dimension.iter() {
        if !exp.is_positive() {
            continue;
        }
        if !first {
            result.push('*');
        }
        first = false;
        push_unit_factor(&mut result, id, exp, symbols)?;
    }

    for (id, &exp) in dimension.iter() {
        if !exp.is_negative() {
            continue;
        }
        if first {
            push_unit_factor(&mut result, id, exp, symbols)?;
            first = false;
        } else {
            result.push('/');
            push_unit_factor(&mut result, id, -exp, symbols)?;
        }
    }

    Some(result)
}

fn push_unit_factor(
    result: &mut String,
    id: &BaseDimId,
    exp: Rational,
    symbols: &BTreeMap<BaseDimId, String>,
) -> Option<()> {
    let symbol = symbols.get(id)?;
    result.push_str(symbol);
    if exp != Rational::ONE {
        result.push_str(&exp.fmt_exponent(ExponentStyle::Compact).to_string());
    }
    Some(())
}

/// Render an exact closed epoch literal in its declared time scale.
#[must_use]
pub fn datetime_literal(
    epoch: &hifitime::Epoch,
    scale: graphcal_compiler::semantic::time_scale::TimeScale,
) -> String {
    let (year, month, day, hour, minute, second, nanos) = epoch.to_gregorian(scale.to_hifitime());
    let fractional = if nanos == 0 {
        String::new()
    } else {
        format!(".{nanos:09}")
    };
    format!(
        "epoch<{scale}>(\"{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{fractional}\")"
    )
}

/// Convert a `hifitime::Epoch` to a jiff `Zoned` datetime in the given timezone
/// and format it as an ISO 8601 string.
fn format_epoch_in_timezone(
    epoch: &hifitime::Epoch,
    time_zone_id: &IanaTimeZoneId,
    time_zones: &TimeZoneRegistry,
) -> Result<String, Box<dyn std::error::Error>> {
    let ts = epoch_to_jiff_timestamp(epoch)?;
    let zdt = ts.to_zoned(time_zones.get(time_zone_id)?);
    Ok(zdt.strftime("%Y-%m-%dT%H:%M:%S%:z[%Q]").to_string())
}

/// Format a `hifitime::Epoch` as an RFC 3339 / ISO 8601 string in UTC
/// (e.g. `"2026-01-01T00:00:00Z"`), for machine consumers such as
/// Vega-Lite temporal data.
///
/// # Errors
///
/// Returns an error for epochs outside jiff's representable timestamp range.
pub(crate) fn epoch_to_rfc3339(epoch: &hifitime::Epoch) -> Result<String, EpochProjectionError> {
    epoch_to_jiff_timestamp(epoch).map(|timestamp| timestamp.to_string())
}

/// Convert a `hifitime::Epoch` to a `jiff::Timestamp` without a floating-point
/// intermediate, preserving every nanosecond.
fn epoch_to_jiff_timestamp(
    epoch: &hifitime::Epoch,
) -> Result<jiff::Timestamp, EpochProjectionError> {
    const NANOS_PER_SECOND: i128 = 1_000_000_000;
    let unix_nanos = epoch.to_unix_duration().total_nanoseconds();
    let seconds = unix_nanos.div_euclid(NANOS_PER_SECOND);
    let nanoseconds = unix_nanos.rem_euclid(NANOS_PER_SECOND);
    let seconds = i64::try_from(seconds).map_err(|_| EpochProjectionError::SecondsOutOfRange)?;
    let nanoseconds =
        i32::try_from(nanoseconds).map_err(|_| EpochProjectionError::SecondsOutOfRange)?;
    jiff::Timestamp::new(seconds, nanoseconds).map_err(Into::into)
}

pub use graphcal_compiler::node_unavailable::{NodeUnavailable, RuntimeUnavailable};

use super::output_decl_name::{OutputDeclName, OutputUnavailable};

/// A plot declaration that could not be evaluated, with the reason.
///
/// Plot evaluation is per-plot best-effort: one failing plot does not stop
/// the others, but the failure must be reported, never silently dropped
/// (#842).
#[derive(Debug, Clone, PartialEq)]
pub struct PlotError {
    /// The plot declaration name.
    pub name: ScopedName,
    /// Typed reason the plot was not rendered, including incompleteness.
    pub reason: super::plot_unavailable::PlotUnavailable,
}

/// The result of evaluating an assertion, naming the declarations a blocked
/// assertion waits on by `N`: runtime identities during evaluation, output
/// names ([`OutputDeclName`]) in an evaluation's result.
#[derive(Debug, Clone, PartialEq)]
pub enum AssertResult<N = OutputDeclName> {
    /// The assertion is not yet checkable, never an expected failure or pass.
    Blocked { reason: NodeUnavailable<N> },
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

impl<N> AssertResult<N> {
    /// The same result with every declaration it names renamed by `rename`.
    #[must_use]
    pub(crate) fn map_names<M>(&self, rename: impl FnMut(&N) -> M) -> AssertResult<M> {
        match self {
            Self::Blocked { reason } => AssertResult::Blocked {
                reason: reason.map_names(rename),
            },
            Self::Pass => AssertResult::Pass,
            Self::Fail { message } => AssertResult::Fail {
                message: message.clone(),
            },
            Self::Error { message } => AssertResult::Error {
                message: message.clone(),
            },
        }
    }
}

/// Which evaluated values a presentation boundary should expose.
///
/// Evaluation always retains every instantiated value so errors, assertions,
/// and debugging remain complete. This view controls presentation only; it
/// never changes name resolution or evaluation semantics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EvalOutputView {
    /// Entry-DAG declarations and outputs selected from included DAGs.
    #[default]
    Surface,
    /// Every value in every instantiated DAG, including internal helpers.
    All,
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
    /// Unfinished origins reached inside invoked DAGs, including private
    /// siblings of otherwise available projected outputs.
    pub unfinished_calls: Vec<OutputDeclName>,
    /// All const, param, and node values in source order with their
    /// declaration type (may contain per-node errors). Const *values* are
    /// compile-time, but a const's display unit (e.g. a dynamic conversion
    /// target) resolves at runtime and can fail per-node.
    ///
    /// Per-category views are derived by [`Self::consts`], [`Self::params`],
    /// and [`Self::nodes`].
    pub entries: Vec<(
        ScopedName,
        Result<Value, OutputUnavailable>,
        ValueDeclCategory,
    )>,
    /// Values belonging to the entry DAG's consumer-facing output surface.
    ///
    /// Internal include-instance values remain in [`Self::entries`] for the
    /// [`EvalOutputView::All`] debug view and for complete error accounting.
    pub(crate) output_surface: std::collections::HashSet<ScopedName>,
    /// Assertion results in source order: (name, result, span).
    pub assertions: Vec<(ScopedName, AssertResult, Span)>,
    /// Evaluated plot specifications in source order.
    pub plots: Vec<PlotSpec>,
    /// Plots that failed to evaluate, with their reasons (#842).
    pub plot_errors: Vec<PlotError>,
    /// Display-only failures; SI values remain available.
    pub presentation_diagnostics: Vec<crate::presentation_evidence::PresentationDiagnostic>,
    /// Evaluated figure specifications in source order.
    pub figures: Vec<FigureSpec>,
    /// Evaluated layer specifications in source order.
    pub layers: Vec<LayerSpec>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: std::collections::HashMap<ScopedName, Vec<ScopedName>>,
    /// Registry metadata needed to render the values (unit symbols, time zones).
    pub render: RenderContext,
    /// Domain constraints for params/nodes, for programmatic access (sweeping/sampling).
    pub(crate) domain_constraints:
        std::collections::HashMap<ScopedName, crate::domain_constraint::ResolvedDomainConstraint>,
}

impl EvalResult {
    /// Names of the values on the entry DAG's consumer-facing output surface.
    #[must_use]
    pub const fn output_surface(&self) -> &std::collections::HashSet<ScopedName> {
        &self.output_surface
    }

    /// Present `imported` entries (values imported from other modules) before
    /// the evaluated ones and replace the consumer-facing output surface.
    #[must_use]
    pub fn with_imported_entries(
        mut self,
        mut imported: Vec<(
            ScopedName,
            Result<Value, OutputUnavailable>,
            ValueDeclCategory,
        )>,
        output_surface: std::collections::HashSet<ScopedName>,
    ) -> Self {
        imported.append(&mut self.entries);
        self.entries = imported;
        self.output_surface = output_surface;
        self
    }

    /// Rename every scoped name this result reports, e.g. to replace private
    /// synthetic include scopes with readable names at a presentation boundary.
    pub fn rename_scoped_names(&mut self, rename: impl Fn(&ScopedName) -> ScopedName) {
        self.entries
            .iter_mut()
            .for_each(|(name, _, _)| *name = rename(name));
        self.output_surface = std::mem::take(&mut self.output_surface)
            .into_iter()
            .map(|name| rename(&name))
            .collect();
        self.assertions
            .iter_mut()
            .for_each(|(name, _, _)| *name = rename(name));
        self.plots
            .iter_mut()
            .for_each(|plot| plot.name = rename(&plot.name));
        self.plot_errors
            .iter_mut()
            .for_each(|plot| plot.name = rename(&plot.name));
        self.figures.iter_mut().for_each(|figure| {
            figure.name = rename(&figure.name);
            figure
                .plot_names
                .iter_mut()
                .for_each(|name| *name = rename(name));
        });
        self.layers.iter_mut().for_each(|layer| {
            layer.name = rename(&layer.name);
            layer
                .plot_names
                .iter_mut()
                .for_each(|name| *name = rename(name));
        });
        self.assumes_map = std::mem::take(&mut self.assumes_map)
            .into_iter()
            .map(|(name, assumers)| {
                (
                    rename(&name),
                    assumers
                        .into_iter()
                        .map(|assumer| rename(&assumer))
                        .collect(),
                )
            })
            .collect();
        self.domain_constraints = std::mem::take(&mut self.domain_constraints)
            .into_iter()
            .map(|(name, constraint)| (rename(&name), constraint))
            .collect();
    }

    fn should_output(
        &self,
        name: &ScopedName,
        result: &Result<Value, OutputUnavailable>,
        view: EvalOutputView,
    ) -> bool {
        matches!(view, EvalOutputView::All)
            || self.output_surface.contains(name)
            // A hidden failure must still explain the command's non-zero exit.
            || result.is_err()
    }

    /// Iterate over values selected by `view` in source order.
    ///
    /// Failed internal values are included even in the surface view so a
    /// non-zero evaluation result is never unexplained.
    pub fn output_values(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<
        Item = &(
            ScopedName,
            Result<Value, OutputUnavailable>,
            ValueDeclCategory,
        ),
    > {
        self.entries
            .iter()
            .filter(move |(name, result, _)| self.should_output(name, result, view))
    }

    fn output_category(
        &self,
        view: EvalOutputView,
        decl_type: ValueDeclCategory,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.entries.iter().filter_map(move |(name, result, kind)| {
            (*kind == decl_type && self.should_output(name, result, view)).then_some((name, result))
        })
    }

    /// Iterate over const values selected by `view` in source order.
    pub fn output_consts(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.output_category(view, ValueDeclCategory::Const)
    }

    /// Iterate over param values selected by `view` in source order.
    pub fn output_params(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.output_category(view, ValueDeclCategory::Param)
    }

    /// Iterate over node values selected by `view` in source order.
    pub fn output_nodes(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.output_category(view, ValueDeclCategory::Node)
    }

    fn category(
        &self,
        decl_type: ValueDeclCategory,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.entries
            .iter()
            .filter_map(move |(name, result, kind)| (*kind == decl_type).then_some((name, result)))
    }

    /// Iterate over every const value in source order.
    pub fn consts(&self) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.category(ValueDeclCategory::Const)
    }

    /// Iterate over every param value in source order.
    pub fn params(&self) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.category(ValueDeclCategory::Param)
    }

    /// Iterate over externally bindable entry parameters in source order.
    ///
    /// Included DAG input ports retain their instance scope; they are bound
    /// by the caller, not by external entry bindings. Selectively projected
    /// ports are nodes, so only local param declarations belong here.
    pub fn entry_params(
        &self,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.params().filter(|(name, _)| name.owner().is_none())
    }

    /// Iterate over every node value in source order.
    pub fn nodes(&self) -> impl Iterator<Item = (&ScopedName, &Result<Value, OutputUnavailable>)> {
        self.category(ValueDeclCategory::Node)
    }

    /// Whether a node, assertion, plot, or invoked DAG remains incomplete.
    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        !self.unfinished_calls.is_empty()
            || self
                .plot_errors
                .iter()
                .any(|plot| plot.reason.is_incomplete())
            || self
                .entries
                .iter()
                .any(|(_, result, _)| result.as_ref().is_err_and(NodeUnavailable::is_incomplete))
            || self
                .assertions
                .iter()
                .any(|(_, result, _)| matches!(result, AssertResult::Blocked { .. }))
    }

    /// Whether a genuine failure occurred, independently of incompleteness.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.entries
            .iter()
            .any(|(_, result, _)| result.as_ref().is_err_and(NodeUnavailable::has_failure))
            || self
                .plot_errors
                .iter()
                .any(|plot| plot.reason.has_failure())
            || !self.presentation_diagnostics.is_empty()
            || self.assertions.iter().any(|(_, r, _)| match r {
                AssertResult::Fail { .. } | AssertResult::Error { .. } => true,
                AssertResult::Blocked { reason } => reason.has_failure(),
                AssertResult::Pass => false,
            })
    }
}

// The typed property registry (names, value types, Vega names) lives in the
// compiler so resolution-time validation and runtime evaluation dispatch on
// one source of truth (#845).
pub use graphcal_compiler::plot_props::{
    CompositionProperty, MarkProperty, PlotProperty, PlotPropertyType,
};

/// A single evaluated plot specification.
#[derive(Debug, Clone)]
pub struct PlotSpec {
    /// Documentation from the producing declaration, independent of its output alias.
    pub doc: Option<String>,
    /// The plot declaration name.
    pub name: ScopedName,
    /// The mark type (point, line, bar, area, rect, tick).
    pub mark_type: graphcal_compiler::desugar::desugared_ast::MarkType,
    /// Evaluated encoding channels (x, y, color, etc.) with their data.
    pub encodings: Vec<(EncodingChannel, PlotFieldValue)>,
    /// Axis metadata per encoding channel.
    /// Used by the CLI to auto-generate axis titles like "Velocity (km/s)".
    pub encoding_meta: Vec<(EncodingChannel, AxisMeta)>,
    /// Display failures associated with this plot's channels; data falls back to SI.
    pub(crate) presentation_diagnostics: Vec<crate::presentation_evidence::PresentationDiagnostic>,
    /// Evaluated mark properties (`stroke_width`, `opacity`, etc.).
    pub mark_properties: Vec<(MarkProperty, PropertyValue)>,
    /// Evaluated plot-level properties (title, width, height, etc.).
    pub properties: Vec<(PlotProperty, PropertyValue)>,
    /// Whether this plot renders standalone. `#[hidden]` plots are only
    /// usable in figure/layer composition (#847).
    pub visibility: graphcal_compiler::plot_visibility::PlotVisibility,
}

/// A single evaluated figure specification.
#[derive(Debug, Clone)]
pub struct FigureSpec {
    /// Documentation from the producing declaration, independent of its output alias.
    pub doc: Option<String>,
    /// The figure declaration name.
    pub name: ScopedName,
    /// The plot names referenced by this figure.
    pub plot_names: Vec<ScopedName>,
    /// Additional evaluated properties (e.g., title).
    pub properties: Vec<(CompositionProperty, PropertyValue)>,
}

/// A single evaluated layer specification.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    /// Documentation from the producing declaration, independent of its output alias.
    pub doc: Option<String>,
    /// The layer declaration name.
    pub name: ScopedName,
    /// The plot names to overlay in this layer.
    pub plot_names: Vec<ScopedName>,
    /// Additional evaluated properties (e.g., title, width, height).
    pub properties: Vec<(CompositionProperty, PropertyValue)>,
}

/// Axis metadata for auto-generating axis titles from dimension/unit info.
#[derive(Debug, Clone, Default)]
pub struct AxisMeta {
    /// The dimension name (e.g., "Velocity", "Length * Time^-1").
    pub dimension_label: Option<String>,
    /// The label of the unit the channel's numbers are in (e.g., "km/s",
    /// "m"): the display unit of an explicit `->` conversion, otherwise the
    /// canonical unit of the SI values. `None` when the numbers carry no unit
    /// (an unconverted dimensionless or a non-quantity channel) or the
    /// dimension has no canonical unit.
    pub unit_label: Option<String>,
}

/// The evaluated data of one plot encoding channel, one value per row.
#[derive(Debug, Clone)]
pub enum PlotFieldValue {
    /// Numeric values (from evaluated numeric expressions/for-comprehensions).
    Numbers(Vec<f64>),
    /// String labels (from index keys, booleans, or a string literal), in
    /// row order.
    Labels(Vec<String>),
    /// Datetime instants as RFC 3339 / ISO 8601 strings, rendered with
    /// Vega-Lite temporal encoding (#846).
    Datetimes(Vec<String>),
}

/// The evaluated value of one plot, mark, or composition property.
///
/// The variant is the property's checked [`PlotPropertyType`]: a `Bool`
/// property such as `filled` evaluates to [`Self::Bool`], never to a string
/// spelling of the boolean.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// A string literal (e.g. `title: "Thrust"`).
    String(String),
    /// A dimensionless number (e.g. `width: 480`).
    Number(f64),
    /// A boolean (e.g. `filled: true`).
    Bool(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dim_id(name: &str) -> BaseDimId {
        BaseDimId::Prelude(
            graphcal_compiler::dimension::PreludeBaseDimension::parse(name)
                .expect("test prelude base name"),
        )
    }

    fn quantity(dimension: Dimension, display_unit: Option<DisplayUnit>) -> Value {
        Value::Quantity {
            si_value: FiniteQuantity::ONE,
            dimension,
            display_unit,
        }
    }

    fn render() -> RenderContext {
        RenderContext::new(
            BTreeMap::from([
                (dim_id("Length"), "m".to_string()),
                (dim_id("Time"), "s".to_string()),
            ]),
            TimeZoneRegistry::bundled(),
        )
    }

    #[test]
    fn keys_render_as_label_position_or_coordinate() {
        use crate::runtime_value::IndexAxis;
        use graphcal_compiler::semantic::index_def::{CoordinateDisplayUnit, CoordinateIndexData};

        let owner = graphcal_compiler::dag_id::DagId::root_in_package("key-render", "main");
        let named = IndexAxis::named_for_test(owner.clone(), "Phase", &["Launch", "Cruise"]);
        let key = KeyValue::at(named, 1).unwrap();
        let value = Value::Key(key.clone());
        assert!(matches!(
            KeyRendering::of(&key),
            KeyRendering::Label { variant, .. } if variant.as_str() == "Cruise"
        ));
        assert_eq!(
            value.format_display(&render(), UnitLabel::Inline).unwrap(),
            "Phase#Cruise"
        );
        assert_eq!(value.display_label(&render()), None);

        let finite = graphcal_compiler::semantic::index_def::FiniteIndex::try_from_u64(3).unwrap();
        let key = KeyValue::at(IndexAxis::finite(finite).unwrap(), 2).unwrap();
        assert_eq!(KeyRendering::of(&key), KeyRendering::Position(2));
        assert_eq!(
            Value::Key(key)
                .format_display(&render(), UnitLabel::Inline)
                .unwrap(),
            "2"
        );

        let hours = CoordinateDisplayUnit {
            label: Some("h".to_string()),
            scale: PositiveFiniteScale::new(3600.0).unwrap(),
        };
        let data =
            CoordinateIndexData::try_range(0.0, 7200.0, 3600.0, Dimension::dimensionless(), hours)
                .unwrap();
        let key = KeyValue::at(IndexAxis::coordinate_for_test(owner, "Hour", data), 2).unwrap();
        let KeyRendering::Coordinate(quantity) = KeyRendering::of(&key) else {
            panic!("expected a coordinate rendering");
        };
        assert_eq!(
            quantity.si_value().unwrap().get().to_bits(),
            7200.0_f64.to_bits()
        );
        let value = Value::Key(key);
        assert_eq!(value.display_label(&render()), Some("h".to_string()));
        assert_eq!(
            value.format_display(&render(), UnitLabel::Inline).unwrap(),
            "2 [h]"
        );
        assert_eq!(
            value.format_display(&render(), UnitLabel::Omitted).unwrap(),
            "2"
        );
        assert!(validate_display_projection(&value).is_ok());
    }

    fn empty_eval_result() -> EvalResult {
        EvalResult {
            unfinished_calls: Vec::new(),
            entries: Vec::new(),
            output_surface: std::collections::HashSet::new(),
            assertions: Vec::new(),
            plots: Vec::new(),
            plot_errors: Vec::new(),
            presentation_diagnostics: Vec::new(),
            figures: Vec::new(),
            layers: Vec::new(),
            assumes_map: std::collections::HashMap::new(),
            render: RenderContext::new(BTreeMap::new(), TimeZoneRegistry::bundled()),
            domain_constraints: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn display_projection_rejects_overflow_and_underflow() {
        let q = |value| FiniteQuantity::try_new(value).unwrap();
        let tiny = DisplayUnit::new("tiny", PositiveFiniteScale::new(1.0e-300).unwrap());
        assert!(quantity_display_value(q(1.0e300), Some(&tiny)).is_err());
        let huge = DisplayUnit::new("huge", PositiveFiniteScale::new(1.0e300).unwrap());
        assert!(quantity_display_value(q(1.0e-300), Some(&huge)).is_err());
        assert!(quantity_display_value(q(0.0), Some(&huge)).unwrap().abs() < f64::MIN_POSITIVE);
        assert_eq!(quantity_display_value(q(2.5), None), Ok(2.5));
    }

    #[test]
    fn epoch_projection_preserves_nanoseconds_without_binary64() {
        let epoch = hifitime::Epoch::from_unix_duration(
            hifitime::Duration::from_total_nanoseconds(1_767_225_600_000_000_001_i128),
        );
        assert_eq!(
            epoch_to_rfc3339(&epoch).unwrap(),
            "2026-01-01T00:00:00.000000001Z"
        );
    }

    #[test]
    fn has_errors_counts_plot_errors() {
        let mut result = empty_eval_result();
        result.plot_errors.push(PlotError {
            name: ScopedName::local(
                graphcal_compiler::syntax::decl_name::DeclName::expect_valid("p"),
            ),
            reason: NodeUnavailable::EvalFailed {
                message: "bad plot".to_string(),
            }
            .into(),
        });

        assert!(result.has_errors());
    }

    #[test]
    fn display_label_falls_back_to_default_unit_symbols() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = quantity(velocity, None);

        assert_eq!(value.display_label(&render()), Some("m/s".to_string()));
    }

    #[test]
    fn display_label_prefers_explicit_display_unit() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = quantity(
            velocity,
            Some(DisplayUnit::new(
                "km/h",
                PositiveFiniteScale::new(1000.0 / 3600.0).unwrap(),
            )),
        );

        assert_eq!(value.display_label(&render()), Some("km/h".to_string()));
    }

    #[test]
    fn display_label_omits_dimensionless_default_unit() {
        let value = quantity(Dimension::dimensionless(), None);

        assert_eq!(value.display_label(&render()), None);
    }

    #[test]
    fn display_label_omits_default_unit_when_symbol_is_missing() {
        let value = quantity(Dimension::base(dim_id("Mass")), None);

        assert_eq!(value.display_label(&render()), None);
    }

    fn struct_value(
        constructor: &str,
        generic_args: Vec<CheckedGenericArg>,
        fields: IndexMap<FieldName, Value>,
    ) -> Value {
        let owner = graphcal_compiler::dag_id::DagId::root_in_package("test", "main");
        Value::Struct {
            type_name: StructTypeRef::with_owner(
                owner,
                graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Mode"),
            ),
            constructor: ConstructorName::expect_valid(constructor),
            generic_args,
            fields,
        }
    }

    #[test]
    fn struct_equality_distinguishes_constructors_of_the_same_type() {
        let coast = struct_value("Coast", Vec::new(), IndexMap::new());
        let burn = struct_value("Burn", Vec::new(), IndexMap::new());

        assert_eq!(coast, struct_value("Coast", Vec::new(), IndexMap::new()));
        assert_ne!(coast, burn);
    }

    #[test]
    fn struct_equality_distinguishes_generic_arguments() {
        let fields = IndexMap::from([(FieldName::expect_valid("v"), Value::Int(1))]);
        let length = struct_value(
            "Mode",
            vec![CheckedGenericArg::Dim(Dimension::base(dim_id("Length")))],
            fields.clone(),
        );
        let time = struct_value(
            "Mode",
            vec![CheckedGenericArg::Dim(Dimension::base(dim_id("Time")))],
            fields,
        );

        assert_ne!(length, time);
    }

    fn datetime(display_tz: Option<IanaTimeZoneId>) -> Value {
        Value::Datetime {
            epoch: hifitime::Epoch::from_unix_seconds(1_730_808_000.0),
            time_scale: graphcal_compiler::semantic::time_scale::TimeScale::UTC,
            display_tz,
        }
    }

    fn tokyo() -> IanaTimeZoneId {
        TimeZoneRegistry::bundled()
            .parse_iana_id("Asia/Tokyo")
            .unwrap()
    }

    #[test]
    fn datetime_equality_includes_display_time_zone() {
        assert_eq!(datetime(Some(tokyo())), datetime(Some(tokyo())));
        assert_ne!(datetime(None), datetime(Some(tokyo())));
    }

    #[test]
    fn render_context_formats_datetimes_in_their_display_zone() {
        let render = render();
        assert_eq!(
            datetime(Some(tokyo()))
                .format_display(&render, UnitLabel::Inline)
                .unwrap(),
            "2024-11-05T21:00:00+09:00[Asia/Tokyo]"
        );
        assert_eq!(
            datetime(None)
                .format_display(&render, UnitLabel::Inline)
                .unwrap(),
            "2024-11-05T12:00:00 UTC"
        );
    }

    #[test]
    fn format_display_places_unit_label_only_when_inline() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = Value::Quantity {
            si_value: FiniteQuantity::try_new(2.5).unwrap(),
            dimension: velocity,
            display_unit: None,
        };

        assert_eq!(
            value.format_display(&render(), UnitLabel::Inline).unwrap(),
            "2.5 [m/s]"
        );
        assert_eq!(
            value.format_display(&render(), UnitLabel::Omitted).unwrap(),
            "2.5"
        );
    }
}
