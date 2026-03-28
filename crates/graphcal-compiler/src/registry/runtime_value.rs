//! Runtime value types used during evaluation.

use indexmap::IndexMap;

use crate::syntax::names::{FieldName, IndexName, StructTypeName, VariantName};

/// A runtime value: either a scalar (f64 in SI units), a bool, a struct, or an indexed collection.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Scalar(f64),
    Bool(bool),
    Int(i64),
    /// A label of a named index (e.g., `Maneuver::Departure`).
    Label {
        index_name: IndexName,
        variant: VariantName,
    },
    Struct {
        type_name: StructTypeName,
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values, preserving declaration order.
    Indexed {
        index_name: IndexName,
        entries: IndexMap<VariantName, Self>,
    },
    /// A range index label during `Unfold` iteration.
    /// Carries the step index and SI value (for arithmetic like `t - prev_t`).
    RangeLabel {
        step_index: usize,
        value: f64,
    },
    /// A datetime instant (internally stored as a `hifitime::Epoch`).
    Datetime(hifitime::Epoch),
}

impl RuntimeValue {
    /// Extract scalar value, returning an error message if this is not a scalar.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub fn expect_scalar(&self, context: &str) -> Result<f64, String> {
        match self {
            Self::Scalar(v) => Ok(*v),
            Self::Bool(_) => Err(format!("expected scalar for {context}, got Bool")),
            Self::Int(i) => Err(format!("expected scalar for {context}, got Int({i})")),
            Self::Label {
                index_name,
                variant,
            } => Err(format!(
                "expected scalar for {context}, got label `{index_name}::{variant}`"
            )),
            Self::Struct { type_name, .. } => Err(format!(
                "expected scalar for {context}, got struct `{type_name}`"
            )),
            Self::Indexed { index_name, .. } => Err(format!(
                "expected scalar for {context}, got indexed value `{index_name}[...]`"
            )),
            Self::RangeLabel { value, .. } => Ok(*value),
            Self::Datetime(_) => Err(format!("expected scalar for {context}, got Datetime")),
        }
    }

    /// Extract boolean value, returning an error message if this is not a Bool.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub fn expect_bool(&self, context: &str) -> Result<bool, String> {
        match self {
            Self::Bool(b) => Ok(*b),
            other => Err(format!("expected Bool for {context}, got {other:?}")),
        }
    }
}
