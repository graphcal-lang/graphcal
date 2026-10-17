//! Typed access to the result of one request.

use graphcal_model_protocol::{OutputSlot, ResultBatch, RowFailure, RowOutcome, RowValues};

use crate::error::ClientError;
use crate::identity::ModelId;
use crate::kind::Kind;
use crate::model::OutputHandle;

/// The result of one request: a row outcome per request row, in order.
#[derive(Debug, Clone)]
pub struct Evaluation {
    model: ModelId,
    batch: ResultBatch,
}

impl Evaluation {
    pub(crate) const fn new(model: ModelId, batch: ResultBatch) -> Self {
        Self { model, batch }
    }

    /// The number of rows.
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.batch.rows()
    }

    /// The validated protocol batch.
    #[must_use]
    pub const fn batch(&self) -> &ResultBatch {
        &self.batch
    }

    /// The outcome of `row`, when it is in range.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<RowOutcome<Row<'_>>> {
        self.batch.row(row).map(|outcome| self.wrap(outcome))
    }

    /// Every row outcome in order.
    pub fn outcomes(&self) -> impl ExactSizeIterator<Item = RowOutcome<Row<'_>>> {
        self.batch.outcomes().map(|outcome| self.wrap(outcome))
    }

    /// The failure of `row`, or `None` when it succeeded or is out of range.
    #[must_use]
    pub fn failure(&self, row: usize) -> Option<&RowFailure> {
        self.batch.failure(row)
    }

    /// The values of `output` in every row; `None` exactly in failed rows.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] for a handle of another model or an output
    /// the session does not read.
    pub fn column<K: Kind>(
        &self,
        output: &OutputHandle<K>,
    ) -> Result<Vec<Option<K::Output>>, ClientError> {
        let slot = self.slot(output)?;
        Ok(self
            .batch
            .outcomes()
            .map(|outcome| match outcome {
                RowOutcome::Success(values) => values.get(slot).and_then(K::output),
                RowOutcome::Failure(_) => None,
            })
            .collect())
    }

    fn slot<K: Kind>(&self, output: &OutputHandle<K>) -> Result<OutputSlot, ClientError> {
        output.port().ensure_model(self.model)?;
        self.batch
            .schema()
            .output_slot(output.name().as_str())
            .ok_or_else(|| ClientError::NotRead {
                name: output.name().clone(),
            })
    }

    fn wrap<'a>(&'a self, outcome: RowOutcome<RowValues<'a>>) -> RowOutcome<Row<'a>> {
        match outcome {
            RowOutcome::Success(values) => RowOutcome::Success(Row {
                evaluation: self,
                values,
            }),
            RowOutcome::Failure(failure) => RowOutcome::Failure(failure),
        }
    }
}

/// The outputs of one successful row.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    evaluation: &'a Evaluation,
    values: RowValues<'a>,
}

impl Row<'_> {
    /// The row index within its request.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.values.row()
    }

    /// The value of `output` in this row.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] for a handle of another model or an output
    /// the session does not read.
    pub fn get<K: Kind>(&self, output: &OutputHandle<K>) -> Result<K::Output, ClientError> {
        let slot = self.evaluation.slot(output)?;
        // A successful row has a value of the output's kind in every read
        // slot; the handle's kind was checked against the same wire type.
        self.values
            .get(slot)
            .and_then(K::output)
            .ok_or_else(|| ClientError::KindMismatch {
                name: output.name().clone(),
                expected: K::NAME,
            })
    }
}
