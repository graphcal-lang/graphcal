//! Declaration scheduling and dependency-cycle diagnostics.
//!
//! A dependency cycle is a topological property of source, knowable without
//! evaluating any value. The checker orders every local DAG's constants and
//! every local callable's params and nodes exactly once; a cycle becomes a
//! [`GraphError::CyclicDependency`](GraphError::CyclicDependency) under `graphcal check`, and the orders
//! are retained for evaluation.

use crate::dag_id::DagId;
use crate::dependency_graph::Cycle;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::entry::Decl;
use crate::resolved_name::ResolvedDeclName;
use crate::semantic_error::SemanticError;
use crate::semantic_error::graph::GraphError;
use crate::source_id::SourceId;
use crate::tir::schedule::{ConstSchedule, RuntimeSchedule, RuntimeScheduleError};
use crate::tir::typed::UncheckedTir;
use crate::tir::typed::dag_slots::LocalDagFacts;

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
    pub(super) fn build(tir: &UncheckedTir, src: SourceId) -> Result<Self, SemanticError> {
        let constants = ConstSchedule::build(tir.dags.local_positioned())
            .map_err(|cycle| cyclic_dependency(tir, &cycle, None, src))?;
        let callables = tir.dags.map_local(|_, dag| {
            RuntimeSchedule::build(dag, |owner| tir.dags.get(owner)).map_err(|error| match error {
                RuntimeScheduleError::Cycle(cycle) => {
                    cyclic_dependency(tir, &cycle, Some(dag.dag_id()), src)
                }
                RuntimeScheduleError::MissingInstance(owner) => SemanticError::internal_error(
                    format!("semantic runtime instance `{owner}` has no compiled DAG"),
                    src,
                    DiagnosticAnchor::WholeFile,
                ),
            })
        })?;
        Ok(Self {
            constants,
            callables,
        })
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
    tir: &UncheckedTir,
    cycle: &Cycle<ResolvedDeclName>,
    callable: Option<&DagId>,
    src: SourceId,
) -> SemanticError {
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
        .dags
        .get(closing.owner())
        .and_then(|dag| dag.decls().get(closing))
        .and_then(|decl| match decl {
            Decl::Const(entry) => Some((entry.name(), entry.span)),
            Decl::Param(entry) => Some((entry.name(), entry.span)),
            Decl::Node(entry) => Some((entry.name(), entry.span)),
            Decl::Assert(_) | Decl::Plot(_) | Decl::Figure(_) | Decl::Layer(_) => None,
        });
    match site {
        Some((name, span)) => SemanticError::located(
            src,
            span,
            GraphError::CyclicDependency {
                name: name.to_string(),
            },
        ),
        None => SemanticError::internal_error(
            format!("cycle node `{closing}` is missing declaration metadata"),
            src,
            DiagnosticAnchor::WholeFile,
        ),
    }
}
