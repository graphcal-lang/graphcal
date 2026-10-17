//! Building typed requests for a session.

use std::sync::Arc;

use graphcal_model_protocol::{InputColumn, RequestBatch, SessionSchema};

use crate::error::ClientError;
use crate::identity::{ModelId, SessionId};
use crate::kind::Kind;
use crate::model::InputHandle;

/// Collects one column per varied parameter of a session.
#[derive(Debug)]
#[must_use]
pub struct BatchBuilder<'s> {
    session: SessionId,
    model: ModelId,
    schema: &'s Arc<SessionSchema>,
    columns: Vec<Option<InputColumn>>,
}

impl<'s> BatchBuilder<'s> {
    pub(crate) fn new(session: SessionId, model: ModelId, schema: &'s Arc<SessionSchema>) -> Self {
        Self {
            session,
            model,
            schema,
            columns: vec![None; schema.inputs().len()],
        }
    }

    /// Supply the values of `input` for every row.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] for a handle of another model, a parameter
    /// the session does not vary or already has a column for, a non-finite
    /// quantity, or a key variant of another parameter.
    pub fn column<K: Kind>(
        mut self,
        input: &InputHandle<K>,
        values: impl IntoIterator<Item = K::Input>,
    ) -> Result<Self, ClientError> {
        input.port().ensure_model(self.model)?;
        let slot = self
            .schema
            .input_slot(input.name().as_str())
            .ok_or_else(|| ClientError::NotVaried {
                name: input.name().clone(),
            })?;
        let column = &mut self.columns[slot.get()];
        if column.is_some() {
            return Err(ClientError::DuplicateColumn {
                name: input.name().clone(),
            });
        }
        *column = Some(K::input_column(
            input.port(),
            input.detail(),
            values.into_iter().collect(),
        )?);
        Ok(self)
    }

    /// The finished request.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when a varied parameter has no column, or the
    /// columns are empty or differ in length.
    pub fn build(self) -> Result<Request, ClientError> {
        let columns = self
            .schema
            .inputs()
            .iter()
            .zip(self.columns)
            .map(|(input, column)| {
                column.ok_or_else(|| ClientError::MissingColumn {
                    name: input.name().clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Request::new(
            self.session,
            RequestBatch::new(Arc::clone(self.schema), columns)?,
        ))
    }
}

/// A validated request of one session.
#[derive(Debug, Clone)]
pub struct Request {
    session: SessionId,
    batch: RequestBatch,
}

impl Request {
    pub(crate) const fn new(session: SessionId, batch: RequestBatch) -> Self {
        Self { session, batch }
    }

    /// The number of rows.
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.batch.rows()
    }

    /// The validated protocol batch.
    #[must_use]
    pub const fn batch(&self) -> &RequestBatch {
        &self.batch
    }

    pub(crate) const fn session(&self) -> SessionId {
        self.session
    }
}
