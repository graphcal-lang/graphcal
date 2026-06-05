//! Graphcal evaluation engine
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

// Modules owned by graphcal-eval.
pub(crate) mod decl_key;
pub(crate) mod domain_check;
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod exec_plan;
pub(crate) mod import_surface;
pub(crate) mod inline_dag;
pub mod loader;
