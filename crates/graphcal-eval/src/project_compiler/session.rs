//! Public whole-project checking session and validated continuation.

#[cfg(test)]
use std::path::Path;
use std::sync::Arc;

use miette::NamedSource;

use crate::eval::types::CompileError;
use crate::loader::LoadedProject;

use super::{CompiledFile, lowering, pipeline};

/// One whole-project compilation session.
pub struct ProjectCompiler<'project> {
    project: &'project LoadedProject,
    host_metadata: crate::host_fns::HostFunctionMetadata,
    cancellation: graphcal_compiler::cancellation::CancellationToken,
    module_resolver: graphcal_compiler::syntax::module_resolve::ModuleResolver,
}

impl<'project> ProjectCompiler<'project> {
    /// Start a compilation session with embedder-supplied host metadata.
    ///
    /// # Errors
    ///
    /// Returns a module-resolution diagnostic when the loaded project cannot
    /// form one canonical project scope.
    pub fn new(
        project: &'project LoadedProject,
        host_metadata: &crate::host_fns::HostFunctionMetadata,
    ) -> Result<Self, CompileError> {
        Self::with_cancellation(
            project,
            host_metadata,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
    }

    /// Start a cooperatively cancellable compilation session.
    ///
    /// # Errors
    ///
    /// Returns a module-resolution diagnostic or cancellation.
    pub fn with_cancellation(
        project: &'project LoadedProject,
        host_metadata: &crate::host_fns::HostFunctionMetadata,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, CompileError> {
        cancellation.checkpoint()?;
        pipeline::validate_project_dag_recursion(project)?;
        let root_source = &project.files[&project.root].named_source;
        let module_resolver = project
            .build_module_resolver()
            .map_err(|error| lowering::module_resolve_compile_error(error, root_source))?;
        Ok(Self {
            project,
            host_metadata: host_metadata.clone(),
            cancellation: cancellation.clone(),
            module_resolver,
        })
    }

    /// Compile every module once and return the reusable checked continuation.
    ///
    /// # Errors
    ///
    /// Returns a compile diagnostic or cancellation.
    pub fn check(self) -> Result<CheckedProject, CompileError> {
        pipeline::check_project_perfile(
            self.project,
            &self.host_metadata,
            self.module_resolver,
            &self.cancellation,
        )
    }
}

/// A fully checked project that has not yet been prepared or evaluated.
///
/// This is the reusable semantic boundary shared by `check`, graph projection,
/// the LSP, and runtime preparation. Private fields prevent callers from
/// fabricating a value that skipped mandatory checks.
pub struct CheckedProject {
    pub(super) compiled: CompiledFile,
    pub(super) source: NamedSource<Arc<String>>,
    pub(super) root_ast: graphcal_compiler::desugar::desugared_ast::File,
    pub(super) module_resolver: graphcal_compiler::syntax::module_resolve::ModuleResolver,
}

impl std::fmt::Debug for CheckedProject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckedProject")
            .field("root", self.compiled.tir.root_dag_id())
            .field("modules", &self.compiled.tir.dag_registry().len())
            .finish_non_exhaustive()
    }
}

impl CheckedProject {
    /// Borrow the checked typed program.
    #[must_use]
    pub const fn tir(&self) -> &graphcal_compiler::tir::typed::TIR {
        &self.compiled.tir
    }

    /// Borrow the canonical resolver built by this compilation session.
    #[must_use]
    pub const fn module_resolver(
        &self,
    ) -> &graphcal_compiler::syntax::module_resolve::ModuleResolver {
        &self.module_resolver
    }

    /// Whether the entry DAG still requires runtime inputs.
    #[must_use]
    pub fn is_library(&self) -> bool {
        self.compiled.tir.is_library()
    }

    /// Consume the checked semantic result at the runtime-preparation boundary.
    pub(crate) fn into_runtime_parts(self) -> CheckedProjectRuntimeParts {
        CheckedProjectRuntimeParts {
            compiled: self.compiled,
            source: self.source,
            root_ast: self.root_ast,
            module_resolver: self.module_resolver,
        }
    }
}

/// Checked semantic products needed to construct a runtime plan.
pub struct CheckedProjectRuntimeParts {
    pub(crate) compiled: CompiledFile,
    pub(crate) source: NamedSource<Arc<String>>,
    pub(crate) root_ast: graphcal_compiler::desugar::desugared_ast::File,
    pub(crate) module_resolver: graphcal_compiler::syntax::module_resolve::ModuleResolver,
}

/// Compile a loaded project into the reusable checked-program boundary.
///
/// # Errors
///
/// Returns a compile diagnostic when any project module is invalid.
pub fn check_project(project: &LoadedProject) -> Result<CheckedProject, CompileError> {
    check_project_with_host_fns(project, &crate::host_fns::demo_registry())
}

/// Check a project with embedder-provided extern implementations and metadata.
///
/// # Errors
///
/// Returns a compile or plugin-signature diagnostic.
pub fn check_project_with_host_fns(
    project: &LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
) -> Result<CheckedProject, CompileError> {
    check_project_with_host_metadata(project, &host_fns.metadata())
}

/// Check a project using non-callable extern signature metadata.
///
/// # Errors
///
/// Returns a compile or plugin-signature diagnostic.
pub fn check_project_with_host_metadata(
    project: &LoadedProject,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
) -> Result<CheckedProject, CompileError> {
    check_project_with_host_metadata_and_cancellation(
        project,
        host_metadata,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Check a project once with cooperative cancellation.
///
/// # Errors
///
/// Returns a compile, plugin-signature, or cancellation diagnostic.
pub fn check_project_with_host_fns_and_cancellation(
    project: &LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProject, CompileError> {
    check_project_with_host_metadata_and_cancellation(project, &host_fns.metadata(), cancellation)
}

/// Check a project with non-callable metadata and cooperative cancellation.
///
/// # Errors
///
/// Returns a compile, plugin-signature, or cancellation diagnostic.
pub fn check_project_with_host_metadata_and_cancellation(
    project: &LoadedProject,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProject, CompileError> {
    ProjectCompiler::with_cancellation(project, host_metadata, cancellation)?.check()
}

/// Test-only projection of a fully checked project into its TIR.
#[cfg(test)]
pub fn compile_to_tir_from_project(
    project: &LoadedProject,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    check_project(project).map(|checked| checked.compiled.tir)
}

/// Test-only convenience projection from source through [`CheckedProject`].
#[cfg(test)]
pub fn compile_to_tir(
    source: &str,
    name: &str,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    let project = LoadedProject::from_source(source, name)?;
    compile_to_tir_from_project(&project)
}

/// Test-only convenience projection for a loaded multi-file project.
#[cfg(test)]
pub fn compile_to_tir_project<F: graphcal_io::FileSystemReader>(
    root_path: &Path,
    project_root: Option<&Path>,
    fs: &F,
) -> Result<(graphcal_compiler::tir::typed::TIR, LoadedProject), CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}
