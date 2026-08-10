use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational, RationalError};
use crate::hir;
use crate::hir::{NominalConstructor, NominalTypeDef};
use crate::ir::resolve::{DeclCategory, ExpectedFail};
use crate::nat::NatPolyForm;
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{
    IndexDef, RegistryBuildError, RegistryBuilder, SemanticRegistry, UnitInfo,
};
use crate::syntax::decl_name::{DeclName, ResolvedDeclName};
use crate::syntax::dimension::{DimName, ResolvedDimName, ResolvedUnitName};
use crate::syntax::index_name::{IndexName, ResolvedIndexName};
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::module_resolve::ModuleResolver;
use crate::syntax::span::Span;
use crate::syntax::type_name::{
    ConstructorName, FieldName, GenericParamName, ResolvedConstructorName, ResolvedStructTypeName,
};

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
    pub(crate) fn format(&self, registry: &SemanticRegistry) -> String {
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
    pub(crate) fn format(&self, registry: &SemanticRegistry) -> String {
        match self {
            Self::Dim(dim) => dim.format(registry),
            Self::Index(index) => format_resolved_index(index),
            Self::Nat(form, _) => form.format(),
            Self::Type(type_expr) => type_expr.format(registry),
        }
    }
}

/// A fully-resolved type expression.
///
/// Unlike the raw AST `TypeExpr`, every name here has been classified as a
/// concrete dimension, struct, generic dim param, or index generic argument.
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
    /// An index argument to a generic type parameter constrained as `Index`.
    ///
    /// This is not a standalone value type and must not appear as a resolved
    /// declaration annotation.
    IndexArg(ResolvedIndex),
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
    pub fn format(&self, registry: &SemanticRegistry) -> String {
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
            Self::IndexArg(index) => format!("index {}", format_resolved_index(index)),
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
                format!("Key<{}>", format_resolved_index(index))
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
                let idx_strs: Vec<String> = indexes.iter().map(format_resolved_index).collect();
                format!("{base_str}[{}]", idx_strs.join(", "))
            }
        }
    }
}

fn format_resolved_index(index: &ResolvedIndex) -> String {
    match index {
        ResolvedIndex::Concrete(name, _) => name.as_str().to_string(),
        ResolvedIndex::GenericParam(name, _) => name.to_string(),
        ResolvedIndex::Finite(form, _) => format!("Fin({})", form.format()),
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
    fn format(&self, registry: &SemanticRegistry) -> String {
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
                crate::registry::format::format_exponent(power)
            )
        }
    }
}

/// Typed identity for a structural finite index used by type inference.
///
/// Generic forms such as `Fin(N + 1)` are carried as normalized
/// [`NatPolyForm`] values. They are rendered to `Fin(...)` only for
/// diagnostics or display adapters; semantic comparisons use the typed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiniteIndexIdentity {
    form: NatPolyForm,
}

impl FiniteIndexIdentity {
    /// Create a finite-index identity from a normalized Nat polynomial form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete `0` or cannot be
    /// represented as a non-empty in-memory finite structural on this target.
    pub(crate) fn try_from_form(
        form: NatPolyForm,
    ) -> Result<Self, crate::registry::types::FiniteIndexError> {
        if form.is_constant() {
            crate::registry::types::FiniteIndex::try_from_u64(form.constant())?;
        }
        Ok(Self { form })
    }

    /// Borrow the normalized Nat form (`N`, `N + 1`, `3`, ...).
    #[must_use]
    pub const fn form(&self) -> &NatPolyForm {
        &self.form
    }

    /// Consume and return the normalized Nat form.
    #[must_use]
    pub fn into_form(self) -> NatPolyForm {
        self.form
    }

    /// Convert to an index type reference without serializing the Nat form
    /// into a recoverable string.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity invariant was violated before this
    /// conversion (for example, a concrete zero-sized `Fin` axis).
    pub(crate) fn to_index_type_ref(
        &self,
    ) -> Result<IndexTypeRef, crate::registry::types::FiniteIndexError> {
        IndexTypeRef::from_finite_index_form(self.form.clone())
    }
}

impl NatPolyForm {
    /// Wrap this normalized Nat form as a typed finite-index index identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid finite structural size.
    #[cfg(test)]
    pub(crate) fn to_finite_index_identity(
        &self,
    ) -> Result<FiniteIndexIdentity, crate::registry::types::FiniteIndexError> {
        FiniteIndexIdentity::try_from_form(self.clone())
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
                    name: IndexName::from_atom(ident.name.clone()),
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

impl ResolvedIndex {
    #[must_use]
    pub fn format_for_diagnostic(&self) -> String {
        match self {
            Self::Concrete(name, _) => name.as_str().to_string(),
            Self::GenericParam(name, _) => name.to_string(),
            Self::Finite(form, _) => format!("Fin({})", form.format()),
        }
    }
}

/// Canonical constructor metadata retained with its owning nominal definition.
#[derive(Debug, Clone)]
pub struct ProjectConstructorDef {
    pub(crate) owning_type: ResolvedStructTypeName,
    pub(crate) type_def: Arc<NominalTypeDef>,
    pub(crate) variant: NominalConstructor,
}

/// Authoritative project type-system definitions keyed by
/// [`ResolvedName`](crate::syntax::names::ResolvedName) identities.
///
/// [`SemanticRegistry`] carries non-nominal HIR services and formatting data.
/// Nominal resolution maps source spellings through [`ModuleResolver`] and
/// reads the resulting owner-qualified identity from this store; imported
/// aliases are never installed as additional canonical definitions.
#[derive(Debug, Default, Clone)]
pub struct ProjectTypeStore {
    dimensions: HashMap<ResolvedDimName, Dimension>,
    units: HashMap<ResolvedUnitName, UnitInfo>,
    indexes: HashMap<ResolvedIndexName, Arc<IndexDef>>,
    struct_types: HashMap<ResolvedStructTypeName, Arc<NominalTypeDef>>,
    constructors: HashMap<ResolvedConstructorName, ProjectConstructorDef>,
}

/// Error from constructing the project type store's prelude entries.
#[derive(Debug, Error)]
pub enum PreludeProjectTypeStoreError {
    /// Built-in dimension exponent arithmetic failed.
    #[error(transparent)]
    Rational(#[from] RationalError),
    /// The prelude registry violated a registry construction invariant.
    #[error(transparent)]
    RegistryBuild(#[from] RegistryBuildError),
}

/// Failure to transfer one complete HIR module into the semantic project type store.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectTypeStoreInsertError {
    #[error("module resolver is missing HIR DAG `{owner}`")]
    MissingModule { owner: crate::dag_id::DagId },
    #[error("HIR nominal type `{identity}` is stored on DAG `{actual_owner}`")]
    NominalOwnerMismatch {
        identity: ResolvedStructTypeName,
        actual_owner: crate::dag_id::DagId,
    },
    #[error("project type store already contains a different definition for `{identity}`")]
    CompetingNominalDefinition { identity: ResolvedStructTypeName },
    #[error("project type store already assigns constructor `{constructor}` to `{first_owner}`")]
    CompetingConstructorOwner {
        constructor: ResolvedConstructorName,
        first_owner: ResolvedStructTypeName,
    },
    #[error("HIR semantic registry is missing dimension `{identity}`")]
    MissingDimension { identity: ResolvedDimName },
    #[error("HIR semantic registry is missing unit `{identity}`")]
    MissingUnit { identity: ResolvedUnitName },
    #[error("HIR semantic registry is missing index `{identity}`")]
    MissingIndex { identity: ResolvedIndexName },
}

impl ProjectTypeStore {
    /// Insert canonical Graphcal prelude dimensions under the synthetic prelude owner.
    ///
    /// # Errors
    ///
    /// Returns an error only if the built-in prelude itself fails to construct,
    /// which would be a compiler bug.
    pub fn insert_graphcal_prelude(&mut self) -> Result<(), PreludeProjectTypeStoreError> {
        let mut builder = RegistryBuilder::new();
        crate::registry::prelude::load_prelude(&mut builder)?;
        let registry = builder.try_build()?;
        let owner = crate::registry::prelude::prelude_dag_id();
        for name in crate::registry::prelude::PRELUDE_DIMENSION_NAMES {
            if let Some(dim) = registry.dimensions.get_dimension(name) {
                self.dimensions.insert(
                    ResolvedDimName::from_def(owner.clone(), DimName::expect_valid(*name)),
                    dim.clone(),
                );
            }
        }
        for name in crate::registry::prelude::PRELUDE_UNIT_NAMES {
            let reference = crate::syntax::dimension::UnitRef::local(
                crate::syntax::dimension::UnitName::expect_valid(*name),
            );
            if let Some(info) = registry.units.get_unit(&reference) {
                self.units.insert(
                    ResolvedUnitName::from_def(owner.clone(), reference.name().clone()),
                    info.clone(),
                );
            }
        }
        Ok(())
    }

    /// Insert a standalone HIR DAG whose semantic registry contains only its
    /// local non-nominal definitions. The DAG identity comes from the body.
    ///
    /// # Errors
    ///
    /// Returns an invariant error if another HIR body already claims the same
    /// canonical nominal identity.
    pub fn insert_local_hir(
        &mut self,
        hir: &crate::ir::lower::HirDag,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        let owner = hir.dag_id();
        for (name, dim) in hir.registry.dimensions.all_dimensions() {
            self.dimensions
                .entry(ResolvedDimName::from_def(owner.clone(), name.clone()))
                .or_insert_with(|| dim.clone());
        }
        for (reference, _, _) in hir.registry.units.all_units() {
            if reference.is_qualified() {
                continue;
            }
            if let Some(info) = hir.registry.units.get_unit(reference) {
                self.units
                    .entry(ResolvedUnitName::from_def(
                        owner.clone(),
                        reference.name().clone(),
                    ))
                    .or_insert_with(|| info.clone());
            }
        }
        for index in hir.registry.indexes.declared_indexes() {
            self.indexes
                .entry(ResolvedIndexName::from_def(
                    owner.clone(),
                    index.name.clone(),
                ))
                .or_insert_with(|| Arc::new(index.clone()));
        }
        self.insert_nominal_types(hir)?;
        Ok(())
    }

    /// Insert exactly one resolver-owned HIR module.
    ///
    /// Unlike the former subtree scan, this operation cannot accidentally copy
    /// a nested DAG or selected import under the caller's owner. Every nominal
    /// definition comes from the HIR DAG's invariant-preserving registry.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for missing frontend facts, owner mismatch,
    /// or a competing canonical nominal definition.
    pub fn insert_resolver_module(
        &mut self,
        hir: &crate::ir::lower::HirDag,
        resolver: &ModuleResolver,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        let owner = hir.dag_id();
        let symbols = resolver.modules().get(owner).ok_or_else(|| {
            ProjectTypeStoreInsertError::MissingModule {
                owner: owner.clone(),
            }
        })?;

        for name in symbols.dimensions().keys() {
            let identity = ResolvedDimName::from_def(owner.clone(), name.clone());
            let dimension = hir
                .registry
                .dimensions
                .get_dimension(name.as_str())
                .ok_or_else(|| ProjectTypeStoreInsertError::MissingDimension {
                    identity: identity.clone(),
                })?;
            self.dimensions
                .entry(identity)
                .or_insert_with(|| dimension.clone());
        }
        for name in symbols.units().keys() {
            let identity = ResolvedUnitName::from_def(owner.clone(), name.clone());
            let reference = crate::syntax::dimension::UnitRef::local(name.clone());
            let info = hir.registry.units.get_unit(&reference).ok_or_else(|| {
                ProjectTypeStoreInsertError::MissingUnit {
                    identity: identity.clone(),
                }
            })?;
            self.units.entry(identity).or_insert_with(|| info.clone());
        }
        for name in symbols.indexes().keys() {
            let identity = ResolvedIndexName::from_def(owner.clone(), name.clone());
            let index = hir
                .registry
                .indexes
                .get_index(name.as_str())
                .ok_or_else(|| ProjectTypeStoreInsertError::MissingIndex {
                    identity: identity.clone(),
                })?;
            self.indexes
                .entry(identity)
                .or_insert_with(|| Arc::new(index.clone()));
        }
        self.insert_nominal_types(hir)?;
        Ok(())
    }

    fn insert_nominal_types(
        &mut self,
        hir: &crate::ir::lower::HirDag,
    ) -> Result<(), ProjectTypeStoreInsertError> {
        let owner = hir.dag_id();
        for definition in hir.nominal_types().values() {
            if definition.identity().owner() != owner {
                return Err(ProjectTypeStoreInsertError::NominalOwnerMismatch {
                    identity: definition.identity().clone(),
                    actual_owner: owner.clone(),
                });
            }
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
                        && (existing.owning_type != *definition.identity()
                            || !Arc::ptr_eq(&existing.type_def, definition))
                    {
                        return Err(ProjectTypeStoreInsertError::CompetingConstructorOwner {
                            constructor: member.identity().clone(),
                            first_owner: existing.owning_type.clone(),
                        });
                    }
                }
            }
        }

        for definition in hir.nominal_types().values() {
            let identity = definition.identity().clone();
            let handle = Arc::clone(
                self.struct_types
                    .entry(identity.clone())
                    .or_insert_with(|| Arc::clone(definition)),
            );
            if let Some(members) = definition.union_members() {
                for member in members {
                    self.constructors
                        .entry(member.identity().clone())
                        .or_insert_with(|| ProjectConstructorDef {
                            owning_type: identity.clone(),
                            type_def: Arc::clone(&handle),
                            variant: member.clone(),
                        });
                }
            }
        }
        Ok(())
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
    ) -> Option<&ProjectConstructorDef> {
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
}

impl DagRegistry {
    fn new(root: DagTIR) -> Self {
        Self {
            root,
            other_dags: HashMap::new(),
        }
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
        if &dag_id == self.root_id() {
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
            self.other_dags.get(dag_id)
        }
    }

    pub(crate) fn get_mut(&mut self, dag_id: &crate::dag_id::DagId) -> Option<&mut DagTIR> {
        if dag_id == self.root.dag_id() {
            Some(&mut self.root)
        } else {
            self.other_dags.get_mut(dag_id)
        }
    }

    /// Iterate over canonical identities and DAG bodies.
    pub fn iter(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
        std::iter::once((self.root.dag_id(), &self.root)).chain(self.other_dags.iter())
    }

    /// Iterate over canonical DAG identities.
    pub fn keys(&self) -> impl Iterator<Item = &crate::dag_id::DagId> {
        std::iter::once(self.root.dag_id()).chain(self.other_dags.keys())
    }

    /// Iterate over DAG bodies.
    pub fn values(&self) -> impl Iterator<Item = &DagTIR> {
        std::iter::once(&self.root).chain(self.other_dags.values())
    }

    /// Number of DAG modules in this registry, including its root.
    #[must_use]
    pub fn len(&self) -> usize {
        self.other_dags.len() + 1
    }

    /// A checked registry is never empty because construction requires a root.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl<'a> IntoIterator for &'a DagRegistry {
    type Item = (&'a crate::dag_id::DagId, &'a DagTIR);
    type IntoIter = std::iter::Chain<
        std::iter::Once<(&'a crate::dag_id::DagId, &'a DagTIR)>,
        std::collections::hash_map::Iter<'a, crate::dag_id::DagId, DagTIR>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        std::iter::once((self.root.dag_id(), &self.root)).chain(self.other_dags.iter())
    }
}

impl std::ops::Index<&crate::dag_id::DagId> for DagRegistry {
    type Output = DagTIR;

    fn index(&self, index: &crate::dag_id::DagId) -> &Self::Output {
        if index == self.root_id() {
            &self.root
        } else {
            &self.other_dags[index]
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

/// Canonical HIR-derived index references used by collection/index inference.
#[derive(Debug, Clone, Default)]
pub struct ResolvedCollectionRefs {
    /// Shared handles to canonical index definitions observed while collecting
    /// refs or owner-qualified declaration types needed by collection semantics.
    pub index_defs: HashMap<ResolvedIndexName, Arc<IndexDef>>,
}

/// Canonical HIR-derived constructor references used by constructor and match inference.
#[derive(Debug, Clone, Default)]
pub struct ResolvedConstructorRefs {
    /// Canonical constructor definitions observed while collecting constructor
    /// calls, const-like constructor refs, and match patterns. HIR carries the
    /// resolved constructor name inline; this map supplies the rich target.
    pub constructor_defs: HashMap<ResolvedConstructorName, ResolvedConstructorTarget>,
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
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-only prevents the public model glob re-export from exposing resolution internals"
)]
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
    /// The bound expression, lowered.
    pub value: hir::Expr,
    /// Span of the whole bound.
    pub span: Span,
    /// Source file whose bytes are indexed by `span` and the expression spans.
    pub src: NamedSource<Arc<String>>,
}

/// A canonical nominal override that an unrebound param default must not use.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-only prevents the public model glob re-export from exposing reconciliation internals"
)]
#[derive(Debug, Clone)]
pub(crate) enum ResolvedOverrideTarget {
    Index {
        overridden: IndexName,
        source: ResolvedIndexName,
        replacement: IndexTypeRef,
    },
    Type {
        overridden: crate::syntax::type_name::StructTypeName,
        source: ResolvedStructTypeName,
        replacement: ResolvedStructTypeName,
    },
}

/// One include-site reconciliation obligation for an unrebound param default.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-only prevents the public model glob re-export from exposing reconciliation internals"
)]
#[derive(Debug, Clone)]
pub(crate) struct OverrideReconciliation {
    pub(crate) source_decl: ResolvedDeclName,
    pub(crate) orphan_decl: DeclName,
    pub(crate) targets: Vec<ResolvedOverrideTarget>,
    pub(crate) src: NamedSource<Arc<String>>,
    pub(crate) include_span: Span,
}

/// A module-owned nominal declaration that may be replaced at an include site.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-only prevents the public model glob re-export from exposing checker internals"
)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum BindableNominalIdentity {
    Index(ResolvedIndexName),
    Type(ResolvedStructTypeName),
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
    /// Canonical HIR-derived collection/index references.
    pub collection_refs: ResolvedCollectionRefs,
    /// Canonical HIR-derived constructor calls and match patterns.
    pub constructor_refs: ResolvedConstructorRefs,
    /// Include override obligations keyed by the canonical param whose default
    /// must remain independent of the replaced nominal declarations.
    pub(crate) override_reconciliations: HashMap<ResolvedDeclName, Vec<OverrideReconciliation>>,
    /// Module-owned `pub(bind)` index and type identities. Only these can
    /// participate in include-time nominal override reconciliation.
    pub(crate) bindable_nominals: HashSet<BindableNominalIdentity>,
    /// Canonical type definitions referenced by this DAG.
    pub type_defs: ResolvedTypeDefs,
    /// Canonical declaration identity for every value name visible in this DAG.
    pub decl_bindings: HashMap<ScopedName, ResolvedDeclName>,
    /// Checked total-cardinality facts for every concrete indexed expression.
    pub materialized_shapes: HashMap<
        crate::tir::materialized_shape::MaterializedExpressionKey,
        crate::tir::materialized_shape::MaterializedShape,
    >,
    /// Checked structured display and plot-channel presentation facts.
    pub presentation: crate::tir::presentation::DagPresentationFacts,
}

/// A resolved constructor and the tagged-union member it constructs.
#[derive(Debug, Clone)]
pub struct ResolvedConstructorTarget {
    pub owning_type: ResolvedStructTypeName,
    pub(crate) type_def: Arc<NominalTypeDef>,
    pub variant: NominalConstructor,
}

// ---------------------------------------------------------------------------
// TIR struct
// ---------------------------------------------------------------------------

/// Mutable assembly state for a project TIR.
///
/// Type resolution creates this builder with a mandatory root DAG. Project
/// lowering may then add file-defined child DAGs, imported DAG modules, aliases,
/// and the completed canonical project type store. [`Self::finish`] consumes the
/// mutable shell and exposes an immutable [`TIR`].
#[derive(Debug)]
pub struct TirBuilder {
    registry: SemanticRegistry,
    project_types: ProjectTypeStore,
    dags: DagRegistry,
    module_aliases: HashMap<ModuleAliasName, crate::dag_id::DagId>,
    extern_functions:
        HashMap<crate::syntax::plugin::ExternFnKey, crate::ir::lower::ExternFunctionEntry>,
}

impl TirBuilder {
    pub(in crate::tir::typed) fn new(
        registry: SemanticRegistry,
        project_types: ProjectTypeStore,
        root: DagTIR,
        extern_functions: HashMap<
            crate::syntax::plugin::ExternFnKey,
            crate::ir::lower::ExternFunctionEntry,
        >,
    ) -> Self {
        Self {
            registry,
            project_types,
            dags: DagRegistry::new(root),
            module_aliases: HashMap::new(),
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

    /// Borrow the root file's phase-safe semantic registry.
    #[must_use]
    pub const fn registry(&self) -> &SemanticRegistry {
        &self.registry
    }

    /// Borrow the owner-qualified type store accumulated for this project.
    #[must_use]
    pub const fn project_type_store(&self) -> &ProjectTypeStore {
        &self.project_types
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

    /// Install the completed canonical type store after child DAG compilation.
    pub fn replace_project_type_store(&mut self, project_types: ProjectTypeStore) {
        self.project_types = project_types;
    }

    /// Bind a source module alias to its canonical callable DAG target.
    pub fn insert_module_alias(
        &mut self,
        alias: ModuleAliasName,
        target: crate::dag_id::DagId,
    ) -> Option<crate::dag_id::DagId> {
        self.module_aliases.insert(alias, target)
    }

    /// Merge one canonical extern signature without replacing an earlier copy.
    pub fn insert_extern_function_if_absent(
        &mut self,
        key: crate::syntax::plugin::ExternFnKey,
        function: crate::ir::lower::ExternFunctionEntry,
    ) {
        self.extern_functions.entry(key).or_insert(function);
    }

    /// Finalize project assembly into an immutable, structurally valid TIR.
    #[must_use]
    pub fn finish(self) -> TIR {
        TIR {
            registry: self.registry,
            project_types: self.project_types,
            dags: self.dags,
            module_aliases: self.module_aliases,
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
    pub(crate) registry: SemanticRegistry,
    pub(in crate::tir::typed) project_types: ProjectTypeStore,
    pub(crate) dags: DagRegistry,
    module_aliases: HashMap<ModuleAliasName, crate::dag_id::DagId>,
    pub(crate) extern_functions:
        HashMap<crate::syntax::plugin::ExternFnKey, crate::ir::lower::ExternFunctionEntry>,
}

impl TIR {
    /// Borrow the root DAG. Root presence is guaranteed by [`DagRegistry`].
    #[must_use]
    pub const fn root(&self) -> &DagTIR {
        self.dags.root()
    }

    pub(crate) const fn root_mut(&mut self) -> &mut DagTIR {
        self.dags.root_mut()
    }

    /// Canonical identity of the root DAG.
    #[must_use]
    pub const fn root_dag_id(&self) -> &crate::dag_id::DagId {
        self.dags.root_id()
    }

    /// Borrow the root file's immutable semantic/formatting registry.
    #[must_use]
    pub const fn registry(&self) -> &SemanticRegistry {
        &self.registry
    }

    /// Borrow the authoritative owner-qualified project type store.
    #[must_use]
    pub const fn project_type_store(&self) -> &ProjectTypeStore {
        &self.project_types
    }

    /// Borrow every reachable DAG through the read-only checked registry.
    #[must_use]
    pub const fn dag_registry(&self) -> &DagRegistry {
        &self.dags
    }

    /// Iterate over every DAG owned by this file, including the root and all
    /// nested descendants. Inline `dag` syntax and file modules use the same
    /// canonical representation and traversal.
    pub fn local_dags(&self) -> impl Iterator<Item = (&crate::dag_id::DagId, &DagTIR)> {
        let root = self.root_dag_id();
        self.dags
            .iter()
            .filter(move |(dag_id, _)| *dag_id == root || dag_id.is_descendant_of(root))
    }

    /// Find the checked DAG environment that physically contains a declaration.
    ///
    /// Included declarations retain instance-qualified semantic owners even
    /// though their instantiated bodies execute inside the importing DAG.
    #[must_use]
    pub fn dag_containing_declaration(&self, declaration: &ResolvedDeclName) -> Option<&DagTIR> {
        self.dags.iter().find_map(|(_, dag)| {
            dag.consts()
                .iter()
                .map(|entry| &entry.name)
                .chain(dag.params().iter().map(|entry| &entry.name))
                .chain(dag.nodes().iter().map(|entry| &entry.name))
                .any(|name| dag.resolved_decl_key_for_local(name) == *declaration)
                .then_some(dag)
        })
    }

    /// Borrow resolved extern function signatures.
    #[must_use]
    pub const fn extern_functions(
        &self,
    ) -> &HashMap<crate::syntax::plugin::ExternFnKey, crate::ir::lower::ExternFunctionEntry> {
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
        self.project_types.get_unit(name)
    }

    /// Look up a declared index by its canonical defining-module identity.
    #[must_use]
    pub fn declared_index_def(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.project_types.get_index(name)
    }

    /// Look up a nominal type by its canonical defining-module identity.
    #[must_use]
    pub fn struct_type_def(&self, name: &ResolvedStructTypeName) -> Option<&NominalTypeDef> {
        self.project_types.get_struct_type(name)
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
            .any(|param| param.default_expr.is_none())
            || self
                .root_declared_indexes()
                .any(crate::registry::types::IndexDef::is_required)
    }

    /// Build a concrete `DeclaredType` map from the root DAG.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] if any resolved type contains unresolved
    /// generic parameters.
    pub fn build_declared_types(
        &self,
        src: &NamedSource<Arc<String>>,
    ) -> Result<HashMap<ScopedName, crate::registry::declared_type::DeclaredType>, GraphcalError>
    {
        self.root().build_declared_types(src)
    }

    /// Resolve a user-typed DAG call path to a canonical compiled module.
    #[must_use]
    pub fn lookup_call_target(&self, path: &crate::syntax::ast::ModulePath) -> Option<&DagTIR> {
        let id = self.resolve_call_path(path)?;
        self.dags.get(&id)
    }

    /// Build the canonical DAG identity that a source call path denotes.
    #[must_use]
    pub fn resolve_call_path(
        &self,
        path: &crate::syntax::ast::ModulePath,
    ) -> Option<crate::dag_id::DagId> {
        let head = path.segments[0].name.as_str();
        match self.module_aliases.get(head) {
            Some(imported) => Some(
                path.segments
                    .iter()
                    .skip(1)
                    .fold(imported.clone(), |owner, segment| {
                        owner.child(segment.name.as_str())
                    }),
            ),
            None if path.segments.len() == 1 => Some(self.root_dag_id().child(head)),
            None => None,
        }
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

/// A duplicate discovered while indexing records that passed frontend checks.
#[derive(Debug)]
pub(super) struct DuplicateDeclarationRecord {
    pub(crate) name: ScopedName,
    pub(crate) span: Span,
}

/// Resolved expected-fail configuration with its authored diagnostic source.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-only prevents the public model glob re-export from exposing diagnostic provenance"
)]
#[derive(Debug, Clone)]
pub(crate) struct ResolvedExpectedFailMetadata {
    pub(crate) expected: ExpectedFail,
    pub(crate) src: NamedSource<Arc<String>>,
    pub(crate) attribute_span: Span,
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
    pub(crate) consts: Vec<crate::ir::lower::ConstEntry>,
    pub(crate) params: Vec<crate::ir::lower::ParamEntry>,
    pub(crate) nodes: Vec<crate::ir::lower::NodeEntry>,
    pub(crate) asserts: Vec<crate::ir::lower::AssertEntry>,
    pub(crate) plots: Vec<crate::ir::lower::PlotEntry>,
    pub(crate) figures: Vec<crate::ir::lower::FigureEntry>,
    pub(crate) layers: Vec<crate::ir::lower::LayerEntry>,
    pub(crate) included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
    pub(super) declaration_index: DagDeclarationIndex,
    pub(crate) semantic: DagSemanticBody,
    pub(crate) source_order: Vec<(ScopedName, DeclCategory)>,
    /// Canonical child-DAG identities and their HIR-retained name spans.
    pub(crate) child_dag_spans: HashMap<crate::dag_id::DagId, Span>,
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    pub(crate) expected_fail: HashMap<ScopedName, ResolvedExpectedFailMetadata>,
    pub(crate) resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>,
    pub(crate) imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    pub(crate) instances: Vec<crate::ir::instance::InstanceRecord>,
    pub(crate) projectable_outputs: std::collections::HashSet<DeclName>,
}

impl DagTIR {
    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        &self.dag_id
    }

    #[must_use]
    pub fn consts(&self) -> &[crate::ir::lower::ConstEntry] {
        &self.consts
    }

    #[must_use]
    pub fn params(&self) -> &[crate::ir::lower::ParamEntry] {
        &self.params
    }

    #[must_use]
    pub fn nodes(&self) -> &[crate::ir::lower::NodeEntry] {
        &self.nodes
    }

    #[must_use]
    pub fn asserts(&self) -> &[crate::ir::lower::AssertEntry] {
        &self.asserts
    }

    #[must_use]
    pub fn plots(&self) -> &[crate::ir::lower::PlotEntry] {
        &self.plots
    }

    #[must_use]
    pub fn figures(&self) -> &[crate::ir::lower::FigureEntry] {
        &self.figures
    }

    #[must_use]
    pub fn layers(&self) -> &[crate::ir::lower::LayerEntry] {
        &self.layers
    }

    #[must_use]
    pub const fn semantic(&self) -> &DagSemanticBody {
        &self.semantic
    }

    /// Look up checked structured presentation for one declaration.
    #[must_use]
    pub fn declaration_presentation(
        &self,
        declaration: &ResolvedDeclName,
    ) -> Option<&crate::tir::presentation::PresentationProvenance> {
        self.semantic.presentation.declarations.get(declaration)
    }

    /// Look up checked plot-channel presentation facts.
    #[must_use]
    pub fn plot_channel_presentations(
        &self,
        plot: &ResolvedDeclName,
    ) -> Option<
        &HashMap<
            crate::syntax::ast::EncodingChannel,
            crate::tir::presentation::PlotChannelPresentation,
        >,
    > {
        self.semantic.presentation.plot_channels.get(plot)
    }

    /// Look up the checked eager shape of one indexed expression.
    #[must_use]
    pub fn materialized_shape(
        &self,
        owner: &ResolvedDeclName,
        span: Span,
    ) -> Option<&crate::tir::materialized_shape::MaterializedShape> {
        self.semantic.materialized_shapes.get(
            &crate::tir::materialized_shape::MaterializedExpressionKey::new(owner.clone(), span),
        )
    }

    /// Explicit template-instance edges owned by this DAG.
    #[must_use]
    pub fn instances(&self) -> &[crate::ir::instance::InstanceRecord] {
        &self.instances
    }

    /// Build identity-to-record indexes after all declaration vectors are installed.
    pub(super) fn index_declaration_records(&mut self) -> Result<(), DuplicateDeclarationRecord> {
        let mut index = DagDeclarationIndex::default();
        for (slot, name, span) in
            self.consts
                .iter()
                .enumerate()
                .map(|(slot, entry)| (ValueDeclarationSlot::Const(slot), &entry.name, entry.span))
                .chain(self.params.iter().enumerate().map(|(slot, entry)| {
                    (ValueDeclarationSlot::Param(slot), &entry.name, entry.span)
                }))
                .chain(self.nodes.iter().enumerate().map(|(slot, entry)| {
                    (ValueDeclarationSlot::Node(slot), &entry.name, entry.span)
                }))
        {
            let key = self.resolved_decl_key_for_local(name);
            if index.values.insert(key, slot).is_some() {
                return Err(DuplicateDeclarationRecord {
                    name: name.clone(),
                    span,
                });
            }
        }
        for (slot, entry) in self.asserts.iter().enumerate() {
            let key = self.resolved_decl_key_for_local(&entry.name);
            if index.assertions.insert(key, slot).is_some() {
                return Err(DuplicateDeclarationRecord {
                    name: entry.name.clone(),
                    span: entry.span,
                });
            }
        }
        self.declaration_index = index;
        Ok(())
    }

    /// Look up the single authoritative HIR expression owned by a const.
    #[must_use]
    pub fn const_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        match self.declaration_index.values.get(key) {
            Some(ValueDeclarationSlot::Const(slot)) => Some(&self.consts[*slot].expr),
            Some(ValueDeclarationSlot::Param(_) | ValueDeclarationSlot::Node(_)) | None => None,
        }
    }

    /// Look up the single authoritative HIR expression owned by a param or node.
    #[must_use]
    pub fn runtime_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        match self.declaration_index.values.get(key) {
            Some(ValueDeclarationSlot::Param(slot)) => self.params[*slot].default_expr.as_ref(),
            Some(ValueDeclarationSlot::Node(slot)) => Some(&self.nodes[*slot].expr),
            Some(ValueDeclarationSlot::Const(_)) | None => None,
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
            .map(|slot| &self.asserts[*slot].body)
    }

    /// Visit every source unit reference used by this DAG.
    pub fn visit_unit_references(&self, visitor: &mut impl FnMut(&hir::ResolvedUnitRef, Span)) {
        self.visit_expressions(&mut |expr| match &expr.kind {
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

    /// Visit every semantic HIR expression node in this DAG.
    pub(crate) fn visit_expressions(&self, visitor: &mut impl FnMut(&hir::Expr)) {
        let visit_root = |expr: &hir::Expr, visitor: &mut _| hir::visit_expr(expr, visitor);

        self.consts
            .iter()
            .map(|entry| &entry.expr)
            .chain(
                self.params
                    .iter()
                    .filter_map(|entry| entry.default_expr.as_ref()),
            )
            .chain(self.nodes.iter().map(|entry| &entry.expr))
            .chain(
                self.semantic
                    .domain_bounds
                    .values()
                    .flatten()
                    .map(|bound| &bound.value),
            )
            .chain(
                self.semantic
                    .type_defs
                    .constrained_fields()
                    .flat_map(|(_, field)| field.domain_bounds().iter())
                    .map(|bound| &bound.value),
            )
            .chain(self.plots.iter().flat_map(|entry| {
                entry
                    .body
                    .encodings
                    .iter()
                    .map(|(_, expr)| expr)
                    .chain(entry.body.mark_properties.iter().map(|field| &field.value))
                    .chain(entry.body.properties.iter().map(|field| &field.value))
            }))
            .chain(
                self.figures
                    .iter()
                    .flat_map(|entry| &entry.fields)
                    .chain(self.layers.iter().flat_map(|entry| &entry.fields))
                    .map(|field| &field.value),
            )
            .chain(
                self.semantic
                    .dynamic_unit_scales
                    .values()
                    .map(|entry| &entry.expr),
            )
            .for_each(|expr| visit_root(expr, visitor));

        self.asserts.iter().for_each(|entry| match &entry.body {
            hir::AssertBody::Expr(expr) => visit_root(expr, visitor),
            hir::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
            } => {
                visit_root(actual, visitor);
                visit_root(expected, visitor);
                visit_root(tolerance, visitor);
            }
        });
    }

    #[must_use]
    pub fn source_order(&self) -> &[(ScopedName, DeclCategory)] {
        &self.source_order
    }

    #[must_use]
    pub const fn assumes_map(&self) -> &HashMap<ScopedName, Vec<ScopedName>> {
        &self.assumes_map
    }

    /// Return one assertion's resolved expected-fail configuration.
    #[must_use]
    pub fn expected_fail(&self, name: &ScopedName) -> Option<&ExpectedFail> {
        self.expected_fail
            .get(name)
            .map(|metadata| &metadata.expected)
    }

    /// Iterate over expected-fail configurations without exposing diagnostic provenance.
    pub fn expected_fail_entries(&self) -> impl Iterator<Item = (&ScopedName, &ExpectedFail)> {
        self.expected_fail
            .iter()
            .map(|(name, metadata)| (name, &metadata.expected))
    }

    #[must_use]
    pub const fn resolved_decl_types(&self) -> &HashMap<ScopedName, ResolvedTypeExpr> {
        &self.resolved_decl_types
    }

    #[must_use]
    pub const fn imported_bindings(
        &self,
    ) -> &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding> {
        &self.imported_bindings
    }

    /// Supply evaluated values for imported bindings owned by one dependency.
    /// Canonical target, declared type, and value remain in the same record.
    pub fn supply_imported_values(
        &mut self,
        owner: &crate::dag_id::DagId,
        values: &HashMap<DeclName, crate::registry::runtime_value::RuntimeValue>,
        declared_types: &HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
    ) {
        for binding in self.imported_bindings.values_mut() {
            if binding.target().owner() != owner {
                continue;
            }
            let source_name = binding.target().to_unowned_def_name();
            if let Some(value) = values.get(&source_name)
                && let Some(declared_type) = declared_types.get(&ScopedName::from(&source_name))
            {
                binding.supply_value(value.clone());
                binding.replace_declared_type(declared_type.clone());
            }
        }
    }
}
