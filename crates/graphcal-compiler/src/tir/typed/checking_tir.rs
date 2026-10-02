//! A project TIR in the middle of its check: its unchecked local bodies
//! with the facts checking has published for them so far.

use super::dag_slots::LocalDagFacts;
use super::model::{DagTIR, TirCore};
use super::program::{TirRead, UncheckedTir};

/// A project TIR in the middle of its check: unchecked local bodies with the
/// checked trees published for them so far.
#[derive(Clone, Copy)]
pub struct CheckingTir<'a, B = crate::tir::texpr::CheckedBodies> {
    pub tir: &'a UncheckedTir,
    /// What checking has published for each local body so far.
    pub bodies: &'a LocalDagFacts<B>,
}

/// What checking has published for one local body so far: its checked trees
/// once they are published.
pub trait PublishedBodies {
    /// The checked trees of the body, once published.
    fn published(&self) -> Option<&crate::tir::texpr::CheckedBodies>;
}

impl PublishedBodies for crate::tir::texpr::CheckedBodies {
    fn published(&self) -> Option<&crate::tir::texpr::CheckedBodies> {
        Some(self)
    }
}

impl<B: PublishedBodies> CheckingTir<'_, B> {
    /// The body at `position` with the checked trees already published for
    /// it: an imported body's, or a local body's once checking published
    /// them.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    pub fn checked_at(
        &self,
        position: super::dag_position::DagPosition,
    ) -> Option<(&DagTIR, &crate::tir::texpr::CheckedBodies)> {
        let bodies = match self.tir.dags.local_fact_at(self.bodies, position) {
            Some(fact) => fact.published()?,
            None => self.tir.dags.shared_at(position)?.bodies(),
        };
        Some((self.tir.dags.at(position), bodies))
    }
}

impl<B: PublishedBodies> TirRead for CheckingTir<'_, B> {
    fn core(&self) -> &TirCore {
        &self.tir.core
    }

    fn root(&self) -> &DagTIR {
        self.tir.root()
    }

    fn dag(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        self.tir.dag(dag_id)
    }

    fn dag_bodies(&self) -> Box<dyn Iterator<Item = &DagTIR> + '_> {
        self.tir.dag_bodies()
    }

    fn checked_bodies(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::texpr::CheckedBodies> {
        self.tir
            .dags
            .local_fact(self.bodies, dag_id)
            .and_then(PublishedBodies::published)
            .or_else(|| self.tir.checked_bodies(dag_id))
    }
}
