//! Rectangular row-major arrays over typed index axes.

use graphcal_compiler::syntax::non_empty::NonEmpty;

use super::RuntimeValue;
use super::index_axis::IndexAxis;
use super::indexed::IndexedValue;

/// A rectangular array: one element per combination of axis keys, stored
/// row-major (the last axis varies fastest).
///
/// Every constructor establishes `data.len() == product(axis lengths)`, so
/// kernels can address elements by position without re-checking the shape.
#[derive(Debug, Clone)]
pub struct DenseArray<T> {
    axes: NonEmpty<IndexAxis>,
    data: Vec<T>,
}

/// Why an indexed value could not be flattened into a [`DenseArray`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseArrayError<E> {
    /// Sibling entries are indexed by different axes, or some are indexed and
    /// others are not.
    Ragged,
    /// A leaf value was rejected by the element conversion.
    Element(E),
}

/// Buffer length that does not match the product of the axis lengths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("dense array has {actual} elements, but its axes have {expected}")]
pub struct DenseShapeError {
    pub expected: usize,
    pub actual: usize,
}

impl<T> DenseArray<T> {
    /// Pair row-major `data` with `axes`.
    ///
    /// # Errors
    ///
    /// Returns [`DenseShapeError`] when `data` does not have exactly one
    /// element per combination of axis keys.
    pub fn try_new(axes: NonEmpty<IndexAxis>, data: Vec<T>) -> Result<Self, DenseShapeError> {
        let expected = axes
            .iter()
            .try_fold(1_usize, |size, axis| size.checked_mul(axis.len()));
        match expected {
            Some(expected) if expected == data.len() => Ok(Self { axes, data }),
            Some(expected) => Err(DenseShapeError {
                expected,
                actual: data.len(),
            }),
            // A buffer can never hold more than `usize::MAX` elements.
            None => Err(DenseShapeError {
                expected: usize::MAX,
                actual: data.len(),
            }),
        }
    }

    /// Flatten an indexed value, converting each leaf with `element`.
    ///
    /// The shape is read from the first entry at every depth; every other
    /// entry must have the same axes.
    ///
    /// # Errors
    ///
    /// Returns [`DenseArrayError::Ragged`] when sibling entries disagree on
    /// their axes, or the first error `element` reports.
    pub fn try_from_indexed<E>(
        value: &IndexedValue<RuntimeValue>,
        mut element: impl FnMut(&RuntimeValue) -> Result<T, E>,
    ) -> Result<Self, DenseArrayError<E>> {
        let axes = shape_of(value);
        let mut data = Vec::new();
        let (_, inner_axes) = axes.split_first();
        for entry in value.values() {
            fill(entry, inner_axes, &mut element, &mut data)?;
        }
        Ok(Self { axes, data })
    }

    /// Split into the axes and the row-major elements.
    #[must_use]
    pub fn into_parts(self) -> (NonEmpty<IndexAxis>, Vec<T>) {
        (self.axes, self.data)
    }

    /// Rebuild the nested indexed value, converting each element with `leaf`.
    ///
    /// # Errors
    ///
    /// Returns the first error `leaf` reports.
    pub fn try_to_indexed<E>(
        &self,
        mut leaf: impl FnMut(&T) -> Result<RuntimeValue, E>,
    ) -> Result<IndexedValue<RuntimeValue>, E> {
        let (first, rest) = self.axes.split_first();
        build_indexed(first, rest, &self.data, &mut leaf)
    }
}

/// The axes of `value` read along its first entries.
fn shape_of(value: &IndexedValue<RuntimeValue>) -> NonEmpty<IndexAxis> {
    let mut inner_axes = Vec::new();
    let mut current = value.values().first();
    while let RuntimeValue::Indexed(inner) = current {
        inner_axes.push(inner.axis().clone());
        current = inner.values().first();
    }
    NonEmpty::new(value.axis().clone(), inner_axes)
}

/// Append the leaves of `value`, which must be indexed by exactly `axes`.
fn fill<T, E>(
    value: &RuntimeValue,
    axes: &[IndexAxis],
    element: &mut impl FnMut(&RuntimeValue) -> Result<T, E>,
    data: &mut Vec<T>,
) -> Result<(), DenseArrayError<E>> {
    match (axes.split_first(), value) {
        (Some((expected, inner_axes)), RuntimeValue::Indexed(indexed))
            if indexed.axis().matches(expected) =>
        {
            indexed
                .values()
                .iter()
                .try_for_each(|entry| fill(entry, inner_axes, element, data))
        }
        (None, RuntimeValue::Indexed(_)) | (Some(_), _) => Err(DenseArrayError::Ragged),
        (None, leaf) => {
            data.push(element(leaf).map_err(DenseArrayError::Element)?);
            Ok(())
        }
    }
}

/// Rebuild the indexed value over `axis` and `inner_axes` from row-major
/// `data`, whose length is the product of their lengths.
fn build_indexed<T, E>(
    axis: &IndexAxis,
    inner_axes: &[IndexAxis],
    data: &[T],
    leaf: &mut impl FnMut(&T) -> Result<RuntimeValue, E>,
) -> Result<IndexedValue<RuntimeValue>, E> {
    let chunk_len = inner_axes.iter().map(IndexAxis::len).product::<usize>();
    let chunks = data.chunks_exact(chunk_len).collect::<Vec<_>>();
    IndexedValue::try_from_axis(axis.clone(), |position, _| {
        let chunk = chunks[position];
        match inner_axes.split_first() {
            None => leaf(&chunk[0]),
            Some((inner, rest)) => {
                build_indexed(inner, rest, chunk, leaf).map(RuntimeValue::Indexed)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::registry::index::FiniteIndex;
    use graphcal_compiler::syntax::non_empty::NonEmpty;

    use super::{DenseArray, DenseArrayError, DenseShapeError};
    use crate::runtime_value::{IndexAxis, IndexedValue, RuntimeValue};

    fn rows() -> IndexAxis {
        IndexAxis::named_for_test(DagId::root_in_package("dense", "main"), "Row", &["A", "B"])
    }

    fn columns() -> IndexAxis {
        IndexAxis::finite(FiniteIndex::try_from_u64(3).unwrap()).unwrap()
    }

    fn int(value: i64) -> RuntimeValue {
        RuntimeValue::Int(value)
    }

    fn row(values: [i64; 3]) -> RuntimeValue {
        RuntimeValue::Indexed(IndexedValue::for_test(
            columns(),
            values.into_iter().map(int).collect(),
        ))
    }

    fn leaf(value: &RuntimeValue) -> Result<i64, &'static str> {
        match value {
            RuntimeValue::Int(value) => Ok(*value),
            _ => Err("not an Int"),
        }
    }

    #[test]
    fn flattens_row_major_and_rebuilds_the_same_value() {
        let matrix = IndexedValue::for_test(rows(), vec![row([1, 2, 3]), row([4, 5, 6])]);
        let dense = DenseArray::try_from_indexed(&matrix, leaf).unwrap();
        let (axes, data) = dense.clone().into_parts();
        assert_eq!(axes.len(), 2);
        assert!(axes.first().matches(&rows()));
        assert!(axes.last().matches(&columns()));
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6]);

        let rebuilt = dense
            .try_to_indexed(|value| Ok::<_, ()>(int(*value)))
            .unwrap();
        let flattened = DenseArray::try_from_indexed(&rebuilt, leaf).unwrap();
        assert_eq!(flattened.into_parts().1, vec![1, 2, 3, 4, 5, 6]);
        assert!(rebuilt.axis().matches(&rows()));
    }

    #[test]
    fn rejects_ragged_and_mixed_entries() {
        let other_columns = RuntimeValue::Indexed(IndexedValue::for_test(
            IndexAxis::finite(FiniteIndex::try_from_u64(1).unwrap()).unwrap(),
            vec![int(7)],
        ));
        let ragged = IndexedValue::for_test(rows(), vec![row([1, 2, 3]), other_columns]);
        assert_eq!(
            DenseArray::try_from_indexed(&ragged, leaf).unwrap_err(),
            DenseArrayError::Ragged
        );

        let mixed = IndexedValue::for_test(rows(), vec![row([1, 2, 3]), int(4)]);
        assert_eq!(
            DenseArray::try_from_indexed(&mixed, leaf).unwrap_err(),
            DenseArrayError::Ragged
        );

        let deeper = IndexedValue::for_test(rows(), vec![int(1), row([1, 2, 3])]);
        assert_eq!(
            DenseArray::try_from_indexed(&deeper, leaf).unwrap_err(),
            DenseArrayError::Ragged
        );

        let bad_leaf = IndexedValue::for_test(rows(), vec![int(1), RuntimeValue::Bool(true)]);
        assert_eq!(
            DenseArray::try_from_indexed(&bad_leaf, leaf).unwrap_err(),
            DenseArrayError::Element("not an Int")
        );
    }

    #[test]
    fn explicit_buffers_must_match_the_axes() {
        let axes = NonEmpty::new(rows(), vec![columns()]);
        assert!(DenseArray::try_new(axes.clone(), vec![0; 6]).is_ok());
        assert_eq!(
            DenseArray::try_new(axes, vec![0; 5]).unwrap_err(),
            DenseShapeError {
                expected: 6,
                actual: 5
            }
        );
    }
}
