//! Encoding and decoding request batches.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, RecordBatch, StructArray};
use arrow_schema::ArrowError;
use thiserror::Error;

use super::schema::{input_fields, request_schema};
use super::value_array::{Presence, ValueDecodeError, decode, decode_key_positions, encode};
use crate::batch::{InputColumn, RequestBatch, RequestBatchError};
use crate::label::PortName;
use crate::session::SessionSchema;
use crate::value::Value;
use crate::wire_type::WireType;

/// Why a received record batch is not a valid request.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RequestDecodeError {
    #[error("the request has {received} columns; the session varies {expected} parameters")]
    ColumnCount { expected: usize, received: usize },
    #[error("the inputs struct contains {count} null row(s)")]
    StructNulls { count: usize },
    #[error("the column for parameter `{input}` contains {count} null value(s)")]
    Nulls { input: PortName, count: usize },
    #[error("the column for parameter `{input}` {source}")]
    Value {
        input: PortName,
        source: ValueDecodeError,
    },
    #[error(transparent)]
    Batch(#[from] RequestBatchError),
}

impl RequestBatch {
    /// Encode the batch in its session's request schema.
    ///
    /// # Errors
    ///
    /// Returns an [`ArrowError`] only if Arrow rejects arrays built from the
    /// schema's own field types, which indicates a bug.
    pub fn to_record_batch(&self) -> Result<RecordBatch, ArrowError> {
        RecordBatch::try_new(request_schema(self.schema()), self.arrays()?)
    }

    /// Encode the batch as a struct with one child per varied parameter (see
    /// [`input_fields`]).
    ///
    /// # Errors
    ///
    /// Returns an [`ArrowError`] only if Arrow rejects arrays built from the
    /// fields' own types, which indicates a bug.
    pub fn to_struct_array(&self) -> Result<StructArray, ArrowError> {
        StructArray::try_new(input_fields(self.schema()), self.arrays()?, None)
    }

    fn arrays(&self) -> Result<Vec<ArrayRef>, ArrowError> {
        self.schema()
            .inputs()
            .iter()
            .zip(self.columns())
            .map(|(input, column)| {
                let rows = (0..self.rows())
                    .map(|row| column.value(input.wire(), row))
                    .collect::<Vec<_>>();
                encode(input.wire(), &rows, Presence::Always)
            })
            .collect()
    }

    /// Decode a received record batch as a request of `schema`.
    ///
    /// The stream schema must already have been validated against
    /// [`request_schema`]. Keys are resolved by their dictionary value.
    ///
    /// # Errors
    ///
    /// Returns [`RequestDecodeError`] for a column of the wrong type, a null,
    /// a non-finite quantity, an unknown variant, or an empty batch.
    pub fn from_record_batch(
        schema: Arc<SessionSchema>,
        batch: &RecordBatch,
    ) -> Result<Self, RequestDecodeError> {
        Self::from_arrays(schema, batch.columns())
    }

    /// Decode a received struct array as a request of `schema`.
    ///
    /// The struct's fields must already have been validated against
    /// [`input_fields`]. Keys are resolved by their dictionary value.
    ///
    /// # Errors
    ///
    /// Returns [`RequestDecodeError`] for a null struct row, a column of the
    /// wrong type, a null, a non-finite quantity, an unknown variant, or an
    /// empty array.
    pub fn from_struct_array(
        schema: Arc<SessionSchema>,
        array: &StructArray,
    ) -> Result<Self, RequestDecodeError> {
        if array.null_count() > 0 {
            return Err(RequestDecodeError::StructNulls {
                count: array.null_count(),
            });
        }
        Self::from_arrays(schema, array.columns())
    }

    fn from_arrays(
        schema: Arc<SessionSchema>,
        arrays: &[ArrayRef],
    ) -> Result<Self, RequestDecodeError> {
        if arrays.len() != schema.inputs().len() {
            return Err(RequestDecodeError::ColumnCount {
                expected: schema.inputs().len(),
                received: arrays.len(),
            });
        }
        let columns = schema
            .inputs()
            .iter()
            .zip(arrays)
            .map(|(input, array)| decode_column(input.name(), input.wire(), array.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(schema, columns)?)
    }
}

fn reject_nulls(input: &PortName, array: &dyn Array) -> Result<(), RequestDecodeError> {
    match array.null_count() {
        0 => Ok(()),
        count => Err(RequestDecodeError::Nulls {
            input: input.clone(),
            count,
        }),
    }
}

fn decode_column(
    input: &PortName,
    wire: &WireType,
    array: &dyn Array,
) -> Result<InputColumn, RequestDecodeError> {
    reject_nulls(input, array)?;
    let value_error = |source| RequestDecodeError::Value {
        input: input.clone(),
        source,
    };
    // A key column keeps the positions its dictionary resolves to.
    if let WireType::Key(key) = wire {
        return decode_key_positions(key, array)
            .map_err(value_error)?
            .into_iter()
            .enumerate()
            .map(|(row, position)| {
                position.ok_or_else(|| value_error(ValueDecodeError::Null { row }))
            })
            .collect::<Result<_, _>>()
            .map(InputColumn::Key);
    }
    let values = decode(wire, array, Presence::Always)
        .map_err(value_error)?
        .into_iter()
        .enumerate()
        .map(|(row, value)| value.ok_or_else(|| value_error(ValueDecodeError::Null { row })))
        .collect::<Result<Vec<_>, _>>()?;
    let primitive = |pick: fn(Value) -> Option<_>| {
        values
            .iter()
            .cloned()
            .map(pick)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| value_error(ValueDecodeError::ArrowType))
    };
    Ok(match wire {
        WireType::Quantity(_) => InputColumn::Quantity(primitive(|value| match value {
            Value::Quantity(value) => Some(value),
            _ => None,
        })?),
        WireType::Datetime(_) => InputColumn::Datetime(primitive(|value| match value {
            Value::Datetime(value) => Some(value),
            _ => None,
        })?),
        WireType::Int => InputColumn::Int(
            values
                .into_iter()
                .map(|value| match value {
                    Value::Int(value) => Ok(value),
                    _ => Err(value_error(ValueDecodeError::ArrowType)),
                })
                .collect::<Result<_, _>>()?,
        ),
        WireType::Bool => InputColumn::Bool(
            values
                .into_iter()
                .map(|value| match value {
                    Value::Bool(value) => Ok(value),
                    _ => Err(value_error(ValueDecodeError::ArrowType)),
                })
                .collect::<Result<_, _>>()?,
        ),
        WireType::Complex(_) => InputColumn::Complex(
            values
                .into_iter()
                .map(|value| match value {
                    Value::Complex(value) => Ok(value),
                    _ => Err(value_error(ValueDecodeError::ArrowType)),
                })
                .collect::<Result<_, _>>()?,
        ),
        WireType::Indexed(_) | WireType::Algebraic(_) => InputColumn::Structured(values),
        WireType::Key(_) => return Err(value_error(ValueDecodeError::ArrowType)),
    })
}

#[cfg(test)]
mod tests {
    use arrow_array::types::Int32Type;
    use arrow_array::{DictionaryArray, Float64Array, Int32Array, StringArray};

    use super::*;
    use crate::axis::KeyRef;
    use crate::finite::FiniteF64;
    use crate::label::VariantName;
    use crate::session::tests::sample_session;

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    fn sample_batch() -> RequestBatch {
        let schema = Arc::new(sample_session());
        let WireType::Key(mode) = schema.inputs()[1].wire() else {
            panic!("mode is a key");
        };
        let variant = |name: &str| {
            mode.position(KeyRef::Variant(&VariantName::new(name).unwrap()))
                .unwrap()
        };
        let (b, a) = (variant("B"), variant("A"));
        RequestBatch::new(
            schema,
            vec![
                InputColumn::Quantity(vec![finite(1.5), finite(2.5)]),
                InputColumn::Key(vec![b, a]),
            ],
        )
        .unwrap()
    }

    #[test]
    fn requests_round_trip() {
        let batch = sample_batch();
        let encoded = batch.to_record_batch().unwrap();
        assert_eq!(encoded.num_rows(), 2);
        let decoded =
            RequestBatch::from_record_batch(Arc::clone(batch.schema()), &encoded).unwrap();
        assert_eq!(decoded, batch);
    }

    #[test]
    fn requests_round_trip_as_struct_arrays() {
        let batch = sample_batch();
        let encoded = batch.to_struct_array().unwrap();
        assert_eq!(encoded.len(), 2);
        assert_eq!(encoded.fields(), &input_fields(batch.schema()));
        let decoded =
            RequestBatch::from_struct_array(Arc::clone(batch.schema()), &encoded).unwrap();
        assert_eq!(decoded, batch);

        let (fields, columns, _) = encoded.into_parts();
        let nulls = StructArray::new(fields, columns, Some(vec![true, false].into()));
        assert!(matches!(
            RequestBatch::from_struct_array(Arc::clone(batch.schema()), &nulls),
            Err(RequestDecodeError::StructNulls { count: 1 })
        ));
    }

    #[test]
    fn keys_resolve_by_dictionary_value_not_code() {
        let batch = sample_batch();
        // The dictionary lists variants in reverse declaration order.
        let mode = DictionaryArray::<Int32Type>::try_new(
            Int32Array::from(vec![0, 1]),
            Arc::new(StringArray::from(vec!["B", "A"])),
        )
        .unwrap();
        let encoded = RecordBatch::try_new(
            request_schema(batch.schema()),
            vec![Arc::new(Float64Array::from(vec![1.5, 2.5])), Arc::new(mode)],
        )
        .unwrap();
        let decoded =
            RequestBatch::from_record_batch(Arc::clone(batch.schema()), &encoded).unwrap();
        assert_eq!(decoded, batch);
    }

    #[test]
    fn rejects_non_finite_values_and_unknown_variants() {
        let batch = sample_batch();
        let schema = request_schema(batch.schema());
        let record = |load: Vec<f64>, mode: Vec<&str>| {
            let mode = DictionaryArray::<Int32Type>::from_iter(mode.into_iter().map(Some));
            RecordBatch::try_new(
                Arc::clone(&schema),
                vec![Arc::new(Float64Array::from(load)), Arc::new(mode)],
            )
            .unwrap()
        };
        assert!(matches!(
            RequestBatch::from_record_batch(
                Arc::clone(batch.schema()),
                &record(vec![f64::NAN], vec!["A"])
            ),
            Err(RequestDecodeError::Value {
                source: ValueDecodeError::NonFinite { row: 0 },
                ..
            })
        ));
        assert!(matches!(
            RequestBatch::from_record_batch(
                Arc::clone(batch.schema()),
                &record(vec![1.0], vec!["C"])
            ),
            Err(RequestDecodeError::Value {
                source: ValueDecodeError::UnknownVariant { .. },
                ..
            })
        ));
    }
}
