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
mod unit_scale;
mod work_budget;

use std::collections::HashMap;

use graphcal_compiler::hir::{NominalField, NominalTypeDef};
use graphcal_compiler::registry::declared_type::{DeclaredGenericArg, IndexTypeRef, StructTypeRef};
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};
use graphcal_compiler::tir::typed::StructFieldConstraintKey;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_constraint::ResolvedDomainConstraint;

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

fn runtime_struct_type_def<'a>(
    type_name: &graphcal_compiler::syntax::type_name::ResolvedStructTypeName,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a NominalTypeDef> {
    ctx.tir.struct_type_def(type_name)
}

fn constructor_fields_for_runtime_struct<'a>(
    type_def: &'a NominalTypeDef,
    constructor: &ConstructorName,
) -> Option<&'a [NominalField]> {
    type_def
        .union_members()?
        .iter()
        .find_map(|member| (member.name() == *constructor).then_some(member.fields()))
}

fn find_struct_field_constraint<'a>(
    constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    generic_args: &[DeclaredGenericArg],
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type.and_then(|owning_type| {
        constraints.get(&StructFieldConstraintKey::for_application(
            owning_type.clone(),
            generic_args.to_vec(),
            constructor.clone(),
            field.clone(),
        ))
    })
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
