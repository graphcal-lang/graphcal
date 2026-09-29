use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::assertion_expectation::ExpectedFail;
use crate::dimension::Dimension;
use crate::generic_param::GenericParamId;
use crate::hir;
use crate::hir::NominalTypeDef;
use crate::registry::checked_type::{CheckedType, IndexTypeRef};
use crate::registry::error::GraphcalError;
use crate::registry::types::{BaseDimensionInfo, FormattingRegistry, IndexDef, UnitInfo};
use crate::resolve::ModuleResolver;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName};

use super::resolved_type::{ResolvedDeclType, ResolvedGenericArg};

/// Convert a [`NatOverflowError`](crate::nat::NatOverflowError)
/// into a spanned [`GraphcalError`].
#[must_use]
pub fn nat_overflow_error(
    err: crate::nat::NatOverflowError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

/// Authoritative project type-system definitions keyed by
/// [`ResolvedName`](crate::resolved_name::ResolvedName) identities.
///
/// Every module's owner-qualified
/// [`ModuleDefinitions`](crate::ir::module_definitions::ModuleDefinitions)
/// fill this store directly. Checked TIR retains a [`FormattingRegistry`] for
/// diagnostics and input boundaries; every canonical type-system lookup reads
/// this store. Source spellings are resolved through [`ModuleResolver`], and
/// imported aliases are never installed as additional canonical definitions.
#[derive(Debug, Default, Clone)]
pub struct ProjectTypeStore {
    base_dimensions: std::collections::BTreeMap<crate::dimension::BaseDimId, BaseDimensionInfo>,
    dimensions: HashMap<ResolvedDimName, Dimension>,
    /// Values that differ when each module's defaulted bindable dimension
    /// ports stay opaque; each such port maps to its own base.
    port_generic_dimensions: HashMap<ResolvedDimName, Dimension>,
    /// Defaulted dimension ports this view keeps rigid (empty for the
    /// canonical store).
    rigid_ports: std::collections::BTreeSet<ResolvedDimName>,
    units: HashMap<ResolvedUnitName, UnitInfo>,
    indexes: HashMap<ResolvedIndexName, Arc<IndexDef>>,
    struct_types: HashMap<ResolvedStructTypeName, Arc<NominalTypeDef>>,
    constructors: HashMap<ResolvedConstructorName, crate::hir::nominal::ResolvedConstructor>,
}

/// Failure to transfer one module's definitions into the semantic project type store.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectTypeStoreInsertError {
    #[error(
        "project type store already contains a different dimension definition for `{identity}`"
    )]
    CompetingDimensionDefinition { identity: ResolvedDimName },
    #[error("project type store already contains a different unit definition for `{identity}`")]
    CompetingUnitDefinition { identity: ResolvedUnitName },
    #[error("project type store already contains a different index definition for `{identity}`")]
    CompetingIndexDefinition { identity: ResolvedIndexName },
    #[error("project type store already contains a different nominal definition for `{identity}`")]
    CompetingNominalDefinition { identity: ResolvedStructTypeName },
    #[error("project type store already assigns constructor `{constructor}` to `{first_owner}`")]
    CompetingConstructorOwner {
        constructor: ResolvedConstructorName,
        first_owner: ResolvedStructTypeName,
    },
}

impl ProjectTypeStore {
    /// Insert canonical Graphcal prelude dimensions, units, and base-dimension
    /// metadata under the synthetic prelude owner.
    ///
    /// # Errors
    ///
    /// Returns an error only if the built-in prelude itself fails to construct,
    /// which would be a compiler bug.
    pub fn insert_graphcal_prelude(
        &mut self,
    ) -> Result<(), crate::registry::prelude::PreludeDefinitionError> {
        let prelude = crate::registry::prelude::prelude_definitions()?;
        self.merge_base_dimensions(&prelude);
        for (identity, dimension) in prelude.dimensions() {
            self.dimensions.insert(identity.clone(), dimension.clone());
        }
        for (identity, info) in prelude.units() {
            self.units.insert(identity.clone(), info.clone());
        }
        Ok(())
    }

    fn merge_base_dimensions(
        &mut self,
        statics: &crate::ir::module_definitions::StaticDefinitions,
    ) {
        for (id, info) in statics.base_dimensions() {
            self.base_dimensions
                .entry(id.clone())
                .or_default()
                .merge_missing(info);
        }
    }

    /// Metadata of every base dimension in the project.
    #[must_use]
    pub const fn base_dimensions(
        &self,
    ) -> &std::collections::BTreeMap<crate::dimension::BaseDimId, BaseDimensionInfo> {
        &self.base_dimensions
    }

    fn insert_dimension_definition(
        &mut self,
        identity: ResolvedDimName,
        dimension: &Dimension,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        match self.dimensions.get(&identity) {
            Some(existing) if existing != dimension => {
                Err(ProjectTypeStoreInsertError::CompetingDimensionDefinition { identity })
            }
            Some(_) => Ok(()),
            None => {
                self.dimensions.insert(identity, dimension.clone());
                Ok(())
            }
        }
    }

    pub(super) fn insert_unit_definition(
        &mut self,
        identity: ResolvedUnitName,
        info: &UnitInfo,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        match self.units.get(&identity) {
            Some(existing) if existing != info => {
                Err(ProjectTypeStoreInsertError::CompetingUnitDefinition { identity })
            }
            Some(_) => Ok(()),
            None => {
                self.units.insert(identity, info.clone());
                Ok(())
            }
        }
    }

    fn insert_index_definition(
        &mut self,
        identity: ResolvedIndexName,
        index: &IndexDef,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        match self.indexes.get(&identity) {
            Some(existing) if existing.as_ref() != index => {
                Err(ProjectTypeStoreInsertError::CompetingIndexDefinition { identity })
            }
            Some(_) => Ok(()),
            None => {
                self.indexes.insert(identity, Arc::new(index.clone()));
                Ok(())
            }
        }
    }

    /// Insert every definition one module owns.
    ///
    /// The definitions are already keyed by canonical identities owned by the
    /// module, so no source spelling is looked up.
    ///
    /// # Errors
    ///
    /// Returns an invariant error if another module already claims the same
    /// canonical identity with a different definition.
    pub fn insert_module(
        &mut self,
        definitions: &crate::ir::module_definitions::ModuleDefinitions,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        let statics = definitions.statics();
        self.merge_base_dimensions(statics);
        for (identity, dimension) in statics.dimensions() {
            self.insert_dimension_definition(identity.clone(), dimension)?;
        }
        self.port_generic_dimensions.extend(
            statics
                .port_generic_dimensions()
                .map(|(identity, dimension)| (identity.clone(), dimension.clone())),
        );
        for (identity, info) in statics.units() {
            self.insert_unit_definition(identity.clone(), info)?;
        }
        for (identity, index) in statics.indexes() {
            self.insert_index_definition(identity.clone(), index)?;
        }
        self.insert_nominal_types(definitions.nominal_types())
    }

    fn insert_nominal_types(
        &mut self,
        nominal_types: &crate::hir::NominalTypeRegistry,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        for definition in nominal_types.values() {
            if let Some(existing) = self.struct_types.get(definition.identity())
                && !Arc::ptr_eq(existing, definition)
            {
                return Err(ProjectTypeStoreInsertError::CompetingNominalDefinition {
                    identity: definition.identity().clone(),
                });
            }
            if let Some(members) = definition.union_members() {
                for member in members {
                    if let Some(existing) = self.constructors.get(member.identity())
                        && !Arc::ptr_eq(existing.definition(), definition)
                    {
                        return Err(ProjectTypeStoreInsertError::CompetingConstructorOwner {
                            constructor: member.identity().clone(),
                            first_owner: existing.owning_type().clone(),
                        });
                    }
                }
            }
        }

        for definition in nominal_types.values() {
            let identity = definition.identity().clone();
            let handle = Arc::clone(
                self.struct_types
                    .entry(identity.clone())
                    .or_insert_with(|| Arc::clone(definition)),
            );
            for constructor in crate::hir::nominal::ResolvedConstructor::members_of(&handle) {
                self.constructors
                    .entry(constructor.identity().clone())
                    .or_insert(constructor);
            }
        }
        Ok(())
    }

    /// Whether `identity` is a defaulted bindable dimension port
    /// (`pub(bind) dim Q = Length;`): its port-generic value is its own base.
    #[must_use]
    fn is_defaulted_dimension_port(&self, identity: &ResolvedDimName) -> bool {
        self.port_generic_dimensions.get(identity)
            == Some(&Dimension::base(crate::dimension::BaseDimId::UserDefined(
                identity.clone(),
            )))
    }

    /// The defaulted dimension ports `substitution` binds.
    ///
    /// A defaulted port is resolved to its default in its template's own
    /// signatures and facts, so whatever a binding specializes is taken from
    /// the view where these ports are rigid ([`Self::with_rigid_dimensions`]).
    #[must_use]
    pub(crate) fn bound_defaulted_dimension_ports(
        &self,
        substitution: &crate::ir::static_substitution::StaticSubstitution,
    ) -> Vec<ResolvedDimName> {
        substitution
            .dimensions
            .keys()
            .filter(|port| self.is_defaulted_dimension_port(port))
            .cloned()
            .collect()
    }

    /// The view in which the defaulted bindable dimension `ports` (and every
    /// port this view already keeps rigid) stay opaque base dimensions, while
    /// every other port keeps its default.
    ///
    /// Every dimension defined over one of `ports` (`QR = Q / Time`) is
    /// recomputed from its port-generic value, so it stays symbolic in the
    /// rigid ports.
    ///
    /// # Errors
    ///
    /// Returns an error when re-expanding a default overflows.
    pub(crate) fn with_rigid_dimensions(
        &self,
        ports: &[ResolvedDimName],
    ) -> Result<Self, crate::ratio::RatioError> {
        let mut rigid = self.clone();
        rigid.rigid_ports.extend(ports.iter().cloned());
        for (identity, generic) in &self.port_generic_dimensions {
            let value =
                generic
                    .iter()
                    .try_fold(Dimension::dimensionless(), |acc, (base, exponent)| {
                        let factor = match base {
                            crate::dimension::BaseDimId::UserDefined(port)
                                if self.is_defaulted_dimension_port(port)
                                    && !rigid.rigid_ports.contains(port) =>
                            {
                                self.dimensions
                                    .get(port)
                                    .cloned()
                                    .unwrap_or_else(|| Dimension::base(base.clone()))
                            }
                            crate::dimension::BaseDimId::UserDefined(_)
                            | crate::dimension::BaseDimId::Prelude(_) => {
                                Dimension::base(base.clone())
                            }
                        };
                        factor
                            .pow(*exponent)
                            .and_then(|factor| acc.checked_mul(&factor))
                    })?;
            rigid.dimensions.insert(identity.clone(), value);
        }
        Ok(rigid)
    }

    #[must_use]
    pub(crate) fn get_dimension(&self, name: &ResolvedDimName) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    #[must_use]
    pub(crate) fn get_unit(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    #[must_use]
    pub(crate) fn get_index(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.indexes.get(name).map(AsRef::as_ref)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn get_index_handle(&self, name: &ResolvedIndexName) -> Option<&Arc<IndexDef>> {
        self.indexes.get(name)
    }

    #[must_use]
    pub(crate) fn get_struct_type(&self, name: &ResolvedStructTypeName) -> Option<&NominalTypeDef> {
        self.struct_types.get(name).map(AsRef::as_ref)
    }

    #[must_use]
    pub(crate) fn get_struct_type_handle(
        &self,
        name: &ResolvedStructTypeName,
    ) -> Option<&Arc<NominalTypeDef>> {
        self.struct_types.get(name)
    }

    /// Look up the owner type and union member for a canonical constructor identity.
    #[must_use]
    pub(crate) fn lookup_constructor(
        &self,
        constructor: &ResolvedConstructorName,
    ) -> Option<&crate::hir::nominal::ResolvedConstructor> {
        self.constructors.get(constructor)
    }
}

/// Module-aware type-resolution context for one DAG body.
#[derive(Debug, Clone, Copy)]
pub struct ModuleTypeContext<'a> {
    pub(in crate::tir::typed) owner: &'a crate::dag_id::DagId,
    pub(in crate::tir::typed) resolver: &'a ModuleResolver,
    pub(in crate::tir::typed) types: &'a ProjectTypeStore,
}

impl<'a> ModuleTypeContext<'a> {
    #[must_use]
    pub(crate) const fn new(
        owner: &'a crate::dag_id::DagId,
        resolver: &'a ModuleResolver,
        types: &'a ProjectTypeStore,
    ) -> Self {
        Self {
            owner,
            resolver,
            types,
        }
    }

    #[must_use]
    pub const fn owner(self) -> &'a crate::dag_id::DagId {
        self.owner
    }
}

/// Owner-qualified key for a domain constraint declared on a struct/union field.
///
/// The owning type carries a canonical owner when module-aware type resolution
/// supplied one. The constructor remains a separate typed leaf because union
/// members can share the same field names with different constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructFieldConstraintKey {
    pub owning_type: crate::registry::checked_type::StructTypeRef,
    pub generic_args: Vec<crate::registry::checked_type::CheckedGenericArg>,
    pub constructor: ConstructorName,
    pub field: FieldName,
}

impl StructFieldConstraintKey {
    /// Construct a key for a non-generic nominal type.
    #[must_use]
    pub const fn new(
        owning_type: crate::registry::checked_type::StructTypeRef,
        constructor: ConstructorName,
        field: FieldName,
    ) -> Self {
        Self {
            owning_type,
            generic_args: Vec::new(),
            constructor,
            field,
        }
    }

    /// Construct a key for one concrete generic nominal application.
    #[must_use]
    pub const fn for_application(
        owning_type: crate::registry::checked_type::StructTypeRef,
        generic_args: Vec<crate::registry::checked_type::CheckedGenericArg>,
        constructor: ConstructorName,
        field: FieldName,
    ) -> Self {
        Self {
            owning_type,
            generic_args,
            constructor,
            field,
        }
    }
}

// ---------------------------------------------------------------------------
// DAG registry
// ---------------------------------------------------------------------------

/// Failure to add a DAG to a TIR registry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagRegistryError {
    /// A canonical DAG identity can occur only once in one compiled registry.
    #[error("DAG `{dag_id}` is already present in the TIR registry")]
    DuplicateDag { dag_id: crate::dag_id::DagId },
}

/// Failure to attach one immutable module store to an assembly registry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagStoreInsertError {
    /// The store contains a body identity already owned by the assembly.
    #[error(transparent)]
    Registry(#[from] DagRegistryError),
    /// The store contains a conflicting instance-specialized unit.
    #[error("runtime unit `{identity}` has competing checked definitions")]
    CompetingRuntimeUnit { identity: ResolvedUnitName },
}

/// Registry of the DAG bodies of a TIR before it is checked.
///
/// The root DAG is stored directly, so its presence is structural. Every other
/// entry is keyed internally from [`DagTIR::dag_id`], so a caller cannot pair a
/// DAG body with a different map key. Imported bodies are already-checked
/// handles owned by the store that published them; they have no mutable
/// accessor.
#[derive(Debug, Clone)]
pub(crate) struct DagRegistry {
    root: DagTIR,
    other_dags: HashMap<crate::dag_id::DagId, DagTIR>,
    shared_dags: HashMap<crate::dag_id::DagId, Arc<super::checked::CheckedDag>>,
}

/// The owned local bodies and the imported checked handles of a registry.
pub(crate) type DagRegistryParts = (
    DagTIR,
    HashMap<crate::dag_id::DagId, DagTIR>,
    HashMap<crate::dag_id::DagId, Arc<super::checked::CheckedDag>>,
);

impl DagRegistry {
    fn new(root: DagTIR) -> Self {
        Self {
            root,
            other_dags: HashMap::new(),
            shared_dags: HashMap::new(),
        }
    }

    pub(crate) fn into_parts(self) -> DagRegistryParts {
        (self.root, self.other_dags, self.shared_dags)
    }

    /// Canonical identity of this registry's root DAG.
    pub(crate) const fn root_id(&self) -> &crate::dag_id::DagId {
        self.root.dag_id()
    }

    /// Borrow the root DAG.
    pub(crate) const fn root(&self) -> &DagTIR {
        &self.root
    }

    #[cfg(test)]
    const fn root_mut(&mut self) -> &mut DagTIR {
        &mut self.root
    }

    pub(crate) fn contains(&self, dag_id: &crate::dag_id::DagId) -> bool {
        dag_id == self.root_id()
            || self.other_dags.contains_key(dag_id)
            || self.shared_dags.contains_key(dag_id)
    }

    fn insert(&mut self, dag: DagTIR) -> Result<(), DagRegistryError> {
        let dag_id = dag.dag_id.clone();
        if self.contains(&dag_id) {
            return Err(DagRegistryError::DuplicateDag { dag_id });
        }
        self.other_dags.insert(dag_id, dag);
        Ok(())
    }

    pub(super) fn insert_shared(
        &mut self,
        dag_id: crate::dag_id::DagId,
        dag: Arc<super::checked::CheckedDag>,
    ) {
        self.shared_dags.insert(dag_id, dag);
    }

    /// Look up one DAG body by canonical identity.
    pub(crate) fn get(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        if dag_id == self.root_id() {
            Some(&self.root)
        } else {
            self.other_dags
                .get(dag_id)
                .or_else(|| self.shared_dags.get(dag_id).map(|dag| &***dag))
        }
    }

    /// The checked handle of an imported body.
    pub(crate) fn shared(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&super::checked::CheckedDag> {
        self.shared_dags.get(dag_id).map(AsRef::as_ref)
    }

    /// Mutably borrow one DAG, first copying a shared (imported) body into
    /// this registry's local bodies. Used only by temporary views that
    /// re-resolve an imported template's signatures.
    pub(crate) fn localized_mut(&mut self, dag_id: &crate::dag_id::DagId) -> Option<&mut DagTIR> {
        if let Some(shared) = self.shared_dags.remove(dag_id) {
            self.other_dags
                .insert(dag_id.clone(), Arc::unwrap_or_clone(shared).into_body());
        }
        if dag_id == self.root.dag_id() {
            Some(&mut self.root)
        } else {
            self.other_dags.get_mut(dag_id)
        }
    }

    /// Iterate over local assembly identities and bodies.
    pub(crate) fn local_iter(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
        std::iter::once((self.root.dag_id(), &self.root)).chain(self.other_dags.iter())
    }

    /// Iterate over canonical identities and DAG bodies.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
        self.local_iter()
            .chain(self.shared_dags.iter().map(|(id, dag)| (id, &***dag)))
    }

    /// Iterate mutably over local DAG bodies while preserving their registry keys.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut DagTIR> {
        std::iter::once(&mut self.root).chain(self.other_dags.values_mut())
    }

    /// Number of DAG modules in this registry, including its root and imports.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.other_dags.len() + self.shared_dags.len() + 1
    }
}

/// Canonical dependency maps for one DAG body, collected from HIR expressions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedDagDependencies {
    /// For each param/node declaration, the canonical declarations it reads via `@`.
    pub runtime_deps: HashMap<ResolvedDeclName, BTreeSet<ResolvedDeclName>>,
    /// For each const declaration, the canonical const declarations it reads.
    pub const_deps: HashMap<ResolvedDeclName, BTreeSet<ResolvedDeclName>>,
}

/// Canonical field type identity inside a resolved struct/tagged-union type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedStructFieldTypeKey {
    /// Canonical owner/name of the type that owns the constructor.
    pub owning_type: ResolvedStructTypeName,
    /// Constructor/union-member leaf inside the owning type.
    pub constructor: ConstructorName,
    /// Field leaf inside the constructor payload.
    pub field: FieldName,
}

/// Semantic resolution of one HIR generic-parameter default.
///
/// The canonical HIR form remains authoritative on [`NominalGenericParam`];
/// this sidecar stores only the later semantic fact used for substitution.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedGenericDefault {
    pub(crate) resolved: ResolvedGenericArg,
}

/// Resolved semantic facts for one nominal field.
///
/// Keeping the target type and its bounds in one value prevents a constrained
/// field from existing without the type needed to validate those bounds.
#[derive(Debug, Clone)]
pub struct ResolvedStructFieldSemantics {
    resolved_type: ResolvedDeclType,
    domain_bounds: Vec<ResolvedDomainBound>,
}

impl ResolvedStructFieldSemantics {
    #[must_use]
    pub const fn new(
        resolved_type: ResolvedDeclType,
        domain_bounds: Vec<ResolvedDomainBound>,
    ) -> Self {
        Self {
            resolved_type,
            domain_bounds,
        }
    }

    /// Return the field annotation resolved in its owning generic scope.
    #[must_use]
    pub const fn resolved_type(&self) -> &ResolvedDeclType {
        &self.resolved_type
    }

    /// Return domain bounds lowered in the same owning generic scope.
    #[must_use]
    pub fn domain_bounds(&self) -> &[ResolvedDomainBound] {
        &self.domain_bounds
    }
}

/// Canonical type definitions referenced by module-aware TIR.
#[derive(Debug, Clone, Default)]
pub struct ResolvedTypeDefs {
    /// Shared handles to project-store nominal definitions keyed by canonical identity.
    pub struct_types: HashMap<ResolvedStructTypeName, Arc<NominalTypeDef>>,
    /// Atomic field semantics resolved in each owning type's generic scope.
    fields: HashMap<ResolvedStructFieldTypeKey, ResolvedStructFieldSemantics>,
    /// Generic parameter defaults resolved in the owning type's generic scope.
    pub(crate) generic_defaults: HashMap<GenericParamId, ResolvedGenericDefault>,
}

impl ResolvedTypeDefs {
    pub(crate) fn extend_from(&mut self, other: &Self) {
        self.struct_types.extend(
            other
                .struct_types
                .iter()
                .map(|(name, definition)| (name.clone(), Arc::clone(definition))),
        );
        self.fields.extend(
            other
                .fields
                .iter()
                .map(|(key, semantics)| (key.clone(), semantics.clone())),
        );
        self.generic_defaults.extend(
            other
                .generic_defaults
                .iter()
                .map(|(key, default)| (key.clone(), default.clone())),
        );
    }

    /// Insert one field's type and bounds as an atomic semantic fact.
    pub(crate) fn insert_field(
        &mut self,
        key: ResolvedStructFieldTypeKey,
        field: ResolvedStructFieldSemantics,
    ) {
        self.fields.insert(key, field);
    }

    /// Return one field's complete resolved semantics.
    #[must_use]
    pub fn field(&self, key: &ResolvedStructFieldTypeKey) -> Option<&ResolvedStructFieldSemantics> {
        self.fields.get(key)
    }

    /// Visit every resolved field semantic record.
    pub fn fields(
        &self,
    ) -> impl Iterator<Item = (&ResolvedStructFieldTypeKey, &ResolvedStructFieldSemantics)> {
        self.fields.iter()
    }

    /// Visit only fields that carry domain bounds.
    pub fn constrained_fields(
        &self,
    ) -> impl Iterator<Item = (&ResolvedStructFieldTypeKey, &ResolvedStructFieldSemantics)> {
        self.fields
            .iter()
            .filter(|(_, field)| !field.domain_bounds.is_empty())
    }

    /// Return a field annotation resolved in its owning type's generic scope.
    #[must_use]
    pub fn field_type(&self, key: &ResolvedStructFieldTypeKey) -> Option<&ResolvedDeclType> {
        self.field(key)
            .map(ResolvedStructFieldSemantics::resolved_type)
    }
}

/// A `min:`/`max:` domain bound with its expression lowered to HIR.
///
/// Declaration bounds cross into HIR with their owning type annotation. TIR
/// moves them into this checked semantic record so dimension checking and
/// evaluation consume the same canonical expression tree.
#[derive(Debug, Clone)]
pub struct ResolvedDomainBound {
    /// Whether this is a `min:` or `max:` bound.
    pub kind: crate::syntax::ast::DomainBoundKind,
    /// The bound expression, strictly lowered.
    pub value: hir::CheckedExpr,
    /// Span of the whole bound.
    pub span: Span,
    /// Source file whose bytes are indexed by `span` and the expression spans.
    pub src: NamedSource<Arc<String>>,
}

/// The checked type of one value declaration.
///
/// The TIR type is resolved once from the declaration's canonical HIR
/// annotation; its concrete declared form is derived at construction, so the
/// two can never disagree and consumers never convert on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedDeclType {
    resolved: ResolvedDeclType,
    declared: CheckedType,
}

impl CheckedDeclType {
    /// Check that a resolved declaration type is concrete.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] when the type contains unresolved generic
    /// parameters.
    pub(crate) fn new(
        resolved: ResolvedDeclType,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Self, GraphcalError> {
        let declared = resolved.to_checked_type(src)?;
        Ok(Self { resolved, declared })
    }

    /// The resolved TIR type, including declaration-level index axes.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedDeclType {
        &self.resolved
    }

    /// The concrete declared type.
    #[must_use]
    pub const fn declared(&self) -> &CheckedType {
        &self.declared
    }
}

/// A value declaration's type annotation after TIR type resolution.
///
/// Domain bounds leave the annotation at this boundary and live in
/// [`DagSemanticBody::domain_bounds`].
#[derive(Debug, Clone)]
pub struct CheckedTypeAnnotation {
    /// The canonical HIR annotation, retained so a rigid template view can
    /// resolve it again against an opaque dimension.
    pub decl_type: hir::DeclType,
    /// Span of the whole annotation.
    pub span: Span,
    pub(crate) checked: CheckedDeclType,
}

impl CheckedTypeAnnotation {
    /// The declaration's checked type.
    #[must_use]
    pub const fn checked(&self) -> &CheckedDeclType {
        &self.checked
    }
}

/// Checked phase: HIR bodies whose value declarations carry their checked
/// types.
#[derive(Debug, Clone, Copy)]
pub enum Typed {}

impl crate::ir::entry::BodyPhase for Typed {
    type Expr = hir::CheckedExpr;
    type TypeAnnotation = CheckedTypeAnnotation;
    type NodeDefinition = hir::node_definition::NodeDefinition;
    type AssertBody = hir::CheckedAssertBody;
    type PlotBody = crate::ir::lower::LoweredPlotBody;
    type CompositionFields = Vec<crate::ir::lower::LoweredPlotField>;
    type UnitIdentity = ResolvedUnitName;
}

/// A checked value, assertion, or visualization declaration.
pub type TypedDecl = crate::ir::entry::Decl<Typed>;
/// A checked const declaration.
pub type TypedConstEntry = crate::ir::entry::ConstEntry<Typed>;
/// A checked param declaration.
pub type TypedParamEntry = crate::ir::entry::ParamEntry<Typed>;
/// A checked node declaration.
pub type TypedNodeEntry = crate::ir::entry::NodeEntry<Typed>;
/// A checked assert declaration.
pub type TypedAssertEntry = crate::ir::entry::AssertEntry<Typed>;
/// A checked plot declaration.
pub type TypedPlotEntry = crate::ir::entry::PlotEntry<Typed>;
/// A checked figure declaration.
pub type TypedFigureEntry = crate::ir::entry::FigureEntry<Typed>;
/// A checked layer declaration.
pub type TypedLayerEntry = crate::ir::entry::LayerEntry<Typed>;

pub(crate) use crate::ir::override_reconciliation::{OverrideReconciliation, OverrideTarget};

/// A module-owned nominal declaration that may be replaced at an include site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum BindableNominalIdentity {
    Index(ResolvedIndexName),
    Type(ResolvedStructTypeName),
}

/// An unbound source-facing name retained only as a diagnostic probe.
///
/// A probe deliberately keeps the DAG and written scoped name as separate
/// fields. It cannot be converted into a [`ResolvedDeclName`], because no
/// authoritative declaration binding established that identity.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unbound declaration probe `{name}` in DAG `{dag_id}`")]
pub struct DiagnosticDeclProbe {
    dag_id: crate::dag_id::DagId,
    name: ScopedName,
}

impl DiagnosticDeclProbe {
    pub(super) const fn new(dag_id: crate::dag_id::DagId, name: ScopedName) -> Self {
        Self { dag_id, name }
    }

    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        &self.dag_id
    }

    #[must_use]
    pub const fn name(&self) -> &ScopedName {
        &self.name
    }
}

/// Result of looking up a declaration identity by its source-facing name.
///
/// Only [`Self::Bound`] carries a canonical semantic identity. Unknown names
/// remain typed diagnostic probes so callers cannot accidentally route values
/// or semantic facts through an identity fabricated from the current DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationIdentityLookup {
    Bound(ResolvedDeclName),
    DiagnosticProbe(DiagnosticDeclProbe),
}

impl DeclarationIdentityLookup {
    /// Require an authoritative binding rather than a diagnostic probe.
    ///
    /// # Errors
    ///
    /// Returns the structured probe when the source-facing name has no
    /// canonical declaration binding in this DAG.
    pub fn into_bound(self) -> Result<ResolvedDeclName, DiagnosticDeclProbe> {
        match self {
            Self::Bound(identity) => Ok(identity),
            Self::DiagnosticProbe(probe) => Err(probe),
        }
    }
}

/// Derived semantic facts for a checked DAG.
///
/// Authoritative HIR bodies remain on the declaration records in [`DagTIR`].
/// This structure stores only facts derived or indexed from those bodies.
#[derive(Debug, Clone, Default)]
pub struct DagSemanticBody {
    /// Domain bounds per declaration, lowered to HIR, in source order.
    pub domain_bounds: HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
    /// Source-qualified dynamic unit definitions keyed by canonical unit identity.
    ///
    /// Each entry carries the validated declared/base dimensions and strictly
    /// lowered HIR scalar expression as one semantic record.
    pub dynamic_unit_scales: HashMap<ResolvedUnitName, crate::ir::lower::DynamicUnitScaleEntry>,
    /// Canonical dependency maps for this DAG.
    pub dependencies: ResolvedDagDependencies,
    /// Include override obligations keyed by the canonical param whose default
    /// must remain independent of the replaced nominal declarations.
    pub(crate) override_reconciliations: HashMap<ResolvedDeclName, Vec<OverrideReconciliation>>,
    /// Module-owned `pub(bind)` index and type identities. Only these can
    /// participate in include-time nominal override reconciliation.
    pub(crate) bindable_nominals: HashSet<BindableNominalIdentity>,
    /// Canonical type definitions referenced by this DAG.
    pub type_defs: ResolvedTypeDefs,
    /// Canonical identities for every declaration record and imported value
    /// binding visible in this DAG.
    pub decl_bindings: HashMap<ScopedName, ResolvedDeclName>,
}

// ---------------------------------------------------------------------------
// TIR struct
// ---------------------------------------------------------------------------

/// A project TIR already carried a different signature for one canonical
/// plugin function.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("TIR already contains a different definition for plugin `{plugin}` function `{name}`")]
pub struct CompetingExternFunctionDefinition {
    pub plugin: crate::plugin_identity::PluginIdentity,
    pub name: crate::syntax::function_name::FnName,
}

/// Resolved extern function signatures keyed by plugin and function identity.
pub type ExternFunctions =
    HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry>;

/// Project-wide type services shared by every TIR state: formatting, the
/// owner-qualified type store, instance runtime units, and extern signatures.
#[derive(Debug, Clone)]
pub(crate) struct TirCore {
    registry: FormattingRegistry,
    project_types: Arc<ProjectTypeStore>,
    runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    extern_functions:
        HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry>,
}

impl TirCore {
    /// Borrow the root file's post-resolution formatting services.
    pub(crate) const fn registry(&self) -> &FormattingRegistry {
        &self.registry
    }

    /// Borrow the authoritative owner-qualified project type store.
    pub(crate) fn project_type_store(&self) -> &ProjectTypeStore {
        self.project_types.as_ref()
    }

    /// Borrow resolved extern function signatures.
    pub(crate) const fn extern_functions(
        &self,
    ) -> &HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry> {
        &self.extern_functions
    }

    pub(crate) fn into_runtime_units(self) -> HashMap<ResolvedUnitName, Arc<UnitInfo>> {
        self.runtime_units
    }

    /// Look up a dimension by its canonical defining-module identity.
    pub(crate) fn dimension(&self, name: &ResolvedDimName) -> Option<&Dimension> {
        self.project_types.get_dimension(name)
    }

    /// Look up a unit by its canonical defining-module identity.
    pub(crate) fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.runtime_units
            .get(name)
            .map(AsRef::as_ref)
            .or_else(|| self.project_types.get_unit(name))
    }

    /// Look up a declared index by its canonical defining-module identity.
    pub(crate) fn declared_index_def(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.project_types.get_index(name)
    }

    /// Resolve a declared axis or derive a structural axis from its cardinality.
    /// Structural `Fin(N)` definitions never depend on a source-registration scan.
    pub(crate) fn index_def<V: crate::registry::checked_type::Concreteness>(
        &self,
        index: &IndexTypeRef<V>,
    ) -> Option<std::borrow::Cow<'_, IndexDef>> {
        match index.finite_index() {
            Some(finite) => Some(std::borrow::Cow::Owned(IndexDef::finite(finite))),
            None => self
                .project_types
                .get_index(index.declared_resolved()?)
                .map(std::borrow::Cow::Borrowed),
        }
    }

    /// Look up a nominal type by its canonical defining-module identity.
    pub(crate) fn struct_type_def(&self, name: &ResolvedStructTypeName) -> Option<&NominalTypeDef> {
        self.project_types.get_struct_type(name)
    }

    /// Iterate every canonical nominal definition in the project type store.
    pub(crate) fn nominal_type_defs(
        &self,
    ) -> impl Iterator<Item = (&ResolvedStructTypeName, &NominalTypeDef)> {
        self.project_types
            .struct_types
            .iter()
            .map(|(identity, definition)| (identity, definition.as_ref()))
    }

    /// Iterate canonical index definitions owned by `owner`.
    pub(crate) fn declared_indexes_of<'a>(
        &'a self,
        owner: &'a crate::dag_id::DagId,
    ) -> impl Iterator<Item = &'a IndexDef> {
        self.project_types
            .indexes
            .iter()
            .filter(move |(name, _)| name.owner() == owner)
            .map(|(_, definition)| definition.as_ref())
    }

    /// Install one instance-owned dynamic unit in the mutable assembly overlay.
    fn insert_runtime_unit(
        &mut self,
        name: ResolvedUnitName,
        info: UnitInfo,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        match self.runtime_units.get(&name) {
            Some(existing) if existing.as_ref() != &info => {
                Err(ProjectTypeStoreInsertError::CompetingUnitDefinition { identity: name })
            }
            Some(_) => Ok(()),
            None => {
                self.runtime_units.insert(name, Arc::new(info));
                Ok(())
            }
        }
    }
}

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
            crate::ir::lower::ExternFunctionEntry,
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
    pub const fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    /// Mutably borrow the file-root DAG during assembly.
    #[cfg(test)]
    pub(crate) const fn root_mut(&mut self) -> &mut DagTIR {
        self.dags.root_mut()
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
        self.dags.insert(dag)
    }

    /// Add immutable checked bodies from a previously frozen module store.
    ///
    /// Only handles are copied. The body and its checked semantic facts remain
    /// owned by the store that published them.
    ///
    /// # Errors
    ///
    /// Returns [`DagStoreInsertError`] when the store repeats a body identity
    /// or a runtime unit with a competing definition.
    pub fn insert_shared_dag_store(
        &mut self,
        store: &super::checked::DagStore,
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
        self.dags
            .insert_shared_store(store)
            .map_err(DagStoreInsertError::Registry)?;
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
        function: crate::ir::lower::ExternFunctionEntry,
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
        hir: &crate::ir::lower::HirDag,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        // Deterministic conflict reporting: earliest declaration first.
        let mut declared: Vec<_> = hir.extern_functions().iter().collect();
        declared.sort_by_key(|(_, function)| function.decl_span.offset());
        declared.into_iter().try_for_each(|(_, function)| {
            crate::ir::extern_fns::merge_extern_function(
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
        self.dags.insert(dag)
    }

    pub(crate) fn into_parts(self) -> (TirCore, DagRegistry) {
        (self.core, self.dags)
    }

    /// Borrow the root DAG. Root presence is guaranteed by [`DagRegistry`].
    pub(crate) const fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    /// Canonical identity of the root DAG.
    pub(crate) const fn root_dag_id(&self) -> &crate::dag_id::DagId {
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
    /// The checked expression facts already published for one DAG.
    fn expression_facts(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::expression_facts::CheckedExpressionFacts>;
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
    ) -> &HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry> {
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
    pub(crate) fn index_def<V: crate::registry::checked_type::Concreteness>(
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

    /// Before any local body is checked, only imported bodies have facts.
    fn expression_facts(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::expression_facts::CheckedExpressionFacts> {
        self.dags
            .shared(dag_id)
            .map(super::checked::CheckedDag::expression_facts)
    }

    /// Before any local body is checked, only imported bodies have trees.
    fn checked_bodies(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::texpr::CheckedBodies> {
        self.dags
            .shared(dag_id)
            .map(super::checked::CheckedDag::bodies)
    }
}

/// A project TIR in the middle of its check: unchecked local bodies with the
/// expression facts published for them so far.
#[derive(Clone, Copy)]
pub(crate) struct CheckingTir<'a> {
    pub(crate) tir: &'a UncheckedTir,
    pub(crate) facts:
        &'a HashMap<crate::dag_id::DagId, crate::tir::expression_facts::CheckedExpressionFacts>,
    pub(crate) bodies: &'a HashMap<crate::dag_id::DagId, crate::tir::texpr::CheckedBodies>,
}

impl TirRead for CheckingTir<'_> {
    fn core(&self) -> &TirCore {
        &self.tir.core
    }

    fn root(&self) -> &DagTIR {
        self.tir.root()
    }

    fn dag(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        self.tir.dag(dag_id)
    }

    fn dag_bodies(&self) -> Box<dyn Iterator<Item = &DagTIR> + '_> {
        self.tir.dag_bodies()
    }

    fn expression_facts(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::expression_facts::CheckedExpressionFacts> {
        self.facts
            .get(dag_id)
            .or_else(|| self.tir.expression_facts(dag_id))
    }

    fn checked_bodies(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::texpr::CheckedBodies> {
        self.bodies
            .get(dag_id)
            .or_else(|| self.tir.checked_bodies(dag_id))
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

pub(crate) use crate::ir::lower::ResolvedExpectedFailMetadata;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpressionRootScope {
    ThisBody,
    ReferencedBody,
}

/// The per-DAG compiled body — every field that's specific to one DAG (the
/// file's own top-level body or an inline `dag X { ... }` child).
///
/// Type resolution installs the file root in a [`TirDraft`], and project
/// assembly adds inline and dependency DAGs through
/// [`TirDraft::add_inline_dag`]. A checked body is a
/// [`CheckedDag`](super::checked::CheckedDag).
#[derive(Debug, Clone)]
pub struct DagTIR {
    pub(crate) dag_id: crate::dag_id::DagId,
    pub(crate) body_revision: crate::body_revision::BodyRevision,
    /// Every declaration owned by this DAG, keyed by canonical identity, in
    /// source order.
    pub(crate) decls: crate::ir::decl_table::DeclTable<Typed>,
    pub(crate) included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
    pub(crate) semantic: DagSemanticBody,
    pub(crate) static_ports: Vec<crate::hir::StaticPort>,
    pub(crate) assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    pub(crate) expected_fail: HashMap<ResolvedDeclName, ResolvedExpectedFailMetadata>,
    pub(crate) imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    pub(crate) semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
    /// How this DAG's bodies name its declarations; built with the DAG.
    pub(crate) frame: crate::ir::instance::frame::InstanceFrame,
    pub(crate) projectable_outputs: std::collections::HashSet<DeclName>,
}

impl DagTIR {
    #[must_use]
    pub const fn body_revision(&self) -> &crate::body_revision::BodyRevision {
        &self.body_revision
    }

    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        &self.dag_id
    }

    /// Every declaration owned by this DAG, in source order.
    #[must_use]
    pub const fn decls(&self) -> &crate::ir::decl_table::DeclTable<Typed> {
        &self.decls
    }

    /// Const declarations in source order.
    pub fn consts(&self) -> impl Iterator<Item = &TypedConstEntry> {
        self.decls.consts()
    }

    /// Param declarations in source order.
    pub fn params(&self) -> impl Iterator<Item = &TypedParamEntry> {
        self.decls.params()
    }

    /// Node declarations in source order.
    pub fn nodes(&self) -> impl Iterator<Item = &TypedNodeEntry> {
        self.decls.nodes()
    }

    /// Assert declarations in source order.
    pub fn asserts(&self) -> impl Iterator<Item = &TypedAssertEntry> {
        self.decls.asserts()
    }

    /// Plot declarations in source order.
    pub fn plots(&self) -> impl Iterator<Item = &TypedPlotEntry> {
        self.decls.plots()
    }

    /// Figure declarations in source order.
    pub fn figures(&self) -> impl Iterator<Item = &TypedFigureEntry> {
        self.decls.figures()
    }

    /// Layer declarations in source order.
    pub fn layers(&self) -> impl Iterator<Item = &TypedLayerEntry> {
        self.decls.layers()
    }

    #[must_use]
    pub const fn semantic(&self) -> &DagSemanticBody {
        &self.semantic
    }

    /// Semantic include edges authored directly by this DAG.
    #[must_use]
    pub fn semantic_instances(&self) -> &[crate::ir::instance::HirInstanceRecord] {
        &self.semantic_instances
    }

    /// The frame this DAG runs its bodies in.
    #[must_use]
    pub const fn frame(&self) -> &crate::ir::instance::frame::InstanceFrame {
        &self.frame
    }

    #[must_use]
    pub const fn is_semantic_instance(&self) -> bool {
        self.frame.is_instance()
    }

    /// Typed Static interface authored directly in this reusable DAG.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::StaticPort] {
        &self.static_ports
    }

    /// Value ports that callers may project from this DAG.
    ///
    /// The set contains public nodes and annotation-free parameter ports after
    /// the DAG's external interface has been checked.
    #[must_use]
    pub const fn projectable_outputs(&self) -> &std::collections::HashSet<DeclName> {
        &self.projectable_outputs
    }

    /// The checked type annotation of one of this DAG's value declarations.
    fn value_decl_annotation(&self, key: &ResolvedDeclName) -> Option<&CheckedTypeAnnotation> {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Const(entry) => Some(&entry.type_ann),
            crate::ir::entry::Decl::Param(entry) => Some(&entry.type_ann),
            crate::ir::entry::Decl::Node(entry) => Some(&entry.type_ann),
            crate::ir::entry::Decl::Assert(_)
            | crate::ir::entry::Decl::Plot(_)
            | crate::ir::entry::Decl::Figure(_)
            | crate::ir::entry::Decl::Layer(_) => None,
        }
    }

    /// The checked type of one of this DAG's value declarations.
    #[must_use]
    pub fn value_decl_type(&self, key: &ResolvedDeclName) -> Option<&CheckedDeclType> {
        self.value_decl_annotation(key)
            .map(CheckedTypeAnnotation::checked)
    }

    /// Replace the checked types of the value declarations named in `types`.
    pub(crate) fn replace_value_decl_types(
        &mut self,
        mut types: HashMap<ResolvedDeclName, CheckedDeclType>,
    ) {
        self.decls.update(|decl| {
            let identity = decl.identity();
            let annotation = match decl {
                crate::ir::entry::Decl::Const(entry) => &mut entry.type_ann,
                crate::ir::entry::Decl::Param(entry) => &mut entry.type_ann,
                crate::ir::entry::Decl::Node(entry) => &mut entry.type_ann,
                crate::ir::entry::Decl::Assert(_)
                | crate::ir::entry::Decl::Plot(_)
                | crate::ir::entry::Decl::Figure(_)
                | crate::ir::entry::Decl::Layer(_) => return,
            };
            if let Some(checked) = types.remove(&identity) {
                annotation.checked = checked;
            }
        });
    }

    /// Every value declaration's identity and checked type annotation, in
    /// source order.
    pub fn value_decl_types(
        &self,
    ) -> impl Iterator<Item = (ResolvedDeclName, &CheckedTypeAnnotation)> {
        self.decls.iter().filter_map(|decl| match decl {
            crate::ir::entry::Decl::Const(entry) => Some((entry.identity(), &entry.type_ann)),
            crate::ir::entry::Decl::Param(entry) => Some((entry.identity(), &entry.type_ann)),
            crate::ir::entry::Decl::Node(entry) => Some((entry.identity(), &entry.type_ann)),
            crate::ir::entry::Decl::Assert(_)
            | crate::ir::entry::Decl::Plot(_)
            | crate::ir::entry::Decl::Figure(_)
            | crate::ir::entry::Decl::Layer(_) => None,
        })
    }

    /// Identities of every value declaration, including required parameters
    /// that have no default expression, in source order.
    pub fn value_declaration_identities(&self) -> impl Iterator<Item = &ResolvedDeclName> {
        self.decls
            .order()
            .iter()
            .filter(|identity| self.value_decl_annotation(identity).is_some())
    }

    /// Look up the single authoritative HIR expression owned by a const.
    #[must_use]
    pub fn const_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Const(entry) => Some(&*entry.expr),
            _ => None,
        }
    }

    /// Look up the single authoritative HIR expression owned by a param or node.
    #[must_use]
    pub fn runtime_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Param(entry) => entry.default.as_deref(),
            crate::ir::entry::Decl::Node(entry) => entry.definition.formula().map(|expr| &**expr),
            _ => None,
        }
    }

    /// Look up an unfinished node's canonical dependency interface.
    #[must_use]
    pub fn todo(
        &self,
        key: &ResolvedDeclName,
    ) -> Option<&crate::syntax::span::Spanned<Vec<crate::syntax::span::Spanned<ResolvedDeclName>>>>
    {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Node(entry) => entry.definition.todo(),
            _ => None,
        }
    }

    /// Look up any value declaration's authoritative HIR expression.
    #[must_use]
    pub fn value_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        self.const_expr(key).or_else(|| self.runtime_expr(key))
    }

    /// Look up the single authoritative HIR body owned by an assertion.
    #[must_use]
    pub fn assert_body(&self, key: &ResolvedDeclName) -> Option<&hir::AssertBody> {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Assert(entry) => Some(&*entry.body),
            _ => None,
        }
    }

    /// Visit every source unit reference used by this DAG.
    pub fn visit_unit_references(&self, visitor: &mut impl FnMut(&hir::ResolvedUnitRef, Span)) {
        self.visit_expressions(&mut |expr| match expr.kind() {
            hir::ExprKind::QuantityLiteral { unit, .. } => unit
                .terms
                .iter()
                .for_each(|term| visitor(&term.name.value, term.name.span)),
            hir::ExprKind::Convert { target, .. } => target
                .terms
                .iter()
                .for_each(|term| visitor(&term.name.value, term.name.span)),
            _ => {}
        });
    }

    /// Visit semantic expressions, including referenced nominal bounds for dependency analysis.
    pub(crate) fn visit_expressions<'a>(&'a self, visitor: &mut dyn FnMut(&'a hir::Expr)) {
        self.owned_expression_roots()
            .chain(self.field_bound_roots(ExpressionRootScope::ReferencedBody))
            .for_each(|root| hir::visit_expr(root, visitor));
    }

    /// Expression roots checked in this body's environment, not foreign nominal definitions.
    #[must_use]
    pub fn owned_expression_roots(&self) -> std::vec::IntoIter<&hir::Expr> {
        // Materialize the root inventory here, rather than specializing this large
        // heterogeneous iterator pipeline in every checking/publication consumer.
        self.declaration_expression_roots()
            .chain(self.field_bound_roots(ExpressionRootScope::ThisBody))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn field_bound_roots(&self, scope: ExpressionRootScope) -> impl Iterator<Item = &hir::Expr> {
        self.semantic
            .type_defs
            .constrained_fields()
            .filter(move |(key, _)| self.field_bound_scope(key) == scope)
            .flat_map(|(_, field)| field.domain_bounds().iter())
            .map(|bound| &*bound.value)
    }

    fn field_bound_scope(&self, key: &ResolvedStructFieldTypeKey) -> ExpressionRootScope {
        if self.frame.owner(key.owning_type.owner()) == self.dag_id() {
            ExpressionRootScope::ThisBody
        } else {
            ExpressionRootScope::ReferencedBody
        }
    }

    fn declaration_expression_roots(&self) -> impl Iterator<Item = &hir::Expr> {
        self.decls
            .consts()
            .map(|entry| &*entry.expr)
            .chain(
                self.decls
                    .params()
                    .filter_map(|entry| entry.default.as_deref()),
            )
            .chain(
                self.decls
                    .nodes()
                    .filter_map(|entry| entry.definition.formula().map(|expr| &**expr)),
            )
            .chain(
                self.semantic
                    .domain_bounds
                    .values()
                    .flatten()
                    .map(|bound| &*bound.value),
            )
            .chain(self.decls.plots().flat_map(|entry| {
                entry
                    .body
                    .encodings
                    .iter()
                    .map(|(_, expr)| &**expr)
                    .chain(entry.body.mark_properties.iter().map(|field| &*field.value))
                    .chain(entry.body.properties.iter().map(|field| &*field.value))
            }))
            .chain(
                self.decls
                    .figures()
                    .flat_map(|entry| &entry.fields)
                    .chain(self.decls.layers().flat_map(|entry| &entry.fields))
                    .map(|field| &*field.value),
            )
            .chain(
                self.semantic
                    .dynamic_unit_scales
                    .values()
                    .map(|entry| &*entry.expr),
            )
            .chain(
                self.decls
                    .asserts()
                    .flat_map(|entry| entry.body.expressions()),
            )
    }

    #[must_use]
    pub const fn assumes_map(&self) -> &HashMap<ResolvedDeclName, Vec<ResolvedDeclName>> {
        &self.assumes_map
    }

    /// Return one assertion's resolved expected-fail configuration.
    #[must_use]
    pub fn expected_fail(&self, assertion: &ResolvedDeclName) -> Option<&ExpectedFail> {
        self.expected_fail
            .get(assertion)
            .map(|metadata| &metadata.expected)
    }

    /// Iterate over expected-fail configurations without exposing diagnostic provenance.
    pub fn expected_fail_entries(
        &self,
    ) -> impl Iterator<Item = (&ResolvedDeclName, &ExpectedFail)> {
        self.expected_fail
            .iter()
            .map(|(name, metadata)| (name, &metadata.expected))
    }

    #[must_use]
    pub const fn imported_bindings(
        &self,
    ) -> &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding> {
        &self.imported_bindings
    }

    /// Every imported binding with the declaration this DAG's bodies read it
    /// as: the imported declaration, re-owned by this DAG's frame when an
    /// enclosing template owns it.
    pub fn imported_binding_destinations(
        &self,
    ) -> impl Iterator<
        Item = (
            &crate::ir::imported_binding::ImportedBinding,
            ResolvedDeclName,
        ),
    > {
        self.imported_bindings
            .values()
            .map(|binding| (binding, self.frame.rebase(binding.target())))
    }

    /// The runtime destination of an imported value: its target re-keyed
    /// through this DAG's frame (identity for canonical DAGs).
    #[must_use]
    pub fn imported_destination(&self, target: &ResolvedDeclName) -> ResolvedDeclName {
        self.frame.rebase(target)
    }
}
