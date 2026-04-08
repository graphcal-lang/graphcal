//! Graphcal evaluation engine
#![allow(
    unused_assignments,
    reason = "miette derive macro generates false-positive unused_assignments warnings"
)]
#![allow(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

// Re-export foundation modules from graphcal-registry.
pub use graphcal_compiler::registry::builtins;
pub use graphcal_compiler::registry::declared_type;
pub use graphcal_compiler::registry::error;
pub use graphcal_compiler::registry::format;
pub use graphcal_compiler::registry::manifest;
pub use graphcal_compiler::registry::prelude;
pub use graphcal_compiler::registry::registry;
pub use graphcal_compiler::registry::resolve_types;
pub use graphcal_compiler::registry::runtime_value;
pub use graphcal_compiler::registry::time_scale;

// Re-export IR modules from graphcal-ir.
pub use graphcal_compiler::ir::ir;
pub use graphcal_compiler::ir::resolve;

// Re-export TIR modules from graphcal-tir.
pub use graphcal_compiler::tir::dim_check;
pub use graphcal_compiler::tir::tir;

// Modules owned by graphcal-eval.
pub mod eval;
pub(crate) mod eval_expr;
pub(crate) mod exec_plan;
pub mod loader;
