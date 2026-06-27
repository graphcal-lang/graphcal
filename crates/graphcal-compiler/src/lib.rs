//! Graphcal Compiler: syntax, registry, IR, and TIR.
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod dag_id;
pub mod desugar;
pub mod dimension;
pub mod hir;
pub mod ir;
pub mod nat;
pub mod registry;
pub mod stack;
pub mod syntax;
pub mod tir;
