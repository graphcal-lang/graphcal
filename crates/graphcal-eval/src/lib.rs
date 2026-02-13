//! Graphcal evaluation engine
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub(crate) mod builtins;
pub(crate) mod const_eval;
pub(crate) mod dag;
pub(crate) mod dim_check;
pub mod error;
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod fn_check;
pub mod loader;
pub(crate) mod prelude;
pub(crate) mod registry;
pub(crate) mod resolve;
