//! The project TIR before it is checked: the DAG registry and the draft,
//! unchecked, and instantiated states of the TIR typestate.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::hir::nominal::NominalTypeDef;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::semantic::checked_type::IndexTypeRef;
use crate::semantic::index_def::IndexDef;
use crate::semantic::unit_scale::UnitInfo;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;

use super::model::{
    CheckedDeclType, CompetingExternFunctionDefinition, DagTIR, ProjectTypeStore,
    ProjectTypeStoreInsertError, TirCore,
};

// ---------------------------------------------------------------------------
// DAG registry
// ---------------------------------------------------------------------------

pub use super::dag_slots::DagRegistryError;

/// Failure to attach one immutable module store to an assembly registry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagStoreInsertError {
    /// The store contains a body identity already owned by the assembly.
    #[error(transparent)]
    Registry(#[from] DagRegistryError),
    /// The store contains a conflicting instance-specialized unit.
    #[error("runtime unit `{identity}` has competing checked definitions")]
    CompetingRuntimeUnit { identity: ResolvedUnitName },
    /// A body of the store calls a DAG the assembly would not have.
    #[error("DAG `{caller}` calls DAG `{target}`, which no installed store or local body has")]
    MissingCallee {
        caller: crate::dag_id::DagId,
        target: crate::dag_id::DagId,
    },
}

/// Registry of the DAG bodies of a TIR before it is checked: owned local
/// bodies without facts, and imported checked handles, each at the position
/// it was given when it joined the draft.
pub(crate) type DagRegistry = super::dag_slots::DagSlots<DagTIR>;

/// A project TIR under assembly: the first state of the TIR typestate.
///
/// Type resolution creates a draft with a mandatory root DAG
/// ([`Self::resolve_root`]). Project checking then adds same-file inline DAGs
/// ([`Self::add_inline_dag`]), imported module stores, and imported extern
/// signatures. [`Self::instantiate`] consumes the draft into an
/// [`InstantiatedTir`], whose only transition is
/// [`InstantiatedTir::check`] into a [`CheckedTir`](super::checked::CheckedTir).
#[derive(Debug, Clone)]
pub struct TirDraft {
    core: TirCore,
    dags: DagRegistry,
}

impl TirDraft {
    pub(in crate::tir::typed) fn new(
        registry: FormattingRegistry,
        project_types: Arc<ProjectTypeStore>,
        root: DagTIR,
        extern_functions: HashMap<
            crate::plugin_identity::ExternFnKey,
            crate::ir::extern_function::ExternFunctionEntry,
        >,
    ) -> Self {
        Self {
            core: TirCore {
                registry,
                project_types,
                runtime_units: HashMap::new(),
                extern_functions,
            },
            dags: DagRegistry::new(root),
        }
    }

    /// Borrow the file-root DAG during assembly.
    #[must_use]
    pub fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    /// Mutably borrow the file-root DAG during assembly.
    #[cfg(test)]
    pub(crate) fn root_mut(&mut self) -> &mut DagTIR {
        self.dags
            .localized_mut(super::dag_position::DagPosition::ROOT)
    }

    /// Borrow the root file's post-resolution formatting services.
    #[must_use]
    pub const fn registry(&self) -> &FormattingRegistry {
        self.core.registry()
    }

    /// Borrow the owner-qualified type store accumulated for this project.
    #[must_use]
    pub fn project_type_store(&self) -> &ProjectTypeStore {
        self.core.project_type_store()
    }

    /// Share the owner-qualified type store with a DAG resolved into this draft.
    pub(in crate::tir::typed) fn project_types(&self) -> Arc<ProjectTypeStore> {
        Arc::clone(&self.core.project_types)
    }

    /// Add a compiled DAG under its own canonical identity.
    ///
    /// # Errors
    ///
    /// Returns [`DagRegistryError::DuplicateDag`] rather than replacing an
    /// existing root, child, or dependency DAG.
    pub(crate) fn insert_dag(&mut self, dag: DagTIR) -> Result<(), DagRegistryError> {
        self.dags.push_local(dag).map(|_| ())
    }

    /// Add immutable checked bodies from previously frozen module stores.
    ///
    /// Only handles are copied. The bodies and their checked semantic facts
    /// remain owned by the stores that published them.
    ///
    /// The imported bodies stay closed under calls: every DAG a body of the
    /// installed stores calls must be a body of this draft or of one of
    /// `stores`, so a store is installed together with (or after) the stores
    /// it calls into. Nothing is installed when a callee is missing.
    ///
    /// # Errors
    ///
    /// Returns [`DagStoreInsertError`] when a store calls a DAG the draft
    /// would not have, repeats a body identity, or has a runtime unit with a
    /// competing definition.
    pub fn install_shared_dag_stores<'s>(
        &mut self,
        stores: impl IntoIterator<Item = &'s super::dag_store::DagStore>,
    ) -> Result<(), DagStoreInsertError> {
        let stores = stores.into_iter().collect::<Vec<_>>();
        let installs = |dag_id: &crate::dag_id::DagId| {
            self.dags.contains(dag_id) || stores.iter().any(|store| store.get(dag_id).is_some())
        };
        if let Some((target, caller)) = stores
            .iter()
            .flat_map(|store| store.external_callees())
            .find(|(target, _)| !installs(target))
        {
            return Err(DagStoreInsertError::MissingCallee {
                caller: caller.clone(),
                target: target.clone(),
            });
        }
        let mut handles = Vec::new();
        for store in stores {
            self.merge_runtime_units(store)?;
            if let Some(dag_id) = store.dags.keys().find(|dag_id| {
                self.dags.contains(dag_id)
                    || handles
                        .iter()
                        .any(|(installed, _): &(&crate::dag_id::DagId, _)| installed == dag_id)
            }) {
                return Err(DagStoreInsertError::Registry(
                    DagRegistryError::DuplicateDag {
                        dag_id: dag_id.clone(),
                    },
                ));
            }
            handles.extend(store.dags.iter());
        }
        // Handles join in identity order, so their positions do not depend
        // on the order of the stores or of the bodies in a store.
        handles.sort_by_key(|(dag_id, _)| *dag_id);
        handles.into_iter().try_for_each(|(_, dag)| {
            self.dags
                .push_shared(Arc::clone(dag))
                .map(|_| ())
                .map_err(DagStoreInsertError::Registry)
        })
    }

    /// Merge the runtime units one store publishes.
    fn merge_runtime_units(
        &mut self,
        store: &super::dag_store::DagStore,
    ) -> Result<(), DagStoreInsertError> {
        if let Some((name, _)) = store.runtime_units.iter().find(|(name, info)| {
            self.core
                .runtime_units
                .get(*name)
                .is_some_and(|existing| existing != *info)
        }) {
            return Err(DagStoreInsertError::CompetingRuntimeUnit {
                identity: name.clone(),
            });
        }
        self.core.runtime_units.extend(
            store
                .runtime_units
                .iter()
                .map(|(name, info)| (name.clone(), Arc::clone(info))),
        );
        Ok(())
    }

    /// Merge one canonical extern signature without replacing an identical copy.
    ///
    /// # Errors
    ///
    /// Returns [`CompetingExternFunctionDefinition`] when the same plugin and
    /// function identity already has a different callable signature.
    pub fn insert_extern_function(
        &mut self,
        key: crate::plugin_identity::ExternFnKey,
        function: crate::ir::extern_function::ExternFunctionEntry,
    ) -> Result<(), CompetingExternFunctionDefinition> {
        match self.core.extern_functions.get(&key) {
            Some(existing) if !existing.has_same_callable_definition(&function) => {
                Err(CompetingExternFunctionDefinition {
                    plugin: key.plugin,
                    name: key.name,
                })
            }
            Some(_) => Ok(()),
            None => {
                self.core.extern_functions.insert(key, function);
                Ok(())
            }
        }
    }

    /// Merge the extern signatures a same-file DAG body declared with its
    /// own `import plugin` blocks.
    ///
    /// Unlike [`Self::insert_extern_function`] (which installs signatures an
    /// already-checked dependency published), these are fresh source
    /// declarations, so they follow the same rule as several declarations in
    /// one body: a structurally equivalent redeclaration is accepted and a
    /// conflicting one is a user-facing diagnostic at the later declaration.
    pub(crate) fn merge_declared_extern_functions(
        &mut self,
        hir: &crate::ir::model::HirDag,
        src: SourceId,
    ) -> Result<(), SemanticError> {
        // Deterministic conflict reporting: earliest declaration first.
        let mut declared: Vec<_> = hir.extern_functions().iter().collect();
        declared.sort_by_key(|(_, function)| function.decl_span.offset());
        declared.into_iter().try_for_each(|(_, function)| {
            crate::ir::extern_function::merge_extern_function(
                &mut self.core.extern_functions,
                function.clone(),
                src,
            )
        })
    }

    /// End assembly: the registry of local and imported bodies is complete.
    pub(crate) fn finish(self) -> UncheckedTir {
        UncheckedTir {
            core: self.core,
            dags: self.dags,
        }
    }
}

/// The representation of a project TIR before it is checked: owned local
/// bodies without facts, plus imported checked handles.
///
/// Only [`TirDraft::finish`] creates one; only
/// [`Self::into_checked`](UncheckedTir::into_checked) turns it into a
/// [`CheckedTir`](super::checked::CheckedTir).
#[derive(Debug, Clone)]
pub(crate) struct UncheckedTir {
    pub(crate) core: TirCore,
    pub(crate) dags: DagRegistry,
}

impl UncheckedTir {
    pub(super) fn insert_materialized_dag(&mut self, dag: DagTIR) -> Result<(), DagRegistryError> {
        self.dags.push_local(dag).map(|_| ())
    }

    pub(crate) fn into_parts(self) -> (TirCore, DagRegistry) {
        (self.core, self.dags)
    }

    /// Borrow the root DAG. Root presence is guaranteed by [`DagRegistry`].
    pub(crate) fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    /// Canonical identity of the root DAG.
    pub(crate) fn root_dag_id(&self) -> &crate::dag_id::DagId {
        self.dags.root_id()
    }

    /// Borrow the root file's post-resolution formatting services.
    pub(crate) const fn registry(&self) -> &FormattingRegistry {
        self.core.registry()
    }

    /// Mutably borrow formatting services while deriving a checking view.
    pub(crate) const fn registry_mut(&mut self) -> &mut FormattingRegistry {
        &mut self.core.registry
    }

    /// Borrow the authoritative owner-qualified project type store.
    pub(crate) fn project_type_store(&self) -> &ProjectTypeStore {
        self.core.project_type_store()
    }

    /// Replace the type store of a derived checking view.
    pub(crate) fn replace_project_types(&mut self, project_types: ProjectTypeStore) {
        self.core.project_types = Arc::new(project_types);
    }

    /// Iterate over every DAG owned by this file, including the root and all
    /// nested descendants. Inline `dag` syntax and file modules use the same
    /// canonical representation and traversal.
    pub(crate) fn local_dags(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
        let root = self.root_dag_id();
        self.dags
            .local_iter()
            .filter(move |(dag_id, _)| *dag_id == root || dag_id.is_descendant_of(root))
    }

    /// Look up a unit by its canonical defining-module identity.
    pub(crate) fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.core.unit_info(name)
    }

    /// Install one instance-owned dynamic unit in the mutable assembly overlay.
    pub(crate) fn insert_runtime_unit(
        &mut self,
        name: ResolvedUnitName,
        info: UnitInfo,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        self.core.insert_runtime_unit(name, info)
    }
}

/// Read access to a project TIR shared by checking (over unchecked local
/// bodies) and checked consumers (over [`CheckedTir`](super::checked::CheckedTir)).
pub(crate) trait TirRead {
    /// Project-wide type services.
    fn core(&self) -> &TirCore;
    /// The file-root DAG body.
    fn root(&self) -> &DagTIR;
    /// One local or imported DAG body.
    fn dag(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR>;
    /// Every local and imported DAG body.
    fn dag_bodies(&self) -> Box<dyn Iterator<Item = &DagTIR> + '_>;
    /// The checked trees already published for one DAG.
    fn checked_bodies(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::texpr::CheckedBodies>;
}

impl dyn TirRead + '_ {
    /// Borrow the root file's post-resolution formatting services.
    pub(crate) fn registry(&self) -> &FormattingRegistry {
        self.core().registry()
    }

    /// Borrow the authoritative owner-qualified project type store.
    pub(crate) fn project_type_store(&self) -> &ProjectTypeStore {
        self.core().project_type_store()
    }

    /// Borrow resolved extern function signatures.
    pub(crate) fn extern_functions(
        &self,
    ) -> &HashMap<
        crate::plugin_identity::ExternFnKey,
        crate::ir::extern_function::ExternFunctionEntry,
    > {
        self.core().extern_functions()
    }

    /// Look up a dimension by its canonical defining-module identity.
    pub(crate) fn dimension(&self, name: &ResolvedDimName) -> Option<&Dimension> {
        self.core().dimension(name)
    }

    /// Look up a unit by its canonical defining-module identity.
    pub(crate) fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.core().unit_info(name)
    }

    /// Look up a declared index by its canonical defining-module identity.
    pub(crate) fn declared_index_def(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.core().declared_index_def(name)
    }

    /// Resolve a declared axis or derive a structural axis from its cardinality.
    pub(crate) fn index_def<V: crate::semantic::checked_type::Concreteness>(
        &self,
        index: &IndexTypeRef<V>,
    ) -> Option<std::borrow::Cow<'_, IndexDef>> {
        self.core().index_def(index)
    }

    /// Look up a nominal type by its canonical defining-module identity.
    pub(crate) fn struct_type_def(&self, name: &ResolvedStructTypeName) -> Option<&NominalTypeDef> {
        self.core().struct_type_def(name)
    }

    /// The checked type of any value declaration in the project.
    pub(crate) fn decl_type(&self, declaration: &ResolvedDeclName) -> Option<&CheckedDeclType> {
        self.dag(declaration.owner())?.value_decl_type(declaration)
    }

    /// Find the DAG carrying resolved field metadata for a nominal type.
    pub(crate) fn dag_with_type_metadata(&self, name: &ResolvedStructTypeName) -> Option<&DagTIR> {
        self.dag_bodies()
            .find(|dag| dag.semantic.type_defs.struct_types.contains_key(name))
    }
}

impl TirRead for UncheckedTir {
    fn core(&self) -> &TirCore {
        &self.core
    }

    fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    fn dag(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        self.dags.get(dag_id)
    }

    fn dag_bodies(&self) -> Box<dyn Iterator<Item = &DagTIR> + '_> {
        Box::new(self.dags.iter().map(|(_, dag)| dag))
    }

    /// Before any local body is checked, only imported bodies have trees.
    fn checked_bodies(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::texpr::CheckedBodies> {
        self.dags
            .shared(dag_id)
            .map(super::checked_dag::CheckedDag::bodies)
    }
}

/// A project TIR whose semantic include edges are materialized as concrete
/// instance DAGs: the second state of the TIR typestate.
///
/// Created only by [`TirDraft::instantiate`]; consumed only by
/// [`Self::check`].
#[derive(Debug)]
pub struct InstantiatedTir {
    pub(crate) tir: UncheckedTir,
}
