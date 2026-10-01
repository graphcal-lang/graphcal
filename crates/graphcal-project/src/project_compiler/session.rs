//! Public whole-project checking session and validated continuation.

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::outcome::{Cancellable, CancellationMode, Outcome, Uncancellable};

use crate::compile_error::{CompileError, PipelineError};
use crate::loader::loaded_project::LoadedProject;

pub use super::checked_project::CheckedProject;
use super::hir_project::HirProject;
use super::module_resolve_errors::module_resolve_compile_error;
use super::pipeline;

/// One whole-project compilation session.
///
/// Configuration is accumulated before a terminal `lower`, `check`,
/// `prepare`, or `eval` operation. The host type records whether the session
/// owns callable implementations or signature metadata only; the mode records
/// whether the embedding shell can cancel it, and so whether its operations
/// fail with an [`Outcome`] or with a
/// [`CompileError`] alone.
pub struct ProjectCompiler<
    'project,
    Host = graphcal_eval::host_fns::HostFunctionRegistry,
    Mode = Uncancellable,
> {
    project: &'project LoadedProject,
    mode: Mode,
    host: Host,
}

impl<'project> ProjectCompiler<'project> {
    /// Start a compilation session with the built-in demo host registry.
    #[must_use]
    pub fn new(project: &'project LoadedProject) -> Self {
        Self {
            project,
            mode: Uncancellable,
            host: graphcal_eval::host_fns::demo_registry(),
        }
    }
}

impl<'project, Host> ProjectCompiler<'project, Host> {
    /// Observe `cancellation`: the session's operations then report
    /// cancellation as
    /// [`Outcome::Cancelled`].
    #[must_use]
    pub fn cancellation(
        self,
        cancellation: &CancellationToken,
    ) -> ProjectCompiler<'project, Host, Cancellable> {
        ProjectCompiler {
            project: self.project,
            mode: Cancellable(cancellation.clone()),
            host: self.host,
        }
    }
}

impl<'project, Host, Mode: CancellationMode> ProjectCompiler<'project, Host, Mode> {
    /// Lower every module exactly once into the authoritative HIR boundary.
    ///
    /// # Errors
    ///
    /// Returns a loading, elaboration, canonical-resolution, or (for a
    /// cancellable session) cancellation failure. Static type/dimension
    /// checks and host verification are not performed.
    pub fn lower(self) -> Result<HirProject<'project, Mode>, Mode::Failure<CompileError>> {
        let Self { project, mode, .. } = self;
        mode.clone().run(|cancellation| {
            lower_project(project, mode, cancellation)
                .map_err(|outcome| outcome.map_failed(|error| error.render(project.sources())))
        })
    }

    /// The session's cancellation.
    pub(crate) const fn mode(&self) -> &Mode {
        &self.mode
    }
}

impl<'project, Mode>
    ProjectCompiler<'project, graphcal_eval::host_fns::HostFunctionRegistry, Mode>
{
    /// Replace callable host functions for checking and runtime preparation.
    #[must_use]
    pub fn host_fns(mut self, host: &graphcal_eval::host_fns::HostFunctionRegistry) -> Self {
        self.host = host.clone();
        self
    }

    /// Switch to signature-only host metadata for static checking.
    #[must_use]
    pub fn host_metadata(
        self,
        host: &graphcal_eval::host_fns::HostFunctionMetadata,
    ) -> ProjectCompiler<'project, graphcal_eval::host_fns::HostFunctionMetadata, Mode> {
        ProjectCompiler {
            project: self.project,
            mode: self.mode,
            host: host.clone(),
        }
    }

    pub(crate) const fn callable_host(&self) -> &graphcal_eval::host_fns::HostFunctionRegistry {
        &self.host
    }
}

impl<Mode: CancellationMode>
    ProjectCompiler<'_, graphcal_eval::host_fns::HostFunctionRegistry, Mode>
{
    /// Lower and perform every mandatory static check.
    ///
    /// # Errors
    ///
    /// Returns a compile or plugin-signature diagnostic, or (for a cancellable
    /// session) cancellation.
    pub fn check(self) -> Result<CheckedProject, Mode::Failure<CompileError>> {
        let metadata = self.host.metadata();
        self.lower()?.check_with_host_metadata(&metadata)
    }
}

impl<Mode: CancellationMode>
    ProjectCompiler<'_, graphcal_eval::host_fns::HostFunctionMetadata, Mode>
{
    /// Lower and check against signature-only host metadata.
    ///
    /// # Errors
    ///
    /// Returns a compile or plugin-signature diagnostic, or (for a cancellable
    /// session) cancellation.
    pub fn check(self) -> Result<CheckedProject, Mode::Failure<CompileError>> {
        let metadata = self.host.clone();
        self.lower()?.check_with_host_metadata(&metadata)
    }
}

/// Lower every module of `project` into the authoritative HIR boundary.
fn lower_project<'project, Mode>(
    project: &'project LoadedProject,
    mode: Mode,
    cancellation: &CancellationToken,
) -> Result<HirProject<'project, Mode>, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    let root_source = project.root_file().source_id();
    let module_resolver = project
        .build_module_resolver()
        .map_err(|error| match error {
            graphcal_compiler::resolve::error::ModuleResolveError::RecursiveIncludeExpansion {
                cycle,
            } => pipeline::recursive_dag_instantiation(project, &cycle),
            error => module_resolve_compile_error(error, root_source),
        })?;
    pipeline::lower_project_perfile(project, module_resolver, mode, cancellation)
}

impl<Mode: CancellationMode> HirProject<'_, Mode> {
    /// Consume HIR and perform every mandatory static check, observing the
    /// cancellation of the session that lowered it.
    ///
    /// # Errors
    ///
    /// Returns a type, dimension, policy, constant, or host-signature
    /// diagnostic, or (for a cancellable session) cancellation.
    pub fn check_with_host_metadata(
        self,
        host_metadata: &graphcal_eval::host_fns::HostFunctionMetadata,
    ) -> Result<CheckedProject, Mode::Failure<CompileError>> {
        let mode = self.mode.clone();
        let sources = std::sync::Arc::clone(&self.sources);
        mode.run(|cancellation| {
            pipeline::check_hir_project(self, host_metadata, cancellation)
                .map_err(|outcome| outcome.map_failed(|error| error.render(&sources)))
        })
    }

    /// Consume HIR and check it against one executable host registry.
    ///
    /// # Errors
    ///
    /// Returns a static-check or host-signature diagnostic, or (for a
    /// cancellable session) cancellation.
    pub fn check_with_host_fns(
        self,
        host_fns: &graphcal_eval::host_fns::HostFunctionRegistry,
    ) -> Result<CheckedProject, Mode::Failure<CompileError>> {
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
