//! Runtime preparation and evaluation for a checked project.

use std::collections::HashMap;
use std::path::Path;

use graphcal_compiler::desugar::desugared_ast::Expr;
use graphcal_compiler::syntax::decl_name::DeclName;

use crate::compile_error::CompileError;
use crate::project_compiler::ProjectCompiler;
use graphcal_eval::eval::types::EvalResult;

mod model_schema;
mod output;
mod prepared;

pub use model_schema::{
    ModelAlgebraicTypeSchema, ModelConstructorSchema, ModelFieldSchema, ModelIndexKind,
    ModelIndexSchema, ModelQuantitySchema, ModelSchemaGraph, ModelTypeId, ModelUnitSchema,
    ModelValueSchema,
};
pub use prepared::{
    InclusiveBounds, InclusiveBoundsError, ModelDefinitionError, ModelExecutionError,
    ModelOutputPort, ModelRowFailure, ModelRowOutcome, ParameterBindingBuilder,
    ParameterBindingRow, ParameterDomain, ParameterPort, ParameterPosition, ParameterValue,
    PreparedModel, PreparedProject, StructuredBindingError, StructuredBindingErrorKind,
    StructuredBindingPathSegment, StructuredValueExpr, TenaxV2Input, TenaxV2InputKind,
    TenaxV2Model, TenaxV2Output, TenaxV2RowOutcome,
};

/// Prepare a loaded project once for repeated typed evaluation.
///
/// # Errors
///
/// Returns a [`CompileError`] for compilation, type, plan, plugin, or
/// cancellation failures.
pub fn prepare_from_project(
    project: &crate::loader::LoadedProject,
) -> Result<PreparedProject, CompileError> {
    ProjectCompiler::new(project).prepare()
}

/// Compile and evaluate a loaded project.
///
/// Imports remain compile-time-only; runtime work covers the entry DAG and
/// explicit include/call instances.
///
/// # Errors
///
/// Returns a [`CompileError`] if any checking, preparation, binding, or
/// evaluation stage fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring a specific hasher"
)]
pub fn compile_and_eval_from_project(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, Expr>,
) -> Result<EvalResult, CompileError> {
    ProjectCompiler::new(project).eval(overrides)
}

/// Load, compile, and evaluate a multi-file project.
///
/// All filesystem access goes through the supplied reader.
///
/// # Errors
///
/// Returns a loading, parsing, checking, or evaluation diagnostic.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring a specific hasher"
)]
pub fn compile_and_eval_project<F: graphcal_io::FileSystemReader>(
    root_path: &Path,
    overrides: &HashMap<DeclName, Expr>,
    project_root: Option<&Path>,
    fs: &F,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    compile_and_eval_from_project(&project, overrides)
}

/// Load, compile, and evaluate a single-file source named `input.gcl`.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails.
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
pub(crate) fn compile_and_eval_with_overrides(
    source: &str,
    name: &str,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::LoadedProject::from_source(source, name)?;
    compile_and_eval_from_project(&project, overrides)
}
