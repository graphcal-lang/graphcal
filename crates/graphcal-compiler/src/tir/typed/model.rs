use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational, RationalError};
use crate::hir;
use crate::ir::lower::{LoweredPlotBody, LoweredPlotField};
use crate::ir::resolve::{DeclCategory, ExpectedFail};
use crate::nat::NatPolyForm;
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{
    IndexDef, Registry, RegistryBuildError, RegistryBuilder, TypeDef, UnionMemberDef, UnitInfo,
};
use crate::syntax::decl_name::{DeclName, ResolvedDeclName};
use crate::syntax::dimension::{DimName, ResolvedDimName, ResolvedUnitName};
use crate::syntax::index_name::{IndexName, ResolvedIndexName};
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::module_resolve::ModuleResolver;
use crate::syntax::span::{Span, Spanned};
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
    pub(crate) fn format(&self, registry: &Registry) -> String {
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
    pub(crate) fn format(&self, registry: &Registry) -> String {
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
    pub fn format(&self, registry: &Registry) -> String {
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
    fn format(&self, registry: &Registry) -> String {
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

/// Canonical type-system definitions keyed by [`ResolvedName`](crate::syntax::names::ResolvedName) identities.
///
/// The standalone [`Registry`] remains leaf-keyed for now because runtime values and
/// declaration types still use local names. This registry is the module-aware
/// lookup side table used by TIR resolution: qualified source paths are first
/// resolved through [`ModuleResolver`] to canonical owners, then looked up here
/// instead of by source alias text or a dotted string.
#[derive(Debug, Clone)]
pub struct ModuleConstructorDef {
    pub(crate) owning_type: ResolvedStructTypeName,
    pub(crate) type_def: TypeDef,
    pub(crate) variant: UnionMemberDef,
}

#[derive(Debug, Default, Clone)]
pub struct ModuleTypeRegistry {
    dimensions: HashMap<ResolvedDimName, Dimension>,
    units: HashMap<ResolvedUnitName, UnitInfo>,
    indexes: HashMap<ResolvedIndexName, IndexDef>,
    struct_types: HashMap<ResolvedStructTypeName, TypeDef>,
    constructors: HashMap<ResolvedConstructorName, ModuleConstructorDef>,
    sources: HashMap<crate::dag_id::DagId, NamedSource<Arc<String>>>,
}

/// Error from constructing the module type registry's prelude entries.
#[derive(Debug, Error)]
pub enum PreludeTypeRegistryError {
    /// Built-in dimension exponent arithmetic failed.
    #[error(transparent)]
    Rational(#[from] RationalError),
    /// The prelude registry violated a registry construction invariant.
    #[error(transparent)]
    RegistryBuild(#[from] RegistryBuildError),
}

impl ModuleTypeRegistry {
    /// Insert canonical Graphcal prelude dimensions under the synthetic prelude owner.
    ///
    /// # Errors
    ///
    /// Returns an error only if the built-in prelude itself fails to construct,
    /// which would be a compiler bug.
    pub fn insert_graphcal_prelude(&mut self) -> Result<(), PreludeTypeRegistryError> {
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

    /// Insert every type-system definition from `registry` under `owner` and
    /// retain the source file indexed by those definitions' spans.
    ///
    /// This is intentionally an owner-qualified view over existing registries,
    /// not a new source of truth. It lets module-aware resolution validate that
    /// `alias.Name` denotes the definition owned by the dependency selected by
    /// the loader while keeping diagnostics anchored to that definition file.
    pub fn insert_registry(
        &mut self,
        owner: &crate::dag_id::DagId,
        registry: &Registry,
        source: NamedSource<Arc<String>>,
    ) {
        for (name, dim) in registry.dimensions.all_dimensions() {
            self.dimensions.insert(
                ResolvedDimName::from_def(owner.clone(), name.clone()),
                dim.clone(),
            );
        }
        for (reference, _, _) in registry.units.all_units() {
            if reference.is_qualified() {
                continue;
            }
            if let Some(info) = registry.units.get_unit(reference) {
                self.units.insert(
                    ResolvedUnitName::from_def(owner.clone(), reference.name().clone()),
                    info.clone(),
                );
            }
        }
        for index in registry.indexes.declared_indexes() {
            self.indexes.insert(
                ResolvedIndexName::from_def(owner.clone(), index.name.clone()),
                index.clone(),
            );
        }
        for type_def in registry.types.all_types() {
            let type_name = ResolvedStructTypeName::from_def(owner.clone(), type_def.name.clone());
            self.struct_types
                .insert(type_name.clone(), type_def.clone());
            if let Some(members) = type_def.union_members() {
                for member in members {
                    self.constructors.insert(
                        ResolvedConstructorName::from_def(owner.clone(), member.name.clone()),
                        ModuleConstructorDef {
                            owning_type: type_name.clone(),
                            type_def: type_def.clone(),
                            variant: member.clone(),
                        },
                    );
                }
            }
        }
        self.sources.insert(owner.clone(), source);
    }

    /// Overlay unit entries as visible from `owner`, resolving aliases and
    /// selective imports to their canonical defining modules.
    pub fn overlay_visible_units(
        &mut self,
        owner: &crate::dag_id::DagId,
        registry: &Registry,
        resolver: &ModuleResolver,
    ) {
        for (reference, _, _) in registry.units.all_units() {
            let Ok(resolved_unit) = resolver.resolve_unit_path(owner, &reference.to_name_path())
            else {
                continue;
            };
            if let Some(info) = registry.units.get_unit(reference) {
                self.units.insert(resolved_unit, info.clone());
            }
        }
    }

    #[must_use]
    pub(crate) fn source_for_owner(
        &self,
        owner: &crate::dag_id::DagId,
    ) -> Option<&NamedSource<Arc<String>>> {
        self.sources.get(owner)
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
        self.indexes.get(name)
    }

    #[must_use]
    pub(crate) fn get_struct_type(&self, name: &ResolvedStructTypeName) -> Option<&TypeDef> {
        self.struct_types.get(name)
    }

    /// Look up the owner type and union member for a canonical constructor identity.
    #[must_use]
    pub(crate) fn lookup_constructor(
        &self,
        constructor: &ResolvedConstructorName,
    ) -> Option<&ModuleConstructorDef> {
        self.constructors.get(constructor)
    }
}

/// Module-aware type-resolution context for one DAG body.
#[derive(Debug, Clone, Copy)]
pub struct ModuleTypeContext<'a> {
    pub(in crate::tir::typed) owner: &'a crate::dag_id::DagId,
    pub(in crate::tir::typed) resolver: &'a ModuleResolver,
    pub(in crate::tir::typed) types: &'a ModuleTypeRegistry,
}

impl<'a> ModuleTypeContext<'a> {
    #[must_use]
    pub(crate) const fn new(
        owner: &'a crate::dag_id::DagId,
        resolver: &'a ModuleResolver,
        types: &'a ModuleTypeRegistry,
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

    #[must_use]
    pub(in crate::tir::typed) const fn with_owner<'b>(
        self,
        owner: &'b crate::dag_id::DagId,
    ) -> ModuleTypeContext<'b>
    where
        'a: 'b,
    {
        ModuleTypeContext {
            owner,
            resolver: self.resolver,
            types: self.types,
        }
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
    pub constructor: ConstructorName,
    pub field: FieldName,
}

impl StructFieldConstraintKey {
    #[must_use]
    pub const fn new(
        owning_type: crate::registry::declared_type::StructTypeRef,
        constructor: ConstructorName,
        field: FieldName,
    ) -> Self {
        Self {
            owning_type,
            constructor,
            field,
        }
    }
}

// ---------------------------------------------------------------------------
// DAG registry
// ---------------------------------------------------------------------------

/// Map from canonical [`DagId`](crate::dag_id::DagId) to its
/// compiled per-DAG TIR.
///
/// Holds every DAG in scope at this file: the file's own top-level body
/// (keyed by [`TIR::root_dag_id`]), every inline `dag X { ... }` child
/// (keyed by `parent_dag_id.child(name)`), and every dep DAG merged in
/// by `merge_dep_dag_tirs` (keyed by the dep's canonical id).
pub type DagRegistry = HashMap<crate::dag_id::DagId, DagTIR>;

/// Canonical dependency maps for one DAG body, collected from HIR expressions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedDagDependencies {
    /// For each param/node declaration, the canonical declarations it reads via `@`.
    pub runtime_deps: HashMap<ResolvedDeclName, BTreeSet<ResolvedDeclName>>,
    /// For each const declaration, the canonical const declarations it reads.
    pub const_deps: HashMap<ResolvedDeclName, BTreeSet<ResolvedDeclName>>,
}

/// HIR expressions for value declarations.
#[derive(Debug, Clone, Default)]
pub struct ResolvedExpressions {
    /// Const declaration expression keyed by its canonical declaration identity.
    pub consts: HashMap<ResolvedDeclName, hir::Expr>,
    /// Param default expression keyed by its canonical declaration identity.
    pub(crate) param_defaults: HashMap<ResolvedDeclName, hir::Expr>,
    /// Node expression keyed by its canonical declaration identity.
    pub nodes: HashMap<ResolvedDeclName, hir::Expr>,
    /// Assert body keyed by its canonical declaration identity.
    pub asserts: HashMap<ResolvedDeclName, hir::AssertBody>,
}

impl ResolvedExpressions {
    /// Look up the HIR expression for a runtime declaration (param default or node).
    #[must_use]
    pub fn runtime_expr(&self, key: &ResolvedDeclName) -> Option<&hir::Expr> {
        self.param_defaults.get(key).or_else(|| self.nodes.get(key))
    }
}

/// Canonical HIR-derived index references used by collection/index inference.
#[derive(Debug, Clone, Default)]
pub struct ResolvedCollectionRefs {
    /// Canonical index definitions observed while collecting the refs
    /// or owner-qualified declaration types that runtime collection semantics
    /// may need (for example `unfold` over a declared indexed node).
    pub index_defs: HashMap<ResolvedIndexName, IndexDef>,
}

/// Canonical HIR-derived constructor references used by constructor and match inference.
#[derive(Debug, Clone, Default)]
pub struct ResolvedConstructorRefs {
    /// Canonical constructor definitions observed while collecting constructor
    /// calls, const-like constructor refs, and match patterns. HIR carries the
    /// resolved constructor name inline; this map supplies the rich target.
    pub constructor_defs: HashMap<ResolvedConstructorName, ResolvedConstructorTarget>,
}

/// Canonical HIR-derived inline-DAG calls used by dim-check/eval routing.
#[derive(Debug, Clone, Default)]
pub struct ResolvedInlineDagRefs {
    /// Full inline-DAG call expression span -> resolved call routing metadata.
    pub(crate) calls: HashMap<Span, ResolvedInlineDagCall>,
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

/// Canonical type definitions referenced by module-aware TIR.
#[derive(Debug, Clone, Default)]
pub struct ResolvedTypeDefs {
    /// Struct/tagged-union definitions keyed by canonical owner/name.
    pub struct_types: HashMap<ResolvedStructTypeName, TypeDef>,
    /// Field type annotations resolved in the owning type's generic scope.
    pub(crate) field_types: HashMap<ResolvedStructFieldTypeKey, ResolvedTypeExpr>,
    /// Field domain bounds lowered to HIR in the owning type's generic scope.
    pub field_bounds: HashMap<ResolvedStructFieldTypeKey, Vec<ResolvedDomainBound>>,
    /// Generic parameter defaults resolved in the owning type's generic scope.
    pub(crate) generic_defaults:
        HashMap<(ResolvedStructTypeName, GenericParamName), ResolvedGenericArg>,
}

impl ResolvedTypeDefs {
    /// Return a field annotation resolved in its owning type's generic scope.
    #[must_use]
    pub fn field_type(&self, key: &ResolvedStructFieldTypeKey) -> Option<&ResolvedTypeExpr> {
        self.field_types.get(key)
    }
}

/// A `min:`/`max:` domain bound with its expression lowered to HIR.
///
/// Domain bounds are full expressions, but the source-shaped declaration
/// entries keep them as resolved syntax AST. Lowering them here at
/// type-resolution time (the only stage that holds a `ModuleResolver`) lets
/// dimension checking and evaluation run on HIR like every other expression.
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

/// Authoritative semantic body facts for a checked DAG.
///
/// The source-shaped declaration entries on [`DagTIR`] retain spans,
/// formatting, and declaration metadata. This structure carries the semantic
/// program model used by checking and evaluation.
#[derive(Debug, Clone, Default)]
pub struct DagSemanticBody {
    /// HIR expressions for const/default/node expressions.
    pub expressions: ResolvedExpressions,
    /// Domain bounds per declaration, lowered to HIR, in source order.
    pub domain_bounds: HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
    /// Plot/figure/layer expressions lowered to HIR, keyed by declaration name.
    pub plot_exprs: ResolvedPlotExprs,
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
    /// Canonical HIR-derived inline-DAG routing identities for calls from this DAG.
    pub(crate) inline_dag_refs: ResolvedInlineDagRefs,
    /// Canonical type definitions referenced by this DAG.
    pub type_defs: ResolvedTypeDefs,
    /// Canonical declaration identity for every value name visible in this DAG.
    pub decl_bindings: HashMap<ScopedName, ResolvedDeclName>,
}

/// Plot/figure/layer expressions lowered to HIR.
///
/// Evaluation walks these lowered bodies instead of the source-shaped
/// declarations; the source entries on [`DagTIR`] keep spans and mark/plot
/// metadata for diagnostics and output shaping.
#[derive(Debug, Clone, Default)]
pub struct ResolvedPlotExprs {
    /// Lowered plot bodies keyed by the plot's declaration name.
    pub plots: HashMap<ScopedName, LoweredPlotBody>,
    /// Lowered figure field expressions keyed by the figure's declaration name.
    pub figures: HashMap<ScopedName, Vec<LoweredPlotField>>,
    /// Lowered layer field expressions keyed by the layer's declaration name.
    pub layers: HashMap<ScopedName, Vec<LoweredPlotField>>,
}

/// A resolved inline-DAG invocation target, bindings, and projected output.
#[derive(Debug, Clone)]
pub struct ResolvedInlineDagCall {
    pub(crate) target: crate::dag_id::DagId,
    /// Param binding name span -> canonical declaration in the target DAG.
    pub(crate) arg_targets: HashMap<Span, ResolvedDeclName>,
    /// Canonical projected declaration in the target DAG.
    pub(crate) output: Spanned<ResolvedDeclName>,
}

/// A resolved constructor and the tagged-union member it constructs.
#[derive(Debug, Clone)]
pub struct ResolvedConstructorTarget {
    pub owning_type: ResolvedStructTypeName,
    pub(crate) type_def: TypeDef,
    pub variant: UnionMemberDef,
}

// ---------------------------------------------------------------------------
// TIR struct
// ---------------------------------------------------------------------------

/// Typed Intermediate Representation of a single Graphcal file.
///
/// Wraps a file-scoped [`Registry`] plus a flat [`DagRegistry`] of every
/// DAG in scope. The file's own top-level body lives at
/// `dags[&root_dag_id]`; inline `dag X { ... }` children live at
/// `dags[&root_dag_id.child(name)]`; cross-file dep DAGs merged in by
/// `merge_dep_dag_tirs` live at their own canonical
/// [`DagId`](crate::dag_id::DagId).
#[derive(Debug, Clone)]
pub struct TIR {
    /// The type/unit/dimension/index/struct registry, shared by every DAG
    /// in this file.
    pub registry: Registry,
    /// Canonical module-owned type-system definitions used by HIR references.
    pub(in crate::tir::typed) module_types: ModuleTypeRegistry,
    /// Canonical id of the file itself; the key under which the file's
    /// own top-level body lives in `dags`.
    pub root_dag_id: crate::dag_id::DagId,
    /// Every DAG reachable from this file. Always contains an entry for
    /// `root_dag_id`. Inline children and merged dep DAGs are inserted by
    /// the project pipeline.
    pub dags: DagRegistry,
    /// Maps each whole-module import alias to the exact canonical file-root or
    /// inline-DAG target it binds. Used by [`TIR::lookup_call_target`] for both
    /// direct `@alias(args)` calls and descendant `@alias.dag(args)` calls.
    pub module_aliases: HashMap<ModuleAliasName, crate::dag_id::DagId>,
    /// Resolved extern function signatures declared by `import plugin`
    /// blocks in this file (and, after `merge_dep_dag_tirs`, its deps),
    /// keyed by canonical plugin identity plus function name.
    pub extern_functions:
        HashMap<crate::syntax::plugin::ExternFnKey, crate::ir::lower::ExternFunctionEntry>,
}

impl TIR {
    /// Borrow the file's own top-level [`DagTIR`].
    ///
    /// # Panics
    ///
    /// Panics if `root_dag_id` is not in `dags`. Construction sites
    /// (`type_resolve_with_modules`) populate this entry; the invariant must
    /// not be broken by callers.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "TIR invariant: root entry always present"
    )]
    pub fn root(&self) -> &DagTIR {
        self.dags
            .get(&self.root_dag_id)
            .expect("TIR.dags must contain root_dag_id")
    }

    /// Mutably borrow the file's own top-level [`DagTIR`].
    ///
    /// # Panics
    ///
    /// Panics if `root_dag_id` is not in `dags`.
    #[expect(
        clippy::expect_used,
        reason = "TIR invariant: root entry always present"
    )]
    pub fn root_mut(&mut self) -> &mut DagTIR {
        self.dags
            .get_mut(&self.root_dag_id)
            .expect("TIR.dags must contain root_dag_id")
    }

    /// Look up a unit by its canonical defining-module identity.
    #[must_use]
    pub fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.module_types.get_unit(name)
    }

    /// Look up a declared index by its canonical defining-module identity.
    ///
    /// The per-DAG collection-reference cache contains indexes mentioned by
    /// value expressions. Prepared recursive schemas also need indexes reached
    /// only through nominal field types, so those consumers use this complete
    /// module registry as their fallback.
    #[must_use]
    pub fn declared_index_def(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.module_types.get_index(name)
    }

    /// Add an inline module's canonical type-system registry after the file TIR
    /// has been constructed.
    pub fn insert_module_registry(
        &mut self,
        owner: &crate::dag_id::DagId,
        registry: &Registry,
        source: NamedSource<Arc<String>>,
        resolver: &ModuleResolver,
    ) {
        self.module_types.insert_registry(owner, registry, source);
        self.module_types
            .overlay_visible_units(owner, registry, resolver);
    }

    /// Returns true if this file declares any required param or required index.
    ///
    /// Such files cannot be evaluated standalone; they must be bound via a
    /// parameterized include from another file.
    #[must_use]
    pub fn is_library(&self) -> bool {
        self.root().params.iter().any(|p| p.default_expr.is_none())
            || self
                .registry
                .indexes
                .declared_indexes()
                .any(crate::registry::types::IndexDef::is_required)
    }

    /// Build a concrete `DeclaredType` map from the file root's resolved
    /// types plus its imported-value metadata. Adds builtin constants as
    /// `Dimensionless`.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] if any resolved type contains unresolved generic
    /// parameters.
    pub fn build_declared_types(
        &self,
        src: &NamedSource<Arc<String>>,
    ) -> Result<HashMap<ScopedName, crate::registry::declared_type::DeclaredType>, GraphcalError>
    {
        self.root().build_declared_types(src)
    }

    /// Resolve a user-typed inline-DAG call path to the corresponding
    /// [`DagTIR`] in [`Self::dags`].
    ///
    /// - A path headed by an imported alias resolves from the exact file-root
    ///   or inline-DAG target bound to that alias. The alias itself is callable;
    ///   remaining segments descend through child DAGs.
    /// - A path with no imported-alias head resolves from
    ///   `root_dag_id.child(name)` for same-file calls.
    ///
    /// Returns `None` when the path doesn't resolve (unknown alias, no
    /// matching DAG, etc.); call sites surface a structured error.
    #[must_use]
    pub fn lookup_call_target(&self, path: &crate::syntax::ast::ModulePath) -> Option<&DagTIR> {
        let id = self.resolve_call_path(path)?;
        self.dags.get(&id)
    }

    /// Build the canonical [`DagId`](crate::dag_id::DagId) that
    /// `path` refers to under this file's scope. An imported alias maps directly
    /// to its exact target even when it is the path's only segment; otherwise a
    /// single-segment path names a same-file child DAG.
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
            None if path.segments.len() == 1 => Some(self.root_dag_id.child(head)),
            None => None,
        }
    }
}

/// The per-DAG compiled body — every field that's specific to one DAG (the
/// file's own top-level body or an inline `dag X { ... }` child).
///
/// Inserted into [`TIR::dags`] by `type_resolve_with_modules` (one entry per
/// file root) and by the project pipeline's
/// `compile_inline_dag_bodies` / `merge_dep_dag_tirs`.
#[derive(Debug, Clone)]
pub struct DagTIR {
    /// Canonical identity of this DAG. Equal to the key under which this
    /// `DagTIR` is stored in [`TIR::dags`]; carried inline so the struct
    /// is self-describing when passed by reference.
    pub dag_id: crate::dag_id::DagId,
    /// Const declarations in source order.
    pub consts: Vec<crate::ir::lower::ConstEntry>,
    /// Param declarations in source order.
    pub params: Vec<crate::ir::lower::ParamEntry>,
    /// Node declarations in source order.
    pub nodes: Vec<crate::ir::lower::NodeEntry>,
    /// Assert declarations in source order.
    pub asserts: Vec<crate::ir::lower::AssertEntry>,
    /// Plot declarations in source order.
    pub plots: Vec<crate::ir::lower::PlotEntry>,
    /// Figure declarations in source order.
    pub figures: Vec<crate::ir::lower::FigureEntry>,
    /// Layer declarations in source order.
    pub layers: Vec<crate::ir::lower::LayerEntry>,
    /// Plot aliases from include brace lists (#847).
    pub(crate) included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
    /// Authoritative semantic facts for this checked DAG body.
    pub semantic: DagSemanticBody,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<ScopedName, ExpectedFail>,
    /// Resolved type for each const/param/node declaration.
    pub resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>,
    /// Source-visible imported bindings with canonical target, type, and value
    /// metadata kept atomically.
    pub imported_bindings: HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    /// Value declarations callers may project from this DAG body.
    ///
    /// Contains explicitly exported nodes and all param input ports. A param
    /// projection reads its effective bound/default value. Private nodes remain
    /// absent, and this compiled set preserves the rule when cross-file DAG
    /// merging no longer has the source AST.
    pub(crate) projectable_outputs: std::collections::HashSet<DeclName>,
}
