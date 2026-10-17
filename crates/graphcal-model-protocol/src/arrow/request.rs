//! Encoding and decoding request batches.

use std::sync::Arc;

use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, BooleanArray, DictionaryArray, Float64Array, Int32Array, Int64Array,
    RecordBatch, StringArray,
};
use arrow_schema::ArrowError;
use thiserror::Error;

use super::schema::request_schema;
use crate::batch::{InputColumn, RequestBatch, RequestBatchError};
use crate::finite::FiniteF64;
use crate::label::{PortName, VariantName};
use crate::session::SessionSchema;
use crate::wire_type::{KeyType, VariantPosition, WireType};

/// Why a received record batch is not a valid request.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RequestDecodeError {
    #[error("the request has {received} columns; the session varies {expected} parameters")]
    ColumnCount { expected: usize, received: usize },
    #[error("the column for parameter `{input}` does not have its Arrow type")]
    ArrowType { input: PortName },
    #[error("the column for parameter `{input}` contains {count} null value(s)")]
    Nulls { input: PortName, count: usize },
    #[error("parameter `{input}` has a non-finite value at row {row}")]
    NonFinite { input: PortName, row: usize },
    #[error("parameter `{input}` has unknown variant `{variant}` at row {row}")]
    UnknownVariant {
        input: PortName,
        variant: String,
        row: usize,
    },
    #[error("parameter `{input}` has an invalid dictionary key at row {row}")]
    DictionaryKey { input: PortName, row: usize },
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
        let columns = self
            .schema()
            .inputs()
            .iter()
            .zip(self.columns())
            .map(|(input, column)| -> Result<ArrayRef, ArrowError> {
                Ok(match (column, input.wire()) {
                    (InputColumn::Quantity(values), _) => Arc::new(Float64Array::from_iter_values(
                        values.iter().map(|value| value.get()),
                    )),
                    (InputColumn::Int(values), _) => Arc::new(Int64Array::from(values.clone())),
                    (InputColumn::Bool(values), _) => Arc::new(BooleanArray::from(values.clone())),
                    (InputColumn::Key(positions), WireType::Key(key)) => {
                        key_array(key, positions.iter().map(|position| Some(*position)))?
                    }
                    (InputColumn::Key(_), _) => {
                        return Err(ArrowError::InvalidArgumentError(format!(
                            "key column for non-key parameter `{}`",
                            input.name()
                        )));
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        RecordBatch::try_new(request_schema(self.schema()), columns)
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
        if batch.num_columns() != schema.inputs().len() {
            return Err(RequestDecodeError::ColumnCount {
                expected: schema.inputs().len(),
                received: batch.num_columns(),
            });
        }
        let columns = schema
            .inputs()
            .iter()
            .zip(batch.columns())
            .map(|(input, array)| decode_column(input.name(), input.wire(), array.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(schema, columns)?)
    }
}

/// A `Dictionary<Int32, Utf8>` array over every variant of `key`, in
/// declaration order.
pub(super) fn key_array(
    key: &KeyType,
    positions: impl Iterator<Item = Option<VariantPosition>>,
) -> Result<ArrayRef, ArrowError> {
    let dictionary = Arc::new(StringArray::from_iter_values(
        key.variants().iter().map(VariantName::as_str),
    ));
    let codes = positions
        .map(|position| {
            position
                .map(|position| {
                    // `KeyType` guarantees the variant count fits in `i32`.
                    i32::try_from(position.get()).map_err(|_| {
                        ArrowError::InvalidArgumentError(format!(
                            "variant position {} exceeds Int32",
                            position.get()
                        ))
                    })
                })
                .transpose()
        })
        .collect::<Result<Int32Array, _>>()?;
    Ok(Arc::new(DictionaryArray::<Int32Type>::try_new(
        codes, dictionary,
    )?))
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
    let arrow_type = || RequestDecodeError::ArrowType {
        input: input.clone(),
    };
    reject_nulls(input, array)?;
    Ok(match wire {
        WireType::Quantity(_) => {
            let values = array
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(arrow_type)?;
            InputColumn::Quantity(
                values
                    .values()
                    .iter()
                    .enumerate()
                    .map(|(row, value)| {
                        FiniteF64::new(*value).map_err(|_| RequestDecodeError::NonFinite {
                            input: input.clone(),
                            row,
                        })
                    })
                    .collect::<Result<_, _>>()?,
            )
        }
        WireType::Int => InputColumn::Int(
            array
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(arrow_type)?
                .values()
                .to_vec(),
        ),
        WireType::Bool => InputColumn::Bool(
            array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .ok_or_else(arrow_type)?
                .values()
                .iter()
                .collect(),
        ),
        WireType::Key(key) => {
            let positions = decode_keys(input, key, array)?
                .into_iter()
                .enumerate()
                .map(|(row, position)| {
                    position.ok_or_else(|| RequestDecodeError::DictionaryKey {
                        input: input.clone(),
                        row,
                    })
                })
                .collect::<Result<_, _>>()?;
            InputColumn::Key(positions)
        }
    })
}

/// Resolve every dictionary entry of a key column by value; a null row
/// decodes as `None`.
pub(super) fn decode_keys(
    input: &PortName,
    key: &KeyType,
    array: &dyn Array,
) -> Result<Vec<Option<VariantPosition>>, RequestDecodeError> {
    let arrow_type = || RequestDecodeError::ArrowType {
        input: input.clone(),
    };
    let dictionary = array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .ok_or_else(arrow_type)?;
    let values = dictionary
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(arrow_type)?;
    // Positions of each dictionary entry within the key type, resolved once.
    let resolved = (0..values.len())
        .map(|entry| {
            (!values.is_null(entry))
                .then(|| key.position(values.value(entry)))
                .flatten()
        })
        .collect::<Vec<_>>();
    dictionary
        .keys()
        .iter()
        .enumerate()
        .map(|(row, code)| {
            let Some(code) = code else {
                return Ok(None);
            };
            let entry = usize::try_from(code)
                .ok()
                .filter(|entry| *entry < values.len())
                .ok_or_else(|| RequestDecodeError::DictionaryKey {
                    input: input.clone(),
                    row,
                })?;
            resolved[entry]
                .ok_or_else(|| RequestDecodeError::UnknownVariant {
                    input: input.clone(),
                    variant: if values.is_null(entry) {
                        "null".to_owned()
                    } else {
                        values.value(entry).to_owned()
                    },
                    row,
                })
                .map(Some)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::sample_session;

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    fn sample_batch() -> RequestBatch {
        let schema = Arc::new(sample_session());
        let WireType::Key(mode) = schema.inputs()[1].wire() else {
            panic!("mode is a key");
        };
        let b = mode.position("B").unwrap();
        let a = mode.position("A").unwrap();
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
            Err(RequestDecodeError::NonFinite { row: 0, .. })
        ));
        assert!(matches!(
            RequestBatch::from_record_batch(
                Arc::clone(batch.schema()),
                &record(vec![1.0], vec!["C"])
            ),
            Err(RequestDecodeError::UnknownVariant { .. })
        ));
    }
}
