//! Runtime preparation and evaluation for a checked project.

use std::collections::HashMap;
use std::path::Path;

use graphcal_compiler::desugar::desugared_ast::Expr;
use graphcal_compiler::syntax::decl_name::DeclName;

use crate::eval::types::{CompileError, EvalResult};
use crate::project_compiler::check_project_with_host_fns_and_cancellation;

mod output;
mod prepare;
mod prepared;

pub use prepared::{
    InclusiveBounds, ModelConstructorSchema, ModelDefinitionError, ModelExecutionError,
    ModelFieldSchema, ModelIndexKind, ModelIndexSchema, ModelOutputPort, ModelQuantitySchema,
    ModelRowFailure, ModelRowOutcome, ModelUnitSchema, ModelValueSchema, ParameterBindingBuilder,
    ParameterBindingRow, ParameterDomain, ParameterPort, ParameterPosition, ParameterValue,
    PreparedModel, PreparedProject, TenaxV2Input, TenaxV2InputKind, TenaxV2Model, TenaxV2Output,
    TenaxV2RowOutcome,
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
    prepare_from_project_with_host_fns(project, &crate::host_fns::demo_registry())
}

/// Prepare with an embedder-supplied host-function registry.
///
/// # Errors
///
/// Returns a compile, plan, interface, or plugin diagnostic.
pub fn prepare_from_project_with_host_fns(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
) -> Result<PreparedProject, CompileError> {
    prepare_from_project_with_host_fns_and_cancellation(
        project,
        host_fns,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Prepare with host functions and cooperative cancellation.
///
/// # Errors
///
/// Returns a compile, plan, interface, plugin, or cancellation diagnostic.
pub fn prepare_from_project_with_host_fns_and_cancellation(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedProject, CompileError> {
    check_project_with_host_fns_and_cancellation(project, host_fns, cancellation)?
        .prepare_with_host_fns_and_cancellation(host_fns, cancellation)
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
    compile_and_eval_from_project_with_cancellation(
        project,
        overrides,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile and evaluate a project with cooperative cancellation.
///
/// # Errors
///
/// Returns a compile, binding, evaluation, or cancellation diagnostic.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring a specific hasher"
)]
pub fn compile_and_eval_from_project_with_cancellation(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, Expr>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalResult, CompileError> {
    compile_and_eval_from_project_with_host_fns_and_cancellation(
        project,
        overrides,
        &crate::host_fns::demo_registry(),
        cancellation,
    )
}

/// Compile and evaluate with an embedder-supplied host-function registry.
///
/// # Errors
///
/// Returns a compile, plugin, binding, or evaluation diagnostic.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring a specific hasher"
)]
pub fn compile_and_eval_from_project_with_host_fns(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, Expr>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
) -> Result<EvalResult, CompileError> {
    compile_and_eval_from_project_with_host_fns_and_cancellation(
        project,
        overrides,
        host_fns,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile and evaluate with host functions and cooperative cancellation.
///
/// # Errors
///
/// Returns a compile, plugin, binding, evaluation, or cancellation diagnostic.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring a specific hasher"
)]
pub fn compile_and_eval_from_project_with_host_fns_and_cancellation(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, Expr>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalResult, CompileError> {
    let prepared = check_project_with_host_fns_and_cancellation(project, host_fns, cancellation)?
        .prepare_with_host_fns_and_cancellation(host_fns, cancellation)?;
    let mut bindings = prepared.binding_builder();
    for (name, expression) in overrides {
        cancellation.checkpoint()?;
        bindings.bind_expression(name, expression)?;
    }
    let row = bindings.finish()?;
    prepared.evaluate_with_cancellation(&row, cancellation)
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
