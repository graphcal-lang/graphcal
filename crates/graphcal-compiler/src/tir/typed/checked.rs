//! The checked project TIR: the final state of the TIR typestate, whose DAGs
//! carry their checked facts, and the immutable stores it publishes.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use crate::dag_id::DagId;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::graphcal_error::GraphcalError;
use crate::hir::nominal::NominalTypeDef;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::semantic::checked_type::IndexTypeRef;
use crate::semantic::index_def::IndexDef;
use crate::semantic::unit_scale::UnitInfo;
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::schedule::{ConstSchedule, RuntimeSchedule};
use crate::tir::texpr::CheckedBodies;

use super::body_scope::BodyScope;
use super::checked_dag::{CheckedDag, PublishedDag};
use super::dag_position::DagPosition;
use super::model::{CheckedDeclType, DagTIR, ProjectTypeStore, TirCore};

use super::program::{TirRead, UncheckedTir};

/// Registry of the checked DAGs of one file and every DAG it imports.
///
/// The root DAG is stored directly, so its presence is structural. Every other
/// entry is keyed from its own [`DagTIR::dag_id`]; the API exposes no
/// insertion, removal, or mutation.
///
/// Every DAG has a [`DagPosition`]: the root first, then the local DAGs,
/// then the imported ones, each in [`DagId`] order, so a program numbers its
/// DAGs the same way in every run.
///
/// The registry is closed under calls: every DAG a body calls is in the
/// registry, and the callee of each call slot of each body is resolved to
/// its position once, when the registry is built.
#[derive(Debug, Clone)]
pub struct CheckedDagRegistry {
    pub(super) root: CheckedDag,
    pub(super) other_dags: IndexMap<DagId, CheckedDag>,
    /// Immutable bodies imported from an already-frozen module store.
    shared_dags: IndexMap<DagId, Arc<CheckedDag>>,
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
    pub(super) fn close(
        root: CheckedDag,
        other_dags: IndexMap<DagId, CheckedDag>,
        shared_dags: IndexMap<DagId, Arc<CheckedDag>>,
    ) -> Result<Self, UnresolvedCallee> {
        let mut registry = Self {
            root,
            other_dags,
            shared_dags,
            callees: Vec::new(),
        };
        registry.callees = registry
            .values()
            .map(|caller| {
                caller
                    .call_targets()
                    .iter()
                    .map(|(_, target)| {
                        registry
                            .get_positioned(target)
                            .map(|(position, _)| position)
                            .ok_or_else(|| UnresolvedCallee {
                                caller: caller.dag_id().clone(),
                                target: target.clone(),
                            })
                    })
                    .collect()
            })
            .collect::<Result<_, _>>()?;
        Ok(registry)
    }

    /// The scope of one DAG, to select its bodies in.
    pub(super) fn scope(&self, dag_id: &DagId) -> Option<BodyScope<'_>> {
        self.get_positioned(dag_id)
            .map(|(position, dag)| BodyScope::of(dag, position, &self.callees[position.index()]))
    }

    /// The scope of the root DAG.
    pub(super) fn root_scope(&self) -> BodyScope<'_> {
        BodyScope::of(
            &self.root,
            DagPosition::ROOT,
            &self.callees[DagPosition::ROOT.index()],
        )
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
        if dag_id == self.root_id() {
            return Some((DagPosition::ROOT, &self.root));
        }
        let locals = self.other_dags.len();
        self.other_dags
            .get_full(dag_id)
            .map(|(position, _, dag)| (DagPosition::new(1 + position), dag))
            .or_else(|| {
                self.shared_dags.get_full(dag_id).map(|(position, _, dag)| {
                    (DagPosition::new(1 + locals + position), dag.as_ref())
                })
            })
    }

    /// Every DAG with its position, in position order.
    pub fn positioned(&self) -> impl Iterator<Item = (DagPosition, &CheckedDag)> {
        self.values()
            .enumerate()
            .map(|(position, dag)| (DagPosition::new(position), dag))
    }

    /// Canonical identity of this registry's root DAG.
    #[must_use]
    pub const fn root_id(&self) -> &DagId {
        &self.root.body.dag_id
    }

    /// Borrow the root DAG.
    #[must_use]
    pub const fn root(&self) -> &CheckedDag {
        &self.root
    }

    /// Look up one DAG by canonical identity.
    #[must_use]
    pub fn get(&self, dag_id: &DagId) -> Option<&CheckedDag> {
        if dag_id == self.root_id() {
            Some(&self.root)
        } else {
            self.other_dags
                .get(dag_id)
                .or_else(|| self.shared_dags.get(dag_id).map(AsRef::as_ref))
        }
    }

    /// Iterate over the identities and bodies this file owns.
    pub(crate) fn local_iter(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        std::iter::once((self.root_id(), &self.root)).chain(self.other_dags.iter())
    }

    /// Iterate over canonical identities and DAG bodies.
    pub fn iter(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        self.local_iter()
            .chain(self.shared_dags.iter().map(|(id, dag)| (id, dag.as_ref())))
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
    pub fn len(&self) -> usize {
        self.other_dags.len() + self.shared_dags.len() + 1
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
        if index == self.root_id() {
            &self.root
        } else {
            self.other_dags
                .get(index)
                .unwrap_or_else(|| self.shared_dags[index].as_ref())
        }
    }
}

impl UncheckedTir {
    /// Pair every local body with the facts its check published: the only
    /// construction of a [`CheckedTir`].
    ///
    pub(crate) fn into_checked(
        self,
        parts: CheckedParts,
        src: &NamedSource<Arc<String>>,
    ) -> Result<CheckedTir, GraphcalError> {
        let CheckedParts {
            mut bodies,
            mut presentation,
            schedules,
        } = parts;
        let CheckedSchedules {
            constants,
            mut callables,
        } = schedules;
        let (core, dags) = self.into_parts();
        let (root, other_dags, shared_dags) = dags.into_parts();
        let internal = |message: String| {
            GraphcalError::internal_error(message, src, DiagnosticAnchor::WholeFile)
        };
        let mut check = |body: DagTIR| {
            let missing =
                |what: &str| internal(format!("DAG `{}` has no checked {what}", body.dag_id()));
            let published = PublishedDag {
                bodies: bodies
                    .remove(body.dag_id())
                    .ok_or_else(|| missing("typed bodies"))?,
                presentation: presentation
                    .remove(body.dag_id())
                    .ok_or_else(|| missing("presentation facts"))?,
                runtime_schedule: callables
                    .remove(body.dag_id())
                    .ok_or_else(|| missing("runtime schedule"))?,
            };
            CheckedDag::new(body, published, src)
        };
        let root = check(root)?;
        let other_dags = other_dags
            .into_iter()
            .map(|(id, body)| check(body).map(|dag| (id, dag)))
            .collect::<Result<_, GraphcalError>>()?;
        // Every call target is in the registry: checking resolved each local
        // body's calls against it, an instance calls its template's targets
        // and its checked defaults', and installing an imported store
        // required the stores it calls into. Failing here is a compiler bug.
        let dags = CheckedDagRegistry::close(root, other_dags, shared_dags.into_iter().collect())
            .map_err(|error| internal(error.to_string()))?;
        Ok(CheckedTir {
            core,
            dags,
            const_schedule: constants,
        })
    }
}

/// The schedules one check computed: the local DAGs' constants, and each
/// local DAG as a callable.
pub(crate) struct CheckedSchedules {
    pub(crate) constants: ConstSchedule,
    pub(crate) callables: HashMap<DagId, RuntimeSchedule>,
}

/// Everything one check published for the local bodies of a TIR, keyed by
/// body; paired with the bodies only by
/// [`UncheckedTir::into_checked`].
pub(crate) struct CheckedParts {
    /// The checked trees of every local body.
    pub(crate) bodies: HashMap<DagId, CheckedBodies>,
    pub(crate) presentation: HashMap<DagId, DagPresentationFacts>,
    pub(crate) schedules: CheckedSchedules,
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
    pub const fn root(&self) -> &CheckedDag {
        self.dags.root()
    }

    /// Canonical identity of the root DAG.
    #[must_use]
    pub const fn root_dag_id(&self) -> &DagId {
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
