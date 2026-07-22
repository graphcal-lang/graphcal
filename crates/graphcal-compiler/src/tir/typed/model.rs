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
    IndexDef, Registry, RegistryBuildError, RegistryBuilder, TypeDef, UnionMemberDef,
};
use crate::syntax::decl_name::{DeclName, ResolvedDeclName};
use crate::syntax::dimension::{DimName, ResolvedDimName};
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
    /// A concrete scalar dimension, e.g. `Length * Time^-2`
    Scalar(Dimension),
    /// A non-generic struct type name, e.g. `TransferResult`.
    Struct(ResolvedStructTypeName, Span),
    /// A generic struct with concrete type arguments, e.g. `Vec3<Length, ECI>`.
    GenericStruct {
        name: ResolvedStructTypeName,
        type_args: Vec<Self>,
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
            Self::Scalar(dim) => {
                let formatted = registry.dimensions.format_dimension(dim);
                if formatted.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    formatted
                }
            }
            Self::Struct(name, _) => name.as_str().to_string(),
            Self::GenericStruct {
                name, type_args, ..
            } => {
                let args: Vec<String> = type_args.iter().map(|a| a.format(registry)).collect();
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

pub(in crate::tir::typed) fn format_resolved_index(index: &ResolvedIndex) -> String {
    match index {
        ResolvedIndex::Concrete(name, _) => name.as_str().to_string(),
        ResolvedIndex::GenericParam(name, _) => name.to_string(),
        ResolvedIndex::NatExpr(form, _) => format!("range({})", form.format()),
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
    pub const fn op(&self) -> MulDivOp {
        match self {
            Self::Concrete { op, .. } | Self::GenericParam { op, .. } => *op,
        }
    }

    /// Format this term as a human-readable string, e.g. `"Length"`, `"/ Time^2"`, `"D^2"`.
    #[must_use]
    pub fn format(&self, registry: &Registry) -> String {
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

/// Typed identity for a Nat-range index used by type inference.
///
/// Generic forms such as `range(N + 1)` are carried as normalized
/// [`NatPolyForm`] values. They are rendered to `range(...)` only for
/// diagnostics or display adapters; semantic comparisons use the typed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatRangeIndexIdentity {
    form: NatPolyForm,
}

impl NatRangeIndexIdentity {
    /// Create a Nat-range identity from a normalized Nat polynomial form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete `0` or cannot be
    /// represented as a non-empty in-memory Nat range on this target.
    pub fn try_from_form(
        form: NatPolyForm,
    ) -> Result<Self, crate::registry::types::NatRangeIndexError> {
        if form.is_constant() {
            crate::registry::types::NatRangeIndex::try_from_u64(form.constant())?;
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
    /// conversion (for example, a concrete zero-sized range).
    pub fn to_index_type_ref(
        &self,
    ) -> Result<IndexTypeRef, crate::registry::types::NatRangeIndexError> {
        IndexTypeRef::from_nat_range_form(self.form.clone())
    }
}

impl NatPolyForm {
    /// Wrap this normalized Nat form as a typed Nat-range index identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid Nat range size.
    #[cfg(test)]
    pub(crate) fn to_nat_range_identity(
        &self,
    ) -> Result<NatRangeIndexIdentity, crate::registry::types::NatRangeIndexError> {
        NatRangeIndexIdentity::try_from_form(self.clone())
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
        NatExpr::Add(lhs, rhs, span) => {
            let l = normalize_nat_expr(lhs, nat_params, src)?;
            let r = normalize_nat_expr(rhs, nat_params, src)?;
            l.add(&r).map_err(|err| nat_overflow_error(err, src, *span))
        }
        NatExpr::Mul(lhs, rhs, span) => {
            let l = normalize_nat_expr(lhs, nat_params, src)?;
            let r = normalize_nat_expr(rhs, nat_params, src)?;
            l.mul(&r).map_err(|err| nat_overflow_error(err, src, *span))
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
    /// A Nat expression in index position (covers literals, variables, addition, and multiplication).
    ///
    /// Examples: `3` → constant form, `N` → single-variable form, `N + 1` → linear,
    /// `M * N` → polynomial.
    NatExpr(NatPolyForm, Span),
}

impl ResolvedIndex {
    #[must_use]
    pub fn format_for_diagnostic(&self) -> String {
        match self {
            Self::Concrete(name, _) => name.as_str().to_string(),
            Self::GenericParam(name, _) => name.to_string(),
            Self::NatExpr(form, _) => format!("range({})", form.format()),
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
    pub owning_type: ResolvedStructTypeName,
    pub type_def: TypeDef,
    pub variant: UnionMemberDef,
}

#[derive(Debug, Default, Clone)]
pub struct ModuleTypeRegistry {
    dimensions: HashMap<ResolvedDimName, Dimension>,
    indexes: HashMap<ResolvedIndexName, IndexDef>,
    struct_types: HashMap<ResolvedStructTypeName, TypeDef>,
    constructors: HashMap<ResolvedConstructorName, ModuleConstructorDef>,
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
        Ok(())
    }

    /// Insert every type-system definition from `registry` under `owner`.
    ///
    /// This is intentionally an owner-qualified view over existing registries,
    /// not a new source of truth. It lets module-aware resolution validate that
    /// `alias.Name` denotes the definition owned by the dependency selected by
    /// the loader.
    pub fn insert_registry(&mut self, owner: &crate::dag_id::DagId, registry: &Registry) {
        for (name, dim) in registry.dimensions.all_dimensions() {
            self.dimensions.insert(
                ResolvedDimName::from_def(owner.clone(), name.clone()),
                dim.clone(),
            );
        }
        for index in registry.indexes.all_indexes() {
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
    }

    #[must_use]
    pub fn get_dimension(&self, name: &ResolvedDimName) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    #[must_use]
    pub fn get_index(&self, name: &ResolvedIndexName) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    #[must_use]
    pub fn get_struct_type(&self, name: &ResolvedStructTypeName) -> Option<&TypeDef> {
        self.struct_types.get(name)
    }

    /// Look up the owner type and union member for a canonical constructor identity.
    #[must_use]
    pub fn lookup_constructor(
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
    pub const fn new(
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

// ---------------------------------------------------------------------------
// Resolved domain constraints
// ---------------------------------------------------------------------------

/// A resolved domain constraint with evaluated SI-unit bounds.
///
/// Produced during module-aware TIR construction by evaluating the bound expressions
/// in `DomainBound` to concrete f64 values (in SI units).
#[derive(Debug, Clone)]
pub struct ResolvedDomainConstraint {
    /// Minimum bound in SI units, or `None` if no `min:` was specified.
    pub min: Option<f64>,
    /// Maximum bound in SI units, or `None` if no `max:` was specified.
    pub max: Option<f64>,
    /// Original min expression text for diagnostics (e.g., `"100 kg"`).
    pub min_display: Option<String>,
    /// Original max expression text for diagnostics (e.g., `"2000 kg"`).
    pub max_display: Option<String>,
    /// Span covering the entire constraint clause for error reporting.
    pub span: Span,
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
    pub param_defaults: HashMap<ResolvedDeclName, hir::Expr>,
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
    pub calls: HashMap<Span, ResolvedInlineDagCall>,
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
    pub field_types: HashMap<ResolvedStructFieldTypeKey, ResolvedTypeExpr>,
    /// Field domain bounds lowered to HIR in the owning type's generic scope.
    pub field_bounds: HashMap<ResolvedStructFieldTypeKey, Vec<ResolvedDomainBound>>,
    /// Generic parameter defaults resolved in the owning type's generic scope.
    pub generic_defaults: HashMap<(ResolvedStructTypeName, GenericParamName), ResolvedTypeExpr>,
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
    /// Dynamic unit scale expressions lowered to HIR, keyed by unit reference.
    ///
    /// Units are file-level declarations, so only the root DAG's semantic
    /// body carries entries; evaluation looks them up through the TIR root.
    /// Module-alias-qualified references (`u.mile`) key the entry the import
    /// merged under that alias.
    pub dynamic_unit_scales: HashMap<crate::syntax::dimension::UnitRef, hir::Expr>,
    /// Canonical dependency maps for this DAG.
    pub dependencies: ResolvedDagDependencies,
    /// Canonical HIR-derived collection/index references.
    pub collection_refs: ResolvedCollectionRefs,
    /// Canonical HIR-derived constructor calls and match patterns.
    pub constructor_refs: ResolvedConstructorRefs,
    /// Canonical HIR-derived inline-DAG routing identities for calls from this DAG.
    pub inline_dag_refs: ResolvedInlineDagRefs,
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
    pub target: crate::dag_id::DagId,
    /// Param binding name span -> canonical declaration in the target DAG.
    pub arg_targets: HashMap<Span, ResolvedDeclName>,
    /// Canonical projected declaration in the target DAG.
    pub output: Spanned<ResolvedDeclName>,
}

/// A resolved constructor and the tagged-union member it constructs.
#[derive(Debug, Clone)]
pub struct ResolvedConstructorTarget {
    pub owning_type: ResolvedStructTypeName,
    pub type_def: TypeDef,
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
    /// Canonical id of the file itself; the key under which the file's
    /// own top-level body lives in `dags`.
    pub root_dag_id: crate::dag_id::DagId,
    /// Every DAG reachable from this file. Always contains an entry for
    /// `root_dag_id`. Inline children and merged dep DAGs are inserted by
    /// the project pipeline.
    pub dags: DagRegistry,
    /// Maps each `import path as alias` (or `import path`) module alias to
    /// the dep file's canonical `DagId`. Used by [`TIR::lookup_call_target`]
    /// to translate user-typed `@alias.dag(args)` references into the
    /// canonical key under which the dep's DAGs were inserted by
    /// `merge_dep_dag_tirs`.
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
                .all_indexes()
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
    /// - Single-segment `[name]` (a same-file call `@name(args)`) → looks
    ///   up `root_dag_id.child(name)`.
    /// - Multi-segment `[alias, name, ...]` (a cross-file qualified call
    ///   `@alias.name(args)`) → translates `alias` via [`Self::module_aliases`]
    ///   to the dep file's `DagId`, then appends the remaining segments.
    ///
    /// Returns `None` when the path doesn't resolve (unknown alias, no
    /// matching DAG, etc.); call sites surface a structured error.
    #[must_use]
    pub fn lookup_call_target(&self, path: &crate::syntax::ast::ModulePath) -> Option<&DagTIR> {
        let id = self.resolve_call_path(path)?;
        self.dags.get(&id)
    }

    /// Build the canonical [`DagId`](crate::dag_id::DagId) that
    /// `path` refers to under this file's scope (alias-translated for
    /// multi-segment paths, file-root-scoped for single-segment paths).
    ///
    /// Returns `None` when the leading alias of a multi-segment path is
    /// unknown.
    #[must_use]
    pub fn resolve_call_path(
        &self,
        path: &crate::syntax::ast::ModulePath,
    ) -> Option<crate::dag_id::DagId> {
        if path.segments.len() == 1 {
            return Some(self.root_dag_id.child(path.segments[0].name.as_str()));
        }
        let alias = path.segments[0].name.as_str();
        let dep_id = self.module_aliases.get(alias)?;
        let mut id = dep_id.clone();
        for seg in &path.segments.as_slice()[1..] {
            id = id.child(seg.name.as_str());
        }
        Some(id)
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
    pub included_plots: Vec<crate::ir::lower::IncludedPlotEntry>,
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
    /// Pre-evaluated values imported from dependency files (passed through from IR).
    pub imported_values: HashMap<
        ScopedName,
        (
            crate::registry::runtime_value::RuntimeValue,
            crate::registry::declared_type::DeclaredType,
        ),
    >,
    /// Declared types for imported names whose values are supplied by a caller
    /// or dependency at evaluation time.
    pub imported_decl_types: HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
    /// Runtime source bindings for imported DAG-body values.
    pub imported_value_sources: HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    /// Names of `pub` nodes declared in this dag body.
    ///
    /// Used by `dim_check` to reject cross-file projection of private
    /// nodes (`@mod.dag(args).private_node` → `ImportPrivateItem`). The
    /// same-file case reads visibility from the AST; cross-file merges
    /// drop the AST, so this set is the compiled proxy.
    pub pub_nodes: std::collections::HashSet<DeclName>,
}
