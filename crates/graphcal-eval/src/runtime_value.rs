//! Runtime value types used during evaluation.

use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::finite_value::{FiniteQuantity, NonFiniteQuantity};

pub(crate) mod dense_array;
mod index_axis;
mod indexed;
mod key_value;
mod struct_value;

pub use index_axis::IndexAxis;
pub use indexed::IndexedValue;
pub use key_value::{KeyElement, KeyValue};
pub use struct_value::{StructFieldsError, StructValue};

/// Error returned when a [`RuntimeValue`] accessor is called on an incompatible variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeValueError {
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
    pub const fn describe(&self) -> RuntimeValueDescription<'_> {
        RuntimeValueDescription(self)
    }

    /// Extract quantity value, returning a structured error if this is not a quantity.
    /// (Type mismatches should be caught by `dim_check`; this is defense-in-depth.)
    pub(crate) fn expect_quantity(
        &self,
        context: &str,
    ) -> Result<FiniteQuantity, RuntimeValueError> {
        match self {
            Self::Quantity(v) => Ok(*v),
            other => Err(RuntimeValueError {
                expected: "quantity",
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
