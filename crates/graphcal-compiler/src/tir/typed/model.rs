use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use thiserror::Error;

use crate::assertion_expectation::ExpectedFail;
use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::generic_param::GenericParamId;
use crate::hir::nominal::NominalTypeDef;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::semantic::checked_type::{CheckedType, IndexTypeRef};
use crate::semantic::dimension_table::BaseDimensionInfo;
use crate::semantic::index_def::IndexDef;
use crate::semantic::unit_scale::UnitInfo;
use crate::semantic_error::SemanticError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::source_id::SourceId;
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName};

use super::resolved_type::{ResolvedDeclType, ResolvedGenericArg};

/// Convert a [`NatOverflowError`](crate::nat::NatOverflowError)
/// into a spanned [`SemanticError`].
#[must_use]
pub fn nat_overflow_error(
    err: crate::nat::NatOverflowError,
    src: SourceId,
    span: Span,
) -> SemanticError {
    SemanticError::located(
        src,
        span,
        EvaluationError::Failed {
            message: err.to_string(),
        },
    )
}

/// Authoritative project type-system definitions keyed by
/// [`ResolvedName`](crate::resolved_name::ResolvedName) identities.
///
/// Every module's owner-qualified
/// [`ModuleDefinitions`](crate::ir::module_definitions::ModuleDefinitions)
/// fill this store directly. Checked TIR retains a [`FormattingRegistry`] for
/// diagnostics and input boundaries; every canonical type-system lookup reads
/// this store. Source spellings are resolved through
/// [`ModuleResolver`](crate::resolve::ModuleResolver), and
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
    ) -> Result<(), crate::ir::prelude_definitions::PreludeDefinitionError> {
        let prelude = crate::ir::prelude_definitions::prelude_definitions()?;
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
        nominal_types: &crate::hir::nominal::NominalTypeRegistry,
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

/// Owner-qualified key for a domain constraint declared on a struct/union field.
///
/// The owning type carries a canonical owner when module-aware type resolution
/// supplied one. The constructor remains a separate typed leaf because union
/// members can share the same field names with different constraints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructFieldConstraintKey {
    pub owning_type: crate::semantic::checked_type::StructTypeRef,
    pub generic_args: Vec<crate::semantic::checked_type::CheckedGenericArg>,
    pub constructor: ConstructorName,
    pub field: FieldName,
}

impl StructFieldConstraintKey {
    /// Construct a key for a non-generic nominal type.
    #[must_use]
    pub const fn new(
        owning_type: crate::semantic::checked_type::StructTypeRef,
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
        owning_type: crate::semantic::checked_type::StructTypeRef,
        generic_args: Vec<crate::semantic::checked_type::CheckedGenericArg>,
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
    pub value: crate::hir::expr::CheckedExpr,
    /// Span of the whole bound.
    pub span: Span,
    /// Source file whose bytes are indexed by `span` and the expression spans.
    pub src: SourceId,
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
    /// Returns a [`SemanticError`] when the type contains unresolved generic
    /// parameters.
    pub(crate) fn new(resolved: ResolvedDeclType, src: SourceId) -> Result<Self, SemanticError> {
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
    pub decl_type: crate::hir::types::DeclType,
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
    type Expr = crate::hir::expr::CheckedExpr;
    type TypeAnnotation = CheckedTypeAnnotation;
    type NodeDefinition = crate::hir::node_definition::NodeDefinition;
    type AssertBody = crate::hir::expr::CheckedAssertBody;
    type PlotBody = crate::ir::model::LoweredPlotBody;
    type CompositionFields = Vec<crate::ir::model::LoweredPlotField>;
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
    #[cfg(any(test, feature = "test-identities"))]
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
    pub dynamic_unit_scales: HashMap<ResolvedUnitName, crate::ir::model::DynamicUnitScaleEntry>,
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
    HashMap<crate::plugin_identity::ExternFnKey, crate::ir::extern_function::ExternFunctionEntry>;

/// Project-wide type services shared by every TIR state: formatting, the
/// owner-qualified type store, instance runtime units, and extern signatures.
#[derive(Debug, Clone)]
pub(crate) struct TirCore {
    pub(super) registry: FormattingRegistry,
    pub(super) project_types: Arc<ProjectTypeStore>,
    pub(super) runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    pub(super) extern_functions: HashMap<
        crate::plugin_identity::ExternFnKey,
        crate::ir::extern_function::ExternFunctionEntry,
    >,
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
    ) -> &HashMap<
        crate::plugin_identity::ExternFnKey,
        crate::ir::extern_function::ExternFunctionEntry,
    > {
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
    pub(crate) fn index_def<V: crate::semantic::checked_type::Concreteness>(
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
    pub(super) fn insert_runtime_unit(
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

pub(crate) use crate::ir::model::ResolvedExpectedFailMetadata;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpressionRootScope {
    ThisBody,
    ReferencedBody,
}

/// The per-DAG compiled body — every field that's specific to one DAG (the
/// file's own top-level body or an inline `dag X { ... }` child).
///
/// Type resolution installs the file root in a
/// [`TirDraft`](super::program::TirDraft), and project
/// assembly adds inline and dependency DAGs through
/// [`TirDraft::add_inline_dag`](super::program::TirDraft::add_inline_dag). A
/// checked body is a
/// [`CheckedDag`](super::checked_dag::CheckedDag).
#[derive(Debug, Clone)]
pub struct DagTIR {
    pub(crate) dag_id: crate::dag_id::DagId,
    /// Every declaration owned by this DAG, keyed by canonical identity, in
    /// source order.
    pub(crate) decls: crate::ir::decl_table::DeclTable<Typed>,
    pub(crate) included_plots: Vec<crate::ir::model::IncludedPlotEntry>,
    pub(crate) semantic: DagSemanticBody,
    pub(crate) static_ports: Vec<crate::hir::source_interface::StaticPort>,
    pub(crate) assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    pub(crate) expected_fail: HashMap<ResolvedDeclName, ResolvedExpectedFailMetadata>,
    pub(crate) imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    pub(crate) semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
    /// How this DAG's bodies name its declarations; built with the DAG, and
    /// assigned only by the type resolver and instance specialization.
    pub(in crate::tir::typed) frame: crate::ir::instance::frame::InstanceFrame,
    pub(crate) projectable_outputs: std::collections::HashSet<DeclName>,
}

impl DagTIR {
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
    ///
    /// Visible only to the checker, which checks each body in the frame of
    /// the DAG that runs it. Evaluation never sees a frame: it reads handles
    /// only resolved, as the references of a
    /// [`ScopedNode`](super::scoped_node::ScopedNode) of a body looked up by its owner's
    /// identity, and reads include-site
    /// projections already resolved through
    /// [`CheckedInstance`](super::CheckedInstance).
    #[must_use]
    pub(in crate::tir) const fn frame(&self) -> &crate::ir::instance::frame::InstanceFrame {
        &self.frame
    }

    #[must_use]
    pub const fn is_semantic_instance(&self) -> bool {
        self.frame.is_instance()
    }

    /// Typed Static interface authored directly in this reusable DAG.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::source_interface::StaticPort] {
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
    pub fn const_expr(&self, key: &ResolvedDeclName) -> Option<&crate::hir::expr::Expr> {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Const(entry) => Some(&*entry.expr),
            _ => None,
        }
    }

    /// Look up the single authoritative HIR expression owned by a param or node.
    #[must_use]
    pub fn runtime_expr(&self, key: &ResolvedDeclName) -> Option<&crate::hir::expr::Expr> {
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
    pub fn value_expr(&self, key: &ResolvedDeclName) -> Option<&crate::hir::expr::Expr> {
        self.const_expr(key).or_else(|| self.runtime_expr(key))
    }

    /// Look up the single authoritative HIR body owned by an assertion.
    #[must_use]
    pub fn assert_body(&self, key: &ResolvedDeclName) -> Option<&crate::hir::expr::AssertBody> {
        match self.decls.get(key)? {
            crate::ir::entry::Decl::Assert(entry) => Some(&*entry.body),
            _ => None,
        }
    }

    /// Visit every source unit reference used by this DAG.
    pub fn visit_unit_references(
        &self,
        visitor: &mut impl FnMut(&crate::hir::expr::LocalUnit, Span),
    ) {
        self.visit_expressions(&mut |expr| match expr.kind() {
            crate::hir::expr::ExprKind::QuantityLiteral { unit, .. } => unit
                .terms
                .iter()
                .for_each(|term| visitor(&term.name.value, term.name.span)),
            crate::hir::expr::ExprKind::Convert { target, .. } => target
                .terms
                .iter()
                .for_each(|term| visitor(&term.name.value, term.name.span)),
            _ => {}
        });
    }

    /// Visit semantic expressions, including referenced nominal bounds for dependency analysis.
    pub(crate) fn visit_expressions<'a>(
        &'a self,
        visitor: &mut dyn FnMut(&'a crate::hir::expr::Expr),
    ) {
        self.owned_expression_roots()
            .chain(self.field_bound_roots(ExpressionRootScope::ReferencedBody))
            .for_each(|root| crate::hir::expr::visit_expr(root, visitor));
    }

    /// Expression roots checked in this body's environment, not foreign nominal definitions.
    #[must_use]
    pub fn owned_expression_roots(&self) -> std::vec::IntoIter<&crate::hir::expr::Expr> {
        // Materialize the root inventory here, rather than specializing this large
        // heterogeneous iterator pipeline in every checking/publication consumer.
        self.declaration_expression_roots()
            .chain(self.field_bound_roots(ExpressionRootScope::ThisBody))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn field_bound_roots(
        &self,
        scope: ExpressionRootScope,
    ) -> impl Iterator<Item = &crate::hir::expr::Expr> {
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

    fn declaration_expression_roots(&self) -> impl Iterator<Item = &crate::hir::expr::Expr> {
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
