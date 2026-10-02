//! Publish the checked plot shapes. Value presentation is selected by execution,
//! not by a second recursively inferred and replayed program.

use crate::outcome::Outcome;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::typed::UncheckedTir;
use crate::tir::typed::dag_slots::LocalDagFacts;

use super::instance_bodies::{InstanceOf, PlotsStage};

/// The presentation facts of every local body: a canonical body's checked plot
/// shapes, or an instance's template shapes specialized with its
/// substitution; then each body's projections of its instances' plots.
pub(super) fn collect_presentation_facts(
    tir: &UncheckedTir,
    plots: &LocalDagFacts<PlotsStage<'_>>,
    src: SourceId,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<LocalDagFacts<DagPresentationFacts>, Outcome<SemanticError>> {
    let presentation = plots.try_map_ref(|stage| match stage {
        PlotsStage::Canonical(shapes) => {
            cancellation.checkpoint()?;
            Ok(DagPresentationFacts {
                plot_channels: shapes.clone(),
            })
        }
        PlotsStage::Instance {
            instance:
                InstanceOf {
                    dag,
                    specialization,
                    ..
                },
            port_generic,
        } => {
            let template = match tir.dags.local_fact(plots, &specialization.template) {
                Some(PlotsStage::Canonical(shapes)) => Some(shapes),
                Some(PlotsStage::Instance { .. }) => None,
                None => tir
                    .dags
                    .shared(&specialization.template)
                    .map(|template| &template.presentation().plot_channels),
            };
            crate::tir::typed::specialization::instance_presentation_facts(
                tir,
                dag.dag_id(),
                dag.frame(),
                specialization,
                port_generic.as_ref().or(template),
                src,
            )
            .map_err(Outcome::Failed)
        }
    })?;
    crate::tir::typed::specialization::add_plot_projections(tir, presentation, src)
        .map_err(Outcome::Failed)
}
