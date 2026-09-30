//! Runtime value types used during evaluation.

use indexmap::IndexMap;

use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::finite_value::{FiniteQuantity, NonFiniteQuantity};
use graphcal_compiler::registry::checked_type::{CheckedGenericArg, IndexTypeRef};
use graphcal_compiler::resolved_name::{ResolvedIndexVariant, ResolvedStructTypeName};
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};

pub mod dense_array;
mod index_axis;
mod indexed;

pub use index_axis::IndexAxis;
pub use indexed::IndexedValue;

/// Error returned when a [`RuntimeValue`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeValueError {
    /// What kind of value was expected (e.g. "quantity", "Bool").
    expected: &'static str,
    /// A description of what the value was being used for.
    context: String,
    /// Description of the value actually encountered.
    actual: String,
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
        generic_args: Vec<CheckedGenericArg>,
        fields: IndexMap<FieldName, Self>,
    },
    /// One entry per key of a concrete index axis.
    Indexed(IndexedValue<Self>),
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
    #[cfg(test)]
    pub fn complex(
        re: f64,
        im: f64,
    ) -> Result<Self, graphcal_compiler::complex_value::ComplexValueError> {
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

    /// Construct a module-aware label value from a resolved index-variant reference.
    #[must_use]
    pub fn resolved_label(resolved: &ResolvedIndexVariant) -> Self {
        Self::Label {
            index_name: IndexTypeRef::from_resolved(resolved.index().clone()),
            variant: resolved.variant().clone(),
        }
    }

    /// Construct a struct value whose type is named directly, for tests.
    #[cfg(test)]
    #[must_use]
    pub const fn struct_with_owner(
        owner: graphcal_compiler::dag_id::DagId,
        type_name: graphcal_compiler::syntax::type_name::StructTypeName,
        constructor: ConstructorName,
        fields: IndexMap<FieldName, Self>,
    ) -> Self {
        Self::Struct {
            type_name: ResolvedStructTypeName::for_test(owner, type_name),
            constructor,
            generic_args: Vec::new(),
            fields,
        }
    }

    /// Describe this value's variant (not its contents) for diagnostics.
    #[must_use]
    pub const fn describe(&self) -> RuntimeValueDescription<'_> {
        RuntimeValueDescription(self)
    }

    /// Extract quantity value, returning a structured error if this is not a quantity.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub fn expect_quantity(&self, context: &str) -> Result<f64, RuntimeValueError> {
        match self {
            Self::Quantity(v) | Self::CoordinateLabel { value: v, .. } => Ok(v.get()),
            other => Err(RuntimeValueError {
                expected: "quantity",
                context: context.to_string(),
                actual: other.describe().to_string(),
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
                actual: other.describe().to_string(),
            }),
        }
    }
}

/// Diagnostic rendering of a [`RuntimeValue`]'s variant, from
/// [`RuntimeValue::describe`].
#[derive(Debug, Clone, Copy)]
pub struct RuntimeValueDescription<'a>(&'a RuntimeValue);

impl std::fmt::Display for RuntimeValueDescription<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            RuntimeValue::Quantity(_) => write!(f, "Quantity"),
            RuntimeValue::Complex(_) => write!(f, "Complex"),
            RuntimeValue::Bool(_) => write!(f, "Bool"),
            RuntimeValue::Int(_) => write!(f, "Int"),
            RuntimeValue::Label {
                index_name,
                variant,
            } => write!(f, "label `{index_name}#{variant}`"),
            RuntimeValue::Struct { constructor, .. } => write!(f, "struct `{constructor}`"),
            RuntimeValue::Indexed(indexed) => {
                write!(f, "indexed value `{}[...]`", indexed.index())
            }
            RuntimeValue::CoordinateLabel { index_name, .. } => {
                write!(f, "coordinate label `{index_name}`")
            }
            RuntimeValue::Datetime(_) => write!(f, "Datetime"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime_value::RuntimeValue;
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::registry::checked_type::IndexTypeRef;
    use graphcal_compiler::syntax::index_name::IndexName;

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
