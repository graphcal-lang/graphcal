//! Graphcal Compiler: syntax, registry, IR, and TIR.
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod ir;
pub mod registry;
pub mod syntax;
pub mod tir;
