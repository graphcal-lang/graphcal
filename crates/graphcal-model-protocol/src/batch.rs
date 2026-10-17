//! Request and result batches validated against a session schema.

use std::sync::Arc;

use thiserror::Error;

use crate::finite::FiniteF64;
use crate::label::PortName;
use crate::outcome::{RowFailure, RowOutcome};
use crate::session::{InputSlot, OutputSlot, SessionSchema};
use crate::value::{Value, ValueRef};
use crate::wire_type::{VariantPosition, WireType};

/// The values of one varied parameter across a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum InputColumn {
    /// Quantity values in the parameter's wire unit.
    Quantity(Vec<FiniteF64>),
    /// Integer values.
    Int(Vec<i64>),
    /// Boolean values.
    Bool(Vec<bool>),
    /// Variant positions within the parameter's key type.
    Key(Vec<VariantPosition>),
}

impl InputColumn {
    /// The number of rows.
    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::Quantity(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::Key(values) => values.len(),
        }
    }

    /// Whether the column has no rows.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn value<'a>(&self, wire: &'a WireType, row: usize) -> Option<ValueRef<'a>> {
        Some(match (self, wire) {
            (Self::Quantity(values), WireType::Quantity(_)) => {
                ValueRef::Quantity(*values.get(row)?)
            }
            (Self::Int(values), WireType::Int) => ValueRef::Int(*values.get(row)?),
            (Self::Bool(values), WireType::Bool) => ValueRef::Bool(*values.get(row)?),
            (Self::Key(values), WireType::Key(key)) => {
                ValueRef::Key(key.variant(*values.get(row)?)?)
            }
            _ => return None,
        })
    }
}

/// Why columns do not form a request batch for a session.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RequestBatchError {
    #[error("the session varies {expected} parameters, but {found} columns were given")]
    ColumnCount { expected: usize, found: usize },
    #[error("the column for parameter `{input}` does not have its wire type")]
    KindMismatch { input: PortName },
    #[error("the column for parameter `{input}` has {found} rows; expected {expected}")]
    LengthMismatch {
        input: PortName,
        expected: usize,
        found: usize,
    },
    #[error("a request batch must contain at least one row")]
    Empty,
    #[error("parameter `{input}` has an out-of-range variant at row {row}")]
    VariantOutOfRange { input: PortName, row: usize },
}

/// One request: a non-empty batch of rows, one column per varied parameter.
///
/// Values may lie outside a parameter's declared domain; such rows fail
/// individually when evaluated.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestBatch {
    schema: Arc<SessionSchema>,
    rows: usize,
    columns: Vec<InputColumn>,
}

impl RequestBatch {
    /// A batch with one column per varied parameter of `schema`, in session
    /// order.
    ///
    /// # Errors
    ///
    /// Returns [`RequestBatchError`] when the columns do not match the
    /// session's inputs, differ in length, are empty, or name a variant
    /// outside its key type.
    pub fn new(
        schema: Arc<SessionSchema>,
        columns: Vec<InputColumn>,
    ) -> Result<Self, RequestBatchError> {
        if columns.len() != schema.inputs().len() {
            return Err(RequestBatchError::ColumnCount {
                expected: schema.inputs().len(),
                found: columns.len(),
            });
        }
        let rows = columns.first().map_or(0, InputColumn::len);
        if rows == 0 {
            return Err(RequestBatchError::Empty);
        }
        for (input, column) in schema.inputs().iter().zip(&columns) {
            let input_name = || input.name().clone();
            match (input.wire(), column) {
                (WireType::Quantity(_), InputColumn::Quantity(_))
                | (WireType::Int, InputColumn::Int(_))
                | (WireType::Bool, InputColumn::Bool(_)) => {}
                (WireType::Key(key), InputColumn::Key(positions)) => {
                    if let Some(row) = positions
                        .iter()
                        .position(|position| key.variant(*position).is_none())
                    {
                        return Err(RequestBatchError::VariantOutOfRange {
                            input: input_name(),
                            row,
                        });
                    }
                }
                _ => {
                    return Err(RequestBatchError::KindMismatch {
                        input: input_name(),
                    });
                }
            }
            if column.len() != rows {
                return Err(RequestBatchError::LengthMismatch {
                    input: input_name(),
                    expected: rows,
                    found: column.len(),
                });
            }
        }
        Ok(Self {
            schema,
            rows,
            columns,
        })
    }

    /// The session schema the batch was validated against.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }

    /// The number of rows (at least one).
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Every column in session order.
    #[must_use]
    pub fn columns(&self) -> &[InputColumn] {
        &self.columns
    }

    /// The column of the varied parameter at `slot`.
    #[must_use]
    pub fn column(&self, slot: InputSlot) -> Option<&InputColumn> {
        self.columns.get(slot.get())
    }

    /// The value of the parameter at `slot` in `row`.
    #[must_use]
    pub fn value(&self, slot: InputSlot, row: usize) -> Option<ValueRef<'_>> {
        let input = self.schema.input(slot)?;
        self.columns.get(slot.get())?.value(input.wire(), row)
    }
}

/// The values of one read output across a result batch; `None` exactly in
/// failed rows.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputColumn {
    /// Quantity values in the output's wire unit.
    Quantity(Vec<Option<FiniteF64>>),
    /// Integer values.
    Int(Vec<Option<i64>>),
    /// Boolean values.
    Bool(Vec<Option<bool>>),
    /// Variant positions within the output's key type.
    Key(Vec<Option<VariantPosition>>),
}

impl OutputColumn {
    const fn empty(wire: &WireType) -> Self {
        match wire {
            WireType::Quantity(_) => Self::Quantity(Vec::new()),
            WireType::Int => Self::Int(Vec::new()),
            WireType::Bool => Self::Bool(Vec::new()),
            WireType::Key(_) => Self::Key(Vec::new()),
        }
    }

    fn push_none(&mut self) {
        match self {
            Self::Quantity(values) => values.push(None),
            Self::Int(values) => values.push(None),
            Self::Bool(values) => values.push(None),
            Self::Key(values) => values.push(None),
        }
    }

    /// Append `value`, which has already been checked against `wire`.
    fn push(&mut self, wire: &WireType, value: &Value) -> bool {
        match (self, value, wire) {
            (Self::Quantity(values), Value::Quantity(value), _) => values.push(Some(*value)),
            (Self::Int(values), Value::Int(value), _) => values.push(Some(*value)),
            (Self::Bool(values), Value::Bool(value), _) => values.push(Some(*value)),
            (Self::Key(values), Value::Key(variant), WireType::Key(key)) => {
                let Some(position) = key.position(variant.as_str()) else {
                    return false;
                };
                values.push(Some(position));
            }
            _ => return false,
        }
        true
    }

    fn value<'a>(&self, wire: &'a WireType, row: usize) -> Option<ValueRef<'a>> {
        Some(match (self, wire) {
            (Self::Quantity(values), WireType::Quantity(_)) => {
                ValueRef::Quantity((*values.get(row)?)?)
            }
            (Self::Int(values), WireType::Int) => ValueRef::Int((*values.get(row)?)?),
            (Self::Bool(values), WireType::Bool) => ValueRef::Bool((*values.get(row)?)?),
            (Self::Key(values), WireType::Key(key)) => {
                ValueRef::Key(key.variant((*values.get(row)?)?)?)
            }
            _ => return None,
        })
    }
}

/// Why a row cannot be added to a result batch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResultBatchError {
    #[error("the session reads {expected} outputs, but {found} values were given")]
    ValueCount { expected: usize, found: usize },
    #[error("the value of output `{output}` does not have its wire type")]
    KindMismatch { output: PortName },
    #[error("a result batch must contain at least one row")]
    Empty,
}

/// Accumulates row outcomes into a [`ResultBatch`].
#[derive(Debug, Clone)]
pub struct ResultBatchBuilder {
    schema: Arc<SessionSchema>,
    failures: Vec<Option<RowFailure>>,
    columns: Vec<OutputColumn>,
}

impl ResultBatchBuilder {
    /// An empty builder for `schema`'s outputs.
    #[must_use]
    pub fn new(schema: Arc<SessionSchema>) -> Self {
        let columns = schema
            .outputs()
            .iter()
            .map(|output| OutputColumn::empty(output.wire()))
            .collect();
        Self {
            schema,
            failures: Vec::new(),
            columns,
        }
    }

    /// The session schema the batch is built for.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }

    /// Append one row outcome; a success lists one value per read output in
    /// session order.
    ///
    /// # Errors
    ///
    /// Returns [`ResultBatchError`] when a success has the wrong number of
    /// values or a value does not have its output's wire type. The builder
    /// is unchanged on error.
    pub fn push(&mut self, outcome: RowOutcome<Vec<Value>>) -> Result<(), ResultBatchError> {
        match outcome {
            RowOutcome::Success(values) => {
                let outputs = self.schema.outputs();
                if values.len() != outputs.len() {
                    return Err(ResultBatchError::ValueCount {
                        expected: outputs.len(),
                        found: values.len(),
                    });
                }
                if let Some(output) = outputs.iter().zip(&values).find_map(|(output, value)| {
                    (!value.conforms_to(output.wire())).then_some(output)
                }) {
                    return Err(ResultBatchError::KindMismatch {
                        output: output.name().clone(),
                    });
                }
                for ((column, output), value) in self.columns.iter_mut().zip(outputs).zip(&values) {
                    // Conformance was checked above, so every push succeeds.
                    let _pushed = column.push(output.wire(), value);
                }
                self.failures.push(None);
            }
            RowOutcome::Failure(failure) => {
                for column in &mut self.columns {
                    column.push_none();
                }
                self.failures.push(Some(failure));
            }
        }
        Ok(())
    }

    /// The finished batch.
    ///
    /// # Errors
    ///
    /// Returns [`ResultBatchError::Empty`] when no row was pushed.
    pub fn finish(self) -> Result<ResultBatch, ResultBatchError> {
        if self.failures.is_empty() {
            return Err(ResultBatchError::Empty);
        }
        Ok(ResultBatch {
            schema: self.schema,
            failures: self.failures,
            columns: self.columns,
        })
    }
}

/// One result: a row outcome for every request row, in request order.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultBatch {
    schema: Arc<SessionSchema>,
    failures: Vec<Option<RowFailure>>,
    columns: Vec<OutputColumn>,
}

impl ResultBatch {
    /// The session schema the batch belongs to.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }

    /// The number of rows (at least one).
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.failures.len()
    }

    /// The outcome of `row`, when it is in range.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<RowOutcome<RowValues<'_>>> {
        self.failures
            .get(row)
            .map(|failure| self.outcome(row, failure.as_ref()))
    }

    fn outcome(&self, row: usize, failure: Option<&RowFailure>) -> RowOutcome<RowValues<'_>> {
        failure.map_or(
            RowOutcome::Success(RowValues { batch: self, row }),
            |failure| RowOutcome::Failure(failure.clone()),
        )
    }

    /// Every row outcome in order.
    pub fn outcomes(&self) -> impl ExactSizeIterator<Item = RowOutcome<RowValues<'_>>> {
        self.failures
            .iter()
            .enumerate()
            .map(move |(row, failure)| self.outcome(row, failure.as_ref()))
    }

    /// The failure of `row`, or `None` when it succeeded or is out of range.
    #[must_use]
    pub fn failure(&self, row: usize) -> Option<&RowFailure> {
        self.failures.get(row)?.as_ref()
    }

    /// Every output column in session order.
    #[must_use]
    pub fn columns(&self) -> &[OutputColumn] {
        &self.columns
    }

    /// The column of the read output at `slot`.
    #[must_use]
    pub fn column(&self, slot: OutputSlot) -> Option<&OutputColumn> {
        self.columns.get(slot.get())
    }
}

/// The outputs of one successful row.
#[derive(Debug, Clone, Copy)]
pub struct RowValues<'a> {
    batch: &'a ResultBatch,
    row: usize,
}

impl<'a> RowValues<'a> {
    /// The row index within its batch.
    #[must_use]
    pub const fn row(self) -> usize {
        self.row
    }

    /// The value of the output at `slot`.
    #[must_use]
    pub fn get(self, slot: OutputSlot) -> Option<ValueRef<'a>> {
        let output = self.batch.schema.output(slot)?;
        self.batch
            .columns
            .get(slot.get())?
            .value(output.wire(), self.row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::VariantName;
    use crate::session::tests::sample_session;

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).unwrap()
    }

    fn request(columns: Vec<InputColumn>) -> Result<RequestBatch, RequestBatchError> {
        RequestBatch::new(Arc::new(sample_session()), columns)
    }

    #[test]
    fn request_batches_match_session_inputs() {
        let batch = request(vec![
            InputColumn::Quantity(vec![finite(1.0), finite(2.0)]),
            InputColumn::Key(vec![VariantPosition::new(1), VariantPosition::new(0)]),
        ])
        .unwrap();
        assert_eq!(batch.rows(), 2);
        let mode = batch.schema().input_slot("mode").unwrap();
        assert!(matches!(
            batch.value(mode, 0),
            Some(ValueRef::Key(variant)) if variant.as_str() == "B"
        ));

        assert!(matches!(
            request(vec![InputColumn::Quantity(vec![finite(1.0)])]),
            Err(RequestBatchError::ColumnCount { .. })
        ));
        assert!(matches!(
            request(vec![
                InputColumn::Int(vec![1]),
                InputColumn::Key(vec![VariantPosition::new(0)]),
            ]),
            Err(RequestBatchError::KindMismatch { .. })
        ));
        assert!(matches!(
            request(vec![
                InputColumn::Quantity(vec![finite(1.0)]),
                InputColumn::Key(vec![VariantPosition::new(0), VariantPosition::new(0)]),
            ]),
            Err(RequestBatchError::LengthMismatch { .. })
        ));
        assert!(matches!(
            request(vec![
                InputColumn::Quantity(Vec::new()),
                InputColumn::Key(Vec::new()),
            ]),
            Err(RequestBatchError::Empty)
        ));
        assert!(matches!(
            request(vec![
                InputColumn::Quantity(vec![finite(1.0)]),
                InputColumn::Key(vec![VariantPosition::new(2)]),
            ]),
            Err(RequestBatchError::VariantOutOfRange { row: 0, .. })
        ));
    }

    #[test]
    fn result_values_exist_exactly_in_successful_rows() {
        let schema = Arc::new(sample_session());
        let mut builder = ResultBatchBuilder::new(Arc::clone(&schema));
        builder
            .push(RowOutcome::Success(vec![
                Value::Bool(true),
                Value::Quantity(finite(3.0)),
            ]))
            .unwrap();
        builder
            .push(RowOutcome::Failure(RowFailure::EvaluationError {
                message: "division by zero".to_owned(),
            }))
            .unwrap();
        assert!(matches!(
            builder.push(RowOutcome::Success(vec![Value::Bool(true)])),
            Err(ResultBatchError::ValueCount { .. })
        ));
        assert!(matches!(
            builder.push(RowOutcome::Success(vec![
                Value::Bool(true),
                Value::Key(VariantName::new("A").unwrap()),
            ])),
            Err(ResultBatchError::KindMismatch { .. })
        ));
        let batch = builder.finish().unwrap();
        assert_eq!(batch.rows(), 2);

        let margin = schema.output_slot("margin").unwrap();
        let RowOutcome::Success(values) = batch.row(0).unwrap() else {
            panic!("row 0 succeeded");
        };
        assert_eq!(values.get(margin), Some(ValueRef::Quantity(finite(3.0))));
        assert!(matches!(batch.row(1), Some(RowOutcome::Failure(_))));
        assert_eq!(
            batch.column(margin),
            Some(&OutputColumn::Quantity(vec![Some(finite(3.0)), None]))
        );
        assert!(
            ResultBatchBuilder::new(schema).finish().is_err(),
            "empty results are rejected"
        );
    }
}
