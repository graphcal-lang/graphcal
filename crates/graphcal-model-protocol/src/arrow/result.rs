//! Encoding and decoding result batches.

use std::sync::Arc;

use arrow_array::{Array, ArrayRef, RecordBatch, StringArray, StructArray, UInt8Array};
use arrow_schema::ArrowError;
use thiserror::Error;

use super::output::OutputDecodeError;
use super::schema::{outcome_fields, result_schema};
use crate::batch::{OutputValues, ResultBatch};
use crate::label::{LabelError, PortName};
use crate::outcome::{RowFailure, RowStatus, UnknownRowStatus};
use crate::session::SessionSchema;

/// Why a received record batch is not a valid result.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ResultDecodeError {
    #[error("the result does not have the `outputs` and `outcome` struct columns")]
    Layout,
    #[error("row {row} succeeded but has no output values")]
    MissingValues { row: usize },
    #[error("row {row} has status {status:?} but has output values")]
    UnexpectedValues { row: usize, status: RowStatus },
    #[error("row {row}: {0}", row = .1)]
    Status(UnknownRowStatus, usize),
    #[error("failed row {row} lacks its {part}")]
    MissingFailurePart { row: usize, part: &'static str },
    #[error("row {row} names an invalid parameter: {source}")]
    Subject { row: usize, source: LabelError },
    #[error(transparent)]
    Outputs(#[from] OutputDecodeError),
}

impl ResultBatch {
    /// Encode the batch in its session's result schema.
    ///
    /// # Errors
    ///
    /// Returns an [`ArrowError`] only if Arrow rejects arrays built from the
    /// schema's own field types, which indicates a bug.
    pub fn to_record_batch(&self) -> Result<RecordBatch, ArrowError> {
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
            Arc::new(self.values().to_struct_array()?),
            Arc::new(StructArray::try_new(outcome_fields(), outcome, None)?),
        ];
        RecordBatch::try_new(result_schema(self.schema()), columns)
    }

    /// Decode a received record batch as a result of `schema`.
    ///
    /// The stream schema must already have been validated against
    /// [`result_schema`].
    ///
    /// # Errors
    ///
    /// Returns [`ResultDecodeError`] for a malformed layout or status, output
    /// values present in a failed row or missing in a successful one, or a
    /// failure without its subject or message.
    pub fn from_record_batch(
        schema: Arc<SessionSchema>,
        batch: &RecordBatch,
    ) -> Result<Self, ResultDecodeError> {
        let structs = batch
            .columns()
            .iter()
            .map(|column| column.as_any().downcast_ref::<StructArray>())
            .collect::<Option<Vec<_>>>()
            .ok_or(ResultDecodeError::Layout)?;
        let [outputs, outcome] = structs.as_slice() else {
            return Err(ResultDecodeError::Layout);
        };
        if outcome.num_columns() != 3 {
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
        let values = OutputValues::from_struct_array(schema, outputs)?;

        let failures = (0..batch.num_rows())
            .map(|row| {
                let status = RowStatus::try_from(statuses.value(row))
                    .map_err(|unknown| ResultDecodeError::Status(unknown, row))?;
                let has_values = matches!(values.row(row), Some(Some(_)));
                match (status, has_values) {
                    (RowStatus::Success, true) => return Ok(None),
                    (RowStatus::Success, false) => {
                        return Err(ResultDecodeError::MissingValues { row });
                    }
                    (_, true) => return Err(ResultDecodeError::UnexpectedValues { row, status }),
                    (_, false) => {}
                }
                let part = |array: &StringArray, part| {
                    (!array.is_null(row))
                        .then(|| array.value(row).to_owned())
                        .ok_or(ResultDecodeError::MissingFailurePart { row, part })
                };
                let message = part(messages, "message")?;
                Ok(Some(match status {
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
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Each row's status was matched against its values above.
        Self::from_parts(values, failures).ok_or(ResultDecodeError::Layout)
    }
}

#[cfg(test)]
mod tests {
    use arrow_array::{BooleanArray, Float64Array};

    use super::*;
    use crate::arrow::schema::output_fields;
    use crate::batch::ResultBatchBuilder;
    use crate::finite::FiniteF64;
    use crate::outcome::RowOutcome;
    use crate::session::tests::sample_session;
    use crate::value::Value;

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
            Err(ResultDecodeError::UnexpectedValues { row: 0, .. })
        ));
    }
}
