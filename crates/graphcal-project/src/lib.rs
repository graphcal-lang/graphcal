//! Graphcal projects: loading, whole-project compilation, runtime
//! preparation, and output assembly over the `graphcal-eval` interpreter.
//!
//! The crate root only declares modules; each public item is reached through
//! the module that owns it.
#![warn(clippy::arithmetic_side_effects)]
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

pub mod binding_error;
pub mod compile_error;
pub mod dependency_ordered;
pub mod graph_ir;
pub(crate) mod import_surface;
pub(crate) mod inline_dag;
pub mod load_error;
pub mod loader;
pub mod package_cache;
pub mod package_snapshot;
pub mod package_sources;
pub mod prepare;
pub mod project_bundle;
pub mod project_compiler;
#[cfg(test)]
mod tests;
