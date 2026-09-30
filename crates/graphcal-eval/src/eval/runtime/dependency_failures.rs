//! The `dependency failed: ...` report of an expression reading failed
//! declarations.

use std::collections::HashMap;

use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::tir::typed::Scoped;

use crate::eval::types::NodeUnavailable;

/// If any declaration referenced by the given expressions failed to
/// evaluate, render a `dependency failed: ...` message naming each failed
/// dependency (direct failures carry their root cause inline).
///
/// Shared by assertions (#814) and plots (#842): a reference to a failed
/// declaration is not "undefined", it is unevaluable, and the report must
/// point at the root cause.
pub(super) fn dependency_failure_message<'a>(
    exprs: impl IntoIterator<Item = Scoped<'a, graphcal_compiler::hir::expr::Expr>>,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let deps: std::collections::BTreeSet<_> = exprs
        .into_iter()
        .flat_map(Scoped::<'_, graphcal_compiler::hir::expr::Expr>::graph_refs)
        .collect();
    let failed: Vec<String> =
        deps.iter()
            .filter_map(|dep| {
                errors.get(dep).map(|err| {
                    let leaf = dep.atom();
                    match err {
                        NodeUnavailable::EvalFailed { message } => format!("{leaf} ({message})"),
                        NodeUnavailable::DependencyFailed { .. } => leaf.to_string(),
                        reason @ (NodeUnavailable::Todo { .. }
                        | NodeUnavailable::Blocked { .. }) => format!("{leaf} ({reason})"),
                    }
                })
            })
            .collect();
    (!failed.is_empty()).then(|| format!("dependency failed: {}", failed.join(", ")))
}
