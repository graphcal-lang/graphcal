//! Request and result batches validated against a session schema.

use std::sync::Arc;

use thiserror::Error;

use crate::finite::FiniteF64;
use crate::label::PortName;
use crate::outcome::{RowFailure, RowOutcome};
use crate::session::{InputSlot, OutputSlot, SessionSchema};
use crate::value::{ComplexValue, Value, ValueRef};
use crate::wire_type::{KeyPosition, WireType};

/// The values of one varied parameter across a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum InputColumn {
    /// Quantity values in the parameter's wire unit.
    Quantity(Vec<FiniteF64>),
    /// Integer values.
    Int(Vec<i64>),
    /// Boolean values.
    Bool(Vec<bool>),
    /// Entry positions within the parameter's key type.
    Key(Vec<KeyPosition>),
    /// Complex values in the parameter's wire unit.
    Complex(Vec<ComplexValue>),
    /// Seconds since J2000 in the parameter's time scale.
    Datetime(Vec<FiniteF64>),
    /// Indexed or algebraic values, each conforming to the parameter's wire
    /// type.
    Structured(Vec<Value>),
}

impl InputColumn {
    /// The number of rows.
    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::Quantity(values) | Self::Datetime(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::Key(values) => values.len(),
            Self::Complex(values) => values.len(),
            Self::Structured(values) => values.len(),
        }
    }

    /// Whether the column has no rows.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn value<'a>(&'a self, wire: &'a WireType, row: usize) -> Option<ValueRef<'a>> {
        Some(match (self, wire) {
            (Self::Quantity(values), WireType::Quantity(_)) => {
                ValueRef::Quantity(*values.get(row)?)
            }
            (Self::Int(values), WireType::Int) => ValueRef::Int(*values.get(row)?),
            (Self::Bool(values), WireType::Bool) => ValueRef::Bool(*values.get(row)?),
            (Self::Key(values), WireType::Key(key)) => ValueRef::Key(key.key(*values.get(row)?)?),
            (Self::Complex(values), WireType::Complex(_)) => ValueRef::Complex(*values.get(row)?),
            (Self::Datetime(values), WireType::Datetime(_)) => {
                ValueRef::Datetime(*values.get(row)?)
            }
            (Self::Structured(values), WireType::Indexed(_) | WireType::Algebraic(_)) => {
                values.get(row)?.as_value_ref()
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
    #[error("parameter `{input}` has an out-of-range key at row {row}")]
    KeyOutOfRange { input: PortName, row: usize },
    #[error("parameter `{input}` has a value of another shape at row {row}")]
    ShapeMismatch { input: PortName, row: usize },
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
    /// session's inputs, differ in length, are empty, or name a key
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
                | (WireType::Complex(_), InputColumn::Complex(_))
                | (WireType::Datetime(_), InputColumn::Datetime(_))
                | (WireType::Int, InputColumn::Int(_))
                | (WireType::Bool, InputColumn::Bool(_)) => {}
                (
                    wire @ (WireType::Indexed(_) | WireType::Algebraic(_)),
                    InputColumn::Structured(values),
                ) => {
                    if let Some(row) = values.iter().position(|value| !value.conforms_to(wire)) {
                        return Err(RequestBatchError::ShapeMismatch {
                            input: input_name(),
                            row,
                        });
                    }
                }
                (WireType::Key(key), InputColumn::Key(positions)) => {
                    if let Some(row) = positions
                        .iter()
                        .position(|position| key.key(*position).is_none())
                    {
                        return Err(RequestBatchError::KeyOutOfRange {
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

/// The values of one read output across a batch of output values; `None`
/// exactly in rows without values.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputColumn {
    /// Quantity values in the output's wire unit.
    Quantity(Vec<Option<FiniteF64>>),
    /// Integer values.
    Int(Vec<Option<i64>>),
    /// Boolean values.
    Bool(Vec<Option<bool>>),
    /// Entry positions within the output's key type.
    Key(Vec<Option<KeyPosition>>),
    /// Complex values in the output's wire unit.
    Complex(Vec<Option<ComplexValue>>),
    /// Seconds since J2000 in the output's time scale.
    Datetime(Vec<Option<FiniteF64>>),
    /// Indexed or algebraic values.
    Structured(Vec<Option<Value>>),
}

impl OutputColumn {
    const fn empty(wire: &WireType) -> Self {
        match wire {
            WireType::Quantity(_) => Self::Quantity(Vec::new()),
            WireType::Complex(_) => Self::Complex(Vec::new()),
            WireType::Int => Self::Int(Vec::new()),
            WireType::Bool => Self::Bool(Vec::new()),
            WireType::Key(_) => Self::Key(Vec::new()),
            WireType::Datetime(_) => Self::Datetime(Vec::new()),
            WireType::Indexed(_) | WireType::Algebraic(_) => Self::Structured(Vec::new()),
        }
    }

    fn push_none(&mut self) {
        match self {
            Self::Quantity(values) | Self::Datetime(values) => values.push(None),
            Self::Int(values) => values.push(None),
            Self::Bool(values) => values.push(None),
            Self::Key(values) => values.push(None),
            Self::Complex(values) => values.push(None),
            Self::Structured(values) => values.push(None),
        }
    }

    /// Append `value`, which has already been checked against `wire`.
    fn push(&mut self, wire: &WireType, value: &Value) -> bool {
        match (self, value, wire) {
            (Self::Quantity(values), Value::Quantity(value), _)
            | (Self::Datetime(values), Value::Datetime(value), _) => values.push(Some(*value)),
            (Self::Int(values), Value::Int(value), _) => values.push(Some(*value)),
            (Self::Bool(values), Value::Bool(value), _) => values.push(Some(*value)),
            (Self::Key(values), Value::Key(value), WireType::Key(key)) => {
                let Some(position) = key.position(value.as_key_ref()) else {
                    return false;
                };
                values.push(Some(position));
            }
            (Self::Complex(values), Value::Complex(value), _) => values.push(Some(*value)),
            (
                Self::Structured(values),
                value @ (Value::Indexed(_) | Value::Algebraic { .. }),
                _,
            ) => {
                values.push(Some(value.clone()));
            }
            _ => return false,
        }
        true
    }

    fn value<'a>(&'a self, wire: &'a WireType, row: usize) -> Option<ValueRef<'a>> {
        Some(match (self, wire) {
            (Self::Quantity(values), WireType::Quantity(_)) => {
                ValueRef::Quantity((*values.get(row)?)?)
            }
            (Self::Int(values), WireType::Int) => ValueRef::Int((*values.get(row)?)?),
            (Self::Bool(values), WireType::Bool) => ValueRef::Bool((*values.get(row)?)?),
            (Self::Key(values), WireType::Key(key)) => {
                ValueRef::Key(key.key((*values.get(row)?)?)?)
            }
            (Self::Complex(values), WireType::Complex(_)) => {
                ValueRef::Complex((*values.get(row)?)?)
            }
            (Self::Datetime(values), WireType::Datetime(_)) => {
                ValueRef::Datetime((*values.get(row)?)?)
            }
            (Self::Structured(values), WireType::Indexed(_) | WireType::Algebraic(_)) => {
                values.get(row)?.as_ref()?.as_value_ref()
            }
            _ => return None,
        })
    }
}

/// Why rows cannot form output values for a session.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutputValuesError {
    #[error("the session reads {expected} outputs, but {found} values were given")]
    ValueCount { expected: usize, found: usize },
    #[error("the value of output `{output}` does not have its wire type")]
    KindMismatch { output: PortName },
    #[error("output values must contain at least one row")]
    Empty,
}

/// Accumulates rows into [`OutputValues`].
#[derive(Debug, Clone)]
pub struct OutputValuesBuilder {
    schema: Arc<SessionSchema>,
    present: Vec<bool>,
    columns: Vec<OutputColumn>,
}

impl OutputValuesBuilder {
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
            present: Vec::new(),
            columns,
        }
    }

    /// The session schema the values are built for.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }

    /// Append one row: one value per read output in session order, or `None`
    /// for a row without values.
    ///
    /// # Errors
    ///
    /// Returns [`OutputValuesError`] when a row has the wrong number of values
    /// or a value does not have its output's wire type. The builder is
    /// unchanged on error.
    pub fn push(&mut self, values: Option<Vec<Value>>) -> Result<(), OutputValuesError> {
        let Some(values) = values else {
            for column in &mut self.columns {
                column.push_none();
            }
            self.present.push(false);
            return Ok(());
        };
        let outputs = self.schema.outputs();
        if values.len() != outputs.len() {
            return Err(OutputValuesError::ValueCount {
                expected: outputs.len(),
                found: values.len(),
            });
        }
        if let Some(output) = outputs
            .iter()
            .zip(&values)
            .find_map(|(output, value)| (!value.conforms_to(output.wire())).then_some(output))
        {
            return Err(OutputValuesError::KindMismatch {
                output: output.name().clone(),
            });
        }
        for ((column, output), value) in self.columns.iter_mut().zip(outputs).zip(&values) {
            // Conformance was checked above, so every push succeeds.
            let _pushed = column.push(output.wire(), value);
        }
        self.present.push(true);
        Ok(())
    }

    /// The finished values.
    ///
    /// # Errors
    ///
    /// Returns [`OutputValuesError::Empty`] when no row was pushed.
    pub fn finish(self) -> Result<OutputValues, OutputValuesError> {
        if self.present.is_empty() {
            return Err(OutputValuesError::Empty);
        }
        Ok(OutputValues {
            schema: self.schema,
            present: self.present,
            columns: self.columns,
        })
    }
}

/// The values of a session's read outputs across a non-empty batch of rows.
///
/// A row either has a value for every read output or has none; why a row has
/// no values (a failed evaluation, for instance) is recorded by the batch
/// that holds the values, not here.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputValues {
    schema: Arc<SessionSchema>,
    present: Vec<bool>,
    columns: Vec<OutputColumn>,
}

impl OutputValues {
    /// The session schema the values belong to.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }

    /// The number of rows (at least one).
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.present.len()
    }

    /// The values of `row`: `None` when `row` is out of range, and
    /// `Some(None)` when the row has no values.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<Option<RowValues<'_>>> {
        self.present
            .get(row)
            .map(|present| present.then_some(RowValues { values: self, row }))
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

/// Accumulates row outcomes into a [`ResultBatch`].
#[derive(Debug, Clone)]
pub struct ResultBatchBuilder {
    values: OutputValuesBuilder,
    failures: Vec<Option<RowFailure>>,
}

impl ResultBatchBuilder {
    /// An empty builder for `schema`'s outputs.
    #[must_use]
    pub fn new(schema: Arc<SessionSchema>) -> Self {
        Self {
            values: OutputValuesBuilder::new(schema),
            failures: Vec::new(),
        }
    }

    /// The session schema the batch is built for.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        self.values.schema()
    }

    /// Append one row outcome; a success lists one value per read output in
    /// session order.
    ///
    /// # Errors
    ///
    /// Returns [`OutputValuesError`] when a success has the wrong number of
    /// values or a value does not have its output's wire type. The builder
    /// is unchanged on error.
    pub fn push(&mut self, outcome: RowOutcome<Vec<Value>>) -> Result<(), OutputValuesError> {
        match outcome {
            RowOutcome::Success(values) => {
                self.values.push(Some(values))?;
                self.failures.push(None);
            }
            RowOutcome::Failure(failure) => {
                self.values.push(None)?;
                self.failures.push(Some(failure));
            }
        }
        Ok(())
    }

    /// The finished batch.
    ///
    /// # Errors
    ///
    /// Returns [`OutputValuesError::Empty`] when no row was pushed.
    pub fn finish(self) -> Result<ResultBatch, OutputValuesError> {
        Ok(ResultBatch {
            values: self.values.finish()?,
            failures: self.failures,
        })
    }
}

/// One result: a row outcome for every request row, in request order.
///
/// A row has output values exactly when it succeeded.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultBatch {
    values: OutputValues,
    failures: Vec<Option<RowFailure>>,
}

impl ResultBatch {
    /// A batch of `values` whose rows failed as `failures` lists, or `None`
    /// when the two disagree in length or in which rows have values.
    #[cfg(feature = "arrow")]
    pub(crate) fn from_parts(
        values: OutputValues,
        failures: Vec<Option<RowFailure>>,
    ) -> Option<Self> {
        let consistent = values.present.len() == failures.len()
            && values
                .present
                .iter()
                .zip(&failures)
                .all(|(present, failure)| *present == failure.is_none());
        consistent.then_some(Self { values, failures })
    }

    /// The session schema the batch belongs to.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        self.values.schema()
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
            RowOutcome::Success(RowValues {
                values: &self.values,
                row,
            }),
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

    /// The output values, present exactly in successful rows.
    #[must_use]
    pub const fn values(&self) -> &OutputValues {
        &self.values
    }
}

/// The output values of one row that has them.
#[derive(Debug, Clone, Copy)]
pub struct RowValues<'a> {
    values: &'a OutputValues,
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
        let output = self.values.schema.output(slot)?;
        self.values
            .columns
            .get(slot.get())?
            .value(output.wire(), self.row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axis::{KeyRef, KeyValue};
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
            InputColumn::Key(vec![KeyPosition::new(1), KeyPosition::new(0)]),
        ])
        .unwrap();
        assert_eq!(batch.rows(), 2);
        let mode = batch.schema().input_slot("mode").unwrap();
        assert!(matches!(
            batch.value(mode, 0),
            Some(ValueRef::Key(KeyRef::Variant(variant))) if variant.as_str() == "B"
        ));

        assert!(matches!(
            request(vec![InputColumn::Quantity(vec![finite(1.0)])]),
            Err(RequestBatchError::ColumnCount { .. })
        ));
        assert!(matches!(
            request(vec![
                InputColumn::Int(vec![1]),
                InputColumn::Key(vec![KeyPosition::new(0)]),
            ]),
            Err(RequestBatchError::KindMismatch { .. })
        ));
        assert!(matches!(
            request(vec![
                InputColumn::Quantity(vec![finite(1.0)]),
                InputColumn::Key(vec![KeyPosition::new(0), KeyPosition::new(0)]),
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
                InputColumn::Key(vec![KeyPosition::new(2)]),
            ]),
            Err(RequestBatchError::KeyOutOfRange { row: 0, .. })
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
            Err(OutputValuesError::ValueCount { .. })
        ));
        assert!(matches!(
            builder.push(RowOutcome::Success(vec![
                Value::Bool(true),
                Value::Key(KeyValue::Variant(VariantName::new("A").unwrap())),
            ])),
            Err(OutputValuesError::KindMismatch { .. })
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
            batch.values().column(margin),
            Some(&OutputColumn::Quantity(vec![Some(finite(3.0)), None]))
        );
        assert!(
            ResultBatchBuilder::new(schema).finish().is_err(),
            "empty results are rejected"
        );
    }
}
