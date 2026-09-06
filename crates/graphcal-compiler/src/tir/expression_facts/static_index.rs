//! Retained static membership checks, independent of expression result shape.

use crate::expression_id::ExprId;
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::index::IndexCardinality;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticIndexUse {
    Key,
    Selection,
}

impl std::fmt::Display for StaticIndexUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Key => "key() position",
            Self::Selection => "index",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticIndexRequirement {
    pub operand: ExprId,
    pub axis: IndexTypeRef,
    pub position: u64,
    pub usage: StaticIndexUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{usage} {position} out of bounds for Fin({size})")]
pub struct StaticIndexError {
    pub usage: StaticIndexUse,
    pub position: u64,
    pub size: usize,
}

impl StaticIndexRequirement {
    pub(crate) fn check(
        &self,
        size: Option<IndexCardinality>,
    ) -> Result<Readiness, StaticIndexError> {
        match size {
            None => Ok(Readiness::Deferred),
            Some(size) if u128::from(self.position) < size.get() as u128 => Ok(Readiness::Ready),
            Some(size) => Err(StaticIndexError {
                usage: self.usage,
                position: self.position,
                size: size.get(),
            }),
        }
    }
}
