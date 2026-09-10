//! Graphcal evaluation engine
#![warn(clippy::arithmetic_side_effects)]
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

// Modules owned by graphcal-eval.
pub(crate) mod assertion_eval;
pub(crate) mod constant_pools;
pub(crate) mod decl_key;
pub(crate) mod declaration_locations;
pub(crate) mod domain_check;
pub(crate) mod domain_constraint;
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod exec_plan;
pub(crate) mod execution_facts;
pub(crate) mod execution_frame;
pub(crate) mod execution_plan;
pub(crate) mod execution_scope;
pub mod graph_ir;
pub mod host_abi;
pub mod host_fns;
pub(crate) mod import_surface;
pub(crate) mod inline_dag;
pub mod loader;
pub mod package_cache;
mod pipeline_metrics;
pub mod presentation_evidence;
pub mod project_bundle;
pub mod project_compiler;
pub(crate) mod runtime_presentation;
