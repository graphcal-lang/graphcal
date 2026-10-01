//! Authoritative whole-project HIR boundary.

use std::collections::{HashMap, HashSet};

use graphcal_compiler::outcome::Uncancellable;
use graphcal_compiler::{dag_id::DagId, syntax::dimension::UnitName};

use super::model::HirFile;
use crate::dependency_ordered::DependencyOrdered;

/// A complete canonically resolved project before static checking.
///
/// Construction is private to [`super::ProjectCompiler`]. A value of this type
/// guarantees that every loaded file and inline DAG has exactly one frozen HIR
/// body, every value-declaration type annotation and domain bound is HIR, every
/// expression reference is canonical, and every cross-module HIR interface
/// required by a dependent module was available during lowering.
/// It carries no checked TIR, runtime values, execution plan, or host metadata.
/// `Mode` is the cancellation of the session that lowered it, which its
/// static checks keep observing.
pub struct HirProject<'project, Mode = Uncancellable> {
    /// One HIR file per loaded source file, in the loader's dependency order
    /// and ending with the root file.
    pub(super) files: DependencyOrdered<HirFile>,
    /// Loader-owned plugin verification inputs are borrowed narrowly; source
    /// ASTs and the rest of `LoadedProject` do not cross the HIR boundary.
    pub(super) plugins: &'project HashMap<
        graphcal_compiler::plugin_identity::PluginIdentity,
        crate::loader::loaded_project::PluginFileEntry,
    >,
    /// Minimal semantic fact needed to reject runtime units at a pure import
    /// boundary. Full frontend registries are discarded after HIR lowering.
    pub(super) exported_runtime_units: HashMap<DagId, HashSet<UnitName>>,
    pub(super) module_resolver: graphcal_compiler::resolve::ModuleResolver,
    pub(super) mode: Mode,
}

impl<Mode> std::fmt::Debug for HirProject<'_, Mode> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut modules = self
            .files
            .iter()
            .map(|file| (file.root.dag_id(), file))
            .collect::<Vec<_>>();
        modules.sort_by_key(|(dag_id, _)| *dag_id);
        formatter
            .debug_struct("HirProject")
            .field("root", self.root())
            .field("modules", &modules)
            .finish()
    }
}

impl<Mode> HirProject<'_, Mode> {
    /// Canonical identity of the entry DAG.
    #[must_use]
    pub const fn root(&self) -> &DagId {
        self.files.root().root.dag_id()
    }

    /// Number of physical modules represented by this HIR project.
    #[must_use]
    pub const fn module_count(&self) -> usize {
        self.files.len()
    }

    /// Number of file-root and inline-DAG HIR modules in the project.
    #[must_use]
    pub fn dag_count(&self) -> usize {
        self.files
            .iter()
            .map(|file| 1usize.saturating_add(file.inline_dags.len()))
            .sum()
    }
}
