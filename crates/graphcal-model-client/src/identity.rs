//! Process-unique identities that tie handles to their model and session.

use std::sync::atomic::{AtomicU64, Ordering};

use graphcal_model_protocol::PortName;

use crate::error::ClientError;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Identifies one [`Model`](crate::Model) value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelId(u64);

impl ModelId {
    pub(crate) fn fresh() -> Self {
        Self(next_id())
    }
}

/// Identifies one [`Session`](crate::Session) value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    pub(crate) fn fresh() -> Self {
        Self(next_id())
    }
}

/// One port of one model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortIdentity {
    model: ModelId,
    name: PortName,
}

impl PortIdentity {
    pub(crate) const fn new(model: ModelId, name: PortName) -> Self {
        Self { model, name }
    }

    /// Check that the port belongs to `model`.
    pub(crate) fn ensure_model(&self, model: ModelId) -> Result<(), ClientError> {
        if self.model == model {
            Ok(())
        } else {
            Err(ClientError::ForeignHandle {
                name: self.name.clone(),
            })
        }
    }

    /// The port name.
    #[must_use]
    pub const fn name(&self) -> &PortName {
        &self.name
    }
}
