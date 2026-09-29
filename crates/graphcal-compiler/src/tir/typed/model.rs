use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::assertion_expectation::ExpectedFail;
use crate::declaration_category::DeclCategory;
use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational};
use crate::hir;
use crate::hir::NominalTypeDef;
use crate::nat::NatPolyForm;
use crate::registry::declared_type::{DeclaredType, IndexDisplayName, IndexTypeRef};
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{BaseDimensionInfo, FormattingRegistry, IndexDef, UnitInfo};
use crate::resolve::ModuleResolver;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName};

// ---------------------------------------------------------------------------
// Resolved type types
// ---------------------------------------------------------------------------

/// A resolved argument of sort `Dim`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDimArg {
    Dimensionless,
    Concrete(Dimension),
    GenericParam(GenericParamName, Span),
    Expr {
        terms: Vec<ResolvedDimTerm>,
        span: Span,
    },
}

impl ResolvedDimArg {
    #[must_use]
    pub(crate) fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Dimensionless => "Dimensionless".to_string(),
            Self::Concrete(dim) => {
                let formatted = registry.dimensions.format_dimension(dim);
                if formatted.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    formatted
                }
            }
            Self::GenericParam(name, _) => name.to_string(),
            Self::Expr { terms, .. } => terms
                .iter()
                .map(|term| term.format(registry))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// A generic argument resolved according to its declared sort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedGenericArg {
    Dim(ResolvedDimArg),
    Index(ResolvedIndex),
    Nat(NatPolyForm, Span),
    Type(ResolvedTypeExpr),
}

impl ResolvedGenericArg {
    #[must_use]
    pub(crate) fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Dim(dim) => dim.format(registry),
            Self::Index(index) => index.to_string(),
            Self::Nat(form, _) => form.format(),
            Self::Type(type_expr) => type_expr.format(registry),
        }
    }
}

/// A fully-resolved type expression.
///
/// Unlike the raw AST `TypeExpr`, every name here has been classified as a
/// concrete dimension, struct, or generic parameter. Index arguments are
/// never type expressions; they live only in [`ResolvedGenericArg::Index`]
/// and the index axes of `Key` / `Indexed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTypeExpr {
    /// `Dimensionless`
    Dimensionless,
    /// `Bool`
    Bool,
    /// `Int`
    Int,
    /// A datetime instant in a specific time scale (e.g., `Datetime` = UTC, `Datetime<TT>`).
    Datetime(TimeScale),
    /// A concrete quantity type, e.g. `Length * Time^-2`.
    Quantity(Dimension),
    /// A dimension-aware complex quantity type, e.g. `Complex<Length>`.
    Complex {
        dimension: ResolvedDimArg,
        span: Span,
    },
    /// An index-key type, e.g. `Key<Maneuver>` or `Key<Fin(3)>`.
    Key { index: ResolvedIndex, span: Span },
    /// A non-generic struct type name, e.g. `TransferResult`.
    Struct(ResolvedStructTypeName, Span),
    /// A generic struct with sort-aware arguments, e.g. `Vec3<Length, ECI>`
    /// or `FixedVec<3>`.
    GenericStruct {
        name: ResolvedStructTypeName,
        generic_args: Vec<ResolvedGenericArg>,
        span: Span,
    },
    /// A single generic dimension parameter, e.g. `D`
    GenericDimParam(GenericParamName, Span),
    /// A generic type parameter, e.g. `F: Type`.
    GenericTypeParam(GenericParamName, Span),
    /// A compound dimension expression containing at least one generic param, e.g. `D^2`
    GenericDimExpr {
        terms: Vec<ResolvedDimTerm>,
        span: Span,
    },
    /// An indexed type, e.g. `Velocity[Maneuver]` or `D[I]`
    Indexed {
        base: Box<Self>,
        indexes: Vec<ResolvedIndex>,
    },
}

impl ResolvedTypeExpr {
    /// Format as a human-readable string, e.g. `"Length / Time^2"`, `"Bool"`, `"Vec3<Length, ECI>"`.
    #[must_use]
    pub fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Dimensionless => "Dimensionless".to_string(),
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Quantity(dim) => {
                let formatted = registry.dimensions.format_dimension(dim);
                if formatted.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    formatted
                }
            }
            Self::Complex { dimension, .. } => {
                format!("Complex<{}>", dimension.format(registry))
            }
            Self::Key { index, .. } => {
                format!("Key<{index}>")
            }
            Self::Struct(name, _) => name.as_str().to_string(),
            Self::GenericStruct {
                name, generic_args, ..
            } => {
                let args: Vec<String> = generic_args
                    .iter()
                    .map(|arg| arg.format(registry))
                    .collect();
                format!("{}<{}>", name.as_str(), args.join(", "))
            }
            Self::GenericDimParam(name, _) | Self::GenericTypeParam(name, _) => name.to_string(),
            Self::GenericDimExpr { terms, .. } => {
                let parts: Vec<String> = terms.iter().map(|t| t.format(registry)).collect();
                parts.join(" ")
            }
            Self::Indexed { base, indexes } => {
                let base_str = base.format(registry);
                let idx_strs: Vec<String> = indexes.iter().map(ToString::to_string).collect();
                format!("{base_str}[{}]", idx_strs.join(", "))
            }
        }
    }
}

/// A single term in a resolved dimension expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDimTerm {
    /// A concrete dimension with power and combining operator.
    Concrete {
        dim: Dimension,
        power: Rational,
        op: MulDivOp,
    },
    /// A generic dimension parameter with power and combining operator.
    GenericParam {
        name: GenericParamName,
        power: Rational,
        op: MulDivOp,
        span: Span,
    },
}

impl ResolvedDimTerm {
    /// Get the combining operator for this term.
    #[must_use]
    pub(crate) const fn op(&self) -> MulDivOp {
        match self {
            Self::Concrete { op, .. } | Self::GenericParam { op, .. } => *op,
        }
    }

    /// Format this term as a human-readable string, e.g. `"Length"`, `"/ Time^2"`, `"D^2"`.
    #[must_use]
    fn format(&self, registry: &FormattingRegistry) -> String {
        let (name, power, op) = match self {
            Self::Concrete { dim, power, op } => {
                (registry.dimensions.format_dimension(dim), *power, *op)
            }
            Self::GenericParam {
                name, power, op, ..
            } => (name.to_string(), *power, *op),
        };
        let prefix = match op {
            MulDivOp::Mul => "",
            MulDivOp::Div => "/ ",
        };
        if power == Rational::ONE {
            format!("{prefix}{name}")
        } else {
            format!(
                "{prefix}{name}{}",
                power.fmt_exponent(crate::ratio::ExponentStyle::Source)
            )
        }
    }
}

/// Normalize an AST `NatExpr` into a `NatPolyForm`.
///
/// All variables referenced must be Nat generic parameters in scope.
/// Returns an error if a variable is not a known Nat param.
pub fn normalize_nat_expr(
    expr: &crate::desugar::desugared_ast::NatExpr,
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<NatPolyForm, GraphcalError> {
    use crate::desugar::desugared_ast::NatExpr;
    match expr {
        NatExpr::Literal(n, _) => Ok(NatPolyForm::from_constant(*n)),
        NatExpr::Var(ident) => {
            let gp = nat_params
                .iter()
                .find(|p| p.as_str() == ident.name.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: IndexName::classify(ident.name.atom().clone()).into(),
                    src: src.clone(),
                    span: ident.span.into(),
                })?;
            Ok(NatPolyForm::from_var(gp.clone()))
        }
        NatExpr::Add(operands, span) => {
            operands
                .iter()
                .try_fold(NatPolyForm::from_constant(0), |sum, operand| {
                    let value = normalize_nat_expr(operand, nat_params, src)?;
                    sum.add(&value)
                        .map_err(|err| nat_overflow_error(err, src, *span))
                })
        }
        NatExpr::Mul(operands, span) => {
            operands
                .iter()
                .try_fold(NatPolyForm::from_constant(1), |product, operand| {
                    let value = normalize_nat_expr(operand, nat_params, src)?;
                    product
                        .mul(&value)
                        .map_err(|err| nat_overflow_error(err, src, *span))
                })
        }
    }
}

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

/// A resolved index in an indexed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedIndex {
    /// A concrete index name, e.g. `Maneuver`.
    Concrete(ResolvedIndexName, Span),
    /// A generic index parameter, e.g. `I`
    GenericParam(GenericParamName, Span),
    /// A structural finite index `Fin(N)` carrying a normalized Nat cardinality.
    Finite(NatPolyForm, Span),
}

/// Renders the source-facing spelling: the declared leaf name, the generic
/// parameter, or `Fin(<Nat form>)` through [`IndexDisplayName`].
impl std::fmt::Display for ResolvedIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Concrete(name, _) => f.write_str(name.as_str()),
            Self::GenericParam(name, _) => name.fmt(f),
            Self::Finite(form, _) => IndexDisplayName::Finite(form.clone()).fmt(f),
        }
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

    #[must_use]
    pub(crate) fn with_rigid_dimension(&self, name: &ResolvedDimName) -> Self {
        let mut rigid = self.clone();
        rigid.dimensions.insert(
            name.clone(),
            Dimension::base(crate::dimension::BaseDimId::UserDefined(name.clone())),
        );
        rigid
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
    pub owning_type: crate::registry::declared_type::StructTypeRef,
    pub generic_args: Vec<crate::registry::declared_type::DeclaredGenericArg>,
    pub constructor: ConstructorName,
    pub field: FieldName,
}

impl StructFieldConstraintKey {
    /// Construct a key for a non-generic nominal type.
    #[must_use]
    pub const fn new(
        owning_type: crate::registry::declared_type::StructTypeRef,
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
        owning_type: crate::registry::declared_type::StructTypeRef,
        generic_args: Vec<crate::registry::declared_type::DeclaredGenericArg>,
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

/// Failure to add a DAG to a checked TIR registry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagRegistryError {
    /// A canonical DAG identity can occur only once in one compiled registry.
    #[error("DAG `{dag_id}` is already present in the TIR registry")]
    DuplicateDag { dag_id: crate::dag_id::DagId },
}

/// Failure to freeze the locally owned portion of an assembly registry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagStoreFreezeError {
    /// Every runtime unit must have a local or imported defining body.
    #[error("runtime unit `{identity}` has no defining DAG in the assembly registry")]
    MissingUnitOwner { identity: ResolvedUnitName },
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

/// Checked registry of compiled DAG modules.
///
/// The root DAG is stored directly, so its presence is structural. Every other
/// entry is keyed internally from [`DagTIR::dag_id`], so a caller cannot pair a
/// DAG body with a different map key. The API exposes no removal operation;
/// consequently every registry always has exactly one designated, reachable root.
#[derive(Debug, Clone)]
pub struct DagRegistry {
    root: DagTIR,
    other_dags: HashMap<crate::dag_id::DagId, DagTIR>,
    /// Immutable bodies imported from an already-frozen module store.
    ///
    /// Assembly owns `root` and `other_dags`; imported bodies are handles only.
    /// Imported bodies have no mutable registry accessor; inserting a detached
    /// copy under an already installed identity is rejected.
    shared_dags: HashMap<crate::dag_id::DagId, Arc<DagTIR>>,
}

/// Immutable canonical bodies published by one module in a compilation session.
///
/// The store is created by consuming an assembly registry. It has no mutation
/// or completion API. Cloning its local index shares body and unit handles;
/// project artifacts share the complete store through `Arc`.
#[derive(Debug, Clone)]
pub struct DagStore {
    dags: HashMap<crate::dag_id::DagId, Arc<DagTIR>>,
    runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
}

impl DagStore {
    /// Look up a canonical immutable body.
    #[must_use]
    pub fn get(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        self.dags.get(dag_id).map(AsRef::as_ref)
    }

    /// Borrow the canonical body handle for pointer-identity checks and sharing.
    #[must_use]
    pub fn handle(&self, dag_id: &crate::dag_id::DagId) -> Option<&Arc<DagTIR>> {
        self.dags.get(dag_id)
    }

    /// Iterate over canonical immutable bodies.
    pub fn iter(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
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

impl DagRegistry {
    fn new(root: DagTIR) -> Self {
        Self {
            root,
            other_dags: HashMap::new(),
            shared_dags: HashMap::new(),
        }
    }

    /// Consume the mutable local assembly bodies into immutable handles.
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

    /// Canonical identity of this registry's root DAG.
    #[must_use]
    pub const fn root_id(&self) -> &crate::dag_id::DagId {
        self.root.dag_id()
    }

    /// Borrow the root DAG.
    #[must_use]
    pub const fn root(&self) -> &DagTIR {
        &self.root
    }

    const fn root_mut(&mut self) -> &mut DagTIR {
        &mut self.root
    }

    fn insert(&mut self, dag: DagTIR) -> Result<(), DagRegistryError> {
        let dag_id = dag.dag_id.clone();
        if &dag_id == self.root_id() || self.shared_dags.contains_key(&dag_id) {
            return Err(DagRegistryError::DuplicateDag { dag_id });
        }
        match self.other_dags.entry(dag_id.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(dag);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(DagRegistryError::DuplicateDag { dag_id })
            }
        }
    }

    /// Look up one DAG by canonical identity.
    #[must_use]
    pub fn get(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        if dag_id == self.root_id() {
            Some(&self.root)
        } else {
            self.other_dags
                .get(dag_id)
                .or_else(|| self.shared_dags.get(dag_id).map(AsRef::as_ref))
        }
    }

    pub(crate) fn get_mut(&mut self, dag_id: &crate::dag_id::DagId) -> Option<&mut DagTIR> {
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
    pub fn iter(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
        self.local_iter()
            .chain(self.shared_dags.iter().map(|(id, dag)| (id, dag.as_ref())))
    }

    /// Iterate over canonical DAG identities.
    pub fn keys(&self) -> impl Iterator<Item = &crate::dag_id::DagId> {
        self.iter().map(|(id, _)| id)
    }

    /// Iterate over identities owned by this mutable assembly registry.
    pub(crate) fn local_keys(&self) -> impl Iterator<Item = &crate::dag_id::DagId> {
        self.local_iter().map(|(id, _)| id)
    }

    /// Iterate mutably over DAG bodies while preserving their registry keys.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut DagTIR> {
        std::iter::once(&mut self.root).chain(self.other_dags.values_mut())
    }

    /// Iterate over DAG bodies.
    pub fn values(&self) -> impl Iterator<Item = &DagTIR> {
        self.iter().map(|(_, dag)| dag)
    }

    /// Add handles to an immutable module store during assembly.
    pub(crate) fn insert_shared_store(&mut self, store: &DagStore) -> Result<(), DagRegistryError> {
        if let Some(dag_id) = store.dags.keys().find(|dag_id| {
            *dag_id == self.root_id()
                || self.other_dags.contains_key(*dag_id)
                || self.shared_dags.contains_key(*dag_id)
        }) {
            return Err(DagRegistryError::DuplicateDag {
                dag_id: dag_id.clone(),
            });
        }
        self.shared_dags.extend(
            store
                .dags
                .iter()
                .map(|(dag_id, dag)| (dag_id.clone(), Arc::clone(dag))),
        );
        Ok(())
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

impl std::ops::Index<&crate::dag_id::DagId> for DagRegistry {
    type Output = DagTIR;

    fn index(&self, index: &crate::dag_id::DagId) -> &Self::Output {
        if index == self.root_id() {
            &self.root
        } else {
            self.other_dags
                .get(index)
                .unwrap_or_else(|| self.shared_dags[index].as_ref())
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
    resolved_type: ResolvedTypeExpr,
    domain_bounds: Vec<ResolvedDomainBound>,
}

impl ResolvedStructFieldSemantics {
    #[must_use]
    pub const fn new(
        resolved_type: ResolvedTypeExpr,
        domain_bounds: Vec<ResolvedDomainBound>,
    ) -> Self {
        Self {
            resolved_type,
            domain_bounds,
        }
    }

    /// Return the field annotation resolved in its owning generic scope.
    #[must_use]
    pub const fn resolved_type(&self) -> &ResolvedTypeExpr {
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
    pub(crate) generic_defaults:
        HashMap<(ResolvedStructTypeName, GenericParamName), ResolvedGenericDefault>,
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
    pub fn field_type(&self, key: &ResolvedStructFieldTypeKey) -> Option<&ResolvedTypeExpr> {
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
    resolved: ResolvedTypeExpr,
    declared: DeclaredType,
}

impl CheckedDeclType {
    /// Check that a resolved declaration type is concrete.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] when the type contains unresolved generic
    /// parameters.
    pub(crate) fn new(
        resolved: ResolvedTypeExpr,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Self, GraphcalError> {
        let declared = super::ops::resolved_to_declared_type(&resolved, src)?;
        Ok(Self { resolved, declared })
    }

    /// The resolved TIR type, including declaration-level index axes.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedTypeExpr {
        &self.resolved
    }

    /// The concrete declared type.
    #[must_use]
    pub const fn declared(&self) -> &DeclaredType {
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
    /// Absent only during assembly; publication requires complete coverage.
    pub(crate) expression_facts: Option<crate::tir::expression_facts::CheckedExpressionFacts>,
    /// Checked structured display and plot-channel presentation facts.
    pub presentation: crate::tir::presentation::DagPresentationFacts,
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

/// Mutable assembly state for a project TIR.
///
/// Type resolution creates this builder with a mandatory root DAG. Project
/// lowering may then add file-defined child DAGs, imported DAG modules, and the
/// completed canonical project type store. [`Self::finish`] consumes the
/// mutable shell and exposes an immutable [`TIR`].
#[derive(Debug)]
pub struct TirBuilder {
    registry: FormattingRegistry,
    project_types: Arc<ProjectTypeStore>,
    dags: DagRegistry,
    runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    extern_functions:
        HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry>,
}

impl TirBuilder {
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
            registry,
            project_types,
            dags: DagRegistry::new(root),
            runtime_units: HashMap::new(),
            extern_functions,
        }
    }

    /// Borrow the file-root DAG during assembly.
    #[must_use]
    pub const fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    /// Mutably borrow the file-root DAG during assembly.
    pub const fn root_mut(&mut self) -> &mut DagTIR {
        self.dags.root_mut()
    }

    /// Borrow the root file's post-resolution formatting services.
    #[must_use]
    pub const fn registry(&self) -> &FormattingRegistry {
        &self.registry
    }

    /// Borrow the owner-qualified type store accumulated for this project.
    #[must_use]
    pub fn project_type_store(&self) -> &ProjectTypeStore {
        self.project_types.as_ref()
    }

    /// Add a compiled DAG under its own canonical identity.
    ///
    /// # Errors
    ///
    /// Returns [`DagRegistryError::DuplicateDag`] rather than replacing an
    /// existing root, child, or dependency DAG.
    pub fn insert_dag(&mut self, dag: DagTIR) -> Result<(), DagRegistryError> {
        self.dags.insert(dag)
    }

    /// Add immutable bodies from a previously frozen module store.
    ///
    /// Only handles are copied. The body and its checked semantic facts remain
    /// owned by the store that published them.
    pub fn insert_shared_dag_store(&mut self, store: &DagStore) -> Result<(), DagStoreInsertError> {
        if let Some((name, _)) = store.runtime_units.iter().find(|(name, info)| {
            self.runtime_units
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
        self.runtime_units.extend(
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
        match self.extern_functions.get(&key) {
            Some(existing) if !existing.has_same_callable_definition(&function) => {
                Err(CompetingExternFunctionDefinition {
                    plugin: key.plugin,
                    name: key.name,
                })
            }
            Some(_) => Ok(()),
            None => {
                self.extern_functions.insert(key, function);
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
    ///
    /// # Errors
    ///
    /// Returns [`GraphcalError::InvalidExternSignature`] when a declaration
    /// conflicts with an already-merged signature of the same plugin function.
    pub fn merge_declared_extern_functions(
        &mut self,
        hir: &crate::ir::lower::HirDag,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        // Deterministic conflict reporting: earliest declaration first.
        let mut declared: Vec<_> = hir.extern_functions().iter().collect();
        declared.sort_by_key(|(_, function)| function.decl_span.offset());
        declared.into_iter().try_for_each(|(_, function)| {
            crate::ir::extern_fns::merge_extern_function(
                &mut self.extern_functions,
                function.clone(),
                src,
            )
        })
    }

    /// Finalize project assembly into an immutable, structurally valid TIR.
    #[must_use]
    pub fn finish(self) -> TIR {
        TIR {
            registry: self.registry,
            project_types: self.project_types,
            dags: self.dags,
            runtime_units: self.runtime_units,
            extern_functions: self.extern_functions,
        }
    }
}

/// Immutable Typed Intermediate Representation of one Graphcal file and every
/// canonical DAG module reachable from it.
///
/// Root presence and DAG key/body identity are enforced by the private checked
/// [`DagRegistry`]. Safe clients can inspect but cannot remove, replace, or
/// re-key finalized DAG bodies.
#[derive(Debug, Clone)]
pub struct TIR {
    pub(crate) registry: FormattingRegistry,
    pub(in crate::tir::typed) project_types: Arc<ProjectTypeStore>,
    pub(crate) dags: DagRegistry,
    pub(crate) runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    pub(crate) extern_functions:
        HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry>,
}

impl TIR {
    pub(super) fn insert_materialized_dag(&mut self, dag: DagTIR) -> Result<(), DagRegistryError> {
        self.dags.insert(dag)
    }

    /// Borrow the root DAG. Root presence is guaranteed by [`DagRegistry`].
    #[must_use]
    pub const fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    #[cfg(test)]
    pub(crate) const fn root_mut(&mut self) -> &mut DagTIR {
        self.dags.root_mut()
    }

    /// Canonical identity of the root DAG.
    #[must_use]
    pub const fn root_dag_id(&self) -> &crate::dag_id::DagId {
        self.dags.root_id()
    }

    /// Borrow the root file's immutable post-resolution formatting services.
    #[must_use]
    pub const fn registry(&self) -> &FormattingRegistry {
        &self.registry
    }

    /// Borrow the authoritative owner-qualified project type store.
    #[must_use]
    pub fn project_type_store(&self) -> &ProjectTypeStore {
        self.project_types.as_ref()
    }

    /// Borrow every local and imported DAG through the read-only checked registry.
    #[must_use]
    pub const fn dag_registry(&self) -> &DagRegistry {
        &self.dags
    }

    /// Consume this TIR's mutable local assembly bodies into an immutable store.
    /// Imported bodies and runtime units remain owned by their publishing module.
    ///
    /// # Errors
    ///
    /// Returns [`DagStoreFreezeError`] if a runtime unit has no defining body.
    pub fn freeze_local_dag_store(self) -> Result<DagStore, DagStoreFreezeError> {
        self.dags.freeze_local(self.runtime_units)
    }

    /// Iterate over every DAG owned by this file, including the root and all
    /// nested descendants. Inline `dag` syntax and file modules use the same
    /// canonical representation and traversal.
    pub fn local_dags(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
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
    pub fn dag_containing_declaration(&self, declaration: &ResolvedDeclName) -> Option<&DagTIR> {
        self.dags
            .get(declaration.owner())
            .filter(|dag| dag.declaration_index.values.contains_key(declaration))
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
    pub const fn extern_functions(
        &self,
    ) -> &HashMap<crate::plugin_identity::ExternFnKey, crate::ir::lower::ExternFunctionEntry> {
        &self.extern_functions
    }

    /// Look up a dimension by its canonical defining-module identity.
    #[must_use]
    pub fn dimension(&self, name: &ResolvedDimName) -> Option<&Dimension> {
        self.project_types.get_dimension(name)
    }

    /// Look up a unit by its canonical defining-module identity.
    #[must_use]
    pub fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.runtime_units
            .get(name)
            .map(AsRef::as_ref)
            .or_else(|| self.project_types.get_unit(name))
    }

    /// Install one instance-owned dynamic unit in the mutable assembly overlay.
    pub(crate) fn insert_runtime_unit(
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

    /// Look up a declared index by its canonical defining-module identity.
    #[must_use]
    pub fn declared_index_def(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.project_types.get_index(name)
    }

    /// Resolve a declared axis or derive a structural axis from its cardinality.
    /// Structural `Fin(N)` definitions never depend on a source-registration scan.
    #[must_use]
    pub fn index_def(&self, index: &IndexTypeRef) -> Option<std::borrow::Cow<'_, IndexDef>> {
        match index.finite_index() {
            Some(finite) => Some(std::borrow::Cow::Owned(IndexDef::finite(finite))),
            None => self
                .project_types
                .get_index(index.declared_resolved()?)
                .map(std::borrow::Cow::Borrowed),
        }
    }

    /// Look up a nominal type by its canonical defining-module identity.
    #[must_use]
    pub fn struct_type_def(&self, name: &ResolvedStructTypeName) -> Option<&NominalTypeDef> {
        self.project_types.get_struct_type(name)
    }

    /// Find the checked DAG carrying resolved field metadata for a nominal type.
    #[must_use]
    pub(crate) fn dag_with_type_metadata(&self, name: &ResolvedStructTypeName) -> Option<&DagTIR> {
        self.dags.iter().find_map(|(_, dag)| {
            dag.semantic
                .type_defs
                .struct_types
                .contains_key(name)
                .then_some(dag)
        })
    }

    /// Iterate every canonical nominal definition in the project type store.
    pub fn nominal_type_defs(
        &self,
    ) -> impl Iterator<Item = (&ResolvedStructTypeName, &NominalTypeDef)> {
        self.project_types
            .struct_types
            .iter()
            .map(|(identity, definition)| (identity, definition.as_ref()))
    }

    /// Iterate canonical index definitions owned by the root module.
    pub fn root_declared_indexes(&self) -> impl Iterator<Item = &IndexDef> {
        let owner = self.root_dag_id();
        self.project_types
            .indexes
            .iter()
            .filter(move |(name, _)| name.owner() == owner)
            .map(|(_, definition)| definition.as_ref())
    }

    /// Returns true if this file declares any required param or required index.
    #[must_use]
    pub fn is_library(&self) -> bool {
        self.root()
            .params
            .iter()
            .any(|param| param.default.is_none())
            || self
                .root_declared_indexes()
                .any(crate::registry::types::IndexDef::is_required)
    }
}

/// Index into an authoritative value-declaration record.
#[derive(Debug, Clone, Copy)]
enum ValueDeclarationSlot {
    Const(usize),
    Param(usize),
    Node(usize),
}

/// Derived lookup indexes for declaration records. Bodies remain owned solely
/// by the category-specific vectors on [`DagTIR`].
#[derive(Debug, Clone, Default)]
pub(super) struct DagDeclarationIndex {
    values: HashMap<ResolvedDeclName, ValueDeclarationSlot>,
    assertions: HashMap<ResolvedDeclName, usize>,
}

/// Checked declaration records could not be indexed.
#[derive(Debug, Error)]
pub(super) enum DeclarationRecordError {
    /// Two records share one canonical identity.
    #[error("duplicate checked declaration record `{name}`")]
    Duplicate { name: ScopedName, span: Span },
    /// A record is owned by a different DAG than the one storing it.
    #[error("checked declaration record `{identity}` is stored in DAG `{dag_id}`")]
    ForeignOwner {
        identity: ResolvedDeclName,
        dag_id: crate::dag_id::DagId,
        span: Span,
    },
}

impl DeclarationRecordError {
    pub(super) const fn span(&self) -> Span {
        match self {
            Self::Duplicate { span, .. } | Self::ForeignOwner { span, .. } => *span,
        }
    }
}

pub(crate) use crate::ir::lower::ResolvedExpectedFailMetadata;

/// One value, assertion, or visualization declaration in evaluation source
/// order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOrderEntry {
    /// Source-facing spelling in this DAG body.
    pub name: ScopedName,
    /// Canonical identity; concrete instances carry their runtime identity.
    pub identity: ResolvedDeclName,
    pub category: DeclCategory,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpressionRootScope {
    ThisBody,
    ReferencedBody,
}

/// The per-DAG compiled body — every field that's specific to one DAG (the
/// file's own top-level body or an inline `dag X { ... }` child).
///
/// Stored in the checked [`DagRegistry`]: type resolution installs the file
/// root, and project assembly adds inline and dependency DAGs through
/// [`TirBuilder::insert_dag`].
#[derive(Debug, Clone)]
pub struct DagTIR {
    pub(crate) dag_id: crate::dag_id::DagId,
    pub(crate) body_revision: crate::body_revision::BodyRevision,
    pub(crate) consts: Vec<TypedConstEntry>,
    pub(crate) params: Vec<TypedParamEntry>,
    pub(crate) nodes: Vec<TypedNodeEntry>,
    pub(crate) asserts: Vec<TypedAssertEntry>,
    pub(crate) plots: Vec<TypedPlotEntry>,
    pub(crate) figures: Vec<TypedFigureEntry>,
    pub(crate) layers: Vec<TypedLayerEntry>,
    pub(crate) included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
    pub(super) declaration_index: DagDeclarationIndex,
    pub(crate) semantic: DagSemanticBody,
    pub(crate) source_order: Vec<SourceOrderEntry>,
    pub(crate) static_ports: Vec<crate::hir::StaticPort>,
    pub(crate) assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    pub(crate) expected_fail: HashMap<ResolvedDeclName, ResolvedExpectedFailMetadata>,
    pub(crate) imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    pub(crate) semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
    pub(crate) semantic_specialization:
        Option<crate::ir::static_substitution::StaticSpecializationId>,
    pub(crate) runtime_owner_rebases: HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
    pub(crate) projectable_outputs: std::collections::HashSet<DeclName>,
}

impl DagTIR {
    #[must_use]
    pub const fn body_revision(&self) -> &crate::body_revision::BodyRevision {
        &self.body_revision
    }

    pub(crate) fn begin_checking_revision(&mut self) {
        self.body_revision = crate::body_revision::BodyRevision::fresh();
        self.semantic.expression_facts = None;
        self.semantic.presentation = crate::tir::presentation::DagPresentationFacts::default();
    }

    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        &self.dag_id
    }

    #[must_use]
    pub fn consts(&self) -> &[TypedConstEntry] {
        &self.consts
    }

    #[must_use]
    pub fn params(&self) -> &[TypedParamEntry] {
        &self.params
    }

    #[must_use]
    pub fn nodes(&self) -> &[TypedNodeEntry] {
        &self.nodes
    }

    #[must_use]
    pub fn asserts(&self) -> &[TypedAssertEntry] {
        &self.asserts
    }

    #[must_use]
    pub fn plots(&self) -> &[TypedPlotEntry] {
        &self.plots
    }

    #[must_use]
    pub fn figures(&self) -> &[TypedFigureEntry] {
        &self.figures
    }

    #[must_use]
    pub fn layers(&self) -> &[TypedLayerEntry] {
        &self.layers
    }

    #[must_use]
    pub const fn semantic(&self) -> &DagSemanticBody {
        &self.semantic
    }

    /// Look up checked plot-channel presentation facts.
    #[must_use]
    pub fn plot_channel_presentations(
        &self,
        plot: &ResolvedDeclName,
    ) -> Option<&HashMap<crate::syntax::ast::EncodingChannel, crate::plot_shape::PlotChannelShape>>
    {
        self.semantic.presentation.plot_channels.get(plot)
    }

    pub fn expression_facts(
        &self,
    ) -> Result<
        &crate::tir::expression_facts::CheckedExpressionFacts,
        crate::tir::expression_facts::ExpressionFactsError,
    > {
        let facts = self
            .semantic
            .expression_facts
            .as_ref()
            .ok_or(crate::tir::expression_facts::ExpressionFactsError::WrongEnvironment)?;
        facts.validate_environment(self.dag_id(), self.body_revision())?;
        Ok(facts)
    }

    /// Semantic include edges authored directly by this DAG.
    #[must_use]
    pub fn semantic_instances(&self) -> &[crate::ir::instance::HirInstanceRecord] {
        &self.semantic_instances
    }

    /// Map a template-owned body reference into this concrete instance.
    #[must_use]
    pub fn runtime_decl_identity(&self, target: &ResolvedDeclName) -> ResolvedDeclName {
        self.runtime_owner_rebases.get(target.owner()).map_or_else(
            || match &self.semantic_specialization {
                Some(specialization) if target.owner() == &specialization.template => {
                    ResolvedDeclName::from_def(self.dag_id.clone(), target.to_unowned_def_name())
                }
                Some(_) | None => target.clone(),
            },
            |owner| ResolvedDeclName::from_def(owner.clone(), target.to_unowned_def_name()),
        )
    }

    /// Map a template-owned unit reference into this concrete instance.
    #[must_use]
    pub fn runtime_unit_identity(
        &self,
        target: &crate::resolved_name::ResolvedUnitName,
    ) -> crate::resolved_name::ResolvedUnitName {
        self.runtime_owner_rebases.get(target.owner()).map_or_else(
            || match &self.semantic_specialization {
                Some(specialization) if target.owner() == &specialization.template => {
                    crate::resolved_name::ResolvedUnitName::from_def(
                        self.dag_id.clone(),
                        target.to_unowned_def_name(),
                    )
                }
                Some(_) | None => target.clone(),
            },
            |owner| {
                crate::resolved_name::ResolvedUnitName::from_def(
                    owner.clone(),
                    target.to_unowned_def_name(),
                )
            },
        )
    }

    /// Apply this instance's Static nominal-type substitution.
    #[must_use]
    pub fn runtime_struct_type_identity(
        &self,
        source: &ResolvedStructTypeName,
    ) -> ResolvedStructTypeName {
        self.semantic_specialization
            .as_ref()
            .and_then(|specialization| specialization.substitution.types.get(source))
            .cloned()
            .unwrap_or_else(|| source.clone())
    }

    /// Apply this instance's Static index substitution to a runtime axis.
    #[must_use]
    pub fn runtime_index_type_ref(&self, source: &ResolvedIndexName) -> IndexTypeRef {
        let target = self
            .semantic_specialization
            .as_ref()
            .and_then(|specialization| specialization.substitution.indexes.get(source));
        match target {
            Some(crate::ir::static_substitution::InstanceIndexBindingTarget::Declared(target)) => {
                IndexTypeRef::from_resolved(target.clone())
            }
            Some(crate::ir::static_substitution::InstanceIndexBindingTarget::Finite(target)) => {
                IndexTypeRef::from_finite_index(*target)
            }
            None => IndexTypeRef::from_resolved(source.clone()),
        }
    }

    #[must_use]
    pub const fn is_semantic_instance(&self) -> bool {
        self.semantic_specialization.is_some()
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

    /// Build identity-to-record indexes after all declaration vectors are installed.
    ///
    /// Every record must be owned by this DAG, so a declaration's owner alone
    /// locates its record project-wide.
    pub(super) fn index_declaration_records(&mut self) -> Result<(), DeclarationRecordError> {
        let mut index = DagDeclarationIndex::default();
        let own = |identity: &ResolvedDeclName, span: Span| {
            if identity.owner() == &self.dag_id {
                Ok(())
            } else {
                Err(DeclarationRecordError::ForeignOwner {
                    identity: identity.clone(),
                    dag_id: self.dag_id.clone(),
                    span,
                })
            }
        };
        for (slot, name, key, span) in self
            .consts
            .iter()
            .enumerate()
            .map(|(slot, entry)| {
                (
                    ValueDeclarationSlot::Const(slot),
                    &entry.name,
                    entry.identity(),
                    entry.span,
                )
            })
            .chain(self.params.iter().enumerate().map(|(slot, entry)| {
                (
                    ValueDeclarationSlot::Param(slot),
                    &entry.name,
                    entry.identity(),
                    entry.span,
                )
            }))
            .chain(self.nodes.iter().enumerate().map(|(slot, entry)| {
                (
                    ValueDeclarationSlot::Node(slot),
                    &entry.name,
                    entry.identity(),
                    entry.span,
                )
            }))
        {
            own(&key, span)?;
            if index.values.insert(key, slot).is_some() {
                return Err(DeclarationRecordError::Duplicate {
                    name: name.clone(),
                    span,
                });
            }
        }
        for (slot, entry) in self.asserts.iter().enumerate() {
            let key = entry.identity();
            own(&key, entry.span)?;
            if index.assertions.insert(key, slot).is_some() {
                return Err(DeclarationRecordError::Duplicate {
                    name: entry.name.clone(),
                    span: entry.span,
                });
            }
        }
        self.declaration_index = index;
        Ok(())
    }

    /// The checked type of one of this DAG's value declarations.
    #[must_use]
    pub fn value_decl_type(&self, key: &ResolvedDeclName) -> Option<&CheckedDeclType> {
        let annotation = match self.declaration_index.values.get(key)? {
            ValueDeclarationSlot::Const(slot) => &self.consts[*slot].type_ann,
            ValueDeclarationSlot::Param(slot) => &self.params[*slot].type_ann,
            ValueDeclarationSlot::Node(slot) => &self.nodes[*slot].type_ann,
        };
        Some(&annotation.checked)
    }

    /// Replace the checked types of the value declarations named in `types`.
    pub(crate) fn replace_value_decl_types(
        &mut self,
        mut types: HashMap<ResolvedDeclName, CheckedDeclType>,
    ) {
        let annotations = self
            .consts
            .iter_mut()
            .map(|entry| (entry.identity(), &mut entry.type_ann))
            .chain(
                self.params
                    .iter_mut()
                    .map(|entry| (entry.identity(), &mut entry.type_ann)),
            )
            .chain(
                self.nodes
                    .iter_mut()
                    .map(|entry| (entry.identity(), &mut entry.type_ann)),
            );
        for (identity, annotation) in annotations {
            if let Some(checked) = types.remove(&identity) {
                annotation.checked = checked;
            }
        }
    }

    /// Every value declaration's identity and checked type annotation, in
    /// const, param, node order.
    pub fn value_decl_types(
        &self,
    ) -> impl Iterator<Item = (ResolvedDeclName, &CheckedTypeAnnotation)> {
        self.consts
            .iter()
            .map(|entry| (entry.identity(), &entry.type_ann))
            .chain(
                self.params
                    .iter()
                    .map(|entry| (entry.identity(), &entry.type_ann)),
            )
            .chain(
                self.nodes
                    .iter()
                    .map(|entry| (entry.identity(), &entry.type_ann)),
            )
    }

    /// Iterate identities from the authoritative value-record index, including
    /// required parameters that have no default expression.
    pub fn value_declaration_identities(&self) -> impl Iterator<Item = &ResolvedDeclName> {
        self.declaration_index.values.keys()
    }

    /// Look up the single authoritative HIR expression owned by a const.
    #[must_use]
    pub fn const_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        match self.declaration_index.values.get(key) {
            Some(ValueDeclarationSlot::Const(slot)) => Some(&*self.consts[*slot].expr),
            Some(ValueDeclarationSlot::Param(_) | ValueDeclarationSlot::Node(_)) | None => None,
        }
    }

    /// Look up the single authoritative HIR expression owned by a param or node.
    #[must_use]
    pub fn runtime_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        match self.declaration_index.values.get(key) {
            Some(ValueDeclarationSlot::Param(slot)) => self.params[*slot].default.as_deref(),
            Some(ValueDeclarationSlot::Node(slot)) => {
                self.nodes[*slot].definition.formula().map(|expr| &**expr)
            }
            Some(ValueDeclarationSlot::Const(_)) | None => None,
        }
    }

    /// Look up an unfinished node's canonical dependency interface.
    #[must_use]
    pub fn todo(
        &self,
        key: &ResolvedDeclName,
    ) -> Option<&crate::syntax::span::Spanned<Vec<crate::syntax::span::Spanned<ResolvedDeclName>>>>
    {
        match self.declaration_index.values.get(key) {
            Some(ValueDeclarationSlot::Node(slot)) => self.nodes[*slot].definition.todo(),
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
        self.declaration_index
            .assertions
            .get(key)
            .map(|slot| &*self.asserts[*slot].body)
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
        let owner = key.owning_type.owner();
        if self.runtime_owner_rebases.get(owner).unwrap_or(owner) == self.dag_id()
            || self
                .semantic_specialization
                .as_ref()
                .is_some_and(|specialization| &specialization.template == owner)
        {
            ExpressionRootScope::ThisBody
        } else {
            ExpressionRootScope::ReferencedBody
        }
    }

    fn declaration_expression_roots(&self) -> impl Iterator<Item = &hir::Expr> {
        self.consts
            .iter()
            .map(|entry| &*entry.expr)
            .chain(
                self.params
                    .iter()
                    .filter_map(|entry| entry.default.as_deref()),
            )
            .chain(
                self.nodes
                    .iter()
                    .filter_map(|entry| entry.definition.formula().map(|expr| &**expr)),
            )
            .chain(
                self.semantic
                    .domain_bounds
                    .values()
                    .flatten()
                    .map(|bound| &*bound.value),
            )
            .chain(self.plots.iter().flat_map(|entry| {
                entry
                    .body
                    .encodings
                    .iter()
                    .map(|(_, expr)| &**expr)
                    .chain(entry.body.mark_properties.iter().map(|field| &*field.value))
                    .chain(entry.body.properties.iter().map(|field| &*field.value))
            }))
            .chain(
                self.figures
                    .iter()
                    .flat_map(|entry| &entry.fields)
                    .chain(self.layers.iter().flat_map(|entry| &entry.fields))
                    .map(|field| &*field.value),
            )
            .chain(
                self.semantic
                    .dynamic_unit_scales
                    .values()
                    .map(|entry| &*entry.expr),
            )
            .chain(
                self.asserts
                    .iter()
                    .flat_map(|entry| entry.body.expressions()),
            )
    }

    #[must_use]
    pub fn source_order(&self) -> &[SourceOrderEntry] {
        &self.source_order
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
}
