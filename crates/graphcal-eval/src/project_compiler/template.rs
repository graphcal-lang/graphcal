//! Canonical cache of reusable pre-HIR module and inline-DAG templates.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::ir::lower::UnfrozenIR;
use graphcal_compiler::registry::types::Registry;

/// Stable reference to one reusable file-root or inline-DAG template.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ModuleTemplateRef {
    /// File that owns source provenance and self-import scope.
    pub(super) source_file: DagId,
    /// Exact canonical file-root or inline-DAG template.
    pub(super) dag_id: DagId,
}

impl ModuleTemplateRef {
    pub(super) fn file(dag_id: DagId) -> Self {
        Self {
            source_file: dag_id.clone(),
            dag_id,
        }
    }

    pub(super) fn is_file_root(&self) -> bool {
        self.source_file == self.dag_id
    }
}

/// Reusable elaborated pre-HIR template shared by every concrete instance.
#[derive(Debug)]
pub(super) struct ElaboratedModuleTemplate {
    pub(super) unfrozen: UnfrozenIR,
    pub(super) frontend_registry: Registry,
}

/// Project-session cache containing exactly one elaborated template per DAG.
#[derive(Default)]
pub(super) struct ModuleTemplateStore {
    templates: HashMap<DagId, Arc<ElaboratedModuleTemplate>>,
}

impl ModuleTemplateStore {
    pub(super) fn get(&self, dag_id: &DagId) -> Option<Arc<ElaboratedModuleTemplate>> {
        self.templates.get(dag_id).map(Arc::clone)
    }

    pub(super) fn insert(
        &mut self,
        dag_id: DagId,
        template: ElaboratedModuleTemplate,
    ) -> Arc<ElaboratedModuleTemplate> {
        match self.templates.entry(dag_id) {
            std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                Arc::clone(entry.insert(Arc::new(template)))
            }
        }
    }
}
