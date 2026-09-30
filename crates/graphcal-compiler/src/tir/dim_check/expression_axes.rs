//! Materialization facts directly from checked types, without rebuilding inferred types.

use miette::NamedSource;
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::semantic::index_def::IndexCardinality;
use crate::syntax::span::Span;
use crate::tir::materialized_shape::MaterializedShapeError;
use crate::tir::static_index::UnavailableIndex;
use crate::tir::typed::program::TirRead;

pub(super) fn checked_index_cardinality(
    tir: &dyn TirRead,
    index: &IndexTypeRef<Symbolic>,
) -> Result<Option<IndexCardinality>, UnavailableIndex> {
    if index
        .finite_index_form()
        .is_some_and(|form| form.constant_value().is_none())
    {
        return Ok(None);
    }
    tir.index_def(index)
        .map(|definition| definition.concrete_cardinality())
        .ok_or_else(|| UnavailableIndex(Box::new(index.clone())))
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
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    ty.materialized_shape(|axis| {
        checked_index_cardinality(tir, axis).map_err(MaterializationError::Index)
    })
    .map(|_| ())
    .map_err(|error| match error {
        MaterializationError::Index(error) => {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::Source(span))
        }
        MaterializationError::Shape(MaterializedShapeError::ExceedsLimit { maximum }) => {
            GraphcalError::MaterializedShapeTooLarge {
                maximum,
                src: src.clone(),
                span: span.into(),
            }
        }
    })
}
