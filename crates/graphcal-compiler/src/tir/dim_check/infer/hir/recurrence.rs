//! Inference of `scan` and `unfold` recurrences.

use crate::graphcal_error::GraphcalError;
use crate::hir::expr::{Expr, LocalDef};
use crate::outcome::Outcome;
use crate::semantic::checked_type::{IndexTypeRef, Symbolic};
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::evaluation::EvaluationError;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::helpers::format_checked_type;

use super::context::Infer;

impl Infer<'_> {
    pub(super) fn infer_hir_scan(
        &self,
        source: &Expr,
        init: &Expr,
        acc: &LocalDef,
        val: &LocalDef,
        body: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let source_type = self.infer_hir_type(source)?;
        let source_rank = source_type.indexed_rank();
        let CheckedType::Indexed { element, index } = source_type else {
            return Err(GraphcalError::located(
                self.env.src,
                source.span,
                EvaluationError::Failed {
                    message: "scan source must be an indexed value".to_string(),
                },
            )
            .into());
        };
        if source_rank > 1 {
            return Err(GraphcalError::located(
                self.env.src,
                source.span,
                DimensionError::MultiAxisScanSource { rank: source_rank },
            )
            .into());
        }
        let accumulator_type = self.infer_hir_type(init)?;
        let scan_locals = self
            .locals
            .child(vec![(acc.id, accumulator_type.clone()), (val.id, *element)]);
        let body_type = self.with_locals(&scan_locals).infer_hir_type(body)?;
        if body_type != accumulator_type {
            return Err(GraphcalError::located(
                self.env.src,
                body.span,
                DimensionError::DimensionMismatch {
                    expected: format_checked_type(&accumulator_type, self.env.registry),
                    found: format_checked_type(&body_type, self.env.registry),
                    help: "scan body must return the same type as the accumulator".to_string(),
                },
            )
            .into());
        }
        Ok(CheckedType::Indexed {
            element: Box::new(accumulator_type),
            index,
        })
    }

    pub(super) fn infer_hir_unfold(
        &self,
        axis: &crate::syntax::span::Spanned<crate::resolved_name::ResolvedIndexName>,
        init: &Expr,
        prev_state: &LocalDef,
        prev_index: &LocalDef,
        current_index: &LocalDef,
        body: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let init_type = self.infer_hir_type(init)?;
        let index = IndexTypeRef::from_resolved(axis.value.clone());
        let idx_def = self
            .env
            .tir
            .declared_index_def(&axis.value)
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("missing resolved unfold axis `{}`", axis.value),
                    self.env.src,
                    crate::diagnostic_anchor::DiagnosticAnchor::Source(axis.span),
                )
            })?;
        if !idx_def.is_coordinate() {
            return Err(GraphcalError::located(
                self.env.src,
                axis.span,
                EvaluationError::Failed {
                    message: format!("unfold requires a coordinate index, got `{index}`"),
                },
            )
            .into());
        }
        // The recurrence coordinate binders are keys of the axis; the coordinate
        // quantity is extracted with coord().
        let coordinate_type = CheckedType::Key(index.clone());
        let unfold_locals = self.locals.child(vec![
            (prev_state.id, init_type.clone()),
            (prev_index.id, coordinate_type.clone()),
            (current_index.id, coordinate_type),
        ]);
        let body_type = self.with_locals(&unfold_locals).infer_hir_type(body)?;
        if body_type != init_type {
            return Err(GraphcalError::located(
                self.env.src,
                body.span,
                DimensionError::DimensionMismatch {
                    expected: format_checked_type(&init_type, self.env.registry),
                    found: format_checked_type(&body_type, self.env.registry),
                    help: "unfold body must return the same type as the previous state".to_string(),
                },
            )
            .into());
        }
        Ok(CheckedType::Indexed {
            element: Box::new(init_type),
            index,
        })
    }
}
