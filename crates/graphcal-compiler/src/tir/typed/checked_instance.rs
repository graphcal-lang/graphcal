//! Semantic instances of a checked TIR, with the projections an include site
//! exposes already resolved in the instance's own frame.
//!
//! An include site's projections name their targets as the template names
//! them ([`LocalDecl`](crate::hir::expr::LocalDecl)), because a template's
//! instance records are shared by every instance of an enclosing template.
//! The DAG that runs the instance resolves them; this module pairs each
//! record with that DAG, found from the record's own instance identity, so a
//! consumer reads projection targets as declaration identities and never
//! chooses the frame they are resolved in.

use crate::ir::instance::{
    HirInstanceRecord, InstanceAssertionProjection, InstancePlotProjection, InstanceValueProjection,
};
use crate::resolved_name::ResolvedDeclName;

use super::checked::CheckedDagRegistry;

use super::checked_dag::CheckedDag;

/// One semantic include edge together with the checked DAG that runs its
/// instance.
#[derive(Debug, Clone, Copy)]
pub struct CheckedInstance<'t> {
    record: &'t HirInstanceRecord,
    dag: &'t CheckedDag,
}

/// One include-site projection whose target is resolved in the instance's
/// frame.
#[derive(Debug, Clone)]
pub struct ResolvedProjection<'t, P> {
    /// The declaration the projection exposes, as the instance runs it.
    pub target: ResolvedDeclName,
    /// The include-site projection, for its exposed name and options.
    pub projection: &'t P,
}

impl CheckedDagRegistry {
    /// The checked instance `record` describes: the record paired with the
    /// DAG its instance identity names.
    ///
    /// Returns `None` when this registry has not materialized the instance.
    #[must_use]
    pub fn semantic_instance<'t>(
        &'t self,
        record: &'t HirInstanceRecord,
    ) -> Option<CheckedInstance<'t>> {
        self.get(record.instance.id().owner())
            .map(|dag| CheckedInstance { record, dag })
    }
}

impl<'t> CheckedInstance<'t> {
    /// The include edge this instance was materialized from.
    #[must_use]
    pub const fn record(self) -> &'t HirInstanceRecord {
        self.record
    }

    /// The checked DAG that runs the instance.
    #[must_use]
    pub const fn dag(self) -> &'t CheckedDag {
        self.dag
    }

    fn resolved<P>(
        self,
        projections: &'t [P],
        target: impl Fn(&P) -> &crate::hir::expr::LocalDecl + 't,
    ) -> impl Iterator<Item = ResolvedProjection<'t, P>> + 't {
        projections
            .iter()
            .map(move |projection| ResolvedProjection {
                target: self.dag.body().frame().resolve(target(projection)),
                projection,
            })
    }

    /// The runtime values the include site exposes, in record order.
    pub fn output_projections(
        self,
    ) -> impl Iterator<Item = ResolvedProjection<'t, InstanceValueProjection>> + 't {
        self.resolved(&self.record.output_projections, |projection| {
            &projection.target
        })
    }

    /// The assertions the include site exposes, in record order.
    pub fn assertion_projections(
        self,
    ) -> impl Iterator<Item = ResolvedProjection<'t, InstanceAssertionProjection>> + 't {
        self.resolved(&self.record.assertion_projections, |projection| {
            &projection.target
        })
    }

    /// The plots the include site requests, in record order. A target may be
    /// owned by an instance nested in this one, when the template forwards a
    /// plot of its own instance.
    pub fn plot_projections(
        self,
    ) -> impl Iterator<Item = ResolvedProjection<'t, InstancePlotProjection>> + 't {
        self.resolved(&self.record.plot_projections, |projection| {
            &projection.target
        })
    }
}
