//! Materialization facts directly from checked types, without rebuilding inferred types.

use miette::NamedSource;
use std::sync::Arc;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::registry::declared_type::{DeclaredType, IndexTypeRef};
use crate::registry::error::GraphcalError;
use crate::registry::index::IndexCardinality;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::tir::expression_facts::{ExpressionFactsError, ExpressionShape};
use crate::tir::materialized_shape::{MaterializedShape, MaterializedShapeError};
use crate::tir::typed::model::TIR;

pub(super) fn checked_index_cardinality(
    tir: &TIR,
    index: &IndexTypeRef,
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

pub(super) fn checked_expression_shape(
    ty: &DeclaredType,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ExpressionShape, GraphcalError> {
    let mut axes = Vec::new();
    let mut current = ty;
    while let DeclaredType::Indexed { element, index } = current {
        axes.push(index);
        current = element;
    }
    let cardinalities = axes
        .iter()
        .map(|axis| checked_index_cardinality(tir, axis))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::Source(span))
        })?;
    cardinalities
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map_or_else(
            || {
                Ok(ExpressionShape::Symbolic(
                    axes.into_iter().cloned().collect(),
                ))
            },
            |values| {
                NonEmpty::try_from_vec(values).map_or(Ok(ExpressionShape::Scalar), |values| {
                    MaterializedShape::try_new(values)
                        .map(ExpressionShape::Concrete)
                        .map_err(|MaterializedShapeError::ExceedsLimit { maximum }| {
                            GraphcalError::MaterializedShapeTooLarge {
                                maximum,
                                src: src.clone(),
                                span: span.into(),
                            }
                        })
                })
            },
        )
}
