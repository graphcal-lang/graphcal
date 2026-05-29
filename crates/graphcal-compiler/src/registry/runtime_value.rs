//! Runtime value types used during evaluation.

use indexmap::IndexMap;

use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::syntax::names::{
    FieldName, IndexName, IndexVariantName, ResolvedIndexVariant, StructTypeName,
};

/// The kind of a [`RuntimeValue`], used in type-mismatch error reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeValueKind {
    Scalar,
    Bool,
    Int,
    Label {
        index_name: IndexTypeRef,
        variant: IndexVariantName,
    },
    Struct {
        type_name: StructTypeRef,
    },
    Indexed {
        index_name: IndexTypeRef,
    },
    RangeLabel,
    Datetime,
}

impl std::fmt::Display for RuntimeValueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scalar => write!(f, "Scalar"),
            Self::Bool => write!(f, "Bool"),
            Self::Int => write!(f, "Int"),
            Self::Label {
                index_name,
                variant,
            } => write!(f, "label `{}`", variant.qualified_by(index_name.name())),
            Self::Struct { type_name } => write!(f, "struct `{type_name}`"),
            Self::Indexed { index_name } => write!(f, "indexed value `{index_name}[...]`"),
            Self::RangeLabel => write!(f, "RangeLabel"),
            Self::Datetime => write!(f, "Datetime"),
        }
    }
}

/// Error returned when a [`RuntimeValue`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeValueError {
    /// What kind of value was expected (e.g. "scalar", "Bool").
    pub expected: &'static str,
    /// A description of what the value was being used for.
    pub context: String,
    /// The actual variant encountered.
    pub actual: RuntimeValueKind,
}

impl std::fmt::Display for RuntimeValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "expected {} for {}, got {}",
            self.expected, self.context, self.actual
        )
    }
}

impl std::error::Error for RuntimeValueError {}

/// A runtime value: either a scalar (f64 in SI units), a bool, a struct, or an indexed collection.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Scalar(f64),
    Bool(bool),
    Int(i64),
    /// A label of a named index (e.g., `Maneuver.Departure`).
    Label {
        index_name: IndexTypeRef,
        variant: IndexVariantName,
    },
    Struct {
        /// Concrete constructor/type leaf for display plus optional canonical owning struct type.
        ///
        /// Tagged-union values keep the constructor leaf here (e.g. `LowThrust`) while
        /// module-aware evaluation stores the owning union's canonical `StructType` identity
        /// in the carrier's `resolved` field.
        type_name: StructTypeRef,
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values, preserving declaration order.
    Indexed {
        index_name: IndexTypeRef,
        entries: IndexMap<IndexVariantName, Self>,
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
    /// Construct an ownerless standalone label value from leaf names.
    #[must_use]
    pub const fn ownerless_label(index_name: IndexName, variant: IndexVariantName) -> Self {
        Self::Label {
            index_name: IndexTypeRef::ownerless(index_name),
            variant,
        }
    }

    /// Construct a module-aware label value from a resolved index-variant reference.
    #[must_use]
    pub fn resolved_label(resolved: &ResolvedIndexVariant) -> Self {
        Self::Label {
            index_name: IndexTypeRef::from_resolved(resolved.index().clone()),
            variant: resolved.variant().clone(),
        }
    }

    /// Construct an ownerless standalone struct value from a concrete type/constructor leaf.
    #[must_use]
    pub const fn ownerless_struct(
        type_name: StructTypeName,
        fields: IndexMap<FieldName, Self>,
    ) -> Self {
        Self::Struct {
            type_name: StructTypeRef::ownerless(type_name),
            fields,
        }
    }

    /// Construct an ownerless standalone indexed value from an index leaf.
    #[must_use]
    pub const fn ownerless_indexed(
        index_name: IndexName,
        entries: IndexMap<IndexVariantName, Self>,
    ) -> Self {
        Self::Indexed {
            index_name: IndexTypeRef::ownerless(index_name),
            entries,
        }
    }

    /// Return the [`RuntimeValueKind`] of this value.
    #[must_use]
    pub fn kind(&self) -> RuntimeValueKind {
        match self {
            Self::Scalar(_) => RuntimeValueKind::Scalar,
            Self::Bool(_) => RuntimeValueKind::Bool,
            Self::Int(_) => RuntimeValueKind::Int,
            Self::Label {
                index_name,
                variant,
            } => RuntimeValueKind::Label {
                index_name: index_name.clone(),
                variant: variant.clone(),
            },
            Self::Struct { type_name, .. } => RuntimeValueKind::Struct {
                type_name: type_name.clone(),
            },
            Self::Indexed { index_name, .. } => RuntimeValueKind::Indexed {
                index_name: index_name.clone(),
            },
            Self::RangeLabel { .. } => RuntimeValueKind::RangeLabel,
            Self::Datetime(_) => RuntimeValueKind::Datetime,
        }
    }

    /// Extract scalar value, returning a structured error if this is not a scalar.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub fn expect_scalar(&self, context: &str) -> Result<f64, RuntimeValueError> {
        match self {
            Self::Scalar(v) | Self::RangeLabel { value: v, .. } => Ok(*v),
            other => Err(RuntimeValueError {
                expected: "scalar",
                context: context.to_string(),
                actual: other.kind(),
            }),
        }
    }

    /// Extract boolean value, returning a structured error if this is not a Bool.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub fn expect_bool(&self, context: &str) -> Result<bool, RuntimeValueError> {
        match self {
            Self::Bool(b) => Ok(*b),
            other => Err(RuntimeValueError {
                expected: "Bool",
                context: context.to_string(),
                actual: other.kind(),
            }),
        }
    }
}
