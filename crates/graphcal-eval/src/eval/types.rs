use indexmap::IndexMap;
use miette::Diagnostic;
use thiserror::Error;

use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{DeclName, FieldName, IndexName, StructTypeName, VariantName};
use graphcal_syntax::span::Span;

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclType {
    Const,
    Param,
    Node,
}

/// Display unit metadata: the unit name(s) and scale factor for pretty-printing.
#[derive(Debug, Clone)]
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
    /// A label value from a named index (e.g., `Maneuver::Departure`).
    Label {
        /// The index name (e.g., `Maneuver`).
        index_name: IndexName,
        /// The variant name (e.g., `Departure`).
        variant: VariantName,
    },
    Struct {
        /// The struct type name.
        type_name: StructTypeName,
        /// The variant name (= type name for single-variant struct sugar).
        variant: VariantName,
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
        time_scale: crate::time_scale::TimeScale,
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
        symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
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
}

/// A runtime error associated with a specific node or param evaluation.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: std::collections::HashMap<String, Vec<String>>,
    /// Base dimension symbols for display (e.g., `BaseDimId::Prelude("Length") → "m"`).
    pub base_dim_symbols: std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
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

/// Top-level compile error that wraps both parse and eval errors.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(#[from] graphcal_syntax::parser::ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(#[from] crate::error::GraphcalError),
}
