//! Encoding and decoding result batches.

use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, StructArray,
    UInt8Array,
};
use arrow_schema::ArrowError;
use thiserror::Error;

use super::schema::{outcome_fields, output_fields, result_schema};
use super::value_array::{ValueDecodeError, decode, encode, key_array, with_placeholders};
use crate::batch::{OutputColumn, ResultBatch, ResultBatchBuilder, ResultBatchError};
use crate::finite::FiniteF64;
use crate::label::{LabelError, PortName};
use crate::outcome::{RowFailure, RowOutcome, RowStatus, UnknownRowStatus};
use crate::session::SessionSchema;
use crate::value::Value;
use crate::wire_type::WireType;

/// Why a received record batch is not a valid result.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ResultDecodeError {
    #[error("the result does not have the `outputs` and `outcome` struct columns")]
    Layout,
    #[error("the column for output `{output}` does not have its Arrow type")]
    ArrowType { output: PortName },
    #[error("row {row} has status {status:?} but output `{output}` is {state}")]
    OutputPresence {
        row: usize,
        status: RowStatus,
        output: PortName,
        state: &'static str,
    },
    #[error("output `{output}` has a non-finite value at row {row}")]
    NonFinite { output: PortName, row: usize },
    #[error("row {row}: {0}", row = .1)]
    Status(UnknownRowStatus, usize),
    #[error("failed row {row} lacks its {part}")]
    MissingFailurePart { row: usize, part: &'static str },
    #[error("row {row} names an invalid parameter: {source}")]
    Subject { row: usize, source: LabelError },
    #[error("the column for output `{output}` {source}")]
    Value {
        output: PortName,
        source: ValueDecodeError,
    },
    #[error(transparent)]
    Batch(#[from] ResultBatchError),
}

impl ResultBatch {
    /// Encode the batch in its session's result schema.
    ///
    /// # Errors
    ///
    /// Returns an [`ArrowError`] only if Arrow rejects arrays built from the
    /// schema's own field types, which indicates a bug.
    pub fn to_record_batch(&self) -> Result<RecordBatch, ArrowError> {
        let schema = self.schema();
        let output_arrays = schema
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

        let mut statuses = Vec::with_capacity(self.rows());
        let mut subjects = Vec::with_capacity(self.rows());
        let mut messages = Vec::with_capacity(self.rows());
        for row in 0..self.rows() {
            match self.failure(row) {
                None => {
                    statuses.push(RowStatus::Success.code());
                    subjects.push(None);
                    messages.push(None);
                }
                Some(failure) => {
                    statuses.push(failure.status().code());
                    subjects.push(match failure {
                        RowFailure::AssertionFailed { assertion, .. } => Some(assertion.clone()),
                        RowFailure::InputOutOfDomain { parameter, .. } => {
                            Some(parameter.as_str().to_owned())
                        }
                        RowFailure::EvaluationError { .. } => None,
                    });
                    messages.push(Some(failure.message().to_owned()));
                }
            }
        }
        let outcome: Vec<ArrayRef> = vec![
            Arc::new(UInt8Array::from(statuses)),
            Arc::new(StringArray::from(subjects)),
            Arc::new(StringArray::from(messages)),
        ];
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StructArray::try_new(
                output_fields(schema),
                output_arrays,
                None,
            )?),
            Arc::new(StructArray::try_new(outcome_fields(), outcome, None)?),
        ];
        RecordBatch::try_new(result_schema(schema), columns)
    }

    /// Decode a received record batch as a result of `schema`.
    ///
    /// The stream schema must already have been validated against
    /// [`result_schema`].
    ///
    /// # Errors
    ///
    /// Returns [`ResultDecodeError`] for a malformed layout or status, an
    /// output value present in a failed row or missing in a successful one,
    /// or a failure without its subject or message.
    pub fn from_record_batch(
        schema: Arc<SessionSchema>,
        batch: &RecordBatch,
    ) -> Result<Self, ResultDecodeError> {
        let mut builder = ResultBatchBuilder::new(schema);
        let schema = Arc::clone(builder.schema());
        let structs = batch
            .columns()
            .iter()
            .map(|column| column.as_any().downcast_ref::<StructArray>())
            .collect::<Option<Vec<_>>>()
            .ok_or(ResultDecodeError::Layout)?;
        let [outputs, outcome] = structs.as_slice() else {
            return Err(ResultDecodeError::Layout);
        };
        if outputs.num_columns() != schema.outputs().len() || outcome.num_columns() != 3 {
            return Err(ResultDecodeError::Layout);
        }
        let statuses = outcome
            .column(0)
            .as_any()
            .downcast_ref::<UInt8Array>()
            .ok_or(ResultDecodeError::Layout)?;
        let subjects = outcome
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or(ResultDecodeError::Layout)?;
        let messages = outcome
            .column(2)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or(ResultDecodeError::Layout)?;
        let columns = schema
            .outputs()
            .iter()
            .zip(outputs.columns())
            .map(|(output, array)| decode_output(output.name(), output.wire(), array.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;

        for row in 0..batch.num_rows() {
            let status = RowStatus::try_from(statuses.value(row))
                .map_err(|unknown| ResultDecodeError::Status(unknown, row))?;
            let presence = |state| {
                move |output: &PortName| ResultDecodeError::OutputPresence {
                    row,
                    status,
                    output: output.clone(),
                    state,
                }
            };
            if status == RowStatus::Success {
                let values = schema
                    .outputs()
                    .iter()
                    .zip(&columns)
                    .map(|(output, column)| {
                        column[row]
                            .clone()
                            .ok_or_else(|| presence("null")(output.name()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                builder.push(RowOutcome::Success(values))?;
            } else {
                if let Some(output) = schema
                    .outputs()
                    .iter()
                    .zip(&columns)
                    .find_map(|(output, column)| column[row].is_some().then_some(output))
                {
                    return Err(presence("present")(output.name()));
                }
                let part = |array: &StringArray, part| {
                    (!array.is_null(row))
                        .then(|| array.value(row).to_owned())
                        .ok_or(ResultDecodeError::MissingFailurePart { row, part })
                };
                let message = part(messages, "message")?;
                let failure = match status {
                    RowStatus::AssertionFailed => RowFailure::AssertionFailed {
                        assertion: part(subjects, "subject")?,
                        message,
                    },
                    RowStatus::InputOutOfDomain => RowFailure::InputOutOfDomain {
                        parameter: PortName::new(part(subjects, "subject")?)
                            .map_err(|source| ResultDecodeError::Subject { row, source })?,
                        message,
                    },
                    RowStatus::EvaluationError | RowStatus::Success => {
                        RowFailure::EvaluationError { message }
                    }
                };
                builder.push(RowOutcome::Failure(failure))?;
            }
        }
        Ok(builder.finish()?)
    }
}

/// Decode one output column into per-row optional values.
fn decode_output(
    output: &PortName,
    wire: &WireType,
    array: &dyn Array,
) -> Result<Vec<Option<Value>>, ResultDecodeError> {
    let arrow_type = || ResultDecodeError::ArrowType {
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
                            ResultDecodeError::NonFinite {
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
            decode(wire, array).map_err(|source| ResultDecodeError::Value {
                output: output.clone(),
                source,
            })?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::sample_session;

    #[test]
    fn results_round_trip_with_every_failure_kind() {
        let schema = Arc::new(sample_session());
        let mut builder = ResultBatchBuilder::new(Arc::clone(&schema));
        builder
            .push(RowOutcome::Success(vec![
                Value::Bool(false),
                Value::Quantity(FiniteF64::new(2.0).unwrap()),
            ]))
            .unwrap();
        for failure in [
            RowFailure::AssertionFailed {
                assertion: "inner::limit".to_owned(),
                message: "limit exceeded".to_owned(),
            },
            RowFailure::InputOutOfDomain {
                parameter: PortName::new("load").unwrap(),
                message: "above 10 m".to_owned(),
            },
            RowFailure::EvaluationError {
                message: "division by zero".to_owned(),
            },
        ] {
            builder.push(RowOutcome::Failure(failure)).unwrap();
        }
        let batch = builder.finish().unwrap();
        let encoded = batch.to_record_batch().unwrap();
        assert_eq!(encoded.num_rows(), 4);
        let decoded = ResultBatch::from_record_batch(schema, &encoded).unwrap();
        assert_eq!(decoded, batch);
    }

    #[test]
    fn rejects_values_in_failed_rows() {
        let schema = Arc::new(sample_session());
        let outputs: Vec<ArrayRef> = vec![
            Arc::new(BooleanArray::from(vec![Some(true)])),
            Arc::new(Float64Array::from(vec![Some(1.0)])),
        ];
        let outcome: Vec<ArrayRef> = vec![
            Arc::new(UInt8Array::from(vec![RowStatus::EvaluationError.code()])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![Some("boom")])),
        ];
        let encoded = RecordBatch::try_new(
            result_schema(&schema),
            vec![
                Arc::new(StructArray::new(output_fields(&schema), outputs, None)),
                Arc::new(StructArray::new(outcome_fields(), outcome, None)),
            ],
        )
        .unwrap();
        assert!(matches!(
            ResultBatch::from_record_batch(schema, &encoded),
            Err(ResultDecodeError::OutputPresence { row: 0, .. })
        ));
    }
}
