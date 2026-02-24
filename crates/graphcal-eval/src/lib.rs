//! Graphcal evaluation engine
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod builtins;
pub(crate) mod dim_check;
pub mod error;
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod exec_plan;
pub(crate) mod fn_check;
pub(crate) mod ir;
pub mod loader;
pub mod manifest;
pub(crate) mod prelude;
pub mod registry;
pub(crate) mod resolve;
pub mod time_scale;
pub mod tir;
