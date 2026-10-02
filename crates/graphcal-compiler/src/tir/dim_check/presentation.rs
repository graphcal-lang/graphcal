//! Publish the checked plot shapes. Value presentation is selected by execution,
//! not by a second recursively inferred and replayed program.

use crate::outcome::Outcome;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::typed::UncheckedTir;
use crate::tir::typed::dag_slots::LocalDagFacts;
use crate::tir::typed::instance_graph::{
    CanonicalFacts, InstanceFacts, InstanceGraph, TemplateFact,
};

use super::instance_bodies::{CanonicalChecked, InstanceChecked, InstanceOf};

/// The presentation facts of every local body: a canonical body's checked plot
/// shapes, or an instance's template shapes specialized with its
/// substitution; then each body's projections of its instances' plots.
pub(super) fn collect_presentation_facts(
    tir: &UncheckedTir,
    graph: &InstanceGraph,
    canonical: &CanonicalFacts<CanonicalChecked>,
    instances: &InstanceFacts<InstanceOf<'_>>,
    checked: &InstanceFacts<InstanceChecked>,
    src: SourceId,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<LocalDagFacts<DagPresentationFacts>, Outcome<SemanticError>> {
    let canonical_facts = canonical.try_map_ref(|checked| {
        cancellation.checkpoint()?;
        Ok::<_, Outcome<SemanticError>>(DagPresentationFacts {
            plot_channels: checked.plot_shapes.clone(),
        })
    })?;
    let instance_facts = instances
        .zip_ref(checked)
        .try_map_ref(|(instance, checked)| {
            let template = match canonical.template(instance.origin.template()) {
                TemplateFact::Local(template) => &template.plot_shapes,
                TemplateFact::Shared(template) => &template.presentation().plot_channels,
            };
            crate::tir::typed::specialization::instance_presentation_facts(
                instance.dag.frame(),
                &instance.substitution,
                checked.port_generic.as_ref().unwrap_or(template),
                src,
            )
            .map_err(Outcome::Failed)
        })?;
    let presentation = graph.join(canonical_facts, instance_facts);
    crate::tir::typed::specialization::add_plot_projections(tir, presentation, src)
        .map_err(Outcome::Failed)
}
