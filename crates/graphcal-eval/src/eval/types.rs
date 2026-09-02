use std::collections::BTreeMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::EncodingChannel;
use graphcal_compiler::dimension::{BaseDimId, Dimension, Rational};
use graphcal_compiler::registry::declared_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::time_zone::{IanaTimeZoneId, TimeZoneRegistry};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::{FieldName, StructTypeName};

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclType {
    Const,
    Param,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PositiveFiniteDisplayScale(f64);

impl PositiveFiniteDisplayScale {
    fn try_new(value: f64) -> Result<Self, DisplayProjectionError> {
        if !value.is_finite() || value <= 0.0 {
            Err(DisplayProjectionError::InvalidScale { scale: value })
        } else {
            Ok(Self(value))
        }
    }

    const fn get(self) -> f64 {
        self.0
    }
}

/// Display unit metadata: the unit name(s) and validated scale factor for pretty-printing.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayUnit {
    /// Human-readable unit string (e.g., "km", "m/s^2", "km/h")
    pub label: String,
    scale: PositiveFiniteDisplayScale,
}

impl DisplayUnit {
    /// Construct display metadata with a positive finite scale.
    ///
    /// # Errors
    ///
    /// Returns [`DisplayProjectionError::InvalidScale`] for zero, negative, or
    /// non-finite scales.
    pub fn try_new(label: impl Into<String>, scale: f64) -> Result<Self, DisplayProjectionError> {
        Ok(Self {
            label: label.into(),
            scale: PositiveFiniteDisplayScale::try_new(scale)?,
        })
    }

    /// Scale factor from SI to this display unit.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale.get()
    }
}

/// Failure to project a finite SI quantity into a requested display unit.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DisplayProjectionError {
    /// Display scales must be finite and strictly positive.
    #[error("display-unit scale must be positive and finite, got {scale}")]
    InvalidScale { scale: f64 },
    /// Public quantity values must remain finite.
    #[error("display input must be finite, got {value}")]
    NonFiniteInput { value: f64 },
    /// Dividing by a tiny valid scale overflowed binary64.
    #[error("display conversion to `{unit}` produced a non-finite value")]
    NonFiniteResult { unit: String },
    /// A nonzero SI value became zero in the requested unit.
    #[error("display conversion to `{unit}` underflowed to zero")]
    Underflow { unit: String },
}

/// Error returned by a display accessor or projection.
#[derive(Debug, Error)]
pub enum DisplayValueError {
    /// The accessor was called on the wrong public value variant.
    #[error(transparent)]
    Value(#[from] ValueError),
    /// Numeric projection could not preserve display invariants.
    #[error(transparent)]
    Projection(#[from] DisplayProjectionError),
}

/// Failure to project an exact hifitime epoch into jiff's timestamp range.
#[derive(Debug, Error)]
pub enum EpochProjectionError {
    /// Whole Unix seconds exceeded jiff's signed-second input representation.
    #[error("datetime Unix seconds are outside the i64 range")]
    SecondsOutOfRange,
    /// Jiff rejected an otherwise exact seconds/nanoseconds pair.
    #[error(transparent)]
    Jiff(#[from] jiff::Error),
}

/// A user-facing evaluated runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Quantity {
        /// The value in base SI units.
        si_value: f64,
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
    /// A label value from a named index (e.g., `Maneuver#Departure`).
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
        time_scale: graphcal_compiler::registry::time_scale::TimeScale,
        /// Optional IANA timezone for display (e.g. `"America/New_York"`).
        display_tz: Option<IanaTimeZoneId>,
        /// Explicit bundled registry used for timezone-aware display.
        time_zones: TimeZoneRegistry,
    },
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Quantity {
                    si_value: l_si,
                    dimension: l_dim,
                    display_unit: l_unit,
                },
                Self::Quantity {
                    si_value: r_si,
                    dimension: r_dim,
                    display_unit: r_unit,
                },
            ) => l_si == r_si && l_dim == r_dim && l_unit == r_unit,
            (
                Self::Complex {
                    si_value: l_si,
                    dimension: l_dim,
                    display_unit: l_unit,
                },
                Self::Complex {
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
                    ..
                },
                Self::Datetime {
                    epoch: r_epoch,
                    time_scale: r_scale,
                    display_tz: r_tz,
                    ..
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
    lhs: &IndexMap<IndexEntryKey, Value>,
    rhs: &IndexMap<IndexEntryKey, Value>,
) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .all(|(variant, value)| rhs.get(variant).is_some_and(|rhs_value| value == rhs_value))
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
    /// Returns [`ValueError`] if this is not a `Quantity`.
    pub fn si_value(&self) -> Result<f64, ValueError> {
        match self {
            Self::Quantity { si_value, .. } => Ok(*si_value),
            other => Err(ValueError {
                expected: "Quantity",
                actual: other.variant_description(),
            }),
        }
    }

    /// Get the Cartesian SI components of a complex quantity.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] if this is not a `Complex` value.
    pub fn complex_si_value(&self) -> Result<ComplexValue, ValueError> {
        match self {
            Self::Complex { si_value, .. } => Ok(*si_value),
            other => Err(ValueError {
                expected: "Complex",
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
    /// otherwise uses registered base-unit symbols (e.g., "m/s", "kg"). If any
    /// referenced base dimension has no registered symbol, no default label is shown.
    #[must_use]
    pub fn display_label(&self, symbols: &BTreeMap<BaseDimId, String>) -> Option<String> {
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
    /// If `symbols` is provided, quantity values include their unit label in brackets
    /// (e.g., `"42.5 [km/h]"`). Without `symbols`, only the numeric value is shown.
    ///
    /// Composite values (`Struct`, `Indexed`) are shown as their variant name or
    /// a placeholder string, not recursively expanded.
    pub fn format_display(
        &self,
        symbols: Option<&BTreeMap<BaseDimId, String>>,
    ) -> Result<String, DisplayProjectionError> {
        Ok(match self {
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::Label {
                index_name,
                variant,
            } => variant.qualified_by(&index_name.display_name()).to_string(),
            Self::Struct { type_name, .. } => type_name.as_str().to_string(),
            Self::Datetime {
                epoch,
                display_tz,
                time_zones,
                ..
            } => format_epoch_with_tz(epoch, display_tz.as_ref(), time_zones),
            Self::Quantity {
                si_value,
                display_unit,
                ..
            } => {
                let formatted = graphcal_compiler::registry::format::format_number(
                    quantity_display_value(*si_value, display_unit.as_ref())?,
                );
                match symbols.and_then(|s| self.display_label(s)) {
                    Some(label) => format!("{formatted} [{label}]"),
                    None => formatted,
                }
            }
            Self::Complex {
                si_value,
                display_unit,
                ..
            } => {
                let re = graphcal_compiler::registry::format::format_number(
                    quantity_display_value(si_value.re(), display_unit.as_ref())?,
                );
                let displayed_im = quantity_display_value(si_value.im(), display_unit.as_ref())?;
                let sign = if displayed_im.is_sign_negative() {
                    "-"
                } else {
                    "+"
                };
                let im = graphcal_compiler::registry::format::format_number(displayed_im.abs());
                let formatted = format!("{re} {sign} {im}i");
                match symbols.and_then(|s| self.display_label(s)) {
                    Some(label) => format!("{formatted} [{label}]"),
                    None => formatted,
                }
            }
            Self::Indexed { .. } => "[...]".to_string(),
        })
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
            epoch,
            display_tz,
            time_zones,
            ..
        } = self
        else {
            return None;
        };
        Some(format_epoch_with_tz(epoch, display_tz.as_ref(), time_zones))
    }
}

/// Compute the displayed quantity value from its SI value and optional display unit.
///
/// Returns `si_value` directly when no display unit is set; otherwise scales it
/// by the unit's conversion factor (`display_value = si_value / scale`).
///
/// # Errors
///
/// Rejects a non-finite input, an overflowing conversion, or a nonzero value
/// that underflows to zero in the requested unit.
pub fn quantity_display_value(
    si_value: f64,
    display_unit: Option<&DisplayUnit>,
) -> Result<f64, DisplayProjectionError> {
    if !si_value.is_finite() {
        return Err(DisplayProjectionError::NonFiniteInput { value: si_value });
    }
    let Some(display_unit) = display_unit else {
        return Ok(si_value);
    };
    let displayed = si_value / display_unit.scale();
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
            quantity_display_value(si_value.re(), display_unit.as_ref())?;
            quantity_display_value(si_value.im(), display_unit.as_ref()).map(|_| ())
        }
        Value::Struct { fields, .. } => fields.values().try_for_each(validate_display_projection),
        Value::Indexed { entries, .. } => {
            entries.values().try_for_each(validate_display_projection)
        }
        Value::Bool(_) | Value::Int(_) | Value::Label { .. } | Value::Datetime { .. } => Ok(()),
    }
}

/// Format a quantity's default unit label from its dimension and registered base-unit symbols.
///
/// This is intentionally kept in the runtime display layer rather than on
/// `Dimension`: dimensions describe physical semantics; unit labels are a
/// presentation concern derived from registry metadata.
#[must_use]
pub(super) fn default_unit_label(
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
        push_unit_factor(&mut result, id, exp, symbols)?;
    }

    for (id, &exp) in dimension.iter() {
        if exp.num() >= 0 {
            continue;
        }
        if first {
            push_unit_factor(&mut result, id, exp, symbols)?;
            first = false;
        } else {
            result.push('/');
            let Ok(positive_exp) = exp.checked_neg() else {
                return None;
            };
            push_unit_factor(&mut result, id, positive_exp, symbols)?;
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
        result.push('^');
        result.push_str(&exp.to_string());
    }
    Some(())
}

/// Format an `hifitime::Epoch` with an optional IANA timezone.
///
/// If `tz` is `Some`, converts to that timezone via jiff and formats as
/// `"2024-11-05T10:00:00+09:00[Asia/Tokyo]"`.
/// Otherwise, falls back to hifitime's `Display` (e.g. `"2024-11-05T12:00:00 UTC"`).
#[must_use]
pub fn format_epoch_with_tz(
    epoch: &hifitime::Epoch,
    tz: Option<&IanaTimeZoneId>,
    time_zones: &TimeZoneRegistry,
) -> String {
    if let Some(time_zone_id) = tz
        && let Ok(formatted) = format_epoch_in_timezone(epoch, time_zone_id, time_zones)
    {
        return formatted;
    }
    format!("{epoch}")
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
pub fn epoch_to_rfc3339(epoch: &hifitime::Epoch) -> Result<String, EpochProjectionError> {
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
    /// Values belonging to the entry DAG's consumer-facing output surface.
    ///
    /// Internal include-instance values remain in [`Self::all`] for the
    /// [`EvalOutputView::All`] debug view and for complete error accounting.
    pub(crate) output_surface: std::collections::HashSet<ScopedName>,
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
    /// Base dimension symbols for display (e.g., prelude `Length` → `m`).
    pub base_dim_symbols:
        std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
    /// Domain constraints for params/nodes, for programmatic access (sweeping/sampling).
    pub(crate) domain_constraints:
        std::collections::HashMap<ScopedName, crate::domain_check::ResolvedDomainConstraint>,
}

impl EvalResult {
    fn should_output(
        &self,
        name: &ScopedName,
        result: &Result<Value, NodeError>,
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
    ) -> impl Iterator<Item = &(ScopedName, Result<Value, NodeError>, DeclType)> {
        self.all
            .iter()
            .filter(move |(name, result, _)| self.should_output(name, result, view))
    }

    fn output_category(
        &self,
        view: EvalOutputView,
        decl_type: DeclType,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, NodeError>)> {
        self.all.iter().filter_map(move |(name, result, kind)| {
            (*kind == decl_type && self.should_output(name, result, view)).then_some((name, result))
        })
    }

    /// Iterate over const values selected by `view` in source order.
    pub fn output_consts(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, NodeError>)> {
        self.output_category(view, DeclType::Const)
    }

    /// Iterate over param values selected by `view` in source order.
    pub fn output_params(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, NodeError>)> {
        self.output_category(view, DeclType::Param)
    }

    /// Iterate over node values selected by `view` in source order.
    pub fn output_nodes(
        &self,
        view: EvalOutputView,
    ) -> impl Iterator<Item = (&ScopedName, &Result<Value, NodeError>)> {
        self.output_category(view, DeclType::Node)
    }

    /// Returns `true` if any const/param/node/plot evaluation failed or any assertion failed.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.all.iter().any(|(_, result, _)| result.is_err())
            || !self.plot_errors.is_empty()
            || self.assertions.iter().any(|(_, r, _)| {
                matches!(r, AssertResult::Fail { .. } | AssertResult::Error { .. })
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
    /// Whether this plot renders standalone. `false` for `#[hidden]`
    /// plots, which are only usable in figure/layer composition (#847).
    pub displayed: bool,
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

/// Top-level compile error for parsing, semantic evaluation, and external
/// parameter binding.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(#[from] graphcal_compiler::syntax::parser::ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(#[from] graphcal_compiler::registry::error::GraphcalError),

    /// A value supplied through an external binding format failed semantic
    /// validation. The boundary source and parameter span deliberately replace
    /// synthetic expression offsets in the underlying error.
    #[error("invalid binding for `{name}`: {reason}")]
    #[diagnostic(code(graphcal::O004))]
    ExternalBinding {
        /// Entry-DAG parameter receiving the value.
        name: DeclName,
        /// Original compiler/evaluator error rendered at the boundary.
        reason: String,
        /// External parameter source, such as inline JSON, stdin, or a file.
        #[source_code]
        src: NamedSource<Arc<String>>,
        /// Span identifying the parameter in the external document.
        #[label("value for parameter `{name}`")]
        span: SourceSpan,
    },
}

impl From<graphcal_compiler::cancellation::Cancelled> for CompileError {
    fn from(cancelled: graphcal_compiler::cancellation::Cancelled) -> Self {
        Self::Eval(graphcal_compiler::registry::error::GraphcalError::from(
            cancelled,
        ))
    }
}

impl CompileError {
    /// Whether this outcome represents cooperative cancellation rather than a
    /// Graphcal source error.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Eval(error) if error.is_cancelled())
    }

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
    /// `ParseError` and external-binding diagnostics always carry a source;
    /// `GraphcalError` may return `None` for a few variants representing
    /// source-less errors (e.g. `FileNotFound`, `CircularImport`).
    #[must_use]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        match self {
            Self::Parse(e) => Some(e.named_source()),
            Self::Eval(e) => e.named_source(),
            Self::ExternalBinding { src, .. } => Some(src),
        }
    }
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

    fn empty_eval_result() -> EvalResult {
        EvalResult {
            consts: Vec::new(),
            params: Vec::new(),
            nodes: Vec::new(),
            all: Vec::new(),
            output_surface: std::collections::HashSet::new(),
            assertions: Vec::new(),
            plots: Vec::new(),
            plot_errors: Vec::new(),
            figures: Vec::new(),
            layers: Vec::new(),
            assumes_map: std::collections::HashMap::new(),
            base_dim_symbols: BTreeMap::new(),
            domain_constraints: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn display_projection_rejects_invalid_scales_overflow_and_underflow() {
        assert!(DisplayUnit::try_new("bad", 0.0).is_err());
        assert!(DisplayUnit::try_new("bad", f64::INFINITY).is_err());

        let tiny = DisplayUnit::try_new("tiny", 1.0e-300).unwrap();
        assert!(quantity_display_value(1.0e300, Some(&tiny)).is_err());
        let huge = DisplayUnit::try_new("huge", 1.0e300).unwrap();
        assert!(quantity_display_value(1.0e-300, Some(&huge)).is_err());
        assert!(quantity_display_value(0.0, Some(&huge)).unwrap().abs() < f64::MIN_POSITIVE);
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
            name: ScopedName::parse("p").unwrap(),
            message: "bad plot".to_string(),
        });

        assert!(result.has_errors());
    }

    #[test]
    fn display_label_falls_back_to_default_unit_symbols() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = quantity(velocity, None);

        assert_eq!(value.display_label(&symbols()), Some("m/s".to_string()));
    }

    #[test]
    fn display_label_prefers_explicit_display_unit() {
        let velocity =
            (Dimension::base(dim_id("Length")) / Dimension::base(dim_id("Time"))).unwrap();
        let value = quantity(
            velocity,
            Some(DisplayUnit::try_new("km/h", 1000.0 / 3600.0).unwrap()),
        );

        assert_eq!(value.display_label(&symbols()), Some("km/h".to_string()));
    }

    #[test]
    fn display_label_omits_dimensionless_default_unit() {
        let value = quantity(Dimension::dimensionless(), None);

        assert_eq!(value.display_label(&symbols()), None);
    }

    #[test]
    fn display_label_omits_default_unit_when_symbol_is_missing() {
        let value = quantity(Dimension::base(dim_id("Mass")), None);

        assert_eq!(value.display_label(&symbols()), None);
    }
}
