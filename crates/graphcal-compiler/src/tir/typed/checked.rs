//! The checked project TIR: the final state of the TIR typestate, whose DAGs
//! carry their checked facts, and the immutable stores it publishes.

use crate::dag_id::DagId;
use crate::diagnostic_anchor::DiagnosticAnchor;
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
use crate::tir::schedule::ConstSchedule;
use crate::tir::texpr::CheckedBodies;

use super::body_scope::BodyScope;
use super::checked_dag::{CheckedDag, PublishedDag};
use super::dag_position::DagPosition;
use super::dag_slots::{DagSlots, LocalDagFacts};
use super::model::{CheckedDeclType, DagTIR, ProjectTypeStore, TirCore};

use super::program::{TirRead, UncheckedTir};

/// Registry of the checked DAGs of one file and every DAG it imports.
///
/// Every DAG keeps the [`DagPosition`] it was given when it joined the draft
/// this registry was checked from. The root is at [`DagPosition::ROOT`], so
/// its presence is structural; the API exposes no insertion, removal, or
/// mutation. Iteration visits the root, then the other local DAGs, then the
/// imported ones, each in [`DagId`] order.
///
/// The registry is closed under calls: every DAG a body calls is in the
/// registry, and the callee of each call slot of each body is resolved to
/// its position once, when the registry is built.
#[derive(Debug, Clone)]
pub struct CheckedDagRegistry {
    pub(super) dags: DagSlots<CheckedDag>,
    /// The position of the callee of each call slot, by caller position.
    callees: Vec<Box<[DagPosition]>>,
}

/// A body calls a DAG its registry does not have.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("DAG `{caller}` calls DAG `{target}`, which is not in its program")]
pub(super) struct UnresolvedCallee {
    caller: DagId,
    target: DagId,
}

impl CheckedDagRegistry {
    /// Close a registry over its DAGs, resolving every call slot of every
    /// body to the position of its callee.
    ///
    /// # Errors
    ///
    /// Returns [`UnresolvedCallee`] when a body calls a DAG outside the
    /// registry.
    pub(super) fn close(dags: DagSlots<CheckedDag>) -> Result<Self, UnresolvedCallee> {
        let callees = dags
            .positioned()
            .map(|(_, caller)| {
                caller
                    .call_targets()
                    .iter()
                    .map(|(_, target)| {
                        dags.position(target).ok_or_else(|| UnresolvedCallee {
                            caller: caller.dag_id().clone(),
                            target: target.clone(),
                        })
                    })
                    .collect()
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { dags, callees })
    }

    /// The scope of one DAG, to select its bodies in.
    pub(super) fn scope(&self, dag_id: &DagId) -> Option<BodyScope<'_>> {
        self.dags
            .position(dag_id)
            .map(|position| self.scope_at(position))
    }

    /// The scope of the DAG at `position`.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another registry with more
    /// DAGs.
    pub(super) fn scope_at(&self, position: DagPosition) -> BodyScope<'_> {
        BodyScope::of(
            self.dags.at(position),
            position,
            &self.callees[position.index()],
        )
    }

    /// The scope of the root DAG.
    pub(super) fn root_scope(&self) -> BodyScope<'_> {
        self.scope_at(DagPosition::ROOT)
    }

    /// The positions of the callees of the body at `caller`, by call slot.
    ///
    /// # Panics
    ///
    /// Panics when `caller` is a position of another registry with more
    /// DAGs.
    #[must_use]
    pub fn callee_positions(&self, caller: DagPosition) -> &[DagPosition] {
        &self.callees[caller.index()]
    }

    /// The position and body of one DAG.
    #[must_use]
    pub fn get_positioned(&self, dag_id: &DagId) -> Option<(DagPosition, &CheckedDag)> {
        self.dags
            .position(dag_id)
            .map(|position| (position, self.dags.at(position)))
    }

    /// The DAG at `position`.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another registry with more
    /// DAGs.
    #[must_use]
    pub fn at(&self, position: DagPosition) -> &CheckedDag {
        self.dags.at(position)
    }

    /// Every DAG with its position, in position order.
    pub fn positioned(&self) -> impl Iterator<Item = (DagPosition, &CheckedDag)> {
        self.dags.positioned()
    }

    /// Canonical identity of this registry's root DAG.
    #[must_use]
    pub fn root_id(&self) -> &DagId {
        self.dags.root_id()
    }

    /// Borrow the root DAG.
    #[must_use]
    pub fn root(&self) -> &CheckedDag {
        self.dags.root()
    }

    /// Look up one DAG by canonical identity.
    #[must_use]
    pub fn get(&self, dag_id: &DagId) -> Option<&CheckedDag> {
        self.dags.get(dag_id)
    }

    /// Iterate over the identities and bodies this file owns.
    pub(crate) fn local_iter(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        self.dags.local_iter()
    }

    /// Iterate over canonical identities and DAG bodies.
    pub fn iter(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        self.dags.iter()
    }

    /// Iterate over canonical DAG identities.
    pub fn keys(&self) -> impl Iterator<Item = &DagId> {
        self.iter().map(|(id, _)| id)
    }

    /// Iterate over DAG bodies.
    pub fn values(&self) -> impl Iterator<Item = &CheckedDag> {
        self.iter().map(|(_, dag)| dag)
    }

    /// Number of DAG modules in this registry, including its root and imports.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.dags.len()
    }

    /// A checked registry is never empty because construction requires a root.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl std::ops::Index<&DagId> for CheckedDagRegistry {
    type Output = CheckedDag;

    fn index(&self, index: &DagId) -> &Self::Output {
        &self.dags[index]
    }
}

impl UncheckedTir {
    /// Pair every local body with the facts its check published for it: the
    /// only construction of a [`CheckedTir`].
    ///
    /// `published` was mapped from this TIR's own registry, so each body
    /// takes its facts without a lookup, and keeps its position.
    pub(crate) fn into_checked(
        self,
        constants: ConstSchedule,
        published: LocalDagFacts<PublishedDag>,
        src: SourceId,
    ) -> Result<CheckedTir, SemanticError> {
        let (core, dags) = self.into_parts();
        let dags = dags.zip_locals(published, CheckedDag::new);
        // Every call target is in the registry: checking resolved each local
        // body's calls against it, an instance calls its template's targets
        // and its checked defaults', and installing an imported store
        // required the stores it calls into. Failing here is a compiler bug.
        let dags = CheckedDagRegistry::close(dags).map_err(|error| {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
        Ok(CheckedTir {
            core,
            dags,
            const_schedule: constants,
        })
    }
}

/// The checked project TIR: the final state of the TIR typestate and the only
/// one evaluation and the language server see.
///
/// Created only by [`InstantiatedTir::check`](super::program::InstantiatedTir::check).
/// It is immutable: safe clients can inspect but cannot remove, replace, or
/// re-key its DAGs, and every DAG carries its checked facts.
#[derive(Debug, Clone)]
pub struct CheckedTir {
    pub(super) core: TirCore,
    pub(super) dags: CheckedDagRegistry,
    const_schedule: ConstSchedule,
}

impl CheckedTir {
    /// Borrow the root DAG. Root presence is guaranteed by [`CheckedDagRegistry`].
    #[must_use]
    pub fn root(&self) -> &CheckedDag {
        self.dags.root()
    }

    /// Canonical identity of the root DAG.
    #[must_use]
    pub fn root_dag_id(&self) -> &DagId {
        self.dags.root_id()
    }

    /// Evaluation order of the local DAGs' constants.
    #[must_use]
    pub const fn const_schedule(&self) -> &ConstSchedule {
        &self.const_schedule
    }

    /// Borrow the root file's immutable post-resolution formatting services.
    #[must_use]
    pub const fn registry(&self) -> &FormattingRegistry {
        self.core.registry()
    }

    /// Borrow the authoritative owner-qualified project type store.
    #[must_use]
    pub fn project_type_store(&self) -> &ProjectTypeStore {
        self.core.project_type_store()
    }

    /// Borrow every local and imported DAG through the read-only checked registry.
    #[must_use]
    pub const fn dag_registry(&self) -> &CheckedDagRegistry {
        &self.dags
    }

    /// Iterate over every DAG owned by this file, including the root and all
    /// nested descendants. Inline `dag` syntax and file modules use the same
    /// canonical representation and traversal.
    pub fn local_dags(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        let root = self.root_dag_id();
        self.dags
            .local_iter()
            .filter(move |(dag_id, _)| *dag_id == root || dag_id.is_descendant_of(root))
    }

    /// Find the checked DAG that owns a declaration record.
    ///
    /// Every record is stored in the DAG named by its canonical owner, so
    /// this is a keyed lookup rather than a search.
    #[must_use]
    pub fn dag_containing_declaration(
        &self,
        declaration: &ResolvedDeclName,
    ) -> Option<&CheckedDag> {
        self.dags
            .get(declaration.owner())
            .filter(|dag| dag.value_decl_type(declaration).is_some())
    }

    /// The checked type of any value declaration in the project.
    #[must_use]
    pub fn decl_type(&self, declaration: &ResolvedDeclName) -> Option<&CheckedDeclType> {
        self.dags
            .get(declaration.owner())?
            .value_decl_type(declaration)
    }

    /// Borrow resolved extern function signatures.
    #[must_use]
    pub const fn extern_functions(&self) -> &super::model::ExternFunctions {
        self.core.extern_functions()
    }

    /// Look up a dimension by its canonical defining-module identity.
    #[must_use]
    pub fn dimension(&self, name: &ResolvedDimName) -> Option<&Dimension> {
        self.core.dimension(name)
    }

    /// Look up a unit by its canonical defining-module identity.
    #[must_use]
    pub fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.core.unit_info(name)
    }

    /// Look up a declared index by its canonical defining-module identity.
    #[must_use]
    pub fn declared_index_def(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.core.declared_index_def(name)
    }

    /// Resolve a declared axis or derive a structural axis from its cardinality.
    #[must_use]
    pub fn index_def<V: crate::semantic::checked_type::Concreteness>(
        &self,
        index: &IndexTypeRef<V>,
    ) -> Option<std::borrow::Cow<'_, IndexDef>> {
        self.core.index_def(index)
    }

    /// Look up a nominal type by its canonical defining-module identity.
    #[must_use]
    pub fn struct_type_def(&self, name: &ResolvedStructTypeName) -> Option<&NominalTypeDef> {
        self.core.struct_type_def(name)
    }

    /// Iterate every canonical nominal definition in the project type store.
    pub fn nominal_type_defs(
        &self,
    ) -> impl Iterator<Item = (&ResolvedStructTypeName, &NominalTypeDef)> {
        self.core.nominal_type_defs()
    }

    /// Iterate canonical index definitions owned by the root module.
    pub fn root_declared_indexes(&self) -> impl Iterator<Item = &IndexDef> {
        self.core.declared_indexes_of(self.root_dag_id())
    }

    /// Returns true if this file declares any required param or required index.
    #[must_use]
    pub fn is_library(&self) -> bool {
        self.root().params().any(|param| param.default.is_none())
            || self
                .root_declared_indexes()
                .any(crate::semantic::index_def::IndexDef::is_required)
    }
}

impl TirRead for CheckedTir {
    fn core(&self) -> &TirCore {
        &self.core
    }

    fn root(&self) -> &DagTIR {
        self.dags.root().body()
    }

    fn dag(&self, dag_id: &DagId) -> Option<&DagTIR> {
        self.dags.get(dag_id).map(|dag| &dag.body)
    }

    fn dag_bodies(&self) -> Box<dyn Iterator<Item = &DagTIR> + '_> {
        Box::new(self.dags.values().map(|dag| &dag.body))
    }

    fn checked_bodies(&self, dag_id: &DagId) -> Option<&CheckedBodies> {
        self.dags.get(dag_id).map(CheckedDag::bodies)
    }
}

impl crate::semantic::index_axis::IndexAxis {
    /// Resolve `index` to its concrete definition in `tir`.
    ///
    /// Returns `None` when the index is unknown, still required (not bound
    /// to a concrete definition), or a coordinate index with a non-finite
    /// coordinate.
    #[must_use]
    pub fn resolve(tir: &CheckedTir, index: &IndexTypeRef) -> Option<Self> {
        let definition = tir.index_def(index)?;
        let kind = definition.concrete()?.clone();
        Self::from_concrete(index.clone(), kind)
    }
}
