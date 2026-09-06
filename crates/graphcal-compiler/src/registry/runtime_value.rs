//! Runtime value types used during evaluation.

use indexmap::IndexMap;

use crate::complex_value::ComplexValue;
use crate::dag_id::DagId;
use crate::finite_value::{FiniteQuantity, NonFiniteQuantity};
use crate::registry::declared_type::{DeclaredGenericArg, IndexTypeRef};
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::type_name::{
    ConstructorName, FieldName, ResolvedStructTypeName, StructTypeName,
};

/// The kind of a [`RuntimeValue`], used in type-mismatch error reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeValueKind {
    Quantity,
    Complex,
    Bool,
    Int,
    Label {
        index_name: IndexTypeRef,
        variant: IndexVariantName,
    },
    Struct {
        type_name: ResolvedStructTypeName,
        constructor: ConstructorName,
    },
    Indexed {
        index_name: IndexTypeRef,
    },
    CoordinateLabel {
        index_name: IndexTypeRef,
    },
    Datetime,
}

impl std::fmt::Display for RuntimeValueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Quantity => write!(f, "Quantity"),
            Self::Complex => write!(f, "Complex"),
            Self::Bool => write!(f, "Bool"),
            Self::Int => write!(f, "Int"),
            Self::Label {
                index_name,
                variant,
            } => {
                let display_index = index_name.display_name();
                write!(f, "label `{}`", variant.qualified_by(&display_index))
            }
            Self::Struct { constructor, .. } => write!(f, "struct `{constructor}`"),
            Self::Indexed { index_name } => write!(f, "indexed value `{index_name}[...]`"),
            Self::CoordinateLabel { index_name } => write!(f, "coordinate label `{index_name}`"),
            Self::Datetime => write!(f, "Datetime"),
        }
    }
}

/// Error returned when a [`RuntimeValue`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeValueError {
    /// What kind of value was expected (e.g. "quantity", "Bool").
    expected: &'static str,
    /// A description of what the value was being used for.
    context: String,
    /// The actual variant encountered.
    actual: RuntimeValueKind,
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

/// The compiler/evaluator boundary representation of a runtime value.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Quantity(FiniteQuantity),
    Complex(ComplexValue),
    Bool(bool),
    Int(i64),
    /// Internal carrier for a named-index loop case.
    ///
    /// The type checker prevents this from escaping as a user value; evaluation
    /// uses it only for index access and `match` dispatch.
    Label {
        index_name: IndexTypeRef,
        variant: IndexVariantName,
    },
    Struct {
        /// Canonical nominal identity, independent of source aliases and display spelling.
        type_name: ResolvedStructTypeName,
        /// Constructor member identity within `type_name` (not a display leaf).
        constructor: ConstructorName,
        /// Concrete generic identity needed by field constraints and equality.
        generic_args: Vec<DeclaredGenericArg>,
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection keyed by named labels or typed positions.
    Indexed {
        index_name: IndexTypeRef,
        entries: IndexMap<IndexEntryKey, Self>,
    },
    /// A coordinate label during coordinate-index iteration.
    /// Carries the index identity, position, and SI value.
    CoordinateLabel {
        index_name: IndexTypeRef,
        position: usize,
        value: FiniteQuantity,
    },
    /// A datetime instant (internally stored as a `hifitime::Epoch`).
    Datetime(hifitime::Epoch),
}

impl RuntimeValue {
    /// Construct a finite quantity runtime value.
    pub fn quantity(value: f64) -> Result<Self, NonFiniteQuantity> {
        FiniteQuantity::try_new(value).map(Self::Quantity)
    }

    /// Construct a finite complex runtime value from Cartesian components.
    pub fn complex(re: f64, im: f64) -> Result<Self, crate::complex_value::ComplexValueError> {
        ComplexValue::try_new(re, im).map(Self::Complex)
    }

    /// Construct a coordinate label with a finite coordinate value.
    pub fn coordinate_label(
        index_name: IndexTypeRef,
        position: usize,
        value: f64,
    ) -> Result<Self, NonFiniteQuantity> {
        FiniteQuantity::try_new(value).map(|value| Self::CoordinateLabel {
            index_name,
            position,
            value,
        })
    }

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

    /// Construct a module-aware label value from a resolved index-variant reference.
    #[must_use]
    pub fn resolved_label(resolved: &ResolvedIndexVariant) -> Self {
        Self::Label {
            index_name: IndexTypeRef::from_resolved(resolved.index().clone()),
            variant: resolved.variant().clone(),
        }
    }

    /// Construct a struct value after resolving the struct leaf into an owner.
    #[must_use]
    pub fn struct_with_owner(
        owner: DagId,
        type_name: StructTypeName,
        constructor: ConstructorName,
        fields: IndexMap<FieldName, Self>,
    ) -> Self {
        Self::Struct {
            type_name: ResolvedStructTypeName::from_def(owner, type_name),
            constructor,
            generic_args: Vec::new(),
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
        }
    }

    /// Return the [`RuntimeValueKind`] of this value.
    #[must_use]
    pub fn kind(&self) -> RuntimeValueKind {
        match self {
            Self::Quantity(_) => RuntimeValueKind::Quantity,
            Self::Complex(_) => RuntimeValueKind::Complex,
            Self::Bool(_) => RuntimeValueKind::Bool,
            Self::Int(_) => RuntimeValueKind::Int,
            Self::Label {
                index_name,
                variant,
            } => RuntimeValueKind::Label {
                index_name: index_name.clone(),
                variant: variant.clone(),
            },
            Self::Struct {
                type_name,
                constructor,
                ..
            } => RuntimeValueKind::Struct {
                type_name: type_name.clone(),
                constructor: constructor.clone(),
            },
            Self::Indexed { index_name, .. } => RuntimeValueKind::Indexed {
                index_name: index_name.clone(),
            },
            Self::CoordinateLabel { index_name, .. } => RuntimeValueKind::CoordinateLabel {
                index_name: index_name.clone(),
            },
            Self::Datetime(_) => RuntimeValueKind::Datetime,
        }
    }

    /// Extract quantity value, returning a structured error if this is not a quantity.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub fn expect_quantity(&self, context: &str) -> Result<f64, RuntimeValueError> {
        match self {
            Self::Quantity(v) | Self::CoordinateLabel { value: v, .. } => Ok(v.get()),
            other => Err(RuntimeValueError {
                expected: "quantity",
                context: context.to_string(),
                actual: other.kind(),
            }),
        }
    }

    /// Extract a complex value, returning a structured error for another variant.
    pub fn expect_complex(&self, context: &str) -> Result<ComplexValue, RuntimeValueError> {
        match self {
            Self::Complex(value) => Ok(*value),
            other => Err(RuntimeValueError {
                expected: "complex quantity",
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

#[cfg(test)]
mod tests {
    use crate::dag_id::DagId;
    use crate::registry::declared_type::IndexTypeRef;
    use crate::registry::runtime_value::RuntimeValue;
    use crate::syntax::index_name::IndexName;

    #[test]
    fn scalar_and_coordinate_ingress_reject_non_finite_values() {
        assert!(RuntimeValue::quantity(f64::NAN).is_err());
        assert!(RuntimeValue::complex(f64::INFINITY, 0.0).is_err());

        let index = IndexTypeRef::with_owner(
            DagId::root_in_package("finite-tests", "root"),
            IndexName::expect_valid("Sample"),
        );
        let coordinate = RuntimeValue::coordinate_label(index.clone(), 0, -0.0).unwrap();
        let RuntimeValue::CoordinateLabel { value, .. } = coordinate else {
            panic!("expected coordinate label");
        };
        assert_eq!(value.get().to_bits(), (-0.0_f64).to_bits());
        assert!(RuntimeValue::coordinate_label(index, 0, f64::NEG_INFINITY).is_err());
    }
}
