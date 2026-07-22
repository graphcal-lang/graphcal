use std::collections::HashMap;

use graphcal_compiler::syntax::decl_name::DeclName;

mod display;
mod plot_data;
mod project;
pub(crate) mod runtime;
#[cfg(test)]
mod tests;
mod types;

pub use graphcal_compiler::registry::format::format_number;
pub use project::{
    compile_and_eval_from_project, compile_and_eval_from_project_with_host_fns,
    compile_and_eval_project, compile_to_tir, compile_to_tir_from_project,
    compile_to_tir_from_project_with_host_fns, compile_to_tir_project,
};
pub use types::{
    AssertResult, AxisMeta, CompileError, CompositionProperty, DeclType, DisplayUnit, EvalResult,
    FigureSpec, LayerSpec, MarkProperty, NodeError, PlotError, PlotFieldValue, PlotProperty,
    PlotSpec, Value, ValueError, format_epoch_with_tz, scalar_display_value,
};

pub fn compile_and_eval(source: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_named(source, "input.gcl")
}

/// Full pipeline with a custom `.gcl` source name (used for file paths in diagnostics).
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails, or if `name` is
/// not a valid `.gcl` source path.
pub fn compile_and_eval_named(source: &str, name: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_with_overrides(source, name, &HashMap::new())
}

/// Full pipeline with parameter overrides.
///
/// Each entry in `overrides` maps a param name to a replacement expression.
/// The overrides are validated (must refer to existing params, not consts/nodes)
/// and then substituted before dimension checking and evaluation.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing, validation, or evaluation fails, or
/// if `name` is not a valid `.gcl` source path.
fn compile_and_eval_with_overrides(
    source: &str,
    name: &str,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::LoadedProject::from_source(source, name)?;
    compile_and_eval_from_project(&project, overrides)
}
