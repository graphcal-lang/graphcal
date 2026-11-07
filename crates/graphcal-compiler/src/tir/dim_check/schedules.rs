//! Declaration scheduling and dependency-cycle diagnostics.
//!
//! A dependency cycle is a topological property of source, knowable without
//! evaluating any value. The checker orders every local DAG's constants and
//! every local callable's params and nodes exactly once; a cycle that is not
//! an accepted recurrence group becomes a
//! [`GraphError::CyclicDependency`](GraphError::CyclicDependency), or a
//! [`GraphError::RejectedRecurrence`](GraphError::RejectedRecurrence) when it
//! has an `unfold` member, under `graphcal check`, and the orders are
//! retained for evaluation.

use std::collections::HashSet;

use crate::dag_id::DagId;
use crate::dependency_graph::Cycle;
use crate::semantic_error::SemanticError;
use crate::semantic_error::graph::CycleMember;
use crate::semantic_error::graph::GraphError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;
use crate::tir::schedule::{ConstSchedule, RuntimeSchedule, ScheduleRejection, ScheduledDecl};
use crate::tir::typed::UncheckedTir;
use crate::tir::typed::dag_position::DagPosition;
use crate::tir::typed::dag_slots::LocalDagFacts;
use crate::tir::typed::instance_graph::InstanceGraph;

/// Schedules computed for one checking revision, paired with the bodies only
/// after the whole TIR has been accepted.
pub(super) struct Schedules {
    /// Evaluation order of the local DAGs' constants.
    pub(super) constants: ConstSchedule,
    /// Each local DAG as a callable.
    pub(super) callables: LocalDagFacts<RuntimeSchedule>,
}

impl Schedules {
    /// Schedule the constants of every local DAG, then each local DAG as a
    /// callable, in local order (the root, which every other local DAG
    /// descends from, then the others in [`DagId`] order).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::CyclicDependency`](GraphError::CyclicDependency) for the first cycle found,
    /// at the declaration that closes it.
    pub(super) fn build(
        tir: &UncheckedTir,
        instances: &InstanceGraph,
        src: SourceId,
    ) -> Result<Self, SemanticError> {
        let constants = ConstSchedule::build(tir.dags.local_positioned())
            .map_err(|cycle| constant_cycle(&cycle, src))?;
        let unit_scale_reads = crate::tir::typed::dynamic_unit_scale_reads(tir);
        let callables = tir.dags.map_local(|position, dag| {
            RuntimeSchedule::build(
                position,
                |position| tir.dags.at(position),
                |position| instances.instances_of(position),
                |component| {
                    crate::tir::recurrence_group::check_component(
                        component,
                        dag.dag_id(),
                        &unit_scale_reads,
                    )
                },
            )
            .map_err(|rejection| match rejection {
                ScheduleRejection::Cycle(cycle) => callable_cycle(
                    &cycle,
                    &IncludeSites::of_callable(position, tir, instances),
                    src,
                ),
                ScheduleRejection::Recurrence { name, span, reason } => SemanticError::located(
                    src,
                    span,
                    GraphError::RejectedRecurrence { name, reason },
                ),
            })
        })?;
        Ok(Self {
            constants,
            callables,
        })
    }
}

/// Report a cycle among constants at the declaration that closes it: the
/// last one on the cycle path, whose read leads back to the declaration
/// where the search entered the cycle. For `x = @y; y = @x` this is `y`.
///
/// Constants are scheduled per DAG, so every member is spelled in the
/// checked source.
fn constant_cycle(cycle: &Cycle<ScheduledDecl>, src: SourceId) -> SemanticError {
    let closing = cycle.path().last().unwrap_or_else(|| cycle.entry());
    cycle_at(src, closing.span(), closing)
}

/// Report a callable's cycle at the declaration that closes it, like
/// [`constant_cycle`].
///
/// A callable's cycle may pass through the instances it includes, whose
/// declarations are spelled in the included DAG's source. Such a cycle is
/// reported at the last declaration on the path that the callable owns
/// itself, so the diagnostic points into the source being checked. A cycle
/// that lies entirely inside one included instance (a binding that reads the
/// instance's own output) is reported at the include declaration that
/// materializes that instance.
fn callable_cycle(
    cycle: &Cycle<ScheduledDecl>,
    sites: &IncludeSites<'_>,
    src: SourceId,
) -> SemanticError {
    if let Some(closing) = cycle
        .path()
        .filter(|declaration| declaration.identity().owner() == sites.callable)
        .last()
    {
        return cycle_at(src, closing.span(), closing);
    }
    let closing = cycle.path().last().unwrap_or_else(|| cycle.entry());
    // Every scheduled instance is reached through an include of the
    // callable, so the site is found; the whole checked source is the
    // honest fallback, never a span of another source.
    let span = sites
        .include_site_of(closing.identity().owner())
        .unwrap_or_else(|| src.whole_span());
    cycle_at(src, span, closing)
}

fn cycle_at(src: SourceId, span: Span, closing: &ScheduledDecl) -> SemanticError {
    SemanticError::located(
        src,
        span,
        GraphError::CyclicDependency {
            name: CycleMember::Declaration(closing.name().clone()),
        },
    )
}

/// The include declarations a callable writes, each with every instance it
/// transitively materializes.
struct IncludeSites<'a> {
    callable: &'a DagId,
    /// Per include declaration of the callable, in edge order: its span in
    /// the callable's source and the instances it reaches.
    sites: Vec<(Span, HashSet<&'a DagId>)>,
}

impl<'a> IncludeSites<'a> {
    fn of_callable(
        callable: DagPosition,
        tir: &'a UncheckedTir,
        instances: &'a InstanceGraph,
    ) -> Self {
        let dag = tir.dags.at(callable);
        let sites = dag
            .semantic_instances()
            .iter()
            .zip(instances.instances_of(callable))
            .map(|(edge, &instance)| {
                let mut reached = HashSet::new();
                let mut pending = vec![instance];
                while let Some(position) = pending.pop() {
                    if reached.insert(tir.dags.at(position).dag_id()) {
                        pending.extend(instances.instances_of(position));
                    }
                }
                (edge.include_span, reached)
            })
            .collect();
        Self {
            callable: dag.dag_id(),
            sites,
        }
    }

    /// The span of the first include declaration that reaches `instance`.
    fn include_site_of(&self, instance: &DagId) -> Option<Span> {
        self.sites
            .iter()
            .find(|(_, reached)| reached.contains(instance))
            .map(|(span, _)| *span)
    }
}
