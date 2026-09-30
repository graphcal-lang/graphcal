//! Public whole-project checking session and validated continuation.

#[cfg(test)]
use std::path::Path;

use crate::eval::types::CompileError;
use crate::loader::LoadedProject;

pub use super::checked_project::CheckedProject;
use super::hir_project::HirProject;
use super::module_resolve_errors::module_resolve_compile_error;
use super::pipeline;

/// One whole-project compilation session.
///
/// Configuration is accumulated before a terminal `lower`, `check`,
/// `prepare`, or `eval` operation. The host type records whether the session
/// owns callable implementations or signature metadata only.
pub struct ProjectCompiler<'project, Host = crate::host_fns::HostFunctionRegistry> {
    project: &'project LoadedProject,
    cancellation: graphcal_compiler::cancellation::CancellationToken,
    host: Host,
}

impl<'project> ProjectCompiler<'project> {
    /// Start a compilation session with the built-in demo host registry.
    #[must_use]
    pub fn new(project: &'project LoadedProject) -> Self {
        Self {
            project,
            cancellation: graphcal_compiler::cancellation::CancellationToken::unbounded(),
            host: crate::host_fns::demo_registry(),
        }
    }
}

impl<'project, Host> ProjectCompiler<'project, Host> {
    /// Replace the cooperative cancellation capability.
    #[must_use]
    pub fn cancellation(
        mut self,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Self {
        self.cancellation = cancellation.clone();
        self
    }

    /// Lower every module exactly once into the authoritative HIR boundary.
    ///
    /// # Errors
    ///
    /// Returns a loading, elaboration, canonical-resolution, or cancellation
    /// diagnostic. Static type/dimension checks and host verification are not
    /// performed.
    pub fn lower(self) -> Result<HirProject<'project>, CompileError> {
        self.cancellation.checkpoint()?;
        let root_source = self.project.root_file().named_source();
        let module_resolver =
            self.project
                .build_module_resolver()
                .map_err(|error| match error {
                    graphcal_compiler::resolve::error::ModuleResolveError::RecursiveIncludeExpansion {
                        cycle,
                    } => pipeline::recursive_dag_instantiation(self.project, &cycle),
                    error => module_resolve_compile_error(error, root_source),
                })?;
        pipeline::lower_project_perfile(self.project, module_resolver, &self.cancellation)
    }

    pub(crate) const fn cancellation_token(
        &self,
    ) -> &graphcal_compiler::cancellation::CancellationToken {
        &self.cancellation
    }
}

impl<'project> ProjectCompiler<'project, crate::host_fns::HostFunctionRegistry> {
    /// Replace callable host functions for checking and runtime preparation.
    #[must_use]
    pub fn host_fns(mut self, host: &crate::host_fns::HostFunctionRegistry) -> Self {
        self.host = host.clone();
        self
    }

    /// Switch to signature-only host metadata for static checking.
    #[must_use]
    pub fn host_metadata(
        self,
        host: &crate::host_fns::HostFunctionMetadata,
    ) -> ProjectCompiler<'project, crate::host_fns::HostFunctionMetadata> {
        ProjectCompiler {
            project: self.project,
            cancellation: self.cancellation,
            host: host.clone(),
        }
    }

    /// Lower and perform every mandatory static check.
    ///
    /// # Errors
    ///
    /// Returns a compile, plugin-signature, or cancellation diagnostic.
    pub fn check(self) -> Result<CheckedProject, CompileError> {
        let metadata = self.host.metadata();
        self.lower()?.check_with_host_metadata(&metadata)
    }

    pub(crate) const fn callable_host(&self) -> &crate::host_fns::HostFunctionRegistry {
        &self.host
    }
}

impl ProjectCompiler<'_, crate::host_fns::HostFunctionMetadata> {
    /// Lower and check against signature-only host metadata.
    ///
    /// # Errors
    ///
    /// Returns a compile, plugin-signature, or cancellation diagnostic.
    pub fn check(self) -> Result<CheckedProject, CompileError> {
        let metadata = self.host.clone();
        self.lower()?.check_with_host_metadata(&metadata)
    }
}

impl HirProject<'_> {
    /// Consume HIR and perform every mandatory static check.
    ///
    /// # Errors
    ///
    /// Returns a type, dimension, policy, constant, host-signature, or
    /// cancellation diagnostic.
    pub fn check_with_host_metadata(
        self,
        host_metadata: &crate::host_fns::HostFunctionMetadata,
    ) -> Result<CheckedProject, CompileError> {
        pipeline::check_hir_project(self, host_metadata)
    }

    /// Consume HIR and check it against one executable host registry.
    ///
    /// # Errors
    ///
    /// Returns a static-check or host-signature diagnostic.
    pub fn check_with_host_fns(
        self,
        host_fns: &crate::host_fns::HostFunctionRegistry,
    ) -> Result<CheckedProject, CompileError> {
        self.check_with_host_metadata(&host_fns.metadata())
    }
}

/// Compile a loaded project into the reusable checked-program boundary.
///
/// # Errors
///
/// Returns a compile diagnostic when any project module is invalid.
pub fn check_project(project: &LoadedProject) -> Result<CheckedProject, CompileError> {
    ProjectCompiler::new(project).check()
}

/// Test-only projection of a fully checked project into its TIR.
#[cfg(test)]
pub fn compile_to_tir_from_project(
    project: &LoadedProject,
) -> Result<graphcal_compiler::tir::typed::CheckedTir, CompileError> {
    check_project(project).map(|checked| checked.compiled.program.into_parts().0)
}

/// Test-only convenience projection from source through [`CheckedProject`].
#[cfg(test)]
pub fn compile_to_tir(
    source: &str,
    name: &str,
) -> Result<graphcal_compiler::tir::typed::CheckedTir, CompileError> {
    let project = LoadedProject::from_source(source, name)?;
    compile_to_tir_from_project(&project)
}

/// Test-only convenience projection for a loaded multi-file project.
#[cfg(test)]
pub fn compile_to_tir_project<F: graphcal_io::FileSystemReader>(
    root_path: &Path,
    project_root: Option<&Path>,
    fs: &F,
) -> Result<(graphcal_compiler::tir::typed::CheckedTir, LoadedProject), CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}
