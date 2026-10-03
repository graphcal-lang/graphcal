//! The checked-project boundary shared by checking, graph projection, the
//! LSP, and runtime preparation.

use std::collections::HashSet;

use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::module_name::ScopedName;

use super::entry_interface::CheckedEntryInterface;

/// Result of checking one file in project context.
pub struct CompiledFile {
    pub(crate) program: graphcal_eval::checked_program::CheckedProgram,
    pub(crate) entry_interface: CheckedEntryInterface,
    pub(crate) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(crate) output_surface: HashSet<ScopedName>,
    pub(crate) include_debug_names:
        graphcal_compiler::display::include_scope_names::IncludeScopeNames,
}

/// A fully checked project that has not yet been prepared or evaluated.
///
/// This is the reusable semantic boundary shared by `check`, graph projection,
/// the LSP, and runtime preparation. Private fields prevent callers from
/// fabricating a value that skipped mandatory checks.
pub struct CheckedProject {
    pub(super) compiled: CompiledFile,
    pub(super) source: SourceId,
    /// The registry every source id of the checked program resolves in.
    pub(super) sources: std::sync::Arc<graphcal_compiler::source_registry::SourceRegistry>,
    pub(super) module_resolver: graphcal_compiler::resolve::ModuleResolver,
    /// Source module paths of the loaded files.
    pub(super) module_paths: graphcal_compiler::display::module_paths::ModulePaths,
}

impl std::fmt::Debug for CheckedProject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckedProject")
            .field("root", self.compiled.program.tir().root_dag_id())
            .field("modules", &self.compiled.program.tir().dag_registry().len())
            .finish_non_exhaustive()
    }
}

impl CheckedProject {
    /// Borrow the checked typed program.
    #[must_use]
    pub const fn tir(&self) -> &graphcal_compiler::tir::typed::CheckedTir {
        self.compiled.program.tir()
    }

    /// Borrow the canonical resolver built by this compilation session.
    #[must_use]
    pub const fn module_resolver(&self) -> &graphcal_compiler::resolve::ModuleResolver {
        &self.module_resolver
    }

    /// The registry every source id of the checked program resolves in.
    #[must_use]
    pub const fn sources(
        &self,
    ) -> &std::sync::Arc<graphcal_compiler::source_registry::SourceRegistry> {
        &self.sources
    }

    /// Whether the entry DAG still requires runtime inputs.
    #[must_use]
    pub fn is_library(&self) -> bool {
        self.compiled.program.tir().is_library()
    }

    /// Consume the checked semantic result at the runtime-preparation boundary.
    pub(crate) fn into_runtime_parts(self) -> CheckedProjectRuntimeParts {
        CheckedProjectRuntimeParts {
            compiled: self.compiled,
            source: self.source,
            sources: self.sources,
            module_resolver: self.module_resolver,
            module_paths: self.module_paths,
        }
    }
}

/// Checked semantic products needed to construct a runtime plan.
pub struct CheckedProjectRuntimeParts {
    pub(crate) compiled: CompiledFile,
    pub(crate) source: SourceId,
    pub(crate) sources: std::sync::Arc<graphcal_compiler::source_registry::SourceRegistry>,
    pub(crate) module_resolver: graphcal_compiler::resolve::ModuleResolver,
    pub(crate) module_paths: graphcal_compiler::display::module_paths::ModulePaths,
}
