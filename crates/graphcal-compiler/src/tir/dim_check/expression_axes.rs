//! Materialization facts directly from checked types, without rebuilding inferred types.

use std::borrow::Cow;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::semantic::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::semantic::index_def::{ConcreteIndexKind, IndexCardinality};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;
use crate::tir::materialized_shape::MaterializedShapeError;
use crate::tir::static_index::UnavailableIndex;
use crate::tir::typed::program::TirRead;

/// The concrete definition of an axis of a checked type, or `None` while its
/// cardinality or definition awaits a Static or generic binding.
pub(super) fn concrete_index_kind<'t>(
    tir: &'t dyn TirRead,
    index: &IndexTypeRef<Symbolic>,
) -> Result<Option<Cow<'t, ConcreteIndexKind>>, UnavailableIndex> {
    if index
        .finite_index_form()
        .is_some_and(|form| form.constant_value().is_none())
    {
        return Ok(None);
    }
    match tir.index_def(index) {
        Some(Cow::Borrowed(definition)) => Ok(definition.concrete().map(Cow::Borrowed)),
        Some(Cow::Owned(definition)) => Ok(definition.concrete().cloned().map(Cow::Owned)),
        None => Err(UnavailableIndex(Box::new(index.clone()))),
    }
}

fn checked_index_cardinality(
    tir: &dyn TirRead,
    index: &IndexTypeRef<Symbolic>,
) -> Result<Option<IndexCardinality>, UnavailableIndex> {
    Ok(concrete_index_kind(tir, index)?.map(|kind| kind.cardinality()))
}

/// Why a checked type cannot be materialized eagerly.
enum MaterializationError {
    Index(UnavailableIndex),
    Shape(MaterializedShapeError),
}

impl From<MaterializedShapeError> for MaterializationError {
    fn from(error: MaterializedShapeError) -> Self {
        Self::Shape(error)
    }
}

/// Require an indexed value whose axes are all known to fit the eager
/// allocation policy. Axes still awaiting a binding are checked once bound.
pub(super) fn check_materializable(
    ty: &CheckedType<Symbolic>,
    tir: &dyn TirRead,
    src: SourceId,
    span: Span,
) -> Result<(), SemanticError> {
    ty.materialized_shape(|axis| {
        checked_index_cardinality(tir, axis).map_err(MaterializationError::Index)
    })
    .map(|_| ())
    .map_err(|error| match error {
        MaterializationError::Index(error) => {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::Source(span))
        }
        MaterializationError::Shape(MaterializedShapeError::ExceedsLimit { maximum }) => {
            SemanticError::located(
                src,
                span,
                DimensionError::MaterializedShapeTooLarge { maximum },
            )
        }
    })
}
