mod aggregations;
mod arithmetic;
mod builtin_call;
mod complex;
mod context;
mod conversions;
mod datetime;
mod hir_eval;
mod linear_algebra;
mod linear_algebra_lu;
pub mod numeric;
pub mod presentation;
mod unit_scale;
mod work_budget;

use graphcal_compiler::registry::declared_type::IndexTypeRef;

use crate::decl_key::RuntimeDeclKey;

pub use crate::execution_facts::RuntimeValueMap;
pub use context::EvalContext;
pub use graphcal_compiler::registry::runtime_value::RuntimeValue;
pub use hir_eval::{HirLocalValueMap, eval_hir_expr, eval_hir_expr_with_presentation};
pub use unit_scale::resolve_unit_scale;
pub(in crate::eval_expr) use unit_scale::{checked_finite_quantity, checked_unit_scaled_value};

pub fn index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::syntax::index_name::ResolvedIndexName,
) -> bool {
    actual.declared_resolved() == Some(expected)
}

fn dag_decl_runtime_key(
    name: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
) -> RuntimeDeclKey {
    RuntimeDeclKey::resolved(name.clone())
}

fn imported_binding_value<'a>(
    target: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    caller_values: &'a RuntimeValueMap,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a RuntimeValue> {
    let key = RuntimeDeclKey::resolved(target.clone());
    if target.owner() == ctx.current_dag.dag_id() {
        caller_values.get(&key)
    } else if target.owner() == ctx.tir.root_dag_id() {
        ctx.root_values.and_then(|values| values.get(&key))
    } else {
        None
    }
}
