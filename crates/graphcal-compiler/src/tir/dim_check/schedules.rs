//! Declaration scheduling and dependency-cycle diagnostics.
//!
//! A dependency cycle is a topological property of source, knowable without
//! evaluating any value. The checker orders every local DAG's constants and
//! every local callable's params and nodes exactly once; a cycle becomes a
//! [`GraphcalError::CyclicDependency`] under `graphcal check`, and the orders
//! are retained for evaluation.

use std::sync::Arc;

use miette::NamedSource;

use crate::dag_id::DagId;
use crate::dependency_graph::Cycle;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::entry::Decl;
use crate::registry::error::GraphcalError;
use crate::resolved_name::ResolvedDeclName;
use crate::tir::schedule::{ConstSchedule, RuntimeSchedule, RuntimeScheduleError};
use crate::tir::typed::TIR;

/// Schedules computed for one checking revision, installed only after the
/// whole TIR has been accepted.
pub(super) struct CheckedSchedules {
    constants: ConstSchedule,
    callables: Vec<(DagId, RuntimeSchedule)>,
}

impl CheckedSchedules {
    /// Schedule the constants of every local DAG, then each local DAG as a
    /// callable in [`DagId`] order.
    ///
    /// # Errors
    ///
    /// Returns [`GraphcalError::CyclicDependency`] for the first cycle found,
    /// at the declaration that closes it.
    pub(super) fn build(tir: &TIR, src: &NamedSource<Arc<String>>) -> Result<Self, GraphcalError> {
        let constants = ConstSchedule::build(tir.dags.local_iter().map(|(_, dag)| dag))
            .map_err(|cycle| cyclic_dependency(tir, &cycle, None, src))?;
        let mut callables = tir
            .dags
            .local_iter()
            .map(|(_, dag)| dag)
            .collect::<Vec<_>>();
        callables.sort_by(|left, right| left.dag_id().cmp(right.dag_id()));
        let callables = callables
            .into_iter()
            .map(|dag| {
                RuntimeSchedule::build(tir, dag)
                    .map(|schedule| (dag.dag_id().clone(), schedule))
                    .map_err(|error| match error {
                        RuntimeScheduleError::Cycle(cycle) => {
                            cyclic_dependency(tir, &cycle, Some(dag.dag_id()), src)
                        }
                        RuntimeScheduleError::MissingInstance(owner) => {
                            GraphcalError::internal_error(
                                format!("semantic runtime instance `{owner}` has no compiled DAG"),
                                src,
                                DiagnosticAnchor::WholeFile,
                            )
                        }
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            constants,
            callables,
        })
    }

    /// Retain the schedules on the accepted TIR.
    pub(super) fn install(
        self,
        tir: &mut TIR,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        for (dag_id, schedule) in self.callables {
            let dag = tir.dags.get_mut(&dag_id).ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("checked DAG `{dag_id}` disappeared while installing its schedule"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            dag.semantic.runtime_schedule = Some(schedule);
        }
        tir.const_schedule = Some(self.constants);
        Ok(())
    }
}

/// Report a cycle at the declaration that closes it: the last one on the
/// cycle path, whose read leads back to the declaration where the search
/// entered the cycle. For `x = @y; y = @x` this is `y`.
///
/// A callable's cycle may pass through the instances it includes, whose
/// declarations are spelled in the included DAG's source. Such a cycle is
/// reported at the last declaration on the path that `callable` owns itself,
/// so the diagnostic points into the source being checked.
fn cyclic_dependency(
    tir: &TIR,
    cycle: &Cycle<ResolvedDeclName>,
    callable: Option<&DagId>,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    let closing = callable
        .and_then(|owner| {
            cycle
                .path()
                .filter(|declaration| declaration.owner() == owner)
                .last()
        })
        .or_else(|| cycle.path().last())
        .unwrap_or_else(|| cycle.entry());
    let site = tir
        .dag_registry()
        .get(closing.owner())
        .and_then(|dag| dag.decls().get(closing))
        .and_then(|decl| match decl {
            Decl::Const(entry) => Some((&entry.name, entry.span)),
            Decl::Param(entry) => Some((&entry.name, entry.span)),
            Decl::Node(entry) => Some((&entry.name, entry.span)),
            Decl::Assert(_) | Decl::Plot(_) | Decl::Figure(_) | Decl::Layer(_) => None,
        });
    match site {
        Some((name, span)) => GraphcalError::CyclicDependency {
            name: name.to_string(),
            src: src.clone(),
            span: span.into(),
        },
        None => GraphcalError::internal_error(
            format!("cycle node `{closing}` is missing declaration metadata"),
            src,
            DiagnosticAnchor::WholeFile,
        ),
    }
}
