//! One inline DAG call (`@target(…)::output`) as a dependency fact of the
//! declaration whose definition makes it.

use crate::dag_id::DagId;
use crate::resolved_name::ResolvedDeclName;

/// An inline call of the DAG `target`, projecting its declaration `output`.
///
/// A call has no instance identity of its own: it is evaluated in place, as
/// part of the expression that makes it. Tools that present a call as an
/// instance identify it by the declaration making it and the call's position
/// among that declaration's calls.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DagCallRead {
    /// The DAG the call instantiates.
    pub target: DagId,
    /// The canonical declaration the call projects out of `target`.
    pub output: ResolvedDeclName,
    /// Whether the call binds any of the target's params or Static
    /// parameters. An unbound call evaluates the target as it stands alone.
    pub binds: bool,
}
