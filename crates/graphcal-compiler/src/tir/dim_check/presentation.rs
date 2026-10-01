//! Publish the checked plot shapes. Value presentation is selected by execution,
//! not by a second recursively inferred and replayed program.

use crate::dag_id::DagId;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::outcome::Outcome;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::typed::UncheckedTir;
use std::collections::HashMap;

pub(super) fn collect_presentation_facts(
    tir: &UncheckedTir,
    shapes: &HashMap<DagId, super::plot::CheckedPlotChannelShapes>,
    src: SourceId,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<DagId, DagPresentationFacts>, Outcome<SemanticError>> {
    tir.local_dags()
        .filter(|(_, dag)| !dag.is_semantic_instance())
        .map(|(owner, _)| {
            cancellation.checkpoint()?;
            let shapes = shapes.get(owner).ok_or_else(|| {
                SemanticError::internal_error(
                    format!("checked plot shapes missing for `{owner}`"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            Ok((
                owner.clone(),
                DagPresentationFacts {
                    plot_channels: shapes.clone(),
                },
            ))
        })
        .collect()
}
