//! Graphcal Compiler: syntax, registry, IR, and TIR.
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod desugar;
pub mod ir;
pub mod registry;
pub mod syntax;
pub mod tir;
