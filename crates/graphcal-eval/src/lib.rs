//! Graphcal interpreter: seals checked programs and executes them.
#![warn(clippy::arithmetic_side_effects)]
#![expect(
    clippy::result_large_err,
    reason = "SemanticError is inherently large and only constructed on the error path"
)]

// Modules owned by graphcal-eval.
pub(crate) mod assertion_eval;
pub mod checked_program;
pub mod constant_pools;
mod deferred_field_checks;
pub mod domain_check;
pub mod domain_constraint;
pub mod eval;
pub mod eval_expr;
pub mod exec_plan;
pub mod execution_check;
pub(crate) mod execution_frame;
pub mod execution_plan;
pub mod host_abi;
pub mod host_fns;
#[cfg(test)]
mod interpreter_tests;
pub mod invariant;
pub mod pipeline_metrics;
pub mod presentation_evidence;
pub mod runtime_presentation;
pub mod runtime_value;
mod static_incompleteness;
#[cfg(test)]
mod test_tir;
