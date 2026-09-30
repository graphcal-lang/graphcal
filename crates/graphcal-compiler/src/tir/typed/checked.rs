//! The checked project TIR: the final state of the TIR typestate, whose DAGs
//! carry their checked facts, and the immutable stores it publishes.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::dag_id::DagId;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::hir::nominal::NominalTypeDef;
use crate::registry::checked_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::types::{FormattingRegistry, IndexDef, UnitInfo};
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::schedule::{ConstSchedule, RuntimeSchedule};
use crate::tir::texpr::CheckedBodies;

use super::model::{
    CheckedDeclType, DagRegistry, DagTIR, ProjectTypeStore, TirCore, TirRead, UncheckedTir,
};

/// A DAG body together with everything its check published: the checked tree
/// of every expression root, presentation facts, and its runtime schedule as a
/// callable.
///
/// A canonical body's trees are the ones its inference emitted; a semantic
/// instance's are its template's, specialized with the instance's Static
/// substitution (their types differ per instance, so they cannot be shared).
///
/// Created only when an [`InstantiatedTir`](super::model::InstantiatedTir) is
/// checked, so its checked trees are always present and cover exactly this
/// body's expression roots.
#[derive(Debug, Clone)]
pub struct CheckedDag {
    body: DagTIR,
    bodies: CheckedBodies,
    presentation: DagPresentationFacts,
    runtime_schedule: RuntimeSchedule,
}

/// The facts one check published for one local body.
struct PublishedDag {
    bodies: CheckedBodies,
    presentation: DagPresentationFacts,
    runtime_schedule: RuntimeSchedule,
}

impl CheckedDag {
    /// Pair a checked body with the facts published for it.
    fn new(
        body: DagTIR,
        published: PublishedDag,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Self, GraphcalError> {
        let internal = |message: String| {
            GraphcalError::internal_error(
                format!("DAG `{}`: {message}", body.dag_id()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        };
        if !published.bodies.cover(body.owned_expression_roots()) {
            return Err(internal(
                "typed bodies do not cover exactly its expression roots".to_owned(),
            ));
        }
        Ok(Self {
            body,
            bodies: published.bodies,
            presentation: published.presentation,
            runtime_schedule: published.runtime_schedule,
        })
    }

    /// The checked tree of every expression root this body owns.
    #[must_use]
    pub const fn bodies(&self) -> &CheckedBodies {
        &self.bodies
    }

    /// Runtime schedule of this DAG as a callable.
    #[must_use]
    pub const fn runtime_schedule(&self) -> &RuntimeSchedule {
        &self.runtime_schedule
    }

    /// Checked structured display and plot-channel presentation facts.
    #[must_use]
    pub const fn presentation(&self) -> &DagPresentationFacts {
        &self.presentation
    }

    /// Look up checked plot-channel presentation facts.
    #[must_use]
    pub fn plot_channel_presentations(
        &self,
        plot: &ResolvedDeclName,
    ) -> Option<&HashMap<crate::syntax::ast::EncodingChannel, crate::plot_shape::PlotChannelShape>>
    {
        self.presentation.plot_channels.get(plot)
    }

    /// The declaration `handle` denotes when this DAG runs the body holding
    /// it.
    ///
    /// The frame is this DAG's own and never leaves it, so a handle is
    /// resolved by the DAG selected to run it, not by a frame its caller
    /// picks.
    #[must_use]
    pub fn resolve(&self, handle: &crate::hir::expr::LocalDecl) -> ResolvedDeclName {
        self.body.frame().resolve(handle)
    }

    /// The unit whose scale `unit` has when this DAG runs the body holding it.
    #[must_use]
    pub fn resolve_unit(&self, unit: &crate::hir::expr::LocalUnit) -> ResolvedUnitName {
        self.body.frame().resolve_unit(unit)
    }

    /// The nominal type `source` stands for when this DAG runs a body naming
    /// it, after the instance's Static type substitution.
    #[must_use]
    pub fn runtime_struct_type(&self, source: &ResolvedStructTypeName) -> ResolvedStructTypeName {
        self.body.frame().struct_type(source)
    }

    /// Release the body, dropping its facts, to re-resolve it in a derived
    /// checking view.
    pub(crate) fn into_body(self) -> DagTIR {
        self.body
    }
}

impl std::ops::Deref for CheckedDag {
    type Target = DagTIR;

    fn deref(&self) -> &DagTIR {
        &self.body
    }
}

/// Registry of the checked DAGs of one file and every DAG it imports.
///
/// The root DAG is stored directly, so its presence is structural. Every other
/// entry is keyed from its own [`DagTIR::dag_id`]; the API exposes no
/// insertion, removal, or mutation.
#[derive(Debug, Clone)]
pub struct CheckedDagRegistry {
    root: CheckedDag,
    other_dags: HashMap<DagId, CheckedDag>,
    /// Immutable bodies imported from an already-frozen module store.
    shared_dags: HashMap<DagId, Arc<CheckedDag>>,
}

impl CheckedDagRegistry {
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

    /// Consume the local checked bodies into immutable handles.
    ///
    /// Imported handles are deliberately not copied into the new store: they
    /// already belong to another canonical store.
    fn freeze_local(
        self,
        runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    ) -> Result<DagStore, DagStoreFreezeError> {
        runtime_units.keys().try_for_each(|identity| {
            self.get(identity.owner()).map(|_| ()).ok_or_else(|| {
                DagStoreFreezeError::MissingUnitOwner {
                    identity: identity.clone(),
                }
            })
        })?;
        let mut dags = self
            .other_dags
            .into_iter()
            .map(|(id, dag)| (id, Arc::new(dag)))
            .collect::<HashMap<_, _>>();
        let root_id = self.root.dag_id().clone();
        dags.insert(root_id, Arc::new(self.root));
        // Imported unit definitions stay in their publishing module, just like
        // imported bodies. Only the final importing TIR needs their lookup index.
        let runtime_units = runtime_units
            .into_iter()
            .filter(|(identity, _)| dags.contains_key(identity.owner()))
            .collect();
        Ok(DagStore {
            dags,
            runtime_units,
        })
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

/// Failure to freeze the locally owned portion of a checked registry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DagStoreFreezeError {
    /// Every runtime unit must have a local or imported defining body.
    #[error("runtime unit `{identity}` has no defining DAG in the assembly registry")]
    MissingUnitOwner { identity: ResolvedUnitName },
}

/// Immutable canonical checked bodies published by one module in a
/// compilation session.
///
/// The store is created by consuming a [`CheckedTir`]. It has no mutation or
/// completion API. Cloning it shares body and unit handles; project artifacts
/// share the complete store through `Arc`.
#[derive(Debug, Clone)]
pub struct DagStore {
    pub(super) dags: HashMap<DagId, Arc<CheckedDag>>,
    pub(super) runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
}

impl DagStore {
    /// Look up a canonical immutable body.
    #[must_use]
    pub fn get(&self, dag_id: &DagId) -> Option<&CheckedDag> {
        self.dags.get(dag_id).map(AsRef::as_ref)
    }

    /// Borrow the canonical body handle for pointer-identity checks and sharing.
    #[must_use]
    pub fn handle(&self, dag_id: &DagId) -> Option<&Arc<CheckedDag>> {
        self.dags.get(dag_id)
    }

    /// Iterate over canonical immutable bodies.
    pub fn iter(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        self.dags.iter().map(|(id, dag)| (id, dag.as_ref()))
    }

    /// Look up a runtime unit overlay owned by this publishing module.
    #[must_use]
    pub fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.runtime_units.get(name).map(AsRef::as_ref)
    }

    /// Number of canonical bodies in this store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dags.len()
    }

    /// Whether this store has no bodies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dags.is_empty()
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
        let mut check = |body: DagTIR| {
            let missing = |what: &str| {
                GraphcalError::internal_error(
                    format!("DAG `{}` has no checked {what}", body.dag_id()),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            };
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
        Ok(CheckedTir {
            core,
            dags: CheckedDagRegistry {
                root,
                other_dags,
                shared_dags,
            },
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

impl DagRegistry {
    /// Add handles to an immutable module store during assembly.
    pub(crate) fn insert_shared_store(
        &mut self,
        store: &DagStore,
    ) -> Result<(), super::model::DagRegistryError> {
        if let Some(dag_id) = store.dags.keys().find(|dag_id| self.contains(dag_id)) {
            return Err(super::model::DagRegistryError::DuplicateDag {
                dag_id: dag_id.clone(),
            });
        }
        store
            .dags
            .iter()
            .for_each(|(dag_id, dag)| self.insert_shared(dag_id.clone(), Arc::clone(dag)));
        Ok(())
    }
}

/// The checked project TIR: the final state of the TIR typestate and the only
/// one evaluation and the language server see.
///
/// Created only by [`InstantiatedTir::check`](super::model::InstantiatedTir::check).
/// It is immutable: safe clients can inspect but cannot remove, replace, or
/// re-key its DAGs, and every DAG carries its checked facts.
#[derive(Debug, Clone)]
pub struct CheckedTir {
    core: TirCore,
    dags: CheckedDagRegistry,
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

    /// Consume this TIR's local bodies into an immutable store. Imported
    /// bodies and runtime units remain owned by their publishing module.
    ///
    /// # Errors
    ///
    /// Returns [`DagStoreFreezeError`] if a runtime unit has no defining body.
    pub fn freeze_local_dag_store(self) -> Result<DagStore, DagStoreFreezeError> {
        self.dags.freeze_local(self.core.into_runtime_units())
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
    pub fn index_def<V: crate::registry::checked_type::Concreteness>(
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
                .any(crate::registry::types::IndexDef::is_required)
    }
}

impl TirRead for CheckedTir {
    fn core(&self) -> &TirCore {
        &self.core
    }

    fn root(&self) -> &DagTIR {
        self.dags.root()
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
