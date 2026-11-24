//! The `dependency failed: ...` report of an expression reading failed
//! declarations.

use std::collections::HashMap;

use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::tir::typed::Scoped;

use crate::eval::types::{NodeUnavailable, RuntimeUnavailable};

use super::root_names::RootNames;

/// If any declaration referenced by the given expressions failed to
/// evaluate, render a `dependency failed: ...` message naming each failed
/// dependency as the root names it (direct failures carry their root cause
/// inline). An unfinished declaration has not failed: the expression is
/// blocked only if its evaluation reaches it. Neither has a declaration with
/// `partial` cells: an expression fails only if it reads a failed cell.
///
/// Shared by assertions (#814) and plots (#842): a reference to a failed
/// declaration is not "undefined", it is unevaluable, and the report must
/// point at the root cause.
pub(super) fn dependency_failure_message<'a>(
    exprs: impl IntoIterator<Item = Scoped<'a, graphcal_compiler::hir::expr::Expr>>,
    errors: &HashMap<ResolvedDeclName, RuntimeUnavailable>,
    partial: &HashMap<ResolvedDeclName, crate::cells::PartialValue>,
    names: &RootNames<'_>,
) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let deps: std::collections::BTreeSet<_> = exprs
        .into_iter()
        .flat_map(Scoped::<'_, graphcal_compiler::hir::expr::Expr>::graph_refs)
        .collect();
    let failed = deps
        .iter()
        .filter(|dep| {
            errors
                .get(*dep)
                .is_some_and(|err| err.has_failure() && !partial.contains_key(*dep))
        })
        .collect::<Vec<_>>();
    (!failed.is_empty()).then(|| failed_dependencies_message(failed, errors, names))
}

/// The `dependency failed: ...` message naming each of `failed`, failed
/// declarations, as the root names it, with its root cause when it failed
/// by itself.
pub(super) fn failed_dependencies_message<'a>(
    failed: impl IntoIterator<Item = &'a ResolvedDeclName>,
    errors: &HashMap<ResolvedDeclName, RuntimeUnavailable>,
    names: &RootNames<'_>,
) -> String {
    let failed = failed
        .into_iter()
        .map(|dep| {
            let name = names.name(dep);
            match errors.get(dep) {
                Some(NodeUnavailable::EvalFailed { message }) => format!("{name} ({message})"),
                Some(reason @ (NodeUnavailable::Todo { .. } | NodeUnavailable::Blocked { .. })) => {
                    format!("{name} ({})", names.present(reason))
                }
                Some(NodeUnavailable::DependencyFailed { .. }) | None => name.to_string(),
            }
        })
        .collect::<Vec<_>>();
    format!("dependency failed: {}", failed.join(", "))
}
