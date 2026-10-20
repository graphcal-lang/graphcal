//! Encoding and decoding the values of a session's read outputs.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, BooleanArray, Float64Array, Int64Array, StructArray};
use arrow_schema::ArrowError;
use thiserror::Error;

use super::schema::output_fields;
use super::value_array::{ValueDecodeError, decode, encode, key_array, with_placeholders};
use crate::batch::{OutputColumn, OutputValues, OutputValuesBuilder, OutputValuesError};
use crate::finite::FiniteF64;
use crate::label::PortName;
use crate::session::SessionSchema;
use crate::value::Value;
use crate::wire_type::WireType;

/// Why a received struct array is not valid output values.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum OutputDecodeError {
    #[error("the outputs have {received} columns; the session reads {expected} outputs")]
    ColumnCount { expected: usize, received: usize },
    #[error("the outputs struct contains {count} null row(s)")]
    Nulls { count: usize },
    #[error("the column for output `{output}` does not have its Arrow type")]
    ArrowType { output: PortName },
    #[error("output `{output}` has a non-finite value at row {row}")]
    NonFinite { output: PortName, row: usize },
    #[error("the column for output `{output}` {source}")]
    Value {
        output: PortName,
        source: ValueDecodeError,
    },
    #[error("row {row} has a value for output `{present}` but none for output `{absent}`")]
    PartialRow {
        row: usize,
        present: PortName,
        absent: PortName,
    },
    #[error(transparent)]
    Values(#[from] OutputValuesError),
}

impl OutputValues {
    /// Encode the values as a struct with one nullable child per read output
    /// (see [`output_fields`]), null exactly in rows without values.
    ///
    /// # Errors
    ///
    /// Returns an [`ArrowError`] only if Arrow rejects arrays built from the
    /// fields' own types, which indicates a bug.
    pub fn to_struct_array(&self) -> Result<StructArray, ArrowError> {
        let schema = self.schema();
        let arrays = schema
            .outputs()
            .iter()
            .zip(self.columns())
            .map(|(output, column)| -> Result<ArrayRef, ArrowError> {
                Ok(match (column, output.wire()) {
                    (OutputColumn::Quantity(values), _) => Arc::new(Float64Array::from_iter(
                        values.iter().map(|value| value.map(FiniteF64::get)),
                    )),
                    (OutputColumn::Int(values), _) => Arc::new(Int64Array::from(values.clone())),
                    (OutputColumn::Bool(values), _) => Arc::new(BooleanArray::from(values.clone())),
                    (OutputColumn::Key(values), WireType::Key(key)) => {
                        key_array(key, values.iter().copied())?
                    }
                    (OutputColumn::Key(_), _) => {
                        return Err(ArrowError::InvalidArgumentError(format!(
                            "key column for non-key output `{}`",
                            output.name()
                        )));
                    }
                    (OutputColumn::Datetime(values), _) => Arc::new(Float64Array::from_iter(
                        values.iter().map(|value| value.map(FiniteF64::get)),
                    )),
                    (OutputColumn::Complex(values), wire) => {
                        let (values, valid) = with_placeholders(
                            wire,
                            values.iter().map(|value| value.map(Value::Complex)),
                        );
                        encode(wire, &values, Some(&valid))?
                    }
                    (OutputColumn::Structured(values), wire) => {
                        let (values, valid) = with_placeholders(wire, values.iter().cloned());
                        encode(wire, &values, Some(&valid))?
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        StructArray::try_new(output_fields(schema), arrays, None)
    }

    /// Decode a received struct array as output values of `schema`.
    ///
    /// The struct's fields must already have been validated against
    /// [`output_fields`]. Keys are resolved by their dictionary value.
    ///
    /// # Errors
    ///
    /// Returns [`OutputDecodeError`] for a column of the wrong count or type,
    /// a null struct row, a non-finite quantity, an unknown variant, a row
    /// with only some of its values, or an empty array.
    pub fn from_struct_array(
        schema: Arc<SessionSchema>,
        array: &StructArray,
    ) -> Result<Self, OutputDecodeError> {
        if array.num_columns() != schema.outputs().len() {
            return Err(OutputDecodeError::ColumnCount {
                expected: schema.outputs().len(),
                received: array.num_columns(),
            });
        }
        if array.null_count() > 0 {
            return Err(OutputDecodeError::Nulls {
                count: array.null_count(),
            });
        }
        let columns = schema
            .outputs()
            .iter()
            .zip(array.columns())
            .map(|(output, array)| decode_output(output.name(), output.wire(), array.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;

        let mut builder = OutputValuesBuilder::new(schema);
        for row in 0..array.len() {
            let mut present = None;
            let mut absent = None;
            for (output, column) in builder.schema().outputs().iter().zip(&columns) {
                let slot = if column[row].is_some() {
                    &mut present
                } else {
                    &mut absent
                };
                slot.get_or_insert_with(|| output.name().clone());
            }
            let values = match (present, absent) {
                (Some(present), Some(absent)) => {
                    return Err(OutputDecodeError::PartialRow {
                        row,
                        present,
                        absent,
                    });
                }
                (Some(_), None) => Some(
                    columns
                        .iter()
                        .filter_map(|column| column[row].clone())
                        .collect(),
                ),
                (None, _) => None,
            };
            builder.push(values)?;
        }
        Ok(builder.finish()?)
    }
}

/// Decode one output column into per-row optional values.
fn decode_output(
    output: &PortName,
    wire: &WireType,
    array: &dyn Array,
) -> Result<Vec<Option<Value>>, OutputDecodeError> {
    let arrow_type = || OutputDecodeError::ArrowType {
        output: output.clone(),
    };
    Ok(match wire {
        WireType::Quantity(_) => array
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(arrow_type)?
            .iter()
            .enumerate()
            .map(|(row, value)| {
                value
                    .map(|value| {
                        FiniteF64::new(value).map(Value::Quantity).map_err(|_| {
                            OutputDecodeError::NonFinite {
                                output: output.clone(),
                                row,
                            }
                        })
                    })
                    .transpose()
            })
            .collect::<Result<_, _>>()?,
        WireType::Int => array
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(arrow_type)?
            .iter()
            .map(|value| value.map(Value::Int))
            .collect(),
        WireType::Bool => array
            .as_any()
            .downcast_ref::<BooleanArray>()
            .ok_or_else(arrow_type)?
            .iter()
            .map(|value| value.map(Value::Bool))
            .collect(),
        WireType::Complex(_)
        | WireType::Datetime(_)
        | WireType::Key(_)
        | WireType::Indexed(_)
        | WireType::Algebraic(_) => {
            decode(wire, array).map_err(|source| OutputDecodeError::Value {
                output: output.clone(),
                source,
            })?
        }
    })
}

#[cfg(test)]
mod tests {
    use arrow_schema::DataType;

    use super::*;
    use crate::session::tests::sample_session;

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    fn sample_values() -> OutputValues {
        let mut builder = OutputValuesBuilder::new(Arc::new(sample_session()));
        builder
            .push(Some(vec![Value::Bool(true), Value::Quantity(finite(2.0))]))
            .unwrap();
        builder.push(None).unwrap();
        builder
            .push(Some(vec![
                Value::Bool(false),
                Value::Quantity(finite(-1.0)),
            ]))
            .unwrap();
        builder.finish().unwrap()
    }

    #[test]
    fn output_values_round_trip_with_absent_rows() {
        let values = sample_values();
        let encoded = values.to_struct_array().unwrap();
        assert_eq!(encoded.len(), 3);
        assert_eq!(encoded.null_count(), 0);
        assert!(encoded.columns().iter().all(|column| column.is_null(1)));
        let decoded =
            OutputValues::from_struct_array(Arc::clone(values.schema()), &encoded).unwrap();
        assert_eq!(decoded, values);
        assert!(matches!(decoded.row(1), Some(None)));
        assert!(matches!(decoded.row(2), Some(Some(_))));
        assert!(decoded.row(3).is_none());
    }

    #[test]
    fn rejects_rows_with_only_some_values() {
        let schema = Arc::new(sample_session());
        let encoded = StructArray::new(
            output_fields(&schema),
            vec![
                Arc::new(BooleanArray::from(vec![Some(true)])),
                Arc::new(Float64Array::from(vec![None])),
            ],
            None,
        );
        assert!(matches!(
            OutputValues::from_struct_array(schema, &encoded),
            Err(OutputDecodeError::PartialRow { row: 0, .. })
        ));
    }

    #[test]
    fn rejects_wrong_column_counts_types_and_struct_nulls() {
        let schema = Arc::new(sample_session());
        let encoded = sample_values().to_struct_array().unwrap();
        let (fields, columns, _) = encoded.into_parts();

        let swapped = StructArray::from(vec![
            (
                Arc::new(arrow_schema::Field::new("failure", DataType::Boolean, true)),
                Arc::clone(&columns[0]),
            ),
            (
                Arc::new(arrow_schema::Field::new("margin", DataType::Boolean, true)),
                Arc::clone(&columns[0]),
            ),
        ]);
        assert!(matches!(
            OutputValues::from_struct_array(Arc::clone(&schema), &swapped),
            Err(OutputDecodeError::ArrowType { .. })
        ));

        let nulls = StructArray::new(fields, columns, Some(vec![true, false, true].into()));
        assert!(matches!(
            OutputValues::from_struct_array(Arc::clone(&schema), &nulls),
            Err(OutputDecodeError::Nulls { count: 1 })
        ));

        let narrow = StructArray::new(
            output_fields(&schema).iter().take(1).cloned().collect(),
            vec![Arc::new(BooleanArray::from(vec![Some(true)])) as ArrayRef],
            None,
        );
        assert!(matches!(
            OutputValues::from_struct_array(schema, &narrow),
            Err(OutputDecodeError::ColumnCount { .. })
        ));
    }

    #[test]
    fn rejects_non_finite_values() {
        let schema = Arc::new(sample_session());
        let encoded = StructArray::new(
            output_fields(&schema),
            vec![
                Arc::new(BooleanArray::from(vec![Some(true)])),
                Arc::new(Float64Array::from(vec![Some(f64::INFINITY)])),
            ],
            None,
        );
        assert!(matches!(
            OutputValues::from_struct_array(schema, &encoded),
            Err(OutputDecodeError::NonFinite { row: 0, .. })
        ));
    }
}
