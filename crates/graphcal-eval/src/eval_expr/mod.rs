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
pub mod presentation;
mod unit_scale;
mod work_budget;

use graphcal_compiler::registry::checked_type::IndexTypeRef;

pub use crate::constant_pools::RuntimeValueMap;
pub use crate::runtime_value::RuntimeValue;
pub use context::EvalSession;
#[cfg(test)]
pub use hir_eval::{HirLocalValueMap, eval_subtree_for_test};
pub use hir_eval::{eval_root, eval_root_with_presentation};
pub(in crate::eval_expr) use unit_scale::{
    checked_unit_scaled_value, resolve_unit_scale, resolved_unit_scale,
};

pub fn index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::resolved_name::ResolvedIndexName,
) -> bool {
    actual.declared_resolved() == Some(expected)
}

fn imported_binding_value<'a>(
    target: &graphcal_compiler::resolved_name::ResolvedDeclName,
    caller_dag: &graphcal_compiler::dag_id::DagId,
    caller_values: &'a RuntimeValueMap,
    ctx: &'a EvalSession<'_>,
) -> Option<&'a RuntimeValue> {
    if target.owner() == caller_dag {
        caller_values.get(target)
    } else if target.owner() == ctx.tir.root_dag_id() {
        ctx.root_values.and_then(|values| values.get(target))
    } else {
        None
    }
}
