//! A checked DAG body: one body paired with every fact its check published.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::resolved_name::{ResolvedDeclName, ResolvedStructTypeName};
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::schedule::RuntimeSchedule;
use crate::tir::texpr::CheckedBodies;

use super::model::DagTIR;

/// A DAG body together with everything its check published: the checked tree
/// of every expression root, presentation facts, and its runtime schedule as a
/// callable.
///
/// A canonical body's trees are the ones its inference emitted; a semantic
/// instance's are its template's, specialized with the instance's Static
/// substitution (their types differ per instance, so they cannot be shared).
///
/// Created only when an [`InstantiatedTir`](super::program::InstantiatedTir) is
/// checked, so its checked trees are always present and cover exactly this
/// body's expression roots.
#[derive(Debug, Clone)]
pub struct CheckedDag {
    pub(super) body: DagTIR,
    bodies: CheckedBodies,
    presentation: DagPresentationFacts,
    runtime_schedule: RuntimeSchedule,
}

/// The facts one check published for one local body.
pub(super) struct PublishedDag {
    pub(super) bodies: CheckedBodies,
    pub(super) presentation: DagPresentationFacts,
    pub(super) runtime_schedule: RuntimeSchedule,
}

impl CheckedDag {
    /// Pair a checked body with the facts published for it.
    pub(super) fn new(
        body: DagTIR,
        published: PublishedDag,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Self, GraphcalError> {
        let internal = |message: String| {
            GraphcalError::internal_error(
                format!("DAG `{}`: {message}", body.dag_id()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        };
        if !published.bodies.cover(body.owned_expression_roots()) {
            return Err(internal(
                "typed bodies do not cover exactly its expression roots".to_owned(),
            ));
        }
        Ok(Self {
            body,
            bodies: published.bodies,
            presentation: published.presentation,
            runtime_schedule: published.runtime_schedule,
        })
    }

    /// The checked tree of every expression root this body owns.
    #[must_use]
    pub(crate) const fn bodies(&self) -> &CheckedBodies {
        &self.bodies
    }

    /// Every concrete constructor application this body's checked trees
    /// make; see [`CheckedBodies::concrete_applications`].
    #[must_use]
    pub fn concrete_constructor_applications(
        &self,
    ) -> Vec<(
        &ResolvedStructTypeName,
        Vec<crate::semantic::checked_type::CheckedGenericArg>,
    )> {
        self.bodies.concrete_applications()
    }

    /// The checked tree of every expression root this body owns, for tests
    /// outside the compiler.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub const fn bodies_for_test(&self) -> &CheckedBodies {
        &self.bodies
    }

    /// Runtime schedule of this DAG as a callable.
    #[must_use]
    pub const fn runtime_schedule(&self) -> &RuntimeSchedule {
        &self.runtime_schedule
    }

    /// Checked structured display and plot-channel presentation facts.
    #[must_use]
    pub const fn presentation(&self) -> &DagPresentationFacts {
        &self.presentation
    }

    /// Look up checked plot-channel presentation facts.
    #[must_use]
    pub fn plot_channel_presentations(
        &self,
        plot: &ResolvedDeclName,
    ) -> Option<&HashMap<crate::syntax::ast::EncodingChannel, crate::plot_shape::PlotChannelShape>>
    {
        self.presentation.plot_channels.get(plot)
    }

    /// Release the body, dropping its facts, to re-resolve it in a derived
    /// checking view.
    pub(crate) fn into_body(self) -> DagTIR {
        self.body
    }
}

impl std::ops::Deref for CheckedDag {
    type Target = DagTIR;

    fn deref(&self) -> &DagTIR {
        &self.body
    }
}
