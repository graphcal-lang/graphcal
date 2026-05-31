//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! The TIR layer resolves ambiguous syntax-level type paths (`NamePath` in
//! `DimTerm::name`, type applications, and `TypeExprKind::Indexed::indexes`) into
//! concrete dimensions, struct types, generic dimension parameters, or generic
//! index parameters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{MulDivOp, TypeExpr, TypeExprKind};
use crate::hir;
use crate::syntax::dimension::{Dimension, Rational};
use crate::syntax::names::{
    ConstructorName, DeclName, DimName, FieldName, GenericParamName, IndexName, ModuleAliasName,
    NameAtom, NameNamespace, NamePath, StructTypeName,
};
use crate::syntax::span::{Span, Spanned};

use crate::ir::lower::IR;
use crate::ir::resolve::{DeclCategory, ExpectedFail};
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{
    IndexDef, Registry, RegistryBuilder, TypeDef, TypeGenericConstraint, UnionMemberDef,
};
use crate::syntax::module_resolve::{ModuleResolveError, ModuleResolver};
use crate::syntax::names::{ResolvedName, ScopedName, namespace};

// ---------------------------------------------------------------------------
// Resolved type types
// ---------------------------------------------------------------------------

/// A fully-resolved type expression.
///
/// Unlike the raw AST `TypeExpr`, every name here has been classified as a
/// concrete dimension, struct, generic dim param, or generic index param.
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
    /// A label of a named index (e.g., `Maneuver` in `m: Maneuver`).
    Label(ResolvedName<namespace::Index>, Span),
    /// A concrete scalar dimension, e.g. `Length * Time^-2`
    Scalar(Dimension),
    /// A non-generic struct type name, e.g. `TransferResult`.
    Struct(ResolvedName<namespace::StructType>, Span),
    /// A generic struct with concrete type arguments, e.g. `Vec3<Length, ECI>`.
    GenericStruct {
        name: ResolvedName<namespace::StructType>,
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
            Self::Label(index, _) => format!("Label({})", index.as_str()),
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
                let idx_strs: Vec<String> = indexes
                    .iter()
                    .map(|i| match i {
                        ResolvedIndex::Concrete(name, _) => name.as_str().to_string(),
                        ResolvedIndex::GenericParam(name, _) => name.to_string(),
                        ResolvedIndex::NatExpr(form, _) => form.format(),
                    })
                    .collect();
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
        power: i32,
        op: MulDivOp,
    },
    /// A generic dimension parameter with power and combining operator.
    GenericParam {
        name: GenericParamName,
        power: i32,
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
        if power == 1 {
            format!("{prefix}{name}")
        } else {
            format!("{prefix}{name}^{power}")
        }
    }
}

/// A monomial: product of variables raised to natural number exponents.
///
/// Represented as a sorted map from variable name to exponent.
/// The empty map represents the constant monomial (= 1).
///
/// Examples:
/// - `{}` represents the constant 1 (used for the constant term in a polynomial)
/// - `{N: 1}` represents `N`
/// - `{M: 1, N: 1}` represents `M * N`
/// - `{N: 2}` represents `N^2`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Monomial(BTreeMap<GenericParamName, u64>);

impl Monomial {
    /// The constant monomial (empty product = 1).
    #[must_use]
    const fn constant() -> Self {
        Self(BTreeMap::new())
    }

    /// A single-variable monomial with exponent 1.
    #[must_use]
    fn var(name: GenericParamName) -> Self {
        let mut m = BTreeMap::new();
        m.insert(name, 1);
        Self(m)
    }

    /// Returns `true` if this is the constant monomial (no variables).
    #[must_use]
    fn is_constant(&self) -> bool {
        self.0.is_empty()
    }

    /// Multiply two monomials: add exponents of each variable.
    #[must_use]
    fn mul(&self, other: &Self) -> Self {
        let mut result = self.0.clone();
        for (var, exp) in &other.0 {
            *result.entry(var.clone()).or_insert(0) += exp;
        }
        Self(result)
    }

    /// Evaluate the monomial given variable bindings.
    /// Returns `None` if any variable is unbound.
    #[must_use]
    fn evaluate(&self, bindings: &HashMap<GenericParamName, u64>) -> Option<u64> {
        let mut result: u64 = 1;
        for (var, exp) in &self.0 {
            let val = bindings.get(var)?;
            result = result.checked_mul(val.checked_pow(u32::try_from(*exp).ok()?)?)?;
        }
        Some(result)
    }

    /// Substitute bound variables, returning a new monomial (with only unbound vars)
    /// and a multiplicative factor from the bound variables.
    /// Returns `None` if arithmetic overflows.
    #[must_use]
    fn substitute(&self, bindings: &HashMap<GenericParamName, u64>) -> Option<(Self, u64)> {
        let mut remaining = BTreeMap::new();
        let mut factor: u64 = 1;
        for (var, exp) in &self.0 {
            if let Some(val) = bindings.get(var) {
                factor = factor.checked_mul(val.checked_pow(u32::try_from(*exp).ok()?)?)?;
            } else {
                remaining.insert(var.clone(), *exp);
            }
        }
        Some((Self(remaining), factor))
    }

    /// Format as a human-readable string, e.g. `""` (empty/constant), `"N"`, `"M * N"`, `"N^2"`.
    #[must_use]
    fn format(&self) -> String {
        let mut parts = Vec::new();
        for (var, exp) in &self.0 {
            if *exp == 1 {
                parts.push(var.to_string());
            } else {
                parts.push(format!("{var}^{exp}"));
            }
        }
        parts.join(" * ")
    }
}

impl PartialOrd for Monomial {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Monomial {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare by iterating entries in sorted order (BTreeMap guarantees this).
        let a: Vec<_> = self.0.iter().collect();
        let b: Vec<_> = other.0.iter().collect();
        a.cmp(&b)
    }
}

/// A normalized polynomial form for Nat expressions.
///
/// This is the canonical representation for Nat arithmetic (Level 1 addition + Level 2
/// multiplication). Each term is a monomial (product of variables with exponents) mapped
/// to its coefficient. Two `NatPolyForm`s are equal iff their normalized terms match.
///
/// Examples:
/// - `3` → `{ {} => 3 }`
/// - `N` → `{ {N:1} => 1 }`
/// - `N + 1` → `{ {N:1} => 1, {} => 1 }`
/// - `M * N` → `{ {M:1, N:1} => 1 }`
/// - `M * N + 3` → `{ {M:1, N:1} => 1, {} => 3 }`
/// - `2 * N^2 + N + 1` → `{ {N:2} => 2, {N:1} => 1, {} => 1 }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatPolyForm {
    /// Monomial → coefficient mapping (only non-zero coefficients).
    terms: BTreeMap<Monomial, u64>,
}

/// Backward-compatible alias.
pub type NatLinearForm = NatPolyForm;

/// Typed identity for a Nat-range index used by type inference.
///
/// Generic forms such as `range(N + 1)` are carried as normalized
/// [`NatPolyForm`] values. They are rendered to `range(...)` only for
/// diagnostics or adapters that still require an [`IndexTypeRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatRangeIndexIdentity {
    form: NatPolyForm,
}

impl NatRangeIndexIdentity {
    /// Create a Nat-range identity from a normalized Nat polynomial form.
    #[must_use]
    pub const fn from_form(form: NatPolyForm) -> Self {
        Self { form }
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

    /// Return the concrete Nat-range identity when this form has a non-zero
    /// constant value representable on this target.
    #[must_use]
    pub fn concrete_index(&self) -> Option<crate::registry::types::NatRangeIndex> {
        self.form
            .is_constant()
            .then(|| self.form.constant())
            .and_then(crate::registry::types::NatRangeIndex::try_from_u64)
    }

    /// Convert to an index type reference without serializing the Nat form into
    /// a recoverable string. Concrete forms keep their typed size; symbolic
    /// forms keep a display-only `range(...)` label.
    #[must_use]
    pub fn to_index_type_ref(&self) -> IndexTypeRef {
        self.concrete_index().map_or_else(
            || IndexTypeRef::from_symbolic_nat_range(self.form.format()),
            IndexTypeRef::from_nat_range,
        )
    }
}

impl NatPolyForm {
    /// Create a polynomial from a constant.
    #[must_use]
    pub fn from_constant(c: u64) -> Self {
        let mut terms = BTreeMap::new();
        if c != 0 {
            terms.insert(Monomial::constant(), c);
        }
        Self { terms }
    }

    /// Create a polynomial from a single variable with coefficient 1.
    #[must_use]
    pub fn from_var(name: GenericParamName) -> Self {
        let mut terms = BTreeMap::new();
        terms.insert(Monomial::var(name), 1);
        Self { terms }
    }

    /// Add two polynomials.
    #[must_use]
    pub fn add(&self, other: &Self) -> Self {
        let mut terms = self.terms.clone();
        for (mono, coeff) in &other.terms {
            let entry = terms.entry(mono.clone()).or_insert(0);
            *entry += coeff;
        }
        // Remove zero-coefficient terms
        terms.retain(|_, c| *c != 0);
        Self { terms }
    }

    /// Multiply two polynomials (distributive law).
    #[must_use]
    pub fn mul(&self, other: &Self) -> Self {
        let mut terms = BTreeMap::new();
        for (m1, c1) in &self.terms {
            for (m2, c2) in &other.terms {
                let mono = m1.mul(m2);
                *terms.entry(mono).or_insert(0) += c1 * c2;
            }
        }
        // Remove zero-coefficient terms
        terms.retain(|_, c| *c != 0);
        Self { terms }
    }

    /// Returns the constant term (coefficient of the empty monomial).
    #[must_use]
    pub fn constant(&self) -> u64 {
        self.terms.get(&Monomial::constant()).copied().unwrap_or(0)
    }

    /// Returns `true` if this form has no variables (is a constant).
    #[must_use]
    pub fn is_constant(&self) -> bool {
        self.terms.iter().all(|(m, _)| m.is_constant())
    }

    /// Evaluate to a concrete value given variable bindings.
    /// Returns `None` if any variable is unbound.
    #[must_use]
    pub fn evaluate(&self, bindings: &HashMap<GenericParamName, u64>) -> Option<u64> {
        let mut result: u64 = 0;
        for (mono, coeff) in &self.terms {
            result = result.checked_add(coeff.checked_mul(mono.evaluate(bindings)?)?)?;
        }
        Some(result)
    }

    /// Format as a human-readable string.
    ///
    /// Examples: `"3"`, `"N"`, `"N + 1"`, `"M * N"`, `"M * N + 3"`, `"2 * N^2 + N + 1"`.
    #[must_use]
    pub fn format(&self) -> String {
        if self.terms.is_empty() {
            return "0".to_string();
        }
        let mut parts = Vec::new();
        // Non-constant terms first (sorted by monomial), then constant.
        for (mono, coeff) in &self.terms {
            if mono.is_constant() {
                continue;
            }
            let mono_str = mono.format();
            if *coeff == 1 {
                parts.push(mono_str);
            } else {
                parts.push(format!("{coeff} * {mono_str}"));
            }
        }
        if let Some(&c) = self.terms.get(&Monomial::constant())
            && (c > 0 || parts.is_empty())
        {
            parts.push(c.to_string());
        }
        if parts.is_empty() {
            "0".to_string()
        } else {
            parts.join(" + ")
        }
    }

    /// Wrap this normalized Nat form as a typed Nat-range index identity.
    #[must_use]
    pub fn to_nat_range_identity(&self) -> NatRangeIndexIdentity {
        NatRangeIndexIdentity::from_form(self.clone())
    }

    /// Check if `self <= other` for all non-negative variable assignments.
    ///
    /// Returns `true` iff for every monomial, the coefficient in `self` is <=
    /// the coefficient in `other`. This is sound because all `Nat` variables
    /// are non-negative, so each monomial evaluates to a non-negative value.
    #[must_use]
    pub fn is_leq(&self, other: &Self) -> bool {
        // Check that every monomial in `self` has coefficient <= in `other`
        for (mono, &coeff) in &self.terms {
            let other_coeff = other.terms.get(mono).copied().unwrap_or(0);
            if coeff > other_coeff {
                return false;
            }
        }
        // Monomials only in `other` have coefficient 0 in self → always <=.
        true
    }

    /// Collect all variable names that appear in any monomial of this polynomial.
    #[must_use]
    pub fn variables(&self) -> std::collections::BTreeSet<GenericParamName> {
        let mut vars = std::collections::BTreeSet::new();
        for mono in self.terms.keys() {
            for var in mono.0.keys() {
                vars.insert(var.clone());
            }
        }
        vars
    }
}

impl std::fmt::Display for NatPolyForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format())
    }
}

/// Normalize an AST `NatExpr` into a `NatPolyForm`.
///
/// All variables referenced must be Nat generic parameters in scope.
/// Returns an error if a variable is not a known Nat param.
pub fn normalize_nat_expr(
    expr: &crate::desugar::resolved_ast::NatExpr,
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<NatPolyForm, GraphcalError> {
    use crate::desugar::resolved_ast::NatExpr;
    match expr {
        NatExpr::Literal(n, _) => Ok(NatPolyForm::from_constant(*n)),
        NatExpr::Var(ident) => {
            let gp = nat_params
                .iter()
                .find(|p| p.as_str() == ident.name.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: IndexName::new(&ident.name),
                    src: src.clone(),
                    span: ident.span.into(),
                })?;
            Ok(NatPolyForm::from_var(gp.clone()))
        }
        NatExpr::Add(lhs, rhs, _) => {
            let l = normalize_nat_expr(lhs, nat_params, src)?;
            let r = normalize_nat_expr(rhs, nat_params, src)?;
            Ok(l.add(&r))
        }
        NatExpr::Mul(lhs, rhs, _) => {
            let l = normalize_nat_expr(lhs, nat_params, src)?;
            let r = normalize_nat_expr(rhs, nat_params, src)?;
            Ok(l.mul(&r))
        }
    }
}

/// A resolved index in an indexed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedIndex {
    /// A concrete index name, e.g. `Maneuver`.
    Concrete(ResolvedName<namespace::Index>, Span),
    /// A generic index parameter, e.g. `I`
    GenericParam(GenericParamName, Span),
    /// A Nat expression in index position (covers literals, variables, addition, and multiplication).
    ///
    /// Examples: `3` → constant form, `N` → single-variable form, `N + 1` → linear,
    /// `M * N` → polynomial.
    NatExpr(NatPolyForm, Span),
}

/// Canonical type-system definitions keyed by [`ResolvedName`] identities.
///
/// The standalone [`Registry`] remains leaf-keyed for now because runtime values and
/// declaration types still use local names. This registry is the module-aware
/// lookup side table used by TIR resolution: qualified source paths are first
/// resolved through [`ModuleResolver`] to canonical owners, then looked up here
/// instead of by source alias text or a dotted string.
#[derive(Debug, Clone)]
pub struct ModuleConstructorDef {
    pub owning_type: ResolvedName<namespace::StructType>,
    pub type_def: TypeDef,
    pub variant: UnionMemberDef,
}

#[derive(Debug, Default, Clone)]
pub struct ModuleTypeRegistry {
    dimensions: HashMap<ResolvedName<namespace::Dim>, Dimension>,
    indexes: HashMap<ResolvedName<namespace::Index>, IndexDef>,
    struct_types: HashMap<ResolvedName<namespace::StructType>, TypeDef>,
    constructors: HashMap<ResolvedName<namespace::Constructor>, ModuleConstructorDef>,
}

impl ModuleTypeRegistry {
    /// Insert canonical Graphcal prelude dimensions under the synthetic prelude owner.
    ///
    /// # Errors
    ///
    /// Returns a rational arithmetic error only if the built-in prelude itself
    /// fails to construct, which would be a compiler bug.
    pub fn insert_graphcal_prelude(
        &mut self,
    ) -> Result<(), crate::syntax::dimension::RationalError> {
        let mut builder = RegistryBuilder::new();
        crate::registry::prelude::load_prelude(&mut builder)?;
        let registry = builder.build();
        let owner = crate::registry::prelude::prelude_dag_id();
        for name in crate::registry::prelude::PRELUDE_DIMENSION_NAMES {
            if let Some(dim) = registry.dimensions.get_dimension(name) {
                self.dimensions.insert(
                    ResolvedName::from_def(owner.clone(), DimName::new(*name)),
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
                ResolvedName::from_def(owner.clone(), name.clone()),
                dim.clone(),
            );
        }
        for index in registry.indexes.all_indexes() {
            self.indexes.insert(
                ResolvedName::from_def(owner.clone(), index.name.clone()),
                index.clone(),
            );
        }
        for type_def in registry.types.all_types() {
            let type_name = ResolvedName::from_def(owner.clone(), type_def.name.clone());
            self.struct_types
                .insert(type_name.clone(), type_def.clone());
            if let Some(members) = type_def.union_members() {
                for member in members {
                    self.constructors.insert(
                        ResolvedName::from_def(owner.clone(), member.name.clone()),
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
    pub fn get_dimension(&self, name: &ResolvedName<namespace::Dim>) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    #[must_use]
    pub fn get_index(&self, name: &ResolvedName<namespace::Index>) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    #[must_use]
    pub fn get_struct_type(&self, name: &ResolvedName<namespace::StructType>) -> Option<&TypeDef> {
        self.struct_types.get(name)
    }

    /// Look up the owner type and union member for a canonical constructor identity.
    #[must_use]
    pub fn lookup_constructor(
        &self,
        constructor: &ResolvedName<namespace::Constructor>,
    ) -> Option<&ModuleConstructorDef> {
        self.constructors.get(constructor)
    }
}

/// Module-aware type-resolution context for one DAG body.
#[derive(Debug, Clone, Copy)]
pub struct ModuleTypeContext<'a> {
    owner: &'a crate::dag_id::DagId,
    resolver: &'a ModuleResolver,
    types: &'a ModuleTypeRegistry,
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
    pub runtime_deps:
        HashMap<ResolvedName<namespace::Decl>, BTreeSet<ResolvedName<namespace::Decl>>>,
    /// For each const declaration, the canonical const declarations it reads.
    pub const_deps: HashMap<ResolvedName<namespace::Decl>, BTreeSet<ResolvedName<namespace::Decl>>>,
    /// Source-span keyed graph references routed through canonical declaration identities.
    pub graph_ref_targets: HashMap<Span, ResolvedName<namespace::Decl>>,
    /// Source-span keyed const references routed through canonical declaration identities.
    pub const_ref_targets: HashMap<Span, ResolvedName<namespace::Decl>>,
}

/// HIR expressions for value declarations.
#[derive(Debug, Clone, Default)]
pub struct ResolvedExpressions {
    /// Const declaration expression keyed by its canonical declaration identity.
    pub consts: HashMap<ResolvedName<namespace::Decl>, hir::Expr>,
    /// Param default expression keyed by its canonical declaration identity.
    pub param_defaults: HashMap<ResolvedName<namespace::Decl>, hir::Expr>,
    /// Node expression keyed by its canonical declaration identity.
    pub nodes: HashMap<ResolvedName<namespace::Decl>, hir::Expr>,
}

impl ResolvedExpressions {
    /// Look up the HIR expression for a runtime declaration (param default or node).
    #[must_use]
    pub fn runtime_expr(&self, key: &ResolvedName<namespace::Decl>) -> Option<&hir::Expr> {
        self.param_defaults.get(key).or_else(|| self.nodes.get(key))
    }
}

/// Canonical HIR-derived index references used by collection/index inference.
#[derive(Debug, Clone, Default)]
pub struct ResolvedCollectionRefs {
    /// Canonical index definitions observed while collecting the refs below
    /// or owner-qualified declaration types that runtime collection semantics
    /// may need (for example `unfold` over a declared indexed node).
    pub index_defs: HashMap<ResolvedName<namespace::Index>, IndexDef>,
    /// `ForBindingIndex::Named` span -> resolved index owner/name.
    pub for_binding_indexes: HashMap<Span, ResolvedName<namespace::Index>>,
    /// Full value-level `Index.Variant` span -> resolved index variant.
    ///
    /// This covers parser-created `VariantLiteral` nodes and locally-resolved
    /// const-like refs that HIR proves are index variants.
    pub variant_literals: HashMap<Span, crate::syntax::names::ResolvedIndexVariant>,
    /// Full map/table `Index.Variant` key span -> resolved index variant.
    pub map_entry_variants: HashMap<Span, crate::syntax::names::ResolvedIndexVariant>,
    /// Full `Index.Variant` argument span -> resolved index variant.
    pub index_access_variants: HashMap<Span, crate::syntax::names::ResolvedIndexVariant>,
    /// Full index-label match-pattern span -> resolved index variant.
    pub match_label_variants: HashMap<Span, crate::syntax::names::ResolvedIndexVariant>,
}

/// Canonical HIR-derived constructor references used by constructor and match inference.
#[derive(Debug, Clone, Default)]
pub struct ResolvedConstructorRefs {
    /// Canonical constructor definitions observed while collecting the refs below.
    pub constructor_defs: HashMap<ResolvedName<namespace::Constructor>, ResolvedConstructorTarget>,
    /// Constructor-call callee span -> resolved constructor target.
    pub constructor_calls: HashMap<Span, ResolvedConstructorTarget>,
    /// Constructor match-pattern path/name span -> resolved constructor pattern.
    pub match_pattern_constructors: HashMap<Span, ResolvedConstructorPattern>,
}

/// Canonical HIR-derived inline-DAG calls used by dim-check/eval routing.
#[derive(Debug, Clone, Default)]
pub struct ResolvedInlineDagRefs {
    /// Full inline-DAG call expression span -> resolved call routing metadata.
    pub calls: HashMap<Span, ResolvedInlineDagCall>,
}

/// Canonical type definitions referenced by module-aware TIR.
#[derive(Debug, Clone, Default)]
pub struct ResolvedTypeDefs {
    /// Struct/tagged-union definitions keyed by canonical owner/name.
    pub struct_types: HashMap<ResolvedName<namespace::StructType>, TypeDef>,
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
    pub decl_bindings: HashMap<ScopedName, ResolvedName<namespace::Decl>>,
}

/// A resolved inline-DAG invocation target, bindings, and projected output.
#[derive(Debug, Clone)]
pub struct ResolvedInlineDagCall {
    pub target: crate::dag_id::DagId,
    /// Param binding name span -> canonical declaration in the target DAG.
    pub arg_targets: HashMap<Span, ResolvedName<namespace::Decl>>,
    /// Canonical projected declaration in the target DAG.
    pub output: Spanned<ResolvedName<namespace::Decl>>,
}

/// A resolved constructor and the tagged-union member it constructs.
#[derive(Debug, Clone)]
pub struct ResolvedConstructorTarget {
    pub constructor: ResolvedName<namespace::Constructor>,
    pub owning_type: ResolvedName<namespace::StructType>,
    pub type_def: TypeDef,
    pub variant: UnionMemberDef,
}

/// A resolved constructor match pattern, including lexical HIR pattern bindings.
#[derive(Debug, Clone)]
pub struct ResolvedConstructorPattern {
    pub target: ResolvedConstructorTarget,
    pub bindings: Vec<ResolvedPatternBinding>,
}

/// A binding inside a resolved constructor match pattern.
#[derive(Debug, Clone)]
pub enum ResolvedPatternBinding {
    Bind {
        field: Spanned<crate::syntax::names::FieldName>,
        local: hir::LocalDef,
    },
    Wildcard {
        field: Spanned<crate::syntax::names::FieldName>,
        span: Span,
    },
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

    /// Construct a minimal `TIR` for callers that need a context to satisfy
    /// the eval pipeline's invariants but never look up an inline DAG.
    ///
    /// Currently used by display-only unit-scale resolution. The returned
    /// TIR has a synthetic root id and empty per-DAG content; calling
    /// [`Self::lookup_call_target`] on it always returns `None`.
    #[must_use]
    pub fn empty_for_eval_helpers(registry: Registry) -> Self {
        let root_dag_id = crate::dag_id::DagId::root("<eval-helper>");
        let mut dags = DagRegistry::new();
        dags.insert(
            root_dag_id.clone(),
            DagTIR {
                dag_id: root_dag_id.clone(),
                consts: Vec::new(),
                params: Vec::new(),
                nodes: Vec::new(),
                asserts: Vec::new(),
                plots: Vec::new(),
                figures: Vec::new(),
                layers: Vec::new(),
                semantic: DagSemanticBody::default(),
                source_order: Vec::new(),
                assert_names: std::collections::HashSet::new(),
                assumes_map: HashMap::new(),
                expected_fail: HashMap::new(),
                resolved_decl_types: HashMap::new(),
                domain_constraints: HashMap::new(),
                imported_values: HashMap::new(),
                imported_decl_types: HashMap::new(),
                imported_value_sources: HashMap::new(),
                pub_nodes: std::collections::HashSet::new(),
            },
        );
        Self {
            registry,
            root_dag_id,
            dags,
            module_aliases: HashMap::new(),
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
    /// Authoritative semantic facts for this checked DAG body.
    pub semantic: DagSemanticBody,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Set of all assert names. Membership-only, never iterated.
    pub assert_names: std::collections::HashSet<ScopedName>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<ScopedName, ExpectedFail>,
    /// Resolved type for each const/param/node declaration.
    pub resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>,
    /// Resolved domain constraints for declarations that have them.
    pub domain_constraints: HashMap<ScopedName, ResolvedDomainConstraint>,
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
    pub pub_nodes: std::collections::HashSet<String>,
}

impl DagTIR {
    /// Build a concrete `DeclaredType` map from this DAG's resolved types
    /// plus its imported-value metadata. Adds builtin constants as
    /// `Dimensionless`.
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
        // Layer the sources so the most authoritative wins on key collisions:
        //   builtins  <  imported_decl_types  <  imported_values  <  resolved_decl_types
        // A DAG's own resolved decls always shadow imports of the same name —
        // necessary because `merge_dependency` may propagate placeholder
        // imported decl types from an inline DAG's self-import back onto the
        // importer for names the importer already declares itself.
        let mut declared_types = HashMap::new();
        for name in crate::registry::builtins::builtin_constants().keys() {
            declared_types.insert(
                ScopedName::local(*name),
                crate::registry::declared_type::DeclaredType::Scalar(Dimension::dimensionless()),
            );
        }
        for (name, dt) in &self.imported_decl_types {
            declared_types.insert(name.clone(), dt.clone());
        }
        for (name, (_rv, dt)) in &self.imported_values {
            declared_types.insert(name.clone(), dt.clone());
        }
        for (name, resolved) in &self.resolved_decl_types {
            let dt = resolved_to_declared_type(resolved, src)?;
            declared_types.insert(name.clone(), dt);
        }
        Ok(declared_types)
    }

    /// Populate this DAG's `pub_nodes` set from its source body.
    pub fn populate_pub_nodes(&mut self, body: &[crate::desugar::resolved_ast::Declaration]) {
        use crate::desugar::resolved_ast::DeclKind;

        for decl in body {
            if let DeclKind::Node(n) = &decl.kind
                && n.visibility.is_public()
            {
                self.pub_nodes.insert(n.name.value.to_string());
            }
        }
    }

    /// Return the resolved declaration key for a declaration visible from this DAG.
    ///
    /// Qualified source keys synthesize a child owner under this DAG so
    /// source-facing entries still use resolved identities instead of
    /// source-keyed runtime maps.
    #[must_use]
    pub fn resolved_decl_key_for_local(
        &self,
        name: &ScopedName,
    ) -> Option<ResolvedName<namespace::Decl>> {
        if let Some(resolved) = self.semantic.decl_bindings.get(name) {
            return Some(resolved.clone());
        }
        if self.resolved_decl_types.contains_key(name)
            || self
                .source_order
                .iter()
                .any(|(source_name, _)| source_name == name)
        {
            return resolved_decl_key(&self.dag_id, name);
        }
        if !name.is_qualified() {
            let mut candidates = self
                .resolved_decl_types
                .keys()
                .filter(|candidate| candidate.member() == name.member())
                .filter_map(|candidate| resolved_decl_key(&self.dag_id, candidate));
            if let Some(candidate) = candidates.next()
                && candidates.next().is_none()
            {
                return Some(candidate);
            }
        }
        resolved_decl_key(&self.dag_id, name)
    }
}

/// Resolve all type annotations in an `IR` using module-aware type-system
/// resolution for syntactic paths.
///
/// Qualified source paths such as `lib.Length`, `lib.Vec3<...>`, and
/// `lib.Phase` are first lowered into HIR canonical references using
/// `module_resolver`; TIR then consumes those HIR references and reads the
/// corresponding definition from `module_types`. Runtime-facing values still
/// keep display leaves for diagnostics, but semantic lookup no longer depends on
/// source alias strings.
pub fn type_resolve_with_modules(
    ir: IR,
    root_dag_id: crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
) -> Result<TIR, GraphcalError> {
    let owner_for_ctx = root_dag_id.clone();
    let ctx = ModuleTypeContext::new(&owner_for_ctx, module_resolver, module_types);
    type_resolve_impl(ir, root_dag_id, src, ctx)
}

fn type_resolve_impl(
    ir: IR,
    root_dag_id: crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<TIR, GraphcalError> {
    let runtime_deps_for_hir = ir.runtime_deps.clone();
    let const_deps_for_hir = ir.const_deps.clone();
    let imported_value_sources_for_hir = ir.imported_value_sources.clone();
    let root_dag = type_resolve_dag(
        ir.consts,
        ir.params,
        ir.nodes,
        &ir.registry,
        src,
        &root_dag_id,
        module_ctx,
        &runtime_deps_for_hir,
        &const_deps_for_hir,
        &imported_value_sources_for_hir,
    )?
    .with_body(
        ir.asserts,
        ir.plots,
        ir.figures,
        ir.layers,
        ir.runtime_deps,
        ir.const_deps,
        ir.source_order,
        ir.assert_names,
        ir.assumes_map,
        ir.expected_fail,
        ir.imported_values,
        ir.imported_decl_types,
        ir.imported_value_sources,
        module_ctx,
        src,
    )?;
    let mut dags = DagRegistry::new();
    dags.insert(root_dag_id.clone(), root_dag);
    Ok(TIR {
        registry: ir.registry,
        root_dag_id,
        dags,
        module_aliases: HashMap::new(),
    })
}

/// Resolve type annotations for one DAG body with module-aware type-system
/// path lookup.
pub fn type_resolve_single_with_modules(
    ir: IR,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
) -> Result<DagTIR, GraphcalError> {
    let ctx = ModuleTypeContext::new(dag_id, module_resolver, module_types);
    type_resolve_single_impl(ir, dag_id, src, ctx)
}

fn type_resolve_single_impl(
    ir: IR,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<DagTIR, GraphcalError> {
    let runtime_deps_for_hir = ir.runtime_deps.clone();
    let const_deps_for_hir = ir.const_deps.clone();
    let imported_value_sources_for_hir = ir.imported_value_sources.clone();
    type_resolve_dag(
        ir.consts,
        ir.params,
        ir.nodes,
        &ir.registry,
        src,
        dag_id,
        module_ctx,
        &runtime_deps_for_hir,
        &const_deps_for_hir,
        &imported_value_sources_for_hir,
    )?
    .with_body(
        ir.asserts,
        ir.plots,
        ir.figures,
        ir.layers,
        ir.runtime_deps,
        ir.const_deps,
        ir.source_order,
        ir.assert_names,
        ir.assumes_map,
        ir.expected_fail,
        ir.imported_values,
        ir.imported_decl_types,
        ir.imported_value_sources,
        module_ctx,
        src,
    )
}

/// Internal helper: resolve type annotations for the const/param/node
/// declarations of a single DAG, returning a partially-built [`DagTIR`].
#[expect(
    clippy::too_many_arguments,
    reason = "orchestrates per-DAG type resolution across IR declarations and semantic body data"
)]
fn type_resolve_dag(
    consts: Vec<crate::ir::lower::ConstEntry>,
    params: Vec<crate::ir::lower::ParamEntry>,
    nodes: Vec<crate::ir::lower::NodeEntry>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    module_ctx: ModuleTypeContext<'_>,
    runtime_deps: &HashMap<ScopedName, BTreeSet<ScopedName>>,
    const_deps: &HashMap<ScopedName, BTreeSet<ScopedName>>,
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
) -> Result<DagTIRSeed, GraphcalError> {
    let mut resolved_decl_types = HashMap::new();
    let no_generic_params: &[GenericParamName] = &[];

    for entry in &consts {
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            dag_id,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            src,
            Some(module_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &params {
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            dag_id,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            src,
            Some(module_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &nodes {
        let resolved = resolve_type_expr_inner(
            &entry.type_ann,
            registry,
            dag_id,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            src,
            Some(module_ctx),
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }

    let expressions = lower_resolved_expressions(
        &consts,
        &params,
        &nodes,
        module_ctx,
        runtime_deps,
        const_deps,
        imported_value_sources,
        src,
    )?;
    let dependencies =
        collect_resolved_dag_dependencies(&consts, &params, &nodes, &expressions, module_ctx, src)?;
    let collection_refs =
        collect_resolved_collection_refs(&expressions, &resolved_decl_types, module_ctx, src)?;
    let constructor_refs = collect_resolved_constructor_refs(&expressions, module_ctx, src)?;
    let inline_dag_refs = collect_resolved_inline_dag_refs(&expressions);
    let type_defs = collect_resolved_type_defs(&resolved_decl_types, &constructor_refs, module_ctx);

    let semantic = DagSemanticBody {
        expressions,
        dependencies,
        collection_refs,
        constructor_refs,
        inline_dag_refs,
        type_defs,
        decl_bindings: HashMap::new(),
    };

    Ok(DagTIRSeed {
        dag_id: dag_id.clone(),
        consts,
        params,
        nodes,
        resolved_decl_types,
        semantic,
    })
}

fn collect_resolved_type_defs(
    resolved_decl_types: &HashMap<ScopedName, ResolvedTypeExpr>,
    constructor_refs: &ResolvedConstructorRefs,
    ctx: ModuleTypeContext<'_>,
) -> ResolvedTypeDefs {
    let mut defs = ResolvedTypeDefs::default();
    if let Some(symbols) = ctx.resolver.modules().get(ctx.owner) {
        for symbol in symbols.struct_types().values() {
            record_resolved_struct_type_def(symbol.resolved(), ctx, &mut defs);
        }
    }
    for resolved in resolved_decl_types.values() {
        collect_struct_type_defs_from_resolved_type(resolved, ctx, &mut defs);
    }
    for target in constructor_refs.constructor_defs.values() {
        defs.struct_types
            .entry(target.owning_type.clone())
            .or_insert_with(|| target.type_def.clone());
    }
    defs
}

fn collect_struct_type_defs_from_resolved_type(
    resolved: &ResolvedTypeExpr,
    ctx: ModuleTypeContext<'_>,
    defs: &mut ResolvedTypeDefs,
) {
    match resolved {
        ResolvedTypeExpr::Struct(name, _) => {
            record_resolved_struct_type_def(name, ctx, defs);
        }
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            record_resolved_struct_type_def(name, ctx, defs);
            for arg in type_args {
                collect_struct_type_defs_from_resolved_type(arg, ctx, defs);
            }
        }
        ResolvedTypeExpr::Indexed { base, indexes: _ } => {
            collect_struct_type_defs_from_resolved_type(base, ctx, defs);
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::Label(_, _)
        | ResolvedTypeExpr::Scalar(_)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => {}
    }
}

fn record_resolved_struct_type_def(
    name: &ResolvedName<namespace::StructType>,
    ctx: ModuleTypeContext<'_>,
    defs: &mut ResolvedTypeDefs,
) {
    if defs.struct_types.contains_key(name) {
        return;
    }
    if let Some(type_def) = ctx.types.get_struct_type(name) {
        defs.struct_types.insert(name.clone(), type_def.clone());
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "HIR lowering needs declaration slices plus dependency maps from IR lowering"
)]
fn lower_resolved_expressions(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    ctx: ModuleTypeContext<'_>,
    runtime_deps: &HashMap<ScopedName, BTreeSet<ScopedName>>,
    const_deps: &HashMap<ScopedName, BTreeSet<ScopedName>>,
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedExpressions, GraphcalError> {
    let generic_scope = hir::GenericScope::new();
    let prelude = hir::PreludeTypeScope::graphcal();
    let decl_bindings = collect_hir_decl_bindings(
        ctx.owner,
        consts,
        params,
        nodes,
        imported_value_sources,
        src,
    )?;
    let expr_ctx = hir::ExprLoweringContext::new(ctx.owner, ctx.resolver, &generic_scope)
        .with_prelude(&prelude)
        .with_decl_bindings(&decl_bindings);
    let mut exprs = ResolvedExpressions::default();

    for entry in consts {
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let hir_expr = lower_expr_or_synthetic_alias(
            &entry.expr,
            expr_ctx,
            ctx,
            const_deps.get(&entry.name),
            src,
        )?;
        exprs.consts.insert(key, hir_expr);
    }
    for entry in params {
        let Some(expr) = &entry.default_expr else {
            continue;
        };
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let hir_expr =
            lower_expr_or_synthetic_alias(expr, expr_ctx, ctx, runtime_deps.get(&entry.name), src)?;
        exprs.param_defaults.insert(key, hir_expr);
    }
    for entry in nodes {
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let hir_expr = lower_expr_or_synthetic_alias(
            &entry.expr,
            expr_ctx,
            ctx,
            runtime_deps.get(&entry.name),
            src,
        )?;
        exprs.nodes.insert(key, hir_expr);
    }

    Ok(exprs)
}

fn lower_expr_or_synthetic_alias(
    expr: &crate::desugar::resolved_ast::Expr,
    expr_ctx: hir::ExprLoweringContext<'_>,
    ctx: ModuleTypeContext<'_>,
    deps: Option<&BTreeSet<ScopedName>>,
    src: &NamedSource<Arc<String>>,
) -> Result<hir::Expr, GraphcalError> {
    match hir::lower_expr(expr, expr_ctx) {
        Ok(expr) => Ok(expr),
        Err(err) => lower_synthetic_alias_expr(expr, ctx, deps)
            .ok_or_else(|| expr_lower_error_to_graphcal(&err, src)),
    }
}

fn lower_synthetic_alias_expr(
    expr: &crate::desugar::resolved_ast::Expr,
    ctx: ModuleTypeContext<'_>,
    deps: Option<&BTreeSet<ScopedName>>,
) -> Option<hir::Expr> {
    let target = single_dep(deps?)?;
    let resolved = resolved_decl_key(ctx.owner, target)?;
    match &expr.kind {
        crate::desugar::resolved_ast::ExprKind::GraphRef(name) if &name.value == target => {
            Some(hir::Expr::new(
                hir::ExprKind::GraphRef(Spanned::new(resolved, name.span)),
                expr.span,
            ))
        }
        crate::desugar::resolved_ast::ExprKind::ConstRef(name) if &name.value == target => {
            Some(hir::Expr::new(
                hir::ExprKind::ConstRef(Spanned::new(hir::ConstRef::Decl(resolved), name.span)),
                expr.span,
            ))
        }
        _ => None,
    }
}

fn single_dep(deps: &BTreeSet<ScopedName>) -> Option<&ScopedName> {
    let mut iter = deps.iter();
    let only = iter.next()?;
    iter.next().is_none().then_some(only)
}

fn collect_resolved_dag_dependencies(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    exprs: &ResolvedExpressions,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDagDependencies, GraphcalError> {
    let mut resolved = ResolvedDagDependencies::default();

    for entry in consts {
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let hir_expr = exprs.consts.get(&key).ok_or_else(|| {
            internal_error(
                format!(
                    "missing HIR expression for const declaration `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let mut deps = hir::collect_expr_dependencies(hir_expr);
        for graph_ref in &deps.graph_refs {
            let kind = ctx
                .resolver
                .decl_symbol_kind(graph_ref)
                .map_err(|err| module_resolve_error(&err, src, entry.span))?;
            if !kind.is_const() {
                return Err(GraphcalError::UnknownConstRef {
                    name: ScopedName::local(graph_ref.as_str()),
                    src: src.clone(),
                    span: entry.span.into(),
                });
            }
            deps.const_refs.insert(graph_ref.clone());
        }
        resolved.graph_ref_targets.extend(deps.graph_ref_targets);
        resolved.const_ref_targets.extend(deps.const_ref_targets);
        resolved.const_deps.insert(key, deps.const_refs);
    }

    for entry in params {
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let deps = exprs.param_defaults.get(&key).map_or_else(
            hir::ExprDependencies::default,
            hir::collect_expr_dependencies,
        );
        resolved.graph_ref_targets.extend(deps.graph_ref_targets);
        resolved.const_ref_targets.extend(deps.const_ref_targets);
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    for entry in nodes {
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let hir_expr = exprs.nodes.get(&key).ok_or_else(|| {
            internal_error(
                format!(
                    "missing HIR expression for node declaration `{}`",
                    entry.name
                ),
                src,
                entry.span,
            )
        })?;
        let mut deps = hir::collect_expr_dependencies(hir_expr);
        if matches!(hir_expr.kind, hir::ExprKind::Unfold { .. }) {
            deps.graph_refs.remove(&key);
        }
        resolved.graph_ref_targets.extend(deps.graph_ref_targets);
        resolved.const_ref_targets.extend(deps.const_ref_targets);
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    Ok(resolved)
}

fn collect_resolved_collection_refs(
    exprs: &ResolvedExpressions,
    resolved_decl_types: &HashMap<ScopedName, ResolvedTypeExpr>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedCollectionRefs, GraphcalError> {
    let mut refs = ResolvedCollectionRefs::default();

    for resolved_type in resolved_decl_types.values() {
        collect_resolved_collection_indexes_from_type(resolved_type, ctx, src, &mut refs)?;
    }

    for hir_expr in exprs
        .consts
        .values()
        .chain(exprs.param_defaults.values())
        .chain(exprs.nodes.values())
    {
        collect_resolved_collection_refs_from_expr(hir_expr, ctx, src, &mut refs)?;
    }

    Ok(refs)
}

fn record_resolved_collection_index(
    index: &ResolvedName<namespace::Index>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    span: Span,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    if refs.index_defs.contains_key(index) {
        return Ok(());
    }
    let def = ctx.types.get_index(index).cloned().ok_or_else(|| {
        internal_error(
            format!("semantic collection metadata references unknown index `{index}`"),
            src,
            span,
        )
    })?;
    refs.index_defs.insert(index.clone(), def);
    Ok(())
}

fn collect_resolved_collection_indexes_from_type(
    resolved_type: &ResolvedTypeExpr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    match resolved_type {
        ResolvedTypeExpr::Label(index, span) => {
            record_resolved_collection_index(index, ctx, src, *span, refs)
        }
        ResolvedTypeExpr::Indexed { base, indexes } => {
            collect_resolved_collection_indexes_from_type(base, ctx, src, refs)?;
            for index in indexes {
                if let ResolvedIndex::Concrete(resolved, span) = index {
                    record_resolved_collection_index(resolved, ctx, src, *span, refs)?;
                }
            }
            Ok(())
        }
        ResolvedTypeExpr::GenericStruct { type_args, .. } => {
            for arg in type_args {
                collect_resolved_collection_indexes_from_type(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::Scalar(_)
        | ResolvedTypeExpr::Struct(_, _)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => Ok(()),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "expression traversal mirrors HIR variants"
)]
fn collect_resolved_collection_refs_from_expr(
    expr: &hir::Expr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::UnitLiteral { .. } => Ok(()),
        hir::ExprKind::ConstRef(target) => {
            if let hir::ConstRef::IndexVariant(variant) = &target.value {
                record_resolved_collection_index(variant.index(), ctx, src, target.span, refs)?;
                refs.variant_literals.insert(target.span, variant.clone());
            }
            Ok(())
        }
        hir::ExprKind::VariantLiteral(variant) => {
            record_resolved_collection_index(variant.value.index(), ctx, src, variant.span, refs)?;
            refs.variant_literals
                .insert(variant.span, variant.value.clone());
            Ok(())
        }
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_resolved_collection_refs_from_expr(lhs, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(rhs, ctx, src, refs)
        }
        hir::ExprKind::UnaryOp { operand, .. } => {
            collect_resolved_collection_refs_from_expr(operand, ctx, src, refs)
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_resolved_collection_refs_from_expr(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_resolved_collection_refs_from_expr(condition, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(then_branch, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(else_branch, ctx, src, refs)
        }
        hir::ExprKind::Convert { expr, .. }
        | hir::ExprKind::DisplayTimezone { expr, .. }
        | hir::ExprKind::FieldAccess { expr, .. } => {
            collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)
        }
        hir::ExprKind::ConstructorCall { fields, .. } => {
            for field in fields {
                collect_resolved_collection_refs_from_expr(&field.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                for key in &entry.keys {
                    match key {
                        hir::expr::MapEntryKey::IndexVariant(variant) => {
                            record_resolved_collection_index(
                                variant.value.index(),
                                ctx,
                                src,
                                variant.span,
                                refs,
                            )?;
                            refs.map_entry_variants
                                .insert(variant.span, variant.value.clone());
                        }
                        hir::expr::MapEntryKey::NatRangeVariant { .. } => {}
                    }
                }
                collect_resolved_collection_refs_from_expr(&entry.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::ForComp { bindings, body } => {
            for binding in bindings {
                match &binding.index {
                    hir::expr::ForBindingIndex::Named(index) => {
                        record_resolved_collection_index(&index.value, ctx, src, index.span, refs)?;
                        refs.for_binding_indexes
                            .insert(index.span, index.value.clone());
                    }
                    hir::expr::ForBindingIndex::Range { .. } => {}
                }
            }
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::IndexAccess { expr, args } => {
            collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)?;
            for arg in args {
                match arg {
                    hir::expr::IndexArg::Variant(variant) => {
                        record_resolved_collection_index(
                            variant.value.index(),
                            ctx,
                            src,
                            variant.span,
                            refs,
                        )?;
                        refs.index_access_variants
                            .insert(variant.span, variant.value.clone());
                    }
                    hir::expr::IndexArg::Expr(expr) => {
                        collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)?;
                    }
                    hir::expr::IndexArg::Var(_) => {}
                }
            }
            Ok(())
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_resolved_collection_refs_from_expr(source, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_resolved_collection_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_resolved_collection_refs_from_expr(scrutinee, ctx, src, refs)?;
            for arm in arms {
                if let hir::expr::MatchPattern::IndexLabel { variant, span } = &arm.pattern {
                    record_resolved_collection_index(
                        variant.value.index(),
                        ctx,
                        src,
                        variant.span,
                        refs,
                    )?;
                    refs.match_label_variants
                        .insert(variant.span, variant.value.clone());
                    refs.match_label_variants
                        .insert(*span, variant.value.clone());
                }
                collect_resolved_collection_refs_from_expr(&arm.body, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                collect_resolved_collection_refs_from_expr(&arg.value, ctx, src, refs)?;
            }
            Ok(())
        }
    }
}

fn collect_resolved_constructor_refs(
    exprs: &ResolvedExpressions,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedConstructorRefs, GraphcalError> {
    let mut refs = ResolvedConstructorRefs::default();

    for hir_expr in exprs
        .consts
        .values()
        .chain(exprs.param_defaults.values())
        .chain(exprs.nodes.values())
    {
        collect_resolved_constructor_refs_from_expr(hir_expr, ctx, src, &mut refs)?;
    }

    Ok(refs)
}

fn record_resolved_constructor_target(
    constructor: &ResolvedName<namespace::Constructor>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    span: Span,
    refs: &mut ResolvedConstructorRefs,
) -> Result<ResolvedConstructorTarget, GraphcalError> {
    if let Some(target) = refs.constructor_defs.get(constructor) {
        return Ok(target.clone());
    }

    let def = ctx.types.lookup_constructor(constructor).ok_or_else(|| {
        internal_error(
            format!("semantic constructor metadata references unknown constructor `{constructor}`"),
            src,
            span,
        )
    })?;
    let target = ResolvedConstructorTarget {
        constructor: constructor.clone(),
        owning_type: def.owning_type.clone(),
        type_def: def.type_def.clone(),
        variant: def.variant.clone(),
    };
    refs.constructor_defs
        .insert(constructor.clone(), target.clone());
    Ok(target)
}

#[expect(
    clippy::too_many_lines,
    reason = "expression traversal mirrors HIR variants"
)]
fn collect_resolved_constructor_refs_from_expr(
    expr: &hir::Expr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedConstructorRefs,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::ConstRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::UnitLiteral { .. }
        | hir::ExprKind::VariantLiteral(_) => Ok(()),
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_resolved_constructor_refs_from_expr(lhs, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(rhs, ctx, src, refs)
        }
        hir::ExprKind::UnaryOp { operand, .. } => {
            collect_resolved_constructor_refs_from_expr(operand, ctx, src, refs)
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_resolved_constructor_refs_from_expr(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_resolved_constructor_refs_from_expr(condition, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(then_branch, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(else_branch, ctx, src, refs)
        }
        hir::ExprKind::Convert { expr, .. }
        | hir::ExprKind::DisplayTimezone { expr, .. }
        | hir::ExprKind::FieldAccess { expr, .. } => {
            collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)
        }
        hir::ExprKind::ConstructorCall { callee, fields, .. } => {
            let target =
                record_resolved_constructor_target(&callee.value, ctx, src, callee.span, refs)?;
            refs.constructor_calls.insert(callee.span, target);
            for field in fields {
                collect_resolved_constructor_refs_from_expr(&field.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_resolved_constructor_refs_from_expr(&entry.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::ForComp { body, .. } => {
            collect_resolved_constructor_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::IndexAccess { expr, args } => {
            collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)?;
            for arg in args {
                if let hir::expr::IndexArg::Expr(expr) = arg {
                    collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)?;
                }
            }
            Ok(())
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_resolved_constructor_refs_from_expr(source, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_resolved_constructor_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_resolved_constructor_refs_from_expr(scrutinee, ctx, src, refs)?;
            for arm in arms {
                if let hir::expr::MatchPattern::Constructor {
                    constructor,
                    bindings,
                    ..
                } = &arm.pattern
                {
                    let target = record_resolved_constructor_target(
                        &constructor.value,
                        ctx,
                        src,
                        constructor.span,
                        refs,
                    )?;
                    let pattern = ResolvedConstructorPattern {
                        target,
                        bindings: bindings.iter().map(resolved_pattern_binding).collect(),
                    };
                    refs.match_pattern_constructors
                        .insert(constructor.span, pattern);
                }
                collect_resolved_constructor_refs_from_expr(&arm.body, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                collect_resolved_constructor_refs_from_expr(&arg.value, ctx, src, refs)?;
            }
            Ok(())
        }
    }
}

fn collect_resolved_inline_dag_refs(exprs: &ResolvedExpressions) -> ResolvedInlineDagRefs {
    let mut refs = ResolvedInlineDagRefs::default();

    for hir_expr in exprs
        .consts
        .values()
        .chain(exprs.param_defaults.values())
        .chain(exprs.nodes.values())
    {
        collect_resolved_inline_dag_refs_from_expr(hir_expr, &mut refs);
    }

    refs
}

fn collect_resolved_inline_dag_refs_from_expr(expr: &hir::Expr, refs: &mut ResolvedInlineDagRefs) {
    match &expr.kind {
        hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::ConstRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::UnitLiteral { .. }
        | hir::ExprKind::VariantLiteral(_) => {}
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_resolved_inline_dag_refs_from_expr(lhs, refs);
            collect_resolved_inline_dag_refs_from_expr(rhs, refs);
        }
        hir::ExprKind::UnaryOp { operand, .. }
        | hir::ExprKind::Convert { expr: operand, .. }
        | hir::ExprKind::DisplayTimezone { expr: operand, .. }
        | hir::ExprKind::FieldAccess { expr: operand, .. } => {
            collect_resolved_inline_dag_refs_from_expr(operand, refs);
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_resolved_inline_dag_refs_from_expr(arg, refs);
            }
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_resolved_inline_dag_refs_from_expr(condition, refs);
            collect_resolved_inline_dag_refs_from_expr(then_branch, refs);
            collect_resolved_inline_dag_refs_from_expr(else_branch, refs);
        }
        hir::ExprKind::ConstructorCall { fields, .. } => {
            for field in fields {
                collect_resolved_inline_dag_refs_from_expr(&field.value, refs);
            }
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_resolved_inline_dag_refs_from_expr(&entry.value, refs);
            }
        }
        hir::ExprKind::ForComp { body, .. } => {
            collect_resolved_inline_dag_refs_from_expr(body, refs);
        }
        hir::ExprKind::IndexAccess { expr, args } => {
            collect_resolved_inline_dag_refs_from_expr(expr, refs);
            for arg in args {
                if let hir::expr::IndexArg::Expr(expr) = arg {
                    collect_resolved_inline_dag_refs_from_expr(expr, refs);
                }
            }
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_resolved_inline_dag_refs_from_expr(source, refs);
            collect_resolved_inline_dag_refs_from_expr(init, refs);
            collect_resolved_inline_dag_refs_from_expr(body, refs);
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_resolved_inline_dag_refs_from_expr(init, refs);
            collect_resolved_inline_dag_refs_from_expr(body, refs);
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_resolved_inline_dag_refs_from_expr(scrutinee, refs);
            for arm in arms {
                collect_resolved_inline_dag_refs_from_expr(&arm.body, refs);
            }
        }
        hir::ExprKind::InlineDagRef {
            target,
            args,
            output,
        } => {
            let arg_targets = args
                .iter()
                .map(|arg| (arg.target.span, arg.target.value.clone()))
                .collect();
            refs.calls.insert(
                expr.span,
                ResolvedInlineDagCall {
                    target: target.value.clone(),
                    arg_targets,
                    output: output.clone(),
                },
            );
            for arg in args {
                collect_resolved_inline_dag_refs_from_expr(&arg.value, refs);
            }
        }
    }
}

fn resolved_pattern_binding(binding: &hir::expr::PatternBinding) -> ResolvedPatternBinding {
    match binding {
        hir::expr::PatternBinding::Bind { field, local } => ResolvedPatternBinding::Bind {
            field: field.clone(),
            local: local.clone(),
        },
        hir::expr::PatternBinding::Wildcard { field, span } => ResolvedPatternBinding::Wildcard {
            field: field.clone(),
            span: *span,
        },
    }
}

fn resolved_decl_key(
    owner: &crate::dag_id::DagId,
    name: &ScopedName,
) -> Option<ResolvedName<namespace::Decl>> {
    let owner = name
        .qualifier()
        .iter()
        .fold(owner.clone(), |owner, segment| {
            owner.child(segment.as_ref())
        });
    let name = DeclName::try_new(name.member()).ok()?;
    Some(ResolvedName::from_def(owner, name))
}

fn collect_hir_decl_bindings(
    owner: &crate::dag_id::DagId,
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ResolvedName<namespace::Decl>>, GraphcalError> {
    let mut bindings = HashMap::new();

    for name in consts
        .iter()
        .map(|entry| &entry.name)
        .chain(params.iter().map(|entry| &entry.name))
        .chain(nodes.iter().map(|entry| &entry.name))
    {
        let resolved = resolved_decl_key(owner, name).ok_or_else(|| {
            internal_error(
                format!("could not build canonical declaration key for `{name}`"),
                src,
                Span::new(0, 0),
            )
        })?;
        bindings.insert(name.clone(), resolved);
    }

    for (name, source) in imported_value_sources {
        bindings.insert(
            name.clone(),
            ResolvedName::from_def(source.dag_id.clone(), source.source_name.clone()),
        );
    }

    Ok(bindings)
}

#[expect(
    clippy::too_many_arguments,
    reason = "collects local and imported declaration binding sources for a completed DAG"
)]
fn collect_resolved_decl_bindings(
    ctx: ModuleTypeContext<'_>,
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    imported_values: &HashMap<
        ScopedName,
        (
            crate::registry::runtime_value::RuntimeValue,
            crate::registry::declared_type::DeclaredType,
        ),
    >,
    imported_decl_types: &HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ResolvedName<namespace::Decl>>, GraphcalError> {
    let mut bindings = collect_hir_decl_bindings(
        ctx.owner,
        consts,
        params,
        nodes,
        imported_value_sources,
        src,
    )?;

    for name in imported_values
        .keys()
        .chain(imported_decl_types.keys())
        .chain(imported_value_sources.keys())
    {
        if bindings.contains_key(name) {
            continue;
        }
        let path = scoped_name_to_name_path(name).ok_or_else(|| {
            internal_error(
                format!("could not convert visible declaration `{name}` to a name path"),
                src,
                Span::new(0, 0),
            )
        })?;
        let resolved = ctx
            .resolver
            .resolve_decl_path(ctx.owner, &path)
            .map_err(|err| module_resolve_error(&err, src, Span::new(0, 0)))?;
        bindings.insert(name.clone(), resolved);
    }

    Ok(bindings)
}

fn scoped_name_to_name_path(name: &ScopedName) -> Option<NamePath> {
    let qualifier = name
        .qualifier()
        .iter()
        .map(|segment| NameAtom::parse(segment.as_ref()).ok())
        .collect::<Option<Vec<_>>>()?;
    let leaf = NameAtom::parse(name.member()).ok()?;
    Some(if qualifier.is_empty() {
        NamePath::local(leaf)
    } else {
        NamePath::qualified_path(qualifier, leaf)
    })
}

fn resolve_expected_fail_keys(
    expected_fail: HashMap<ScopedName, ExpectedFail>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ExpectedFail>, GraphcalError> {
    expected_fail
        .into_iter()
        .map(|(assert_name, expected)| {
            let resolved = match expected {
                ExpectedFail::All => ExpectedFail::All,
                ExpectedFail::Variants(keys) => {
                    let resolved_keys = keys
                        .into_iter()
                        .map(|key| {
                            key.into_iter()
                                .map(|part| {
                                    if part.source_index_path.is_none() {
                                        return Ok(part);
                                    }
                                    let index_path =
                                        part.source_index_path.clone().unwrap_or_else(|| {
                                            NamePath::from(part.index.name().clone())
                                        });
                                    let resolved = ctx
                                        .resolver
                                        .resolve_index_variant_parts(
                                            ctx.owner,
                                            &index_path,
                                            &part.variant,
                                        )
                                        .map_err(|err| {
                                            module_resolve_error(&err, src, part.span)
                                        })?;
                                    Ok(part.with_resolved_variant(resolved))
                                })
                                .collect::<Result<_, GraphcalError>>()
                        })
                        .collect::<Result<_, GraphcalError>>()?;
                    ExpectedFail::Variants(resolved_keys)
                }
            };
            Ok((assert_name, resolved))
        })
        .collect()
}

/// Partially-built [`DagTIR`] returned by [`type_resolve_dag`]; finalized
/// by [`DagTIRSeed::with_body`] which fills in the rest of the per-DAG
/// fields.
struct DagTIRSeed {
    dag_id: crate::dag_id::DagId,
    consts: Vec<crate::ir::lower::ConstEntry>,
    params: Vec<crate::ir::lower::ParamEntry>,
    nodes: Vec<crate::ir::lower::NodeEntry>,
    resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>,
    semantic: DagSemanticBody,
}

impl DagTIRSeed {
    #[expect(
        clippy::too_many_arguments,
        reason = "single conversion that absorbs every IR field beyond the resolved decls"
    )]
    fn with_body(
        self,
        asserts: Vec<crate::ir::lower::AssertEntry>,
        plots: Vec<crate::ir::lower::PlotEntry>,
        figures: Vec<crate::ir::lower::FigureEntry>,
        layers: Vec<crate::ir::lower::LayerEntry>,
        _runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
        _const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
        source_order: Vec<(ScopedName, DeclCategory)>,
        assert_names: std::collections::HashSet<ScopedName>,
        assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
        expected_fail: HashMap<ScopedName, ExpectedFail>,
        imported_values: HashMap<
            ScopedName,
            (
                crate::registry::runtime_value::RuntimeValue,
                crate::registry::declared_type::DeclaredType,
            ),
        >,
        imported_decl_types: HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
        imported_value_sources: HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
        module_ctx: ModuleTypeContext<'_>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<DagTIR, GraphcalError> {
        let decl_bindings = collect_resolved_decl_bindings(
            module_ctx,
            &self.consts,
            &self.params,
            &self.nodes,
            &imported_values,
            &imported_decl_types,
            &imported_value_sources,
            src,
        )?;
        let expected_fail = resolve_expected_fail_keys(expected_fail, module_ctx, src)?;

        let mut semantic = self.semantic;
        semantic.decl_bindings = decl_bindings;

        Ok(DagTIR {
            dag_id: self.dag_id,
            consts: self.consts,
            params: self.params,
            nodes: self.nodes,
            asserts,
            plots,
            figures,
            layers,
            semantic,
            source_order,
            assert_names,
            assumes_map,
            expected_fail,
            resolved_decl_types: self.resolved_decl_types,
            domain_constraints: HashMap::new(), // Resolved later in compile()
            imported_values,
            imported_decl_types,
            imported_value_sources,
            pub_nodes: std::collections::HashSet::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Conversion to DeclaredType
// ---------------------------------------------------------------------------

/// Convert a non-generic [`ResolvedTypeExpr`] to a `DeclaredType`.
///
/// This is used by downstream stages (`dim_check`, `eval`) that work with concrete
/// types. Generic variants (`GenericDimParam`, `GenericDimExpr`, generic indexes)
/// cannot be converted and will return an error.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if the resolved type contains unresolved generic
/// parameters.
pub fn resolved_to_declared_type(
    resolved: &ResolvedTypeExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::registry::declared_type::DeclaredType, GraphcalError> {
    use crate::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(DeclaredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(DeclaredType::Bool),
        ResolvedTypeExpr::Int => Ok(DeclaredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(DeclaredType::Datetime(*scale)),
        ResolvedTypeExpr::Label(index, _) => Ok(DeclaredType::Label(IndexTypeRef::from_resolved(
            index.clone(),
        ))),
        ResolvedTypeExpr::Scalar(dim) => Ok(DeclaredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(DeclaredType::Struct(
            StructTypeRef::from_resolved(name.clone()),
            vec![],
        )),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut declared_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                declared_args.push(resolved_to_declared_type(arg, src)?);
            }
            Ok(DeclaredType::Struct(
                StructTypeRef::from_resolved(name.clone()),
                declared_args,
            ))
        }
        ResolvedTypeExpr::GenericDimParam(name, span) => Err(GraphcalError::EvalError {
            message: format!("cannot use generic dimension parameter `{name}` as a concrete type"),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::GenericTypeParam(name, span) => Err(GraphcalError::EvalError {
            message: format!("cannot use generic type parameter `{name}` as a concrete type"),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::GenericDimExpr { span, .. } => Err(GraphcalError::EvalError {
            message: "cannot use generic dimension expression as a concrete type".to_string(),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::Indexed { base, indexes } => {
            let mut result = resolved_to_declared_type(base, src)?;
            for idx in indexes.iter().rev() {
                match idx {
                    ResolvedIndex::Concrete(name, _) => {
                        result = DeclaredType::Indexed {
                            element: Box::new(result),
                            index: IndexTypeRef::from_resolved(name.clone()),
                        };
                    }
                    ResolvedIndex::NatExpr(form, span) => {
                        if !form.is_constant() {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "cannot use generic nat expression `{}` as a concrete type",
                                    form.format()
                                ),
                                src: src.clone(),
                                span: (*span).into(),
                            });
                        }
                        let nat_range = form.to_nat_range_identity().concrete_index().ok_or_else(|| {
                            GraphcalError::EvalError {
                                message: format!(
                                    "nat range size `{}` is not representable as a concrete non-empty index",
                                    form.format()
                                ),
                                src: src.clone(),
                                span: (*span).into(),
                            }
                        })?;
                        result = DeclaredType::Indexed {
                            element: Box::new(result),
                            index: IndexTypeRef::from_nat_range(nat_range),
                        };
                    }
                    ResolvedIndex::GenericParam(name, span) => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "cannot use generic index parameter `{name}` as a concrete type"
                            ),
                            src: src.clone(),
                            span: (*span).into(),
                        });
                    }
                }
            }
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
// Nat polynomial form unification
// ---------------------------------------------------------------------------

/// Solve a polynomial equation `form = target` for Nat generic params.
///
/// Substitutes already-bound variables, then:
/// - If no unbound vars remain: checks evaluated form == target.
/// - If exactly one unbound var appears only linearly (degree 1): solves the linear equation.
/// - Otherwise: returns an error (ambiguous or non-linear in unbound vars).
fn unify_nat_poly_form(
    form: &NatPolyForm,
    target: u64,
    nat_sub: &mut HashMap<GenericParamName, u64>,
    actual_idx: &IndexName,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    // Substitute already-bound variables in each monomial, collecting
    // a reduced polynomial in only unbound variables + a constant part.
    let mut reduced_constant: u64 = 0;
    // (reduced_monomial, coefficient) pairs for terms with unbound variables
    let mut reduced_terms: BTreeMap<Monomial, u64> = BTreeMap::new();

    for (mono, coeff) in &form.terms {
        let Some((remaining_mono, factor)) = mono.substitute(nat_sub) else {
            // Arithmetic overflow during substitution
            return Err(GraphcalError::IndexMismatch {
                expected: IndexName::new(format!("range({})", form.format())),
                found: actual_idx.clone(),
                src: src.clone(),
                span: span.into(),
            });
        };
        let term_value = coeff * factor;
        if remaining_mono.is_constant() {
            reduced_constant += term_value;
        } else {
            *reduced_terms.entry(remaining_mono).or_insert(0) += term_value;
        }
    }
    // Remove zero terms
    reduced_terms.retain(|_, c| *c != 0);

    if reduced_terms.is_empty() {
        // All variables bound — check equality
        if reduced_constant != target {
            let expected = form.evaluate(nat_sub).map_or_else(
                || IndexName::new(format!("range({})", form.format())),
                |n| {
                    crate::registry::types::NatRangeIndex::try_from_u64(n).map_or_else(
                        || IndexName::new(format!("range({n})")),
                        crate::registry::types::NatRangeIndex::display_name,
                    )
                },
            );
            return Err(GraphcalError::IndexMismatch {
                expected,
                found: actual_idx.clone(),
                src: src.clone(),
                span: span.into(),
            });
        }
        return Ok(());
    }

    // Check if exactly one unbound variable appears, only at degree 1
    let mut unbound_vars = std::collections::BTreeSet::new();
    for mono in reduced_terms.keys() {
        for var in mono.0.keys() {
            unbound_vars.insert(var.clone());
        }
    }

    if let [var] = unbound_vars.iter().collect::<Vec<_>>().as_slice() {
        let var = (*var).clone();
        // Check all remaining monomials are linear in this variable
        let all_linear = reduced_terms
            .keys()
            .all(|m| m.0.len() == 1 && m.0.get(&var) == Some(&1));

        if all_linear {
            // Solve: coeff * var + reduced_constant = target
            let total_coeff: u64 = reduced_terms.values().sum();
            if target < reduced_constant {
                return Err(GraphcalError::IndexMismatch {
                    expected: IndexName::new(format!("range({})", form.format())),
                    found: actual_idx.clone(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let remainder = target - reduced_constant;
            if total_coeff == 0 || !remainder.is_multiple_of(total_coeff) {
                return Err(GraphcalError::IndexMismatch {
                    expected: IndexName::new(format!("range({})", form.format())),
                    found: actual_idx.clone(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let value = remainder / total_coeff;
            bind_or_check(nat_sub, var, value, |prev, _| {
                GraphcalError::IndexMismatch {
                    expected: crate::registry::types::NatRangeIndex::try_from_u64(*prev)
                        .map_or_else(
                            || IndexName::new(format!("range({prev})")),
                            crate::registry::types::NatRangeIndex::display_name,
                        ),
                    found: actual_idx.clone(),
                    src: src.clone(),
                    span: span.into(),
                }
            })?;
            return Ok(());
        }
    }

    // Multiple unbound variables or non-linear — ambiguous
    let var_names: Vec<&str> = unbound_vars.iter().map(GenericParamName::as_str).collect();
    Err(GraphcalError::EvalError {
        message: format!(
            "cannot infer Nat parameters [{}] from a single index — \
             provide more arguments or use explicit type annotations",
            var_names.join(", ")
        ),
        src: src.clone(),
        span: span.into(),
    })
}

// ---------------------------------------------------------------------------
// Unification
// ---------------------------------------------------------------------------

/// Bind a generic parameter in a substitution map, or check consistency if already bound.
///
/// If `key` is not yet in `sub`, inserts `(key, value)`. If `key` is already bound
/// to a value equal to `value`, succeeds. Otherwise, calls `on_conflict` with the
/// previously bound value and the new value to produce an error.
fn bind_or_check<K, V, E>(
    sub: &mut HashMap<K, V>,
    key: K,
    value: V,
    on_conflict: impl FnOnce(&V, &V) -> E,
) -> Result<(), E>
where
    K: Eq + std::hash::Hash,
    V: PartialEq,
{
    if let Some(prev) = sub.get(&key) {
        if *prev != value {
            return Err(on_conflict(prev, &value));
        }
    } else {
        sub.insert(key, value);
    }
    Ok(())
}

/// Unify a resolved type expression against an actual inferred type,
/// binding generic dimension and index parameters.
///
/// For example, if `resolved` is `GenericDimParam("D")` and `actual` is
/// `Scalar(Length)`, binds `D = Length` in `dim_sub`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] on type mismatch or conflicting bindings.
#[expect(
    clippy::too_many_lines,
    reason = "complex generic unification requires many match arms"
)]
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "unification needs all substitution maps, registry, and source context"
)]
pub fn unify_resolved_type(
    resolved: &ResolvedTypeExpr,
    actual: &crate::tir::dim_check::InferredType,
    dim_sub: &mut HashMap<GenericParamName, Dimension>,
    index_sub: &mut HashMap<GenericParamName, IndexTypeRef>,
    nat_sub: &mut HashMap<GenericParamName, u64>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    use crate::tir::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Indexed { base, indexes } => {
            // Peel off index layers from actual type, binding index generics.
            // Iterate forward: first index in the list is the outermost Indexed layer.
            let mut current = actual;
            for idx in indexes {
                let InferredType::Indexed {
                    element,
                    index: actual_idx,
                } = current
                else {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "indexed type".to_string(),
                        found: crate::tir::dim_check::format_inferred_type(current, registry),
                        help: "expected an indexed value".to_string(),
                        src: src.clone(),
                        span: span.into(),
                    });
                };
                match idx {
                    ResolvedIndex::GenericParam(gp, _) => {
                        bind_or_check(
                            index_sub,
                            gp.clone(),
                            actual_idx.type_ref().clone(),
                            |prev, _| GraphcalError::IndexMismatch {
                                expected: prev.name().clone(),
                                found: actual_idx.name().clone(),
                                src: src.clone(),
                                span: span.into(),
                            },
                        )?;
                    }
                    ResolvedIndex::Concrete(name, _) => {
                        if actual_idx.resolved() != name {
                            return Err(GraphcalError::IndexMismatch {
                                expected: name.to_unowned_def_name(),
                                found: actual_idx.name().clone(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    }
                    ResolvedIndex::NatExpr(form, _) => {
                        // Extract the concrete nat value from the typed actual Nat-range identity.
                        let actual_nat = actual_idx
                            .nat_range_form()
                            .filter(|actual_form| actual_form.is_constant())
                            .map(NatPolyForm::constant)
                            .ok_or_else(|| GraphcalError::IndexMismatch {
                                expected: IndexName::new(format!("range({})", form.format())),
                                found: actual_idx.name().clone(),
                                src: src.clone(),
                                span: span.into(),
                            })?;
                        // Solve the polynomial equation: form = actual_nat
                        unify_nat_poly_form(form, actual_nat, nat_sub, actual_idx, src, span)?;
                    }
                }
                current = element;
            }
            unify_resolved_type(
                base, current, dim_sub, index_sub, nat_sub, registry, src, span,
            )
        }

        ResolvedTypeExpr::Bool => {
            if *actual != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected Bool argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Int => {
            if !actual.is_int_like() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected Int argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Datetime(expected_scale) => {
            if *actual != InferredType::Datetime(*expected_scale) {
                let expected_str = if expected_scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{expected_scale}>")
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: expected_str,
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: "expected Datetime argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Label(expected_index, _) => {
            let InferredType::Label(actual_index) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("Label({})", expected_index.as_str()),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected a label of index `{}`", expected_index.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if actual_index.resolved() != expected_index {
                return Err(GraphcalError::IndexMismatch {
                    expected: expected_index.to_unowned_def_name(),
                    found: actual_index.name().clone(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Dimensionless => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;
            if !actual_dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    help: "expected Dimensionless argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Scalar(expected_dim) => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;
            if *expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(expected_dim),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    help: "dimension mismatch in function argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericStruct { name, .. } | ResolvedTypeExpr::Struct(name, _) => {
            // Type args matching is not needed here since function generics
            // don't use TypeApplication in their signatures (yet). When both
            // sides carry canonical struct identities, compare owners as well.
            let InferredType::Struct(actual_name, _) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.as_str().to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{}`", name.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if actual_name.resolved() != name {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.as_str().to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{}`", name.as_str()),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericDimParam(gp, _) => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;
            bind_or_check(dim_sub, gp.clone(), actual_dim, |prev, new| {
                GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(prev),
                    found: registry.dimensions.format_dimension(new),
                    help: format!(
                        "generic `{gp}` was bound to {} but this argument requires {}",
                        registry.dimensions.format_dimension(prev),
                        registry.dimensions.format_dimension(new),
                    ),
                    src: src.clone(),
                    span: span.into(),
                }
            })
        }

        ResolvedTypeExpr::GenericTypeParam(gp, gp_span) => Err(GraphcalError::EvalError {
            message: format!(
                "cannot infer unconstrained generic type parameter `{gp}` in this position yet"
            ),
            src: src.clone(),
            span: (*gp_span).into(),
        }),

        ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;

            // Single generic term with power: D^n means D = actual^(1/n)
            if terms.len() == 1
                && let ResolvedDimTerm::GenericParam {
                    name: gp,
                    power,
                    op: MulDivOp::Mul,
                    ..
                } = &terms[0]
            {
                let bound_dim = if *power == 1 {
                    actual_dim
                } else {
                    let exponent =
                        Rational::try_new(1, *power).map_err(|_| GraphcalError::InternalError {
                            message: format!("generic dimension parameter `{gp}` has zero power"),
                            src: src.clone(),
                            span: span.into(),
                        })?;
                    actual_dim
                        .pow(exponent)
                        .map_err(|_| GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: span.into(),
                        })?
                };
                bind_or_check(dim_sub, gp.clone(), bound_dim, |prev, new| {
                    GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(prev),
                        found: registry.dimensions.format_dimension(new),
                        help: format!(
                            "generic `{gp}` was bound to {} but this argument requires {}",
                            registry.dimensions.format_dimension(prev),
                            registry.dimensions.format_dimension(new),
                        ),
                        src: src.clone(),
                        span: span.into(),
                    }
                })?;
                return Ok(());
            }

            // General case: compute expected dimension from already-bound generics + concrete terms
            let mut expected_dim = Dimension::dimensionless();
            for term in terms {
                let overflow_err = || GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: span.into(),
                };
                let term_dim = match term {
                    ResolvedDimTerm::Concrete { dim, power, .. } => dim
                        .pow(Rational::from_int(*power))
                        .map_err(|_| overflow_err())?,
                    ResolvedDimTerm::GenericParam {
                        name: gp, power, ..
                    } => {
                        if let Some(prev) = dim_sub.get(gp) {
                            prev.pow(Rational::from_int(*power))
                                .map_err(|_| overflow_err())?
                        } else {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!("generic `{gp}` (unresolved)"),
                                found: registry.dimensions.format_dimension(&actual_dim),
                                help: format!(
                                    "generic `{gp}` could not be inferred from this argument"
                                ),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    }
                };
                expected_dim = match term.op() {
                    MulDivOp::Mul => (expected_dim * term_dim).map_err(|_| overflow_err())?,
                    MulDivOp::Div => (expected_dim / term_dim).map_err(|_| overflow_err())?,
                };
            }

            if expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&expected_dim),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    help: "dimension mismatch in function argument".to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Substitution
// ---------------------------------------------------------------------------

/// Substitute generic parameters in a resolved type, producing an `InferredType`.
///
/// This replaces `resolve_type_with_substitution()` from `dim_check.rs`.
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
#[expect(
    clippy::too_many_lines,
    reason = "single dispatch over ResolvedTypeExpr variants with per-variant generic-substitution + dimension-arithmetic overflow handling"
)]
pub fn substitute_resolved_type(
    resolved: &ResolvedTypeExpr,
    dim_sub: &HashMap<GenericParamName, Dimension>,
    index_sub: &HashMap<GenericParamName, IndexTypeRef>,
    nat_sub: &HashMap<GenericParamName, u64>,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::tir::dim_check::InferredType, GraphcalError> {
    use crate::tir::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(InferredType::Bool),
        ResolvedTypeExpr::Int => Ok(InferredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(InferredType::Datetime(*scale)),
        ResolvedTypeExpr::Label(index, _) => Ok(InferredType::Label(
            crate::tir::dim_check::InferredIndex::from_resolved(index.clone()),
        )),
        ResolvedTypeExpr::Scalar(dim) => Ok(InferredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(InferredType::Struct(
            crate::tir::dim_check::InferredStructType::from_resolved(name.clone()),
            vec![],
        )),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut inferred_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                inferred_args.push(substitute_resolved_type(
                    arg, dim_sub, index_sub, nat_sub, src,
                )?);
            }
            Ok(InferredType::Struct(
                crate::tir::dim_check::InferredStructType::from_resolved(name.clone()),
                inferred_args,
            ))
        }

        ResolvedTypeExpr::GenericDimParam(gp, span) => dim_sub.get(gp).map_or_else(
            || {
                Err(GraphcalError::EvalError {
                    message: format!("generic `{gp}` not bound during substitution"),
                    src: src.clone(),
                    span: (*span).into(),
                })
            },
            |dim| Ok(InferredType::Scalar(dim.clone())),
        ),

        ResolvedTypeExpr::GenericTypeParam(gp, span) => Err(GraphcalError::EvalError {
            message: format!("generic type parameter `{gp}` not bound during substitution"),
            src: src.clone(),
            span: (*span).into(),
        }),

        ResolvedTypeExpr::GenericDimExpr { terms, span } => {
            let overflow_err = || GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: (*span).into(),
            };
            let mut result = Dimension::dimensionless();
            for term in terms {
                let term_dim = match term {
                    ResolvedDimTerm::Concrete { dim, power, .. } => dim
                        .pow(Rational::from_int(*power))
                        .map_err(|_| overflow_err())?,
                    ResolvedDimTerm::GenericParam {
                        name: gp,
                        power,
                        span: term_span,
                        ..
                    } => {
                        let base = dim_sub.get(gp).ok_or_else(|| GraphcalError::EvalError {
                            message: format!("generic `{gp}` not bound during substitution"),
                            src: src.clone(),
                            span: (*term_span).into(),
                        })?;
                        base.pow(Rational::from_int(*power))
                            .map_err(|_| overflow_err())?
                    }
                };
                result = match term.op() {
                    MulDivOp::Mul => (result * term_dim).map_err(|_| overflow_err())?,
                    MulDivOp::Div => (result / term_dim).map_err(|_| overflow_err())?,
                };
            }
            Ok(InferredType::Scalar(result))
        }

        ResolvedTypeExpr::Indexed { base, indexes } => {
            let mut result = substitute_resolved_type(base, dim_sub, index_sub, nat_sub, src)?;
            for idx in indexes.iter().rev() {
                let resolved_idx = match idx {
                    ResolvedIndex::Concrete(name, _) => {
                        result = InferredType::Indexed {
                            element: Box::new(result),
                            index: crate::tir::dim_check::InferredIndex::from_resolved(
                                name.clone(),
                            ),
                        };
                        continue;
                    }
                    ResolvedIndex::GenericParam(gp, span) => {
                        crate::tir::dim_check::InferredIndex::from_ref(
                            index_sub
                                .get(gp)
                                .cloned()
                                .ok_or_else(|| GraphcalError::EvalError {
                                    message: format!(
                                        "generic index `{gp}` not bound during substitution"
                                    ),
                                    src: src.clone(),
                                    span: (*span).into(),
                                })?,
                        )
                    }
                    ResolvedIndex::NatExpr(form, span) => {
                        let n = form.evaluate(nat_sub).ok_or_else(|| {
                            let vars = form.variables();
                            let unbound: Vec<&str> = vars
                                .iter()
                                .filter(|k| !nat_sub.contains_key(*k))
                                .map(GenericParamName::as_str)
                                .collect();
                            GraphcalError::EvalError {
                                message: format!(
                                    "generic nat parameter(s) [{}] not bound during substitution",
                                    unbound.join(", ")
                                ),
                                src: src.clone(),
                                span: (*span).into(),
                            }
                        })?;
                        crate::tir::dim_check::InferredIndex::from_nat_range_form(
                            NatPolyForm::from_constant(n),
                        )
                    }
                };
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: resolved_idx,
                };
            }
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract scalar dimension from an `InferredType`.
fn expect_scalar_from_inferred(
    inferred: &crate::tir::dim_check::InferredType,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    match inferred {
        crate::tir::dim_check::InferredType::Scalar(d) => Ok(d.clone()),
        other => Err(GraphcalError::DimensionMismatch {
            expected: "scalar dimension".to_string(),
            found: crate::tir::dim_check::format_inferred_type(other, registry),
            help: "expected a scalar value, not a struct or indexed type".to_string(),
            src: src.clone(),
            span: span.into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Type resolution (single TypeExpr)
// ---------------------------------------------------------------------------

fn require_local_type_level_path<'a>(
    path: &'a NamePath,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<&'a str, GraphcalError> {
    path.as_bare()
        .map(super::super::syntax::names::NameAtom::as_str)
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!(
                "qualified type-level reference `{path}` needs module-aware resolution"
            ),
            src: src.clone(),
            span: span.into(),
        })
}

fn module_resolve_error(
    err: &ModuleResolveError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

fn internal_error(message: String, src: &NamedSource<Arc<String>>, span: Span) -> GraphcalError {
    GraphcalError::InternalError {
        message,
        src: src.clone(),
        span: span.into(),
    }
}

const fn module_lookup_is_absent(err: &ModuleResolveError) -> bool {
    matches!(err, ModuleResolveError::UnknownName { .. })
}

fn expr_lower_error_to_graphcal(
    err: &hir::ExprLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    match err {
        hir::ExprLowerError::UnknownLocalRef { name, span } => {
            return GraphcalError::UnknownLocalRef {
                name: name.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ExtraMapVariant {
            index_name,
            variant_name,
            span,
        } => {
            return GraphcalError::ExtraVariants {
                index_name: index_name.clone(),
                extra: vec![variant_name.clone()],
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownIndexVariant { index, variant },
            span,
        } => {
            return GraphcalError::UnknownVariant {
                index_name: index.to_unowned_def_name(),
                variant_name: variant.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    namespace, name, ..
                },
            span,
        } if *namespace == namespace::Index::DISPLAY_NAME => {
            if let Ok(index_name) = IndexName::try_new(name.clone()) {
                return GraphcalError::UnknownIndex {
                    name: index_name,
                    src: src.clone(),
                    span: (*span).into(),
                };
            }
        }
        hir::ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    namespace, name, ..
                },
            span,
        } if *namespace == namespace::Decl::DISPLAY_NAME => {
            return GraphcalError::UnknownLocalRef {
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        _ => {}
    }
    let span = match err {
        hir::ExprLowerError::Type(err) => return hir_lower_error_to_graphcal(err, src),
        hir::ExprLowerError::ModuleResolve { span, .. }
        | hir::ExprLowerError::InvalidScopedNameSegment { span, .. }
        | hir::ExprLowerError::UnknownLocalRef { span, .. }
        | hir::ExprLowerError::TooManyLocals { span }
        | hir::ExprLowerError::EmptyMapEntry { span }
        | hir::ExprLowerError::ExtraMapVariant { span, .. }
        | hir::ExprLowerError::UnknownFunction { span, .. }
        | hir::ExprLowerError::UnknownPattern { span, .. } => *span,
        hir::ExprLowerError::DuplicateLocalBinding { duplicate, .. } => *duplicate,
    };
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

fn type_lower_error_to_graphcal(
    err: &hir::HirLowerError,
    type_ann: &TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    if let hir::HirLowerError::UnknownTypePath { path, span } = err {
        if type_expr_has_index_name_at_span(type_ann, *span)
            && let Ok(name) = IndexName::try_new(path.clone())
        {
            return GraphcalError::UnknownIndex {
                name,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        if type_expr_has_dim_term_at_span(type_ann, *span)
            && let Ok(name) = DimName::try_new(path.clone())
        {
            return GraphcalError::UnknownDimension {
                name,
                src: src.clone(),
                span: (*span).into(),
            };
        }
    }
    hir_lower_error_to_graphcal(err, src)
}

fn type_expr_has_index_name_at_span(type_ann: &TypeExpr, span: Span) -> bool {
    match &type_ann.kind {
        TypeExprKind::Indexed { base, indexes } => {
            type_expr_has_index_name_at_span(base, span)
                || indexes.iter().any(|index| match index {
                    crate::desugar::resolved_ast::IndexExpr::Name(name) => name.span == span,
                    crate::desugar::resolved_ast::IndexExpr::NatExpr(_) => false,
                })
        }
        TypeExprKind::TypeApplication { type_args, .. }
        | TypeExprKind::DatetimeApplication { type_args } => type_args
            .iter()
            .any(|arg| type_expr_has_index_name_at_span(arg, span)),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => false,
    }
}

fn type_expr_has_dim_term_at_span(type_ann: &TypeExpr, span: Span) -> bool {
    match &type_ann.kind {
        TypeExprKind::DimExpr(dim_expr) => dim_expr
            .terms
            .iter()
            .any(|item| item.term.name.span == span),
        TypeExprKind::Indexed { base, .. } => type_expr_has_dim_term_at_span(base, span),
        TypeExprKind::TypeApplication { type_args, .. }
        | TypeExprKind::DatetimeApplication { type_args } => type_args
            .iter()
            .any(|arg| type_expr_has_dim_term_at_span(arg, span)),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => false,
    }
}

fn hir_lower_error_to_graphcal(
    err: &hir::HirLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    let span = match &err {
        hir::HirLowerError::ModuleResolve { span, .. }
        | hir::HirLowerError::UnknownTypePath { span, .. }
        | hir::HirLowerError::GenericConstraintMismatch { span, .. }
        | hir::HirLowerError::UnknownGenericParam { span, .. }
        | hir::HirLowerError::ExpectedTimeScaleName { span }
        | hir::HirLowerError::UnknownTimeScale { span, .. }
        | hir::HirLowerError::WrongDatetimeArgCount { span, .. } => *span,
        hir::HirLowerError::DuplicateGenericParam { duplicate, .. } => *duplicate,
    };
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

#[derive(Clone, Copy)]
struct HirTypeResolutionContext<'a> {
    src: &'a NamedSource<Arc<String>>,
    resolver: &'a ModuleResolver,
    module_types: &'a ModuleTypeRegistry,
    registry: Option<&'a Registry>,
    prelude: &'a hir::PreludeTypeScope,
}

/// Resolve an already-lowered HIR type expression into the TIR type
/// representation.
///
/// This is the new semantic entry point for module-aware TIR type resolution:
/// source paths should be lowered to HIR first, then TIR consumes canonical
/// `ResolvedName<Ns>` and lexical generic IDs from HIR instead of performing
/// source-path lookup itself.
pub fn resolve_hir_type_expr(
    type_ann: &hir::TypeExpr,
    _registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let prelude = hir::PreludeTypeScope::graphcal();
    let ctx = HirTypeResolutionContext {
        src,
        resolver: module_ctx.resolver,
        module_types: module_ctx.types,
        registry: None,
        prelude: &prelude,
    };
    resolve_hir_type_expr_inner(type_ann, ctx)
}

fn resolve_ast_type_expr_via_hir(
    type_ann: &TypeExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let generic_scope = hir::GenericScope::new();
    let prelude = hir::PreludeTypeScope::graphcal();
    let lower_ctx =
        hir::TypeLoweringContext::new(module_ctx.owner, module_ctx.resolver, &generic_scope)
            .with_prelude(&prelude);
    let hir_type = hir::lower_type_expr(type_ann, lower_ctx)
        .map_err(|err| type_lower_error_to_graphcal(&err, type_ann, src))?;
    let resolve_ctx = HirTypeResolutionContext {
        src,
        resolver: module_ctx.resolver,
        module_types: module_ctx.types,
        registry: Some(registry),
        prelude: &prelude,
    };
    resolve_hir_type_expr_inner(&hir_type, resolve_ctx)
}

fn resolve_hir_type_expr_inner(
    type_ann: &hir::TypeExpr,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    match &type_ann.kind {
        hir::TypeExprKind::Builtin(builtin) => Ok(resolve_hir_builtin_type(*builtin)),
        hir::TypeExprKind::DimExpr(dim_expr) => resolve_hir_dim_expr(dim_expr, ctx),
        hir::TypeExprKind::Label(index) => {
            hir_index_name(&index.value, index.span, ctx)?;
            Ok(ResolvedTypeExpr::Label(index.value.clone(), index.span))
        }
        hir::TypeExprKind::Struct(name) => {
            hir_struct_type_def(&name.value, name.span, ctx)?;
            Ok(ResolvedTypeExpr::Struct(name.value.clone(), name.span))
        }
        hir::TypeExprKind::GenericTypeParam(param) => Ok(ResolvedTypeExpr::GenericTypeParam(
            param.value.name.clone(),
            param.span,
        )),
        hir::TypeExprKind::TypeApplication { name, type_args } => {
            resolve_hir_type_application(type_ann, name, type_args, ctx)
        }
        hir::TypeExprKind::Indexed { base, indexes } => {
            let resolved_base = resolve_hir_type_expr_inner(base, ctx)?;
            let resolved_indexes = indexes
                .iter()
                .map(|index| resolve_hir_index_ref(index, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::Indexed {
                base: Box::new(resolved_base),
                indexes: resolved_indexes,
            })
        }
    }
}

const fn resolve_hir_builtin_type(builtin: hir::BuiltinType) -> ResolvedTypeExpr {
    match builtin {
        hir::BuiltinType::Dimensionless => ResolvedTypeExpr::Dimensionless,
        hir::BuiltinType::Bool => ResolvedTypeExpr::Bool,
        hir::BuiltinType::Int => ResolvedTypeExpr::Int,
        hir::BuiltinType::Datetime(scale) => ResolvedTypeExpr::Datetime(scale.scale()),
    }
}

fn hir_dimension(
    name: &ResolvedName<namespace::Dim>,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<Dimension, GraphcalError> {
    ctx.module_types
        .get_dimension(name)
        .cloned()
        .or_else(|| {
            ctx.registry.and_then(|registry| {
                registry
                    .dimensions
                    .get_dimension(name.to_unowned_def_name().as_str())
                    .cloned()
            })
        })
        .ok_or_else(|| GraphcalError::UnknownDimension {
            name: name.to_unowned_def_name(),
            src: ctx.src.clone(),
            span: span.into(),
        })
}

fn hir_index_name(
    name: &ResolvedName<namespace::Index>,
    span: Span,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<IndexName, GraphcalError> {
    if ctx.module_types.get_index(name).is_some() {
        Ok(name.to_unowned_def_name())
    } else {
        Err(GraphcalError::UnknownIndex {
            name: name.to_unowned_def_name(),
            src: ctx.src.clone(),
            span: span.into(),
        })
    }
}

fn hir_struct_type_def<'a>(
    name: &ResolvedName<namespace::StructType>,
    span: Span,
    ctx: HirTypeResolutionContext<'a>,
) -> Result<&'a TypeDef, GraphcalError> {
    ctx.module_types
        .get_struct_type(name)
        .ok_or_else(|| GraphcalError::UnknownStructType {
            name: name.to_string(),
            src: ctx.src.clone(),
            span: span.into(),
        })
}

fn resolve_hir_dim_expr(
    dim_expr: &hir::DimExpr,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let terms = dim_expr
        .terms
        .iter()
        .map(|item| resolve_hir_dim_expr_item(item, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    if let [
        ResolvedDimTerm::GenericParam {
            name,
            power: 1,
            op: MulDivOp::Mul,
            span,
        },
    ] = terms.as_slice()
    {
        return Ok(ResolvedTypeExpr::GenericDimParam(name.clone(), *span));
    }

    let has_generic = terms
        .iter()
        .any(|term| matches!(term, ResolvedDimTerm::GenericParam { .. }));
    if has_generic {
        return Ok(ResolvedTypeExpr::GenericDimExpr {
            terms,
            span: dim_expr.span,
        });
    }

    let result = terms.iter().try_fold(
        Dimension::dimensionless(),
        |acc, term| -> Result<Dimension, GraphcalError> {
            let ResolvedDimTerm::Concrete { dim, power, op } = term else {
                return Err(GraphcalError::InternalError {
                    message: "generic dimension term reached concrete dimension folding"
                        .to_string(),
                    src: ctx.src.clone(),
                    span: dim_expr.span.into(),
                });
            };
            let overflow_err = || GraphcalError::DimensionOverflow {
                src: ctx.src.clone(),
                span: dim_expr.span.into(),
            };
            let powered = dim
                .pow(Rational::from_int(*power))
                .map_err(|_| overflow_err())?;
            match op {
                MulDivOp::Mul => (acc * powered).map_err(|_| overflow_err()),
                MulDivOp::Div => (acc / powered).map_err(|_| overflow_err()),
            }
        },
    )?;
    Ok(ResolvedTypeExpr::Scalar(result))
}

fn resolve_hir_dim_expr_item(
    item: &hir::DimExprItem,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedDimTerm, GraphcalError> {
    let power = item.term.power.unwrap_or(1);
    match &item.term.target {
        hir::DimTermTarget::Dimension(name) => Ok(ResolvedDimTerm::Concrete {
            dim: hir_dimension(&name.value, name.span, ctx)?,
            power,
            op: item.op,
        }),
        hir::DimTermTarget::GenericParam(param) => Ok(ResolvedDimTerm::GenericParam {
            name: param.value.name.clone(),
            power,
            op: item.op,
            span: item.term.span,
        }),
    }
}

fn resolve_hir_index_ref(
    index: &hir::IndexRef,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedIndex, GraphcalError> {
    match index {
        hir::IndexRef::Concrete(name) => {
            hir_index_name(&name.value, name.span, ctx)?;
            Ok(ResolvedIndex::Concrete(name.value.clone(), name.span))
        }
        hir::IndexRef::GenericParam(param) => Ok(ResolvedIndex::GenericParam(
            param.value.name.clone(),
            param.span,
        )),
        hir::IndexRef::NatExpr(nat_expr) => Ok(ResolvedIndex::NatExpr(
            normalize_hir_nat_expr(nat_expr),
            nat_expr.span(),
        )),
    }
}

fn normalize_hir_nat_expr(expr: &hir::NatExpr) -> NatPolyForm {
    match expr {
        hir::NatExpr::Literal(value, _) => NatPolyForm::from_constant(*value),
        hir::NatExpr::Param(param) => NatPolyForm::from_var(param.value.name.clone()),
        hir::NatExpr::Add(lhs, rhs, _) => {
            normalize_hir_nat_expr(lhs).add(&normalize_hir_nat_expr(rhs))
        }
        hir::NatExpr::Mul(lhs, rhs, _) => {
            normalize_hir_nat_expr(lhs).mul(&normalize_hir_nat_expr(rhs))
        }
    }
}

fn resolve_hir_type_application(
    type_ann: &hir::TypeExpr,
    name: &crate::syntax::span::Spanned<ResolvedName<namespace::StructType>>,
    type_args: &[hir::TypeExpr],
    ctx: HirTypeResolutionContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let type_def = hir_struct_type_def(&name.value, name.span, ctx)?;
    let total_params = type_def.generic_params.len();
    let required_count = type_def
        .generic_params
        .iter()
        .take_while(|p| p.default.is_none())
        .count();
    if type_args.len() < required_count || type_args.len() > total_params {
        let hint = if required_count == total_params {
            format!("{total_params}")
        } else {
            format!("{required_count}..{total_params}")
        };
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` expects {hint} type argument(s), got {}",
                name.value.as_str(),
                type_args.len()
            ),
            src: ctx.src.clone(),
            span: type_ann.span.into(),
        });
    }

    let mut resolved_args = type_args
        .iter()
        .map(|arg| resolve_hir_type_expr_inner(arg, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    for param in type_def.generic_params.iter().skip(type_args.len()) {
        let default_expr = param
            .default
            .as_ref()
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: generic parameter `{}` has no default",
                    param.name
                ),
                src: ctx.src.clone(),
                span: type_ann.span.into(),
            })?;
        let default_hir = lower_type_generic_default(default_expr, &name.value, type_def, ctx)?;
        resolved_args.push(resolve_hir_type_expr_inner(&default_hir, ctx)?);
    }

    Ok(ResolvedTypeExpr::GenericStruct {
        name: name.value.clone(),
        type_args: resolved_args,
        span: type_ann.span,
    })
}

fn lower_type_generic_default(
    default_expr: &TypeExpr,
    type_owner: &ResolvedName<namespace::StructType>,
    type_def: &TypeDef,
    ctx: HirTypeResolutionContext<'_>,
) -> Result<hir::TypeExpr, GraphcalError> {
    let mut scope = hir::GenericScope::new();
    for param in &type_def.generic_params {
        let constraint = match param.constraint {
            TypeGenericConstraint::Dim => crate::syntax::ast::GenericConstraint::Dim,
            TypeGenericConstraint::Index => crate::syntax::ast::GenericConstraint::Index,
            TypeGenericConstraint::Nat => crate::syntax::ast::GenericConstraint::Nat,
            TypeGenericConstraint::Unconstrained => crate::syntax::ast::GenericConstraint::Type,
        };
        let id = hir::GenericParamId::new(
            hir::GenericParamOwner::Type(type_owner.clone()),
            param.name.clone(),
        );
        scope
            .insert_binding(hir::GenericParamBinding::new(
                id,
                constraint,
                default_expr.span,
            ))
            .map_err(|err| hir_lower_error_to_graphcal(&err, ctx.src))?;
    }

    let lower_ctx = hir::TypeLoweringContext::new(type_owner.owner(), ctx.resolver, &scope)
        .with_prelude(ctx.prelude);
    hir::lower_type_expr(default_expr, lower_ctx)
        .map_err(|err| hir_lower_error_to_graphcal(&err, ctx.src))
}

#[expect(
    clippy::too_many_arguments,
    reason = "resolves one AST index path against generic params, local registry, and module context"
)]
fn resolve_index_expr_name(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedIndex, GraphcalError> {
    if let Some(atom) = path.as_bare() {
        let text = atom.as_str();
        if let Some(gp) = nat_params.iter().find(|p| p.as_str() == text) {
            return Ok(ResolvedIndex::NatExpr(
                NatPolyForm::from_var(gp.clone()),
                span,
            ));
        }
        if let Some(gp) = index_params.iter().find(|p| p.as_str() == text) {
            return Ok(ResolvedIndex::GenericParam(gp.clone(), span));
        }
    }

    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_index_path(ctx.owner, path) {
            Ok(resolved) => {
                if ctx.types.get_index(&resolved).is_some() {
                    return Ok(ResolvedIndex::Concrete(resolved, span));
                }
                return Err(GraphcalError::UnknownIndex {
                    name: resolved.to_unowned_def_name(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Err(err) if path.is_bare() && module_lookup_is_absent(&err) => {}
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let text = require_local_type_level_path(path, span, src)?;
    if registry.indexes.get_index(text).is_some() {
        Ok(ResolvedIndex::Concrete(
            ResolvedName::from_def(owner.clone(), IndexName::new(text)),
            span,
        ))
    } else {
        Err(GraphcalError::UnknownIndex {
            name: IndexName::new(text),
            src: src.clone(),
            span: span.into(),
        })
    }
}

fn resolve_concrete_index_path(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<Option<ResolvedName<namespace::Index>>, GraphcalError> {
    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_index_path(ctx.owner, path) {
            Ok(resolved) => {
                let Some(index) = ctx.types.get_index(&resolved) else {
                    return Err(GraphcalError::UnknownIndex {
                        name: resolved.to_unowned_def_name(),
                        src: src.clone(),
                        span: span.into(),
                    });
                };
                if matches!(
                    index.kind,
                    crate::registry::types::IndexKind::Named { .. }
                        | crate::registry::types::IndexKind::RequiredNamed
                ) {
                    return Ok(Some(resolved));
                }
                return Ok(None);
            }
            Err(err) if module_lookup_is_absent(&err) => {}
            Err(_) if path.is_bare() => {
                // A bare non-local name may still be a prelude or registry-only
                // compatibility entry. Fall through to the leaf-keyed registry.
            }
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let Some(atom) = path.as_bare() else {
        return Ok(None);
    };
    let Some(index) = registry.indexes.get_index(atom.as_str()) else {
        return Ok(None);
    };
    Ok(matches!(
        index.kind,
        crate::registry::types::IndexKind::Named { .. }
            | crate::registry::types::IndexKind::RequiredNamed
    )
    .then(|| ResolvedName::from_def(owner.clone(), IndexName::from_atom(atom.clone()))))
}

type ResolvedStructTypeLookup<'a> = Option<(ResolvedName<namespace::StructType>, &'a TypeDef)>;

fn resolve_struct_type_path<'a>(
    path: &NamePath,
    span: Span,
    registry: &'a Registry,
    owner: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'a>>,
) -> Result<ResolvedStructTypeLookup<'a>, GraphcalError> {
    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_struct_type_path(ctx.owner, path) {
            Ok(resolved) => {
                if let Some(type_def) = ctx.types.get_struct_type(&resolved) {
                    return Ok(Some((resolved, type_def)));
                }
                return Err(GraphcalError::UnknownStructType {
                    name: resolved.to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Err(err) if module_lookup_is_absent(&err) => {}
            Err(_err) if path.is_bare() => {}
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let Some(atom) = path.as_bare() else {
        return Ok(None);
    };
    Ok(registry.types.get_type(atom.as_str()).map(|type_def| {
        (
            ResolvedName::from_def(owner.clone(), StructTypeName::from_atom(atom.clone())),
            type_def,
        )
    }))
}

fn resolve_dimension_path(
    path: &NamePath,
    span: Span,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<Option<Dimension>, GraphcalError> {
    if let Some(ctx) = module_ctx {
        match ctx.resolver.resolve_dimension_path(ctx.owner, path) {
            Ok(resolved) => {
                return ctx
                    .types
                    .get_dimension(&resolved)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| GraphcalError::UnknownDimension {
                        name: resolved.to_unowned_def_name(),
                        src: src.clone(),
                        span: span.into(),
                    });
            }
            Err(err) if path.is_bare() && module_lookup_is_absent(&err) => {}
            Err(err) => return Err(module_resolve_error(&err, src, span)),
        }
    }

    let text = require_local_type_level_path(path, span, src)?;
    Ok(registry.dimensions.get_dimension(text).cloned())
}

/// Resolve a `TypeExpr` into a `ResolvedTypeExpr`.
///
/// `dim_params` and `index_params` are the generic parameters in scope (empty
/// for top-level declarations, non-empty inside function signatures).
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a name cannot be resolved (not a known
/// dimension, struct, index, or in-scope generic parameter).
pub fn resolve_type_expr(
    type_ann: &TypeExpr,
    registry: &Registry,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let owner = crate::dag_id::DagId::root("<type-resolution>");
    resolve_type_expr_inner(
        type_ann,
        registry,
        &owner,
        dim_params,
        index_params,
        nat_params,
        src,
        None,
    )
}

/// Resolve a `TypeExpr` with an optional module-aware path context.
pub fn resolve_type_expr_with_modules(
    type_ann: &TypeExpr,
    registry: &Registry,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: ModuleTypeContext<'_>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    resolve_type_expr_inner(
        type_ann,
        registry,
        module_ctx.owner,
        dim_params,
        index_params,
        nat_params,
        src,
        Some(module_ctx),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "recursive resolver threads generic parameter scopes and optional module context"
)]
fn resolve_type_expr_inner(
    type_ann: &TypeExpr,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    if let Some(ctx) = module_ctx
        && dim_params.is_empty()
        && index_params.is_empty()
        && nat_params.is_empty()
    {
        return resolve_ast_type_expr_via_hir(type_ann, registry, src, ctx);
    }

    match &type_ann.kind {
        TypeExprKind::Dimensionless => Ok(ResolvedTypeExpr::Dimensionless),
        TypeExprKind::Bool => Ok(ResolvedTypeExpr::Bool),
        TypeExprKind::Int => Ok(ResolvedTypeExpr::Int),
        TypeExprKind::Datetime => Ok(ResolvedTypeExpr::Datetime(TimeScale::UTC)),
        TypeExprKind::DatetimeApplication { type_args } => {
            resolve_datetime_application(type_ann, type_args, src)
        }

        TypeExprKind::Indexed { base, indexes } => {
            let resolved_base = resolve_type_expr_inner(
                base,
                registry,
                owner,
                dim_params,
                index_params,
                nat_params,
                src,
                module_ctx,
            )?;
            let mut resolved_indexes = Vec::with_capacity(indexes.len());
            for idx in indexes {
                match idx {
                    crate::desugar::resolved_ast::IndexExpr::NatExpr(nat_expr) => {
                        let form = normalize_nat_expr(nat_expr, nat_params, src)?;
                        resolved_indexes.push(ResolvedIndex::NatExpr(form, nat_expr.span()));
                    }
                    crate::desugar::resolved_ast::IndexExpr::Name(path) => {
                        resolved_indexes.push(resolve_index_expr_name(
                            &path.value,
                            path.span,
                            registry,
                            owner,
                            index_params,
                            nat_params,
                            src,
                            module_ctx,
                        )?);
                    }
                }
            }
            Ok(ResolvedTypeExpr::Indexed {
                base: Box::new(resolved_base),
                indexes: resolved_indexes,
            })
        }

        TypeExprKind::DimExpr(dim_expr) => {
            resolve_dim_expr(dim_expr, registry, owner, dim_params, src, module_ctx)
        }

        TypeExprKind::TypeApplication { name, type_args } => resolve_type_application(
            type_ann,
            name,
            type_args,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        ),
    }
}

/// Resolve a dimension expression to either a [`ResolvedTypeExpr::Scalar`],
/// [`ResolvedTypeExpr::GenericDimExpr`], [`ResolvedTypeExpr::Label`],
/// [`ResolvedTypeExpr::Struct`], or [`ResolvedTypeExpr::GenericDimParam`].
///
/// A single-term, no-power expression is first checked against named indexes,
/// struct types, and generic dimension parameters. Multi-term expressions with
/// generic params become `GenericDimExpr`; fully concrete expressions become `Scalar`.
fn resolve_dim_expr(
    dim_expr: &crate::desugar::resolved_ast::DimExpr,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    // Single-term, no power: may be a nominal type-level reference rather than
    // a scalar dimension expression.
    if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() {
        let term = &dim_expr.terms[0].term;
        if let Some(index) = resolve_concrete_index_path(
            &term.name.value,
            term.name.span,
            registry,
            owner,
            src,
            module_ctx,
        )? {
            return Ok(ResolvedTypeExpr::Label(index, term.span));
        }
        if let Some((type_name, _)) = resolve_struct_type_path(
            &term.name.value,
            term.name.span,
            registry,
            owner,
            src,
            module_ctx,
        )? {
            return Ok(ResolvedTypeExpr::Struct(type_name, term.span));
        }
        if let Some(atom) = term.name.value.as_bare()
            && let Some(gp) = dim_params.iter().find(|p| p.as_str() == atom.as_str())
        {
            return Ok(ResolvedTypeExpr::GenericDimParam(gp.clone(), term.span));
        }
    }

    let has_generic = dim_expr.terms.iter().any(|item| {
        item.term
            .name
            .value
            .as_bare()
            .is_some_and(|atom| dim_params.iter().any(|p| p.as_str() == atom.as_str()))
    });

    if has_generic {
        let terms = dim_expr
            .terms
            .iter()
            .map(|item| {
                resolve_dim_term_in_generic_expr(item, registry, dim_params, src, module_ctx)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolvedTypeExpr::GenericDimExpr {
            terms,
            span: dim_expr.span,
        })
    } else {
        let result = dim_expr.terms.iter().try_fold(
            Dimension::dimensionless(),
            |acc, item| -> Result<Dimension, GraphcalError> {
                let base = concrete_dimension_for_term(item, registry, src, module_ctx)?;
                let exp = item.term.power.unwrap_or(1);
                let overflow_err = || GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: item.term.span.into(),
                };
                let powered = base
                    .pow(Rational::from_int(exp))
                    .map_err(|_| overflow_err())?;
                match item.op {
                    MulDivOp::Mul => (acc * powered).map_err(|_| overflow_err()),
                    MulDivOp::Div => (acc / powered).map_err(|_| overflow_err()),
                }
            },
        )?;
        Ok(ResolvedTypeExpr::Scalar(result))
    }
}

fn resolve_dim_term_in_generic_expr(
    item: &crate::desugar::resolved_ast::DimExprItem,
    registry: &Registry,
    dim_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedDimTerm, GraphcalError> {
    let power = item.term.power.unwrap_or(1);
    let op = item.op;
    if let Some(atom) = item.term.name.value.as_bare()
        && let Some(gp) = dim_params.iter().find(|p| p.as_str() == atom.as_str())
    {
        return Ok(ResolvedDimTerm::GenericParam {
            name: gp.clone(),
            power,
            op,
            span: item.term.span,
        });
    }
    concrete_dimension_for_term(item, registry, src, module_ctx)
        .map(|dim| ResolvedDimTerm::Concrete { dim, power, op })
}

fn concrete_dimension_for_term(
    item: &crate::desugar::resolved_ast::DimExprItem,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<Dimension, GraphcalError> {
    resolve_dimension_path(
        &item.term.name.value,
        item.term.name.span,
        registry,
        src,
        module_ctx,
    )?
    .ok_or_else(|| {
        let name = item
            .term
            .name
            .value
            .as_bare()
            .map_or_else(|| item.term.name.value.display_path(), ToString::to_string);
        GraphcalError::UnknownDimension {
            name: DimName::new(name),
            src: src.clone(),
            span: item.term.span.into(),
        }
    })
}

/// Resolve a `Datetime<TimeScale>` application to a [`ResolvedTypeExpr::Datetime`].
///
/// The argument list is expected to hold exactly one type argument that
/// parses as a [`TimeScale`] identifier (`UTC`, `TAI`, `TT`, …). Surfaced as
/// a dedicated helper rather than living inside [`resolve_type_application`]
/// so the dispatch in [`resolve_type_expr`] is on the AST variant rather than
/// a string compare of the built-in name.
fn resolve_datetime_application(
    type_ann: &TypeExpr,
    type_args: &[TypeExpr],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    if type_args.len() != 1 {
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `Datetime` expects 0 or 1 type argument(s), got {}",
                type_args.len()
            ),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    let arg = &type_args[0];
    match &arg.kind {
        TypeExprKind::DimExpr(dim_expr)
            if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() =>
        {
            let term = &dim_expr.terms[0].term;
            let name = require_local_type_level_path(&term.name.value, term.name.span, src)?;
            name.parse::<TimeScale>().map_or_else(
                |_| {
                    Err(GraphcalError::EvalError {
                        message: format!(
                            "unknown time scale `{name}`; \
                         expected one of: UTC, TAI, TT, TDB, ET, GPST, GST, BDT"
                        ),
                        src: src.clone(),
                        span: arg.span.into(),
                    })
                },
                |scale| Ok(ResolvedTypeExpr::Datetime(scale)),
            )
        }
        _ => Err(GraphcalError::EvalError {
            message: "expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)".to_string(),
            src: src.clone(),
            span: arg.span.into(),
        }),
    }
}

/// Resolve a user-defined type application like `Vec3<Length, ECI>` to a
/// [`ResolvedTypeExpr`] by looking the name up in the type registry and
/// substituting defaults for any trailing optional parameters.
///
/// Built-in parameterized types (`Datetime<...>`) reach [`resolve_type_expr`]
/// through their own AST variant and never enter this function.
#[expect(
    clippy::too_many_arguments,
    reason = "passes full type resolution context from resolve_type_expr"
)]
fn resolve_type_application(
    type_ann: &TypeExpr,
    name: &crate::syntax::span::Spanned<NamePath>,
    type_args: &[TypeExpr],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
    module_ctx: Option<ModuleTypeContext<'_>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let (type_name, type_def) =
        resolve_struct_type_path(&name.value, name.span, registry, owner, src, module_ctx)?
            .ok_or_else(|| GraphcalError::UnknownStructType {
                name: name.value.display_path(),
                src: src.clone(),
                span: name.span.into(),
            })?;
    let total_params = type_def.generic_params.len();
    let required_count = type_def
        .generic_params
        .iter()
        .take_while(|p| p.default.is_none())
        .count();
    if type_args.len() < required_count || type_args.len() > total_params {
        let hint = if required_count == total_params {
            format!("{total_params}")
        } else {
            format!("{required_count}..{total_params}")
        };
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` expects {hint} type argument(s), got {}",
                type_name.as_str(),
                type_args.len()
            ),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    // Resolve each explicit type argument, then fill in defaults
    let mut resolved_args = Vec::with_capacity(total_params);
    for arg in type_args {
        let resolved = resolve_type_expr_inner(
            arg,
            registry,
            owner,
            dim_params,
            index_params,
            nat_params,
            src,
            module_ctx,
        )?;
        resolved_args.push(resolved);
    }
    // Fill in defaults for any remaining params
    for param in type_def.generic_params.iter().skip(type_args.len()) {
        let default_expr = param
            .default
            .as_ref()
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: generic parameter `{}` has no default",
                    param.name
                ),
                src: src.clone(),
                span: type_ann.span.into(),
            })?;
        let default_ctx = module_ctx.map_or(module_ctx, |ctx| {
            Some(ModuleTypeContext::new(
                type_name.owner(),
                ctx.resolver,
                ctx.types,
            ))
        });
        let resolved = resolve_type_expr_inner(
            default_expr,
            registry,
            type_name.owner(),
            dim_params,
            index_params,
            nat_params,
            src,
            default_ctx,
        )?;
        resolved_args.push(resolved);
    }
    Ok(ResolvedTypeExpr::GenericStruct {
        name: type_name,
        type_args: resolved_args,
        span: type_ann.span,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::prelude::load_prelude;
    use crate::registry::types::RegistryBuilder;
    use crate::syntax::dimension::BaseDimId;
    use crate::syntax::parser::Parser;

    fn make_registry() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.build()
    }

    fn make_dim_term_name(
        name: &str,
    ) -> crate::syntax::span::Spanned<crate::syntax::names::NamePath> {
        crate::syntax::span::Spanned::new(
            crate::syntax::names::NamePath::from(name),
            Span::new(0, 0),
        )
    }

    /// Create a simple dimension `TypeExpr` from a name string like `"Velocity"`.
    fn make_dim_type_expr(name: &str) -> crate::desugar::resolved_ast::TypeExpr {
        crate::desugar::resolved_ast::TypeExpr {
            kind: crate::desugar::resolved_ast::TypeExprKind::DimExpr(
                crate::desugar::resolved_ast::DimExpr {
                    terms: vec![crate::desugar::resolved_ast::DimExprItem {
                        op: crate::desugar::resolved_ast::MulDivOp::Mul,
                        term: crate::desugar::resolved_ast::DimTerm {
                            name: make_dim_term_name(name),
                            power: None,
                            span: Span::new(0, 0),
                        },
                    }],
                    span: Span::new(0, 0),
                },
            ),
            constraints: vec![],
            span: Span::new(0, 0),
        }
    }

    fn make_registry_with_struct() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.register_type(crate::registry::types::TypeDef {
            name: StructTypeName::new("TransferResult"),
            generic_params: vec![],
            kind: crate::registry::types::TypeDefKind::Union {
                members: vec![crate::registry::types::UnionMemberDef {
                    name: crate::syntax::names::ConstructorName::new("TransferResult"),
                    fields: vec![
                        crate::registry::types::StructField {
                            name: crate::syntax::names::FieldName::new("dv1"),
                            type_ann: make_dim_type_expr("Velocity"),
                        },
                        crate::registry::types::StructField {
                            name: crate::syntax::names::FieldName::new("dv2"),
                            type_ann: make_dim_type_expr("Velocity"),
                        },
                    ],
                }],
            },
        });
        b.build()
    }

    fn make_registry_with_index() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.register_index(crate::registry::types::IndexDef {
            name: IndexName::new("Maneuver"),
            kind: crate::registry::types::IndexKind::Named {
                variants: vec![
                    crate::syntax::names::IndexVariantName::new("Departure"),
                    crate::syntax::names::IndexVariantName::new("Insertion"),
                ],
            },
        });
        b.build()
    }

    fn make_src() -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(String::new()))
    }

    /// Parse a type annotation from a param declaration and return the `TypeExpr`.
    fn parse_type(source: &str) -> TypeExpr {
        // Wrap in a param declaration so the parser can handle it
        let full = format!("param x: {source} = 0.0;");
        let raw_file = Parser::new(&full).parse_file().unwrap();
        let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
        match &file.declarations[0].kind {
            crate::desugar::resolved_ast::DeclKind::Param(p) => p.type_ann.clone(),
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn resolve_dimensionless() {
        let r = make_registry();
        let te = parse_type("Dimensionless");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Dimensionless);
    }

    #[test]
    fn resolve_bool() {
        let r = make_registry();
        let te = parse_type("Bool");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Bool);
    }

    #[test]
    fn resolve_int() {
        let r = make_registry();
        let te = parse_type("Int");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Int);
    }

    #[test]
    fn resolve_concrete_dimension() {
        let r = make_registry();
        let te = parse_type("Length");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(
            resolved,
            ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
        );
    }

    #[test]
    fn resolve_compound_dimension() {
        let r = make_registry();
        let te = parse_type("Length / Time^2");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        let expected = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
            / Dimension::base(BaseDimId::Prelude("Time".to_string()))
                .pow_int(2)
                .unwrap())
        .unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
    }

    #[test]
    fn resolve_struct_type() {
        let r = make_registry_with_struct();
        let te = parse_type("TransferResult");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert!(
            matches!(resolved, ResolvedTypeExpr::Struct(name, _) if name.as_str() == "TransferResult")
        );
    }

    #[test]
    fn resolve_generic_dim_param() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        let te = parse_type("D");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
        assert!(
            matches!(resolved, ResolvedTypeExpr::GenericDimParam(name, _) if name.as_str() == "D")
        );
    }

    #[test]
    fn resolve_generic_dim_expr_with_power() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        let te = parse_type("D^2");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
                assert_eq!(terms.len(), 1);
                match &terms[0] {
                    ResolvedDimTerm::GenericParam { name, power, .. } => {
                        assert_eq!(name.as_str(), "D");
                        assert_eq!(*power, 2);
                    }
                    ResolvedDimTerm::Concrete { .. } => panic!("expected GenericParam term"),
                }
            }
            _ => panic!("expected GenericDimExpr"),
        }
    }

    #[test]
    fn resolve_mixed_generic_concrete() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        // D * Length  — this is a DimExpr with a generic and a concrete term
        let te = parse_type("D * Length");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
                assert_eq!(terms.len(), 2);
                assert!(
                    matches!(&terms[0], ResolvedDimTerm::GenericParam { name, .. } if name.as_str() == "D")
                );
                assert!(matches!(&terms[1], ResolvedDimTerm::Concrete { .. }));
            }
            _ => panic!("expected GenericDimExpr, got {resolved:?}"),
        }
    }

    #[test]
    fn resolve_concrete_indexed() {
        let r = make_registry_with_index();
        let te = parse_type("Length[Maneuver]");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::Indexed { base, indexes } => {
                assert_eq!(
                    *base,
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert_eq!(indexes.len(), 1);
                assert!(
                    matches!(&indexes[0], ResolvedIndex::Concrete(name, _) if name.as_str() == "Maneuver")
                );
            }
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn resolve_generic_indexed() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        let index_params = vec![GenericParamName::new("I")];
        let te = parse_type("D[I]");
        let resolved =
            resolve_type_expr(&te, &r, &dim_params, &index_params, &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::Indexed { base, indexes } => {
                assert!(
                    matches!(*base, ResolvedTypeExpr::GenericDimParam(ref name, _) if name.as_str() == "D")
                );
                assert_eq!(indexes.len(), 1);
                assert!(
                    matches!(&indexes[0], ResolvedIndex::GenericParam(name, _) if name.as_str() == "I")
                );
            }
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn resolve_unknown_dimension_error() {
        let r = make_registry();
        let te = parse_type("UnknownDim");
        let err = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownDimension { .. }));
    }

    #[test]
    fn resolve_unknown_index_error() {
        let r = make_registry();
        let te = parse_type("Length[UnknownIdx]");
        let err = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownIndex { .. }));
    }

    #[test]
    fn resolve_struct_takes_priority_over_dim_param() {
        // When a name matches both a struct and a generic param,
        // struct should win (structs are concrete, params are only
        // in scope inside a function that has that param).
        // In practice this shouldn't happen because struct names are
        // PascalCase and generic params are single letters, but let's
        // make sure the priority is correct.
        let r = make_registry_with_struct();
        let dim_params = vec![GenericParamName::new("TransferResult")];
        let te = parse_type("TransferResult");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &[], &make_src()).unwrap();
        assert!(matches!(resolved, ResolvedTypeExpr::Struct(..)));
    }

    #[test]
    fn resolve_velocity_derived_dimension() {
        let r = make_registry();
        let te = parse_type("Velocity");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        let expected = (Dimension::base(BaseDimId::Prelude("Length".to_string()))
            / Dimension::base(BaseDimId::Prelude("Time".to_string())))
        .unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
    }

    // --- module-aware type resolution integration tests ---

    /// Single-file integration helper: lower + type-resolve + compile each
    /// inline dag body using the dumb `lower_dag_body_to_ir` primitive
    /// directly (no self-import preprocessing — fixtures exercised here
    /// either don't use self-imports or are expected to surface errors that
    /// fall out of the unprocessed body).
    fn parse_and_type_resolve(source: &str) -> Result<TIR, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
        let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let ir = crate::ir::lower::lower(&file, &src)?;
        let parent_dag_id =
            crate::dag_id::DagId::from_relative_path(std::path::Path::new("test.gcl")).unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(parent_dag_id.clone(), &file.declarations)
            .map_err(|err| {
                internal_error(
                    format!("test module resolver failed for root module: {err}"),
                    &src,
                    Span::new(0, 0),
                )
            })?;
        for decl in &file.declarations {
            if let crate::desugar::resolved_ast::DeclKind::Dag(dag) = &decl.kind {
                resolver
                    .add_module(parent_dag_id.child(dag.name.value.as_str()), &dag.body)
                    .map_err(|err| {
                        internal_error(
                            format!(
                                "test module resolver failed for inline dag `{}`: {err}",
                                dag.name.value
                            ),
                            &src,
                            Span::new(0, 0),
                        )
                    })?;
            }
        }
        let mut module_types = ModuleTypeRegistry::default();
        module_types.insert_graphcal_prelude().map_err(|err| {
            internal_error(
                format!("test module type prelude failed: {err}"),
                &src,
                Span::new(0, 0),
            )
        })?;
        module_types.insert_registry(&parent_dag_id, &ir.registry);
        let mut tir =
            type_resolve_with_modules(ir, parent_dag_id.clone(), &src, &resolver, &module_types)?;
        compile_inline_dag_bodies_test(&mut tir, &src, &parent_dag_id, &file.declarations)?;
        Ok(tir)
    }

    /// Compile each inline dag body in `tir` with no self-import
    /// preprocessing. Used by compiler-side integration tests that don't
    /// have access to the eval crate's project pipeline.
    fn compile_inline_dag_bodies_test(
        tir: &mut TIR,
        src: &NamedSource<Arc<String>>,
        parent_dag_id: &crate::dag_id::DagId,
        parent_declarations: &[crate::desugar::resolved_ast::Declaration],
    ) -> Result<(), GraphcalError> {
        let dag_bodies = tir
            .registry
            .dags
            .all_dags()
            .map(|(name, dag)| (name.clone(), dag.body.clone()))
            .collect::<Vec<_>>();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(parent_dag_id.clone(), parent_declarations)
            .map_err(|err| {
                internal_error(
                    format!("test module resolver failed for parent module: {err}"),
                    src,
                    Span::new(0, 0),
                )
            })?;
        for (name, body) in &dag_bodies {
            resolver
                .add_module(parent_dag_id.child(name.as_str()), body)
                .map_err(|err| {
                    internal_error(
                        format!("test module resolver failed for inline dag `{name}`: {err}"),
                        src,
                        Span::new(0, 0),
                    )
                })?;
        }
        let mut module_types = ModuleTypeRegistry::default();
        module_types.insert_graphcal_prelude().map_err(|err| {
            internal_error(
                format!("test module type prelude failed: {err}"),
                src,
                Span::new(0, 0),
            )
        })?;
        module_types.insert_registry(parent_dag_id, &tir.registry);

        for (name, body) in dag_bodies {
            let dag_body_ir = crate::ir::lower::lower_dag_body_to_ir(
                &name,
                &body,
                &tir.registry,
                &crate::ir::resolve::ImportedValueNames::default(),
                HashMap::new(),
                HashMap::new(),
                src,
                parent_dag_id,
            )?;
            let dag_id = parent_dag_id.child(name.as_str());
            let mut compiled_dag = type_resolve_single_with_modules(
                dag_body_ir,
                &dag_id,
                src,
                &resolver,
                &module_types,
            )?;
            compiled_dag.populate_pub_nodes(&body);
            tir.dags.insert(dag_id, compiled_dag);
        }
        Ok(())
    }

    #[test]
    fn module_aware_type_resolve_records_semantic_deps() {
        let source = "const node C: Dimensionless = 1.0;\n\
                      const node D: Dimensionless = C;\n\
                      param p: Dimensionless;\n\
                      node x: Dimensionless = @p + D;";
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
        let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let dag_id =
            crate::dag_id::DagId::from_relative_path(std::path::Path::new("test.gcl")).unwrap();
        let ir = crate::ir::lower::lower(&file, &src).unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(dag_id.clone(), &file.declarations)
            .unwrap();
        let mut module_types = ModuleTypeRegistry::default();
        module_types.insert_graphcal_prelude().unwrap();
        module_types.insert_registry(&dag_id, &ir.registry);

        let tir =
            type_resolve_with_modules(ir, dag_id.clone(), &src, &resolver, &module_types).unwrap();
        let deps = &tir.root().semantic.dependencies;
        let c = ResolvedName::from_def(dag_id.clone(), DeclName::new("C"));
        let d = ResolvedName::from_def(dag_id.clone(), DeclName::new("D"));
        let p = ResolvedName::from_def(dag_id.clone(), DeclName::new("p"));
        let x = ResolvedName::from_def(dag_id, DeclName::new("x"));

        assert!(deps.const_deps[&d].contains(&c));
        assert!(deps.const_deps[&c].is_empty());
        assert!(deps.runtime_deps[&x].contains(&p));
        assert!(deps.runtime_deps[&p].is_empty());
    }

    #[test]
    fn type_resolve_rocket() {
        let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // All declarations should have resolved types
        assert!(
            tir.root()
                .resolved_decl_types
                .contains_key(&ScopedName::local("dry_mass"))
        );
        assert!(
            tir.root()
                .resolved_decl_types
                .contains_key(&ScopedName::local("delta_v"))
        );
        assert!(
            tir.root()
                .resolved_decl_types
                .contains_key(&ScopedName::local("g0"))
        );
    }

    #[test]
    fn type_resolve_indexed() {
        let source = include_str!("../../../../tests/fixtures/valid/indexed.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // delta_v should be Velocity[Maneuver]
        let dv_type = &tir.root().resolved_decl_types[&ScopedName::local("delta_v")];
        assert!(matches!(dv_type, ResolvedTypeExpr::Indexed { .. }));
    }

    #[test]
    fn type_resolve_hohmann() {
        // hohmann.gcl uses DAG+include. Project-level `graphcal check`
        // accepts it (see the CLI tests), but single-file TIR resolution
        // rejects it: there's no project loader to resolve cross-DAG
        // references like `import hohmann.{...}`, and `@transfer` from the
        // unexpanded include surfaces as an unknown graph reference. The
        // resolver fails on the first unresolved name it encounters, which
        // is enough to assert `UnknownGraphRef`.
        let source = include_str!("../../../../tests/fixtures/valid/hohmann.gcl");
        let err = parse_and_type_resolve(source).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
    }

    #[test]
    fn type_resolve_generics() {
        let source = include_str!("../../../../tests/fixtures/valid/generics.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // pos_eci should be a GenericStruct with type args
        let pos_type = &tir.root().resolved_decl_types[&ScopedName::local("pos_eci")];
        match pos_type {
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            } => {
                assert_eq!(name.as_str(), "Vec3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(
                    type_args[0],
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert!(
                    matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Eci")
                );
            }
            other => panic!("expected GenericStruct, got {other:?}"),
        }
        // x_pos should be scalar Length
        assert_eq!(
            tir.root().resolved_decl_types[&ScopedName::local("x_pos")],
            ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
        );
    }

    #[test]
    fn type_resolve_default_type_params() {
        let source = include_str!("../../../../tests/fixtures/valid/generics.gcl");
        let tir = parse_and_type_resolve(source).unwrap();

        // pos3_eci: Pos3<Length, Eci> — explicit, 2 type args
        let pos3_eci = &tir.root().resolved_decl_types[&ScopedName::local("pos3_eci")];
        match pos3_eci {
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            } => {
                assert_eq!(name.as_str(), "Pos3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(
                    type_args[0],
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert!(
                    matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Eci")
                );
            }
            other => panic!("expected GenericStruct, got {other:?}"),
        }

        // pos3_default: Pos3<Length> — default fills in Unframed
        let pos3_default = &tir.root().resolved_decl_types[&ScopedName::local("pos3_default")];
        match pos3_default {
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            } => {
                assert_eq!(name.as_str(), "Pos3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(
                    type_args[0],
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert!(
                    matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Unframed"),
                    "expected Struct(Unframed), got {:?}",
                    type_args[1]
                );
            }
            other => panic!("expected GenericStruct, got {other:?}"),
        }
    }

    // --- resolved_to_declared_type() tests ---

    use crate::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};

    #[test]
    fn generic_index_substitution_preserves_resolved_owner() {
        use crate::tir::dim_check::{InferredIndex, InferredType};

        let src = make_src();
        let registry = make_registry();
        let owner = crate::dag_id::DagId::root("a");
        let resolved_index = ResolvedName::from_def(owner, IndexName::new("Phase"));
        let generic = GenericParamName::new("I");
        let resolved_type = ResolvedTypeExpr::Indexed {
            base: Box::new(ResolvedTypeExpr::Dimensionless),
            indexes: vec![ResolvedIndex::GenericParam(
                generic.clone(),
                Span::new(0, 0),
            )],
        };
        let actual = InferredType::Indexed {
            element: Box::new(InferredType::Scalar(Dimension::dimensionless())),
            index: InferredIndex::from_resolved(resolved_index.clone()),
        };
        let mut dim_sub = HashMap::new();
        let mut index_sub = HashMap::new();
        let mut nat_sub = HashMap::new();

        unify_resolved_type(
            &resolved_type,
            &actual,
            &mut dim_sub,
            &mut index_sub,
            &mut nat_sub,
            &registry,
            &src,
            Span::new(0, 0),
        )
        .unwrap();
        assert_eq!(index_sub[&generic].resolved(), &resolved_index);

        let substituted =
            substitute_resolved_type(&resolved_type, &dim_sub, &index_sub, &nat_sub, &src).unwrap();
        let InferredType::Indexed { index, .. } = substituted else {
            panic!("expected indexed type after substitution");
        };
        assert_eq!(index.resolved(), &resolved_index);
    }

    #[test]
    fn convert_dimensionless() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Dimensionless, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Scalar(Dimension::dimensionless()));
    }

    #[test]
    fn convert_bool() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Bool, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Bool);
    }

    #[test]
    fn convert_int() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Int, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Int);
    }

    #[test]
    fn convert_scalar() {
        let dim = Dimension::base(BaseDimId::Prelude("Length".to_string()));
        let dt =
            resolved_to_declared_type(&ResolvedTypeExpr::Scalar(dim.clone()), &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Scalar(dim));
    }

    #[test]
    fn convert_struct() {
        let owner = crate::dag_id::DagId::root("test");
        let resolved = ResolvedName::from_def(owner, StructTypeName::new("Foo"));
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Struct(resolved.clone(), Span::new(0, 0)),
            &make_src(),
        )
        .unwrap();
        assert_eq!(
            dt,
            DeclaredType::Struct(StructTypeRef::from_resolved(resolved), vec![])
        );
    }

    #[test]
    fn convert_indexed() {
        let owner = crate::dag_id::DagId::root("test");
        let resolved_index = ResolvedName::from_def(owner, IndexName::new("M"));
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Indexed {
                base: Box::new(ResolvedTypeExpr::Scalar(Dimension::base(
                    BaseDimId::Prelude("Length".to_string()),
                ))),
                indexes: vec![ResolvedIndex::Concrete(
                    resolved_index.clone(),
                    Span::new(0, 0),
                )],
            },
            &make_src(),
        )
        .unwrap();
        assert_eq!(
            dt,
            DeclaredType::Indexed {
                element: Box::new(DeclaredType::Scalar(Dimension::base(BaseDimId::Prelude(
                    "Length".to_string()
                )))),
                index: IndexTypeRef::from_resolved(resolved_index),
            }
        );
    }

    #[test]
    fn convert_generic_dim_param_fails() {
        let err = resolved_to_declared_type(
            &ResolvedTypeExpr::GenericDimParam(GenericParamName::new("D"), Span::new(0, 0)),
            &make_src(),
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::EvalError { .. }));
    }

    #[test]
    fn convert_generic_index_fails() {
        let err = resolved_to_declared_type(
            &ResolvedTypeExpr::Indexed {
                base: Box::new(ResolvedTypeExpr::Dimensionless),
                indexes: vec![ResolvedIndex::GenericParam(
                    GenericParamName::new("I"),
                    Span::new(0, 0),
                )],
            },
            &make_src(),
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::EvalError { .. }));
    }

    // --- Datetime type resolution tests ---

    #[test]
    fn resolve_bare_datetime() {
        let r = make_registry();
        let te = parse_type("Datetime");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
    }

    #[test]
    fn resolve_datetime_utc() {
        let r = make_registry();
        let te = parse_type("Datetime<UTC>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
    }

    #[test]
    fn resolve_datetime_tt() {
        let r = make_registry();
        let te = parse_type("Datetime<TT>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TT));
    }

    #[test]
    fn resolve_datetime_tai() {
        let r = make_registry();
        let te = parse_type("Datetime<TAI>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TAI));
    }

    #[test]
    fn resolve_datetime_gpst() {
        let r = make_registry();
        let te = parse_type("Datetime<GPST>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::GPST));
    }

    #[test]
    fn resolve_datetime_unknown_scale_error() {
        let r = make_registry();
        let te = parse_type("Datetime<XYZ>");
        let err = resolve_type_expr(&te, &r, &[], &[], &[], &make_src()).unwrap_err();
        assert!(matches!(err, GraphcalError::EvalError { .. }));
    }

    #[test]
    fn convert_datetime_utc() {
        let dt =
            resolved_to_declared_type(&ResolvedTypeExpr::Datetime(TimeScale::UTC), &make_src())
                .unwrap();
        assert_eq!(dt, DeclaredType::Datetime(TimeScale::UTC));
    }

    #[test]
    fn convert_datetime_tt() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Datetime(TimeScale::TT), &make_src())
            .unwrap();
        assert_eq!(dt, DeclaredType::Datetime(TimeScale::TT));
    }

    // -----------------------------------------------------------------------
    // NatLinearForm::is_leq tests
    // -----------------------------------------------------------------------

    #[test]
    fn nat_leq_constant_equal() {
        let a = NatPolyForm::from_constant(3);
        let b = NatPolyForm::from_constant(3);
        assert!(a.is_leq(&b));
    }

    #[test]
    fn nat_leq_constant_less() {
        let a = NatPolyForm::from_constant(2);
        let b = NatPolyForm::from_constant(5);
        assert!(a.is_leq(&b));
    }

    #[test]
    fn nat_leq_constant_greater() {
        let a = NatPolyForm::from_constant(5);
        let b = NatPolyForm::from_constant(3);
        assert!(!a.is_leq(&b));
    }

    #[test]
    fn nat_leq_same_var() {
        // N <= N
        let a = NatPolyForm::from_var(GenericParamName::new("N"));
        let b = NatPolyForm::from_var(GenericParamName::new("N"));
        assert!(a.is_leq(&b));
    }

    #[test]
    fn nat_leq_var_plus_constant() {
        // N <= N + 1
        let a = NatPolyForm::from_var(GenericParamName::new("N"));
        let b =
            NatPolyForm::from_var(GenericParamName::new("N")).add(&NatPolyForm::from_constant(1));
        assert!(a.is_leq(&b));
    }

    #[test]
    fn nat_leq_var_plus_constant_reverse() {
        // N + 1 <= N → false
        let a =
            NatPolyForm::from_var(GenericParamName::new("N")).add(&NatPolyForm::from_constant(1));
        let b = NatPolyForm::from_var(GenericParamName::new("N"));
        assert!(!a.is_leq(&b));
    }

    #[test]
    fn nat_leq_different_vars() {
        // N <= M → false (N could be larger)
        let a = NatPolyForm::from_var(GenericParamName::new("N"));
        let b = NatPolyForm::from_var(GenericParamName::new("M"));
        assert!(!a.is_leq(&b));
    }

    #[test]
    fn nat_leq_zero_leq_anything() {
        // 0 <= N
        let a = NatPolyForm::from_constant(0);
        let b = NatPolyForm::from_var(GenericParamName::new("N"));
        assert!(a.is_leq(&b));
    }

    // -----------------------------------------------------------------------
    // NatRangeIndexIdentity typed-reference tests
    // -----------------------------------------------------------------------

    #[test]
    fn nat_range_identity_concrete_to_index_type_ref() {
        let reference = NatPolyForm::from_constant(3)
            .to_nat_range_identity()
            .to_index_type_ref();
        assert_eq!(
            reference
                .nat_range()
                .map(crate::registry::types::NatRangeIndex::size_u64),
            Some(3)
        );
        assert_eq!(reference.name().as_str(), "range(3)");
    }

    #[test]
    fn nat_range_identity_symbolic_to_display_only_index_type_ref() {
        let reference = NatPolyForm::from_var(GenericParamName::new("N"))
            .add(&NatPolyForm::from_constant(1))
            .to_nat_range_identity()
            .to_index_type_ref();
        assert_eq!(reference.nat_range(), None);
        assert_eq!(reference.name().as_str(), "range(N + 1)");
    }

    // -----------------------------------------------------------------------
    // NatPolyForm multiplication tests (Level 2)
    // -----------------------------------------------------------------------

    #[test]
    fn nat_mul_constants() {
        let a = NatPolyForm::from_constant(3);
        let b = NatPolyForm::from_constant(4);
        assert_eq!(a.mul(&b), NatPolyForm::from_constant(12));
    }

    #[test]
    fn nat_mul_var_by_constant() {
        // N * 3
        let n = NatPolyForm::from_var(GenericParamName::new("N"));
        let three = NatPolyForm::from_constant(3);
        let result = n.mul(&three);
        // Should format as "3 * N"
        assert_eq!(result.format(), "3 * N");
        // Evaluate with N=5 → 15
        let mut bindings = HashMap::new();
        bindings.insert(GenericParamName::new("N"), 5);
        assert_eq!(result.evaluate(&bindings), Some(15));
    }

    #[test]
    fn nat_mul_two_vars() {
        // M * N
        let m = NatPolyForm::from_var(GenericParamName::new("M"));
        let n = NatPolyForm::from_var(GenericParamName::new("N"));
        let result = m.mul(&n);
        assert_eq!(result.format(), "M * N");
        let mut bindings = HashMap::new();
        bindings.insert(GenericParamName::new("M"), 3);
        bindings.insert(GenericParamName::new("N"), 4);
        assert_eq!(result.evaluate(&bindings), Some(12));
    }

    #[test]
    fn nat_mul_distributive() {
        // (M + 1) * N = M * N + N
        let m = NatPolyForm::from_var(GenericParamName::new("M"));
        let n = NatPolyForm::from_var(GenericParamName::new("N"));
        let m_plus_1 = m.add(&NatPolyForm::from_constant(1));
        let result = m_plus_1.mul(&n);
        // Evaluate with M=2, N=3 → (2+1)*3 = 9
        let mut bindings = HashMap::new();
        bindings.insert(GenericParamName::new("M"), 2);
        bindings.insert(GenericParamName::new("N"), 3);
        assert_eq!(result.evaluate(&bindings), Some(9));
    }

    #[test]
    fn nat_mul_mixed_add() {
        // M * N + 1
        let m = NatPolyForm::from_var(GenericParamName::new("M"));
        let n = NatPolyForm::from_var(GenericParamName::new("N"));
        let result = m.mul(&n).add(&NatPolyForm::from_constant(1));
        assert_eq!(result.format(), "M * N + 1");
        let mut bindings = HashMap::new();
        bindings.insert(GenericParamName::new("M"), 2);
        bindings.insert(GenericParamName::new("N"), 3);
        assert_eq!(result.evaluate(&bindings), Some(7));
    }

    #[test]
    fn nat_poly_is_constant() {
        let c = NatPolyForm::from_constant(5);
        assert!(c.is_constant());

        let n = NatPolyForm::from_var(GenericParamName::new("N"));
        assert!(!n.is_constant());

        let mn = NatPolyForm::from_var(GenericParamName::new("M"))
            .mul(&NatPolyForm::from_var(GenericParamName::new("N")));
        assert!(!mn.is_constant());
    }

    #[test]
    fn nat_poly_leq_with_mul() {
        // M * N <= M * N + 1
        let mn = NatPolyForm::from_var(GenericParamName::new("M"))
            .mul(&NatPolyForm::from_var(GenericParamName::new("N")));
        let mn_plus_1 = mn.add(&NatPolyForm::from_constant(1));
        assert!(mn.is_leq(&mn_plus_1));
        assert!(!mn_plus_1.is_leq(&mn));
    }

    #[test]
    fn nat_poly_format_zero() {
        let z = NatPolyForm::from_constant(0);
        assert_eq!(z.format(), "0");
    }
}
