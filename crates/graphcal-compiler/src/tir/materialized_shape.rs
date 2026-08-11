//! Checked cardinality facts for eagerly materialized indexed values.
//!
//! Individual axes are already bounded by [`IndexCardinality`]. This module
//! enforces the separate invariant that the product of every concrete axis in
//! one materialized value also fits the workspace eager-allocation policy.

use std::num::NonZeroUsize;

use thiserror::Error;

use crate::registry::types::{IndexCardinality, MAX_INDEX_CARDINALITY};
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;

/// Largest number of scalar leaves that one indexed value may materialize.
///
/// This deliberately matches the existing single-axis policy: adding axes may
/// describe the same total number of values, but may not multiply the eager
/// allocation beyond that limit.
pub const MAX_EAGER_CARDINALITY: usize = MAX_INDEX_CARDINALITY;

/// Validated total number of scalar leaves in an eagerly materialized shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EagerCardinality(NonZeroUsize);

impl EagerCardinality {
    fn try_from_axes(axes: &NonEmpty<IndexCardinality>) -> Result<Self, MaterializedShapeError> {
        let total = axes.iter().try_fold(1_usize, |total, axis| {
            total
                .checked_mul(axis.get())
                .filter(|total| *total <= MAX_EAGER_CARDINALITY)
                .ok_or(MaterializedShapeError::ExceedsLimit {
                    maximum: MAX_EAGER_CARDINALITY,
                })
        })?;
        let total = NonZeroUsize::new(total).ok_or(MaterializedShapeError::ExceedsLimit {
            maximum: MAX_EAGER_CARDINALITY,
        })?;
        Ok(Self(total))
    }

    /// Return the validated total cardinality.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }
}

/// Stable identity of one materialized expression within a checked DAG.
///
/// Source spans alone are not identities: instantiated declarations can retain
/// the same definition-site offsets. Pairing the canonical owning declaration
/// with the span keeps repeated instances distinct without string conventions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MaterializedExpressionKey {
    owner: ResolvedDeclName,
    span: Span,
}

impl MaterializedExpressionKey {
    /// Pair a canonical declaration owner with one expression span.
    #[must_use]
    pub const fn new(owner: ResolvedDeclName, span: Span) -> Self {
        Self { owner, span }
    }

    /// Canonical declaration whose body owns the expression.
    #[must_use]
    pub const fn owner(&self) -> &ResolvedDeclName {
        &self.owner
    }

    /// Definition-site expression span within the owner's source.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

/// Concrete non-empty indexed shape whose total cardinality is policy-bounded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MaterializedShape {
    axes: NonEmpty<IndexCardinality>,
    total: EagerCardinality,
}

impl MaterializedShape {
    /// Validate the product of concrete axis cardinalities before evaluation.
    ///
    /// # Errors
    ///
    /// Returns [`MaterializedShapeError::ExceedsLimit`] when the product
    /// overflows or exceeds [`MAX_EAGER_CARDINALITY`].
    pub fn try_new(axes: NonEmpty<IndexCardinality>) -> Result<Self, MaterializedShapeError> {
        let total = EagerCardinality::try_from_axes(&axes)?;
        Ok(Self { axes, total })
    }

    /// Concrete axis cardinalities in outer-to-inner type order.
    #[must_use]
    pub const fn axes(&self) -> &NonEmpty<IndexCardinality> {
        &self.axes
    }

    /// Number of indexed axes.
    #[must_use]
    pub const fn rank(&self) -> usize {
        self.axes.len()
    }

    /// Validated product of all axis cardinalities.
    #[must_use]
    pub const fn total(&self) -> EagerCardinality {
        self.total
    }
}

/// Failure to validate an eager materialized shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MaterializedShapeError {
    /// The axis product overflowed or crossed the workspace policy.
    #[error("materialized shape exceeds the eager limit of {maximum} scalar values")]
    ExceedsLimit { maximum: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cardinality(value: u64) -> IndexCardinality {
        IndexCardinality::try_from_u64(value).unwrap()
    }

    #[test]
    fn total_shape_limit_applies_to_the_axis_product() {
        let boundary =
            MaterializedShape::try_new(NonEmpty::new(cardinality(1_000), vec![cardinality(1_000)]))
                .unwrap();
        assert_eq!(boundary.total().get(), MAX_EAGER_CARDINALITY);

        assert!(matches!(
            MaterializedShape::try_new(NonEmpty::new(
                cardinality(1_000_000),
                vec![cardinality(1_000_000)],
            )),
            Err(MaterializedShapeError::ExceedsLimit {
                maximum: MAX_EAGER_CARDINALITY
            })
        ));
    }
}
