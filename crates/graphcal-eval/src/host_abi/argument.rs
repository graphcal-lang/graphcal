//! Validated arguments a host function receives.
//!
//! Arguments are built once, at the evaluator's (or another embedder's)
//! boundary, from typed values: every scalar and array element already
//! satisfies the ABI slot policy of its kind, so a host function and the
//! plugin host marshal them without validating them again.

use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::function_signature::ScalarValueKind;
use thiserror::Error;

use super::scalar::{HostInt, HostScalar, encode_bool};
use crate::runtime_value::IndexAxis;
use crate::runtime_value::dense_array::DenseArray;

/// Why a row-major shape cannot describe a dense array's elements.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DenseShapeError {
    /// A dense array needs at least one axis, and every axis an entry.
    #[error("array shapes require one or more non-empty axes")]
    EmptyAxis,
    /// The product of the extents does not fit `usize`.
    #[error("array shape cardinality overflowed usize")]
    CardinalityOverflow,
    /// The extents describe another number of elements.
    #[error("array shape {shape:?} requires {expected} values, found {found}")]
    CountMismatch {
        /// The row-major extents.
        shape: Vec<usize>,
        /// Their product.
        expected: usize,
        /// The number of elements supplied.
        found: usize,
    },
}

/// Check that non-empty row-major `shape` describes exactly `len` elements.
///
/// # Errors
///
/// Returns [`DenseShapeError`] for an empty axis, an overflowing cardinality,
/// or a mismatched element count.
pub fn check_dense_shape(shape: &[usize], len: usize) -> Result<(), DenseShapeError> {
    if shape.is_empty() || shape.contains(&0) {
        return Err(DenseShapeError::EmptyAxis);
    }
    let expected = shape.iter().try_fold(1_usize, |size, extent| {
        size.checked_mul(*extent)
            .ok_or(DenseShapeError::CardinalityOverflow)
    })?;
    if expected == len {
        Ok(())
    } else {
        Err(DenseShapeError::CountMismatch {
            shape: shape.to_vec(),
            expected,
            found: len,
        })
    }
}

/// Validated row-major elements of one array argument, all of one kind.
#[derive(Debug, Clone, PartialEq)]
pub enum HostArrayElements {
    /// Finite SI quantities.
    Quantity(Vec<FiniteQuantity>),
    /// Booleans.
    Bool(Vec<bool>),
    /// Integers representable exactly as binary64.
    Int(Vec<HostInt>),
}

impl HostArrayElements {
    /// The number of elements.
    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::Quantity(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::Int(values) => values.len(),
        }
    }

    /// Whether there are no elements (never true of an array argument).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The raw binary64 ABI slots of the elements, in row-major order.
    #[must_use]
    pub fn abi_slots(&self) -> Vec<f64> {
        match self {
            Self::Quantity(values) => values.iter().map(|value| value.get()).collect(),
            Self::Bool(values) => values.iter().map(|value| encode_bool(*value)).collect(),
            Self::Int(values) => values.iter().map(|value| value.abi_slot()).collect(),
        }
    }

    /// Whether the elements have the semantic kind `kind` declares.
    #[must_use]
    pub const fn have_kind<D>(&self, kind: &ScalarValueKind<D>) -> bool {
        matches!(
            (self, kind),
            (Self::Quantity(_), ScalarValueKind::Quantity(_))
                | (Self::Bool(_), ScalarValueKind::Bool)
                | (Self::Int(_), ScalarValueKind::Int)
        )
    }
}

/// A dense row-major array argument with validated elements.
#[derive(Debug, Clone, PartialEq)]
pub struct HostArgumentArray {
    shape: Vec<usize>,
    elements: HostArrayElements,
}

impl HostArgumentArray {
    /// Pair validated `elements` with the non-empty row-major `shape` they fill.
    ///
    /// # Errors
    ///
    /// Returns [`DenseShapeError`] when `shape` does not describe exactly the
    /// elements.
    pub fn try_new(
        shape: Vec<usize>,
        elements: HostArrayElements,
    ) -> Result<Self, DenseShapeError> {
        check_dense_shape(&shape, elements.len())?;
        Ok(Self { shape, elements })
    }

    /// The array of a dense runtime array, whose axes are non-empty and match
    /// its element count by construction, with its values as `elements`.
    pub(crate) fn from_dense<T>(
        array: DenseArray<T>,
        elements: impl FnOnce(Vec<T>) -> HostArrayElements,
    ) -> Self {
        let (axes, values) = array.into_parts();
        Self {
            shape: axes.iter().map(IndexAxis::len).collect(),
            elements: elements(values),
        }
    }

    /// Ordered row-major extents.
    #[must_use]
    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    /// The validated elements.
    #[must_use]
    pub const fn elements(&self) -> &HostArrayElements {
        &self.elements
    }
}

/// One validated argument of a host function.
#[derive(Debug, Clone, PartialEq)]
pub enum HostArgument {
    /// A single scalar.
    Scalar(HostScalar),
    /// A dense row-major array.
    Array(HostArgumentArray),
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::function_signature::DimMonomial;
    use graphcal_compiler::syntax::dimension::DimVarName;

    use super::*;

    fn quantities(values: &[f64]) -> HostArrayElements {
        HostArrayElements::Quantity(
            values
                .iter()
                .map(|value| FiniteQuantity::try_new(*value).unwrap())
                .collect(),
        )
    }

    #[test]
    fn dense_shapes_must_describe_exactly_their_elements() {
        assert_eq!(check_dense_shape(&[2, 3], 6), Ok(()));
        assert_eq!(check_dense_shape(&[], 0), Err(DenseShapeError::EmptyAxis));
        assert_eq!(
            check_dense_shape(&[2, 0], 0),
            Err(DenseShapeError::EmptyAxis)
        );
        assert_eq!(
            check_dense_shape(&[usize::MAX, 2], 1),
            Err(DenseShapeError::CardinalityOverflow)
        );
        let mismatch = check_dense_shape(&[2, 3], 5).unwrap_err();
        assert_eq!(
            mismatch.to_string(),
            "array shape [2, 3] requires 6 values, found 5"
        );
        assert!(HostArgumentArray::try_new(vec![3], quantities(&[1.0, 2.0])).is_err());
        let array = HostArgumentArray::try_new(vec![1, 2], quantities(&[1.0, -2.0])).unwrap();
        assert_eq!(array.shape(), [1, 2]);
        assert_eq!(array.elements().abi_slots(), [1.0, -2.0]);
    }

    #[test]
    fn array_elements_encode_and_classify_by_kind() {
        let ints = HostArrayElements::Int(vec![
            HostInt::try_new(7).unwrap(),
            HostInt::try_new(-8).unwrap(),
        ]);
        let flags = HostArrayElements::Bool(vec![false, true, false]);
        assert_eq!((ints.len(), ints.is_empty()), (2, false));
        assert_eq!(ints.abi_slots(), [7.0, -8.0]);
        assert_eq!(flags.abi_slots(), [0.0, 1.0, 0.0]);
        let quantity_kind: ScalarValueKind<DimVarName> =
            ScalarValueKind::Quantity(DimMonomial::fixed(Dimension::dimensionless()));
        assert!(quantities(&[1.0]).have_kind(&quantity_kind));
        assert!(!quantities(&[1.0]).have_kind(&ScalarValueKind::<DimVarName>::Int));
        assert!(ints.have_kind(&ScalarValueKind::<DimVarName>::Int));
        assert!(!ints.have_kind(&ScalarValueKind::<DimVarName>::Bool));
        assert!(flags.have_kind(&ScalarValueKind::<DimVarName>::Bool));
        assert!(!flags.have_kind(&quantity_kind));
    }
}
