//! Static membership checks, independent of expression result shape.
//!
//! A static position (a `key()` position or an `Int` index selector) is proven
//! in range of its axis once the axis's cardinality is known; until then it
//! waits for the Static or generic binding that fixes the axis.

use crate::expression_id::ExprId;
use std::borrow::Cow;

use crate::semantic::checked_type::{IndexTypeRef, Symbolic};
use crate::semantic::index_def::{ConcreteIndexKind, FiniteIndex, IndexCardinality};
use thiserror::Error;

/// An axis whose definition is unavailable to the checked program.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("checked expression references an unavailable index: {0}")]
pub struct UnavailableIndex(pub Box<IndexTypeRef<Symbolic>>);

/// The concrete definition of an axis, or `None` while it awaits a Static or
/// generic binding.
pub type AxisDefinition<'a> = dyn Fn(&IndexTypeRef<Symbolic>) -> Result<Option<Cow<'a, ConcreteIndexKind>>, UnavailableIndex>
    + 'a;

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
    pub axis: IndexTypeRef<Symbolic>,
    pub position: u64,
    pub usage: StaticIndexUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{usage} {position} out of bounds for {axis}")]
pub struct StaticIndexError {
    pub usage: StaticIndexUse,
    pub position: u64,
    pub axis: FiniteIndex,
}

impl StaticIndexRequirement {
    pub fn check(&self, size: Option<IndexCardinality>) -> Result<Readiness, StaticIndexError> {
        check_static_position(self.position, self.usage, size)
    }
}

/// Whether a static `position` is in range of an axis of `size`, or awaits it.
pub fn check_static_position(
    position: u64,
    usage: StaticIndexUse,
    size: Option<IndexCardinality>,
) -> Result<Readiness, StaticIndexError> {
    match size {
        None => Ok(Readiness::Deferred),
        Some(size) if u128::from(position) < size.get() as u128 => Ok(Readiness::Ready),
        Some(size) => Err(StaticIndexError {
            usage,
            position,
            axis: FiniteIndex::new(size),
        }),
    }
}
