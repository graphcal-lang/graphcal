use std::collections::HashMap;

use graphcal_syntax::names::DeclName;

mod display;
mod project;
mod runtime;
#[cfg(test)]
mod tests;
mod types;

pub use display::format_number;
pub(crate) use display::format_unit_expr;
pub use project::{
    compile_and_eval_from_project, compile_and_eval_project, compile_to_tir,
    compile_to_tir_from_project, compile_to_tir_project,
};
pub use types::{
    AssertResult, CompileError, DeclType, DisplayUnit, EvalResult, NodeError, Value, ValueError,
    format_epoch_with_tz,
};

pub fn compile_and_eval(source: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_named(source, "input")
}

/// Full pipeline with a custom source name (used for file paths in diagnostics).
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails.
pub fn compile_and_eval_named(source: &str, name: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_with_overrides(source, name, &HashMap::new(), true)
}

/// Full pipeline with parameter overrides.
///
/// Each entry in `overrides` maps a param name to a replacement expression.
/// The overrides are validated (must refer to existing params, not consts/nodes)
/// and then substituted before dimension checking and evaluation.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing, validation, or evaluation fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_with_overrides(
    source: &str,
    name: &str,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
    allow_defaults: bool,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::LoadedProject::from_source(source, name)?;
    compile_and_eval_from_project(&project, overrides, allow_defaults)
}
