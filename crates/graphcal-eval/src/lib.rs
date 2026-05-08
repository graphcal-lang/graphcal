//! Graphcal evaluation engine
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

// Modules owned by graphcal-eval.
pub(crate) mod domain_check;
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod exec_plan;
pub(crate) mod inline_dag;
pub mod loader;
