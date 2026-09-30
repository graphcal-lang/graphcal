//! The checked-project boundary shared by checking, graph projection, the
//! LSP, and runtime preparation.

use std::sync::Arc;

use miette::NamedSource;

use super::model::CompiledFile;

/// A fully checked project that has not yet been prepared or evaluated.
///
/// This is the reusable semantic boundary shared by `check`, graph projection,
/// the LSP, and runtime preparation. Private fields prevent callers from
/// fabricating a value that skipped mandatory checks.
pub struct CheckedProject {
    pub(super) compiled: CompiledFile,
    pub(super) source: NamedSource<Arc<String>>,
    pub(super) module_resolver: graphcal_compiler::resolve::ModuleResolver,
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
            module_resolver: self.module_resolver,
        }
    }
}

/// Checked semantic products needed to construct a runtime plan.
pub struct CheckedProjectRuntimeParts {
    pub(crate) compiled: CompiledFile,
    pub(crate) source: NamedSource<Arc<String>>,
    pub(crate) module_resolver: graphcal_compiler::resolve::ModuleResolver,
}
