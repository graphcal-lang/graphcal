//! Materialization facts directly from checked types, without rebuilding inferred types.

use miette::NamedSource;
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::registry::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::registry::error::GraphcalError;
use crate::registry::index::IndexCardinality;
use crate::syntax::span::Span;
use crate::tir::expression_facts::ExpressionFactsError;
use crate::tir::materialized_shape::MaterializedShapeError;
use crate::tir::typed::model::UncheckedTir;

pub(super) fn checked_index_cardinality(
    tir: &UncheckedTir,
    index: &IndexTypeRef<Symbolic>,
) -> Result<Option<IndexCardinality>, ExpressionFactsError> {
    if index
        .finite_index_form()
        .is_some_and(|form| form.constant_value().is_none())
    {
        return Ok(None);
    }
    tir.index_def(index)
        .map(|definition| definition.concrete_cardinality())
        .ok_or_else(|| ExpressionFactsError::MissingIndex(Box::new(index.clone())))
}

/// Why a checked type cannot be materialized eagerly.
enum MaterializationError {
    Facts(ExpressionFactsError),
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
    tir: &UncheckedTir,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    ty.materialized_shape(|axis| {
        checked_index_cardinality(tir, axis).map_err(MaterializationError::Facts)
    })
    .map(|_| ())
    .map_err(|error| match error {
        MaterializationError::Facts(error) => {
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
