mod aggregations;
mod arithmetic;
mod complex;
mod context;
mod conversions;
mod datetime;
mod hir_eval;
mod linear_algebra;
mod linear_algebra_lu;
pub mod numeric;
mod operations;
mod presentation;
mod unit_scale;
mod work_budget;

pub use crate::constant_pools::RuntimeValueMap;
pub use crate::runtime_value::RuntimeValue;
pub use context::EvalSession;
#[cfg(any(test, feature = "test-internals"))]
pub use hir_eval::{HirLocalValueMap, eval_subtree_for_test};
pub use hir_eval::{eval_root, eval_root_with_presentation};
#[cfg(any(test, feature = "test-internals"))]
pub use hir_eval::{reset_cloned_runtime_node_count, take_cloned_runtime_node_count};

/// Compute every pending display unit of `presented` against `values`, the
/// complete values of the root frame.
///
/// # Errors
///
/// Returns a [`GraphcalError`](graphcal_compiler::graphcal_error::GraphcalError)
/// for an invariant violation or cancellation; an ordinary display failure is
/// kept on its leaf.
pub fn resolve_presentation(
    presented: crate::runtime_presentation::EvaluatedRuntimeValue,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<
    crate::runtime_presentation::ResolvedValue,
    graphcal_compiler::outcome::Outcome<graphcal_compiler::graphcal_error::GraphcalError>,
> {
    presentation::resolve(presented, values, ctx, hir_eval::eval_executable)
}
