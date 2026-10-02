//! Runtime value types used during evaluation.

use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::finite_value::{FiniteQuantity, NonFiniteQuantity};

pub(crate) mod dense_array;
mod indexed;

pub use graphcal_compiler::semantic::index_axis::IndexAxis;
pub use graphcal_compiler::semantic::key_value::{KeyElement, KeyValue};
pub use graphcal_compiler::semantic::struct_value::{StructFieldsError, StructValue};
pub use indexed::IndexedValue;

/// The compiler/evaluator boundary representation of a runtime value.
///
/// Equality is structural and is the language's `==`: quantities and complex
/// values compare numerically, keys by axis identity and position, struct
/// values by nominal application and fields, and indexed values by axis
/// identity and entries.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    Quantity(FiniteQuantity),
    Complex(ComplexValue),
    Bool(bool),
    Int(i64),
    /// A `Key<I>` value: one entry of a concrete axis.
    Key(KeyValue),
    /// A constructor applied to exactly its declared fields.
    Struct(StructValue<Self>),
    /// One entry per key of a concrete index axis.
    Indexed(IndexedValue<Self>),
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

    /// Describe this value's variant (not its contents) for diagnostics.
    #[must_use]
    pub(crate) const fn describe(&self) -> RuntimeValueDescription<'_> {
        RuntimeValueDescription(self)
    }
}

/// Diagnostic rendering of a [`RuntimeValue`]'s variant, from
/// [`RuntimeValue::describe`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeValueDescription<'a>(&'a RuntimeValue);

impl std::fmt::Display for RuntimeValueDescription<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            RuntimeValue::Quantity(_) => write!(f, "Quantity"),
            RuntimeValue::Complex(_) => write!(f, "Complex"),
            RuntimeValue::Bool(_) => write!(f, "Bool"),
            RuntimeValue::Int(_) => write!(f, "Int"),
            RuntimeValue::Key(key) => write!(f, "key of `{}`", key.index()),
            RuntimeValue::Struct(value) => write!(f, "struct `{}`", value.constructor()),
            RuntimeValue::Indexed(indexed) => {
                write!(f, "indexed value `{}[...]`", indexed.index())
            }
            RuntimeValue::Datetime(_) => write!(f, "Datetime"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime_value::RuntimeValue;

    #[test]
    fn scalar_ingress_rejects_non_finite_values() {
        assert!(RuntimeValue::quantity(f64::NAN).is_err());
        assert!(RuntimeValue::complex(f64::INFINITY, 0.0).is_err());
        assert_eq!(
            RuntimeValue::quantity(-0.0).unwrap(),
            RuntimeValue::quantity(0.0).unwrap()
        );
    }
}
