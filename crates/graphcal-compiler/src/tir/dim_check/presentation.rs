//! Publish the checked plot shapes. Value presentation is selected by execution,
//! not by a second recursively inferred and replayed program.

use crate::dag_id::DagId;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::registry::error::GraphcalError;
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::typed::TIR;
use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;

pub(super) fn collect_presentation_facts(
    tir: &TIR,
    shapes: &HashMap<DagId, super::plot::CheckedPlotChannelShapes>,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<DagId, DagPresentationFacts>, GraphcalError> {
    tir.local_dags()
        .filter(|(_, dag)| !dag.is_semantic_instance())
        .map(|(owner, _)| {
            cancellation.checkpoint()?;
            let shapes = shapes.get(owner).ok_or_else(|| {
                GraphcalError::internal_error(
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
