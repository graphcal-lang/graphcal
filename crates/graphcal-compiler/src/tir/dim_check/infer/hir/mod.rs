//! HIR-backed expression type inference.
//!
//! This is the semantic expression type checker for module-aware declaration and
//! assertion bodies. It consumes HIR references directly: canonical declaration,
//! index, constructor, DAG-call refs, lexical `LocalId`s, and typed built-in
//! function variants. It must not fall back to source/syntax-AST inference.
//!
//! Every rule is a method on [`context::Infer`], which carries the read-only
//! [`InferEnv`], the owning declaration, the lexical locals, and the
//! operation-scoped control state. The rules are split by the expression forms
//! they type; [`dispatch`] routes each HIR expression kind to its rule.

mod calls;
mod context;
mod conversion_calls;
mod dag_call;
mod dispatch;
mod extern_call;
mod generics;
mod indexing;
mod map_literal;
mod match_expr;
mod nat_forms;
mod nominal;
mod observations;
mod operators;
mod override_deps;
mod recurrence;
mod refs;

pub(in crate::tir::dim_check) use context::InferEnv;
pub(in crate::tir::dim_check) use generics::{
    concrete_generic_substitutions, resolved_field_type, resolved_type_field_key,
};
pub(in crate::tir::dim_check) use observations::BodyObservations;
