//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! The TIR layer resolves ambiguous AST names (`Ident` in `DimTerm::name` and
//! `TypeExprKind::Indexed::indexes`) into concrete dimensions, struct types,
//! generic dimension parameters, or generic index parameters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{MulDivOp, TypeExpr, TypeExprKind};
use crate::syntax::dimension::{Dimension, Rational};
use crate::syntax::names::{DimName, GenericParamName, IndexName, StructTypeName};
use crate::syntax::span::Span;

use crate::ir::lower::IR;
use crate::ir::resolve::{DeclCategory, ExpectedFail};
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::Registry;
use crate::syntax::names::ScopedName;

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
    Label(IndexName, Span),
    /// A concrete scalar dimension, e.g. `Length * Time^-2`
    Scalar(Dimension),
    /// A non-generic struct type name, e.g. `TransferResult`
    Struct(StructTypeName, Span),
    /// A generic struct with concrete type arguments, e.g. `Vec3<Length, ECI>`
    GenericStruct {
        name: StructTypeName,
        type_args: Vec<Self>,
        span: Span,
    },
    /// A single generic dimension parameter, e.g. `D`
    GenericDimParam(GenericParamName, Span),
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
            Self::Label(index, _) => format!("Label({index})"),
            Self::Scalar(dim) => {
                let formatted = registry.dimensions.format_dimension(dim);
                if formatted.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    formatted
                }
            }
            Self::Struct(name, _) => name.to_string(),
            Self::GenericStruct {
                name, type_args, ..
            } => {
                let args: Vec<String> = type_args.iter().map(|a| a.format(registry)).collect();
                format!("{}<{}>", name, args.join(", "))
            }
            Self::GenericDimParam(name, _) => name.to_string(),
            Self::GenericDimExpr { terms, .. } => {
                let parts: Vec<String> = terms.iter().map(|t| t.format(registry)).collect();
                parts.join(" ")
            }
            Self::Indexed { base, indexes } => {
                let base_str = base.format(registry);
                let idx_strs: Vec<String> = indexes
                    .iter()
                    .map(|i| match i {
                        ResolvedIndex::Concrete(name, _) => name.to_string(),
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

    /// Generate a canonical synthetic index name for this nat form.
    ///
    /// For constants, produces `__nat_range_3`.
    /// For symbolic forms, produces `__nat_range_N + 1`, `__nat_range_M * N`, etc.
    #[must_use]
    pub fn to_index_name_str(&self) -> String {
        if self.is_constant() {
            crate::registry::types::nat_range_index_name(self.constant())
        } else {
            format!("__nat_range_{}", self.format())
        }
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

    /// Parse a `NatPolyForm` from a nat-range index name suffix.
    ///
    /// Given an index name like `__nat_range_3` or `__nat_range_N + 1`,
    /// strips the prefix and parses the suffix into a `NatPolyForm`.
    /// Returns `None` if the name doesn't have the expected prefix or
    /// the suffix cannot be parsed.
    #[must_use]
    pub fn from_index_name(name: &str) -> Option<Self> {
        let suffix = name.strip_prefix("__nat_range_")?;
        Self::parse_poly_form(suffix)
    }

    /// Parse a string like `"3"`, `"N"`, `"N + 1"`, `"M * N"`, `"2 * N^2 + 1"`
    /// into a `NatPolyForm`.
    #[must_use]
    fn parse_poly_form(s: &str) -> Option<Self> {
        let mut terms = BTreeMap::new();
        for part in s.split(" + ") {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            // Split on " * " to get factors of this term
            let factors: Vec<&str> = part.split(" * ").collect();
            let mut coeff: u64 = 1;
            let mut mono_vars = BTreeMap::new();
            for factor in &factors {
                let factor = factor.trim();
                if let Ok(n) = factor.parse::<u64>() {
                    // Numeric factor → coefficient
                    coeff *= n;
                } else if let Some((var_name, exp_str)) = factor.split_once('^') {
                    // Variable with exponent: "N^2"
                    let exp: u64 = exp_str.parse().ok()?;
                    *mono_vars
                        .entry(GenericParamName::new(var_name.trim()))
                        .or_insert(0) += exp;
                } else {
                    // Plain variable name
                    *mono_vars.entry(GenericParamName::new(factor)).or_insert(0) += 1;
                }
            }
            let mono = Monomial(mono_vars);
            *terms.entry(mono).or_insert(0) += coeff;
        }
        terms.retain(|_, c| *c != 0);
        Some(Self { terms })
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
                .find(|p| p.as_str() == ident.name)
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
    /// A concrete index name, e.g. `Maneuver`
    Concrete(IndexName, Span),
    /// A generic index parameter, e.g. `I`
    GenericParam(GenericParamName, Span),
    /// A Nat expression in index position (covers literals, variables, addition, and multiplication).
    ///
    /// Examples: `3` → constant form, `N` → single-variable form, `N + 1` → linear,
    /// `M * N` → polynomial.
    NatExpr(NatPolyForm, Span),
}

// ---------------------------------------------------------------------------
// Resolved domain constraints
// ---------------------------------------------------------------------------

/// A resolved domain constraint with evaluated SI-unit bounds.
///
/// Produced during `type_resolve()` by evaluating the bound expressions
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

// ---------------------------------------------------------------------------
// DAG registry
// ---------------------------------------------------------------------------

/// Map from canonical [`DagId`](crate::syntax::dag_id::DagId) to its
/// compiled per-DAG TIR.
///
/// Holds every DAG in scope at this file: the file's own top-level body
/// (keyed by [`TIR::root_dag_id`]), every inline `dag X { ... }` child
/// (keyed by `parent_dag_id.child(name)`), and every dep DAG merged in
/// by `merge_dep_dag_tirs` (keyed by the dep's canonical id).
pub type DagRegistry = HashMap<crate::syntax::dag_id::DagId, DagTIR>;

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
/// [`DagId`](crate::syntax::dag_id::DagId).
#[derive(Debug, Clone)]
pub struct TIR {
    /// The type/unit/dimension/index/struct registry, shared by every DAG
    /// in this file.
    pub registry: Registry,
    /// Canonical id of the file itself; the key under which the file's
    /// own top-level body lives in `dags`.
    pub root_dag_id: crate::syntax::dag_id::DagId,
    /// Every DAG reachable from this file. Always contains an entry for
    /// `root_dag_id`. Inline children and merged dep DAGs are inserted by
    /// the project pipeline.
    pub dags: DagRegistry,
    /// Maps each `import path as alias` (or `import path`) module alias to
    /// the dep file's canonical `DagId`. Used by [`TIR::lookup_call_target`]
    /// to translate user-typed `@alias.dag(args)` references into the
    /// canonical key under which the dep's DAGs were inserted by
    /// `merge_dep_dag_tirs`.
    pub module_aliases: HashMap<String, crate::syntax::dag_id::DagId>,
}

impl TIR {
    /// Borrow the file's own top-level [`DagTIR`].
    ///
    /// # Panics
    ///
    /// Panics if `root_dag_id` is not in `dags`. Construction sites
    /// (`type_resolve_single`) populate this entry; the invariant must
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

    /// Build the canonical [`DagId`](crate::syntax::dag_id::DagId) that
    /// `path` refers to under this file's scope (alias-translated for
    /// multi-segment paths, file-root-scoped for single-segment paths).
    ///
    /// Returns `None` when the leading alias of a multi-segment path is
    /// unknown.
    #[must_use]
    pub fn resolve_call_path(
        &self,
        path: &crate::syntax::ast::ModulePath,
    ) -> Option<crate::syntax::dag_id::DagId> {
        if path.segments.is_empty() {
            return None;
        }
        if path.segments.len() == 1 {
            return Some(self.root_dag_id.child(path.segments[0].name.as_str()));
        }
        let alias = path.segments[0].name.as_str();
        let dep_id = self.module_aliases.get(alias)?;
        let mut id = dep_id.clone();
        for seg in &path.segments[1..] {
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
        let root_dag_id = crate::syntax::dag_id::DagId::root("<eval-helper>");
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
                runtime_deps: HashMap::new(),
                const_deps: HashMap::new(),
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
/// Inserted into [`TIR::dags`] by `type_resolve_single` (one entry per
/// file root) and by the project pipeline's
/// `compile_inline_dag_bodies` / `merge_dep_dag_tirs`.
#[derive(Debug, Clone)]
pub struct DagTIR {
    /// Canonical identity of this DAG. Equal to the key under which this
    /// `DagTIR` is stored in [`TIR::dags`]; carried inline so the struct
    /// is self-describing when passed by reference.
    pub dag_id: crate::syntax::dag_id::DagId,
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
    /// For each param/node, the set of `@`-references (runtime deps).
    /// Outer map: key-lookup only, order irrelevant.
    /// Inner set: `BTreeSet` for deterministic iteration when building the DAG.
    pub runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
    /// For each const, the set of const-references (const deps).
    /// Outer map: key-lookup only, order irrelevant.
    /// Inner set: `BTreeSet` for deterministic iteration when building the DAG.
    pub const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
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
    /// nodes (`@mod::dag(args)::private_node` → `ImportPrivateItem`). The
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
            if !decl.visibility.is_public() {
                continue;
            }
            if let DeclKind::Node(n) = &decl.kind {
                self.pub_nodes.insert(n.name.value.to_string());
            }
        }
    }
}

/// Resolve all type annotations in an `IR`, producing a [`TIR`] whose
/// `dags` registry contains exactly one entry: the file's own root.
///
/// Inline `dag { ... }` declarations are NOT compiled here; the project
/// pipeline compiles them explicitly via
/// `graphcal_eval::inline_dag::compile_inline_dag_bodies` after the
/// file-level type resolution.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if any type annotation references an unknown
/// dimension, struct, or index.
pub fn type_resolve(
    ir: IR,
    root_dag_id: crate::syntax::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<TIR, GraphcalError> {
    let root_dag = type_resolve_dag(
        ir.consts,
        ir.params,
        ir.nodes,
        &ir.registry,
        src,
        &root_dag_id,
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
    );
    let mut dags = DagRegistry::new();
    dags.insert(root_dag_id.clone(), root_dag);
    Ok(TIR {
        registry: ir.registry,
        root_dag_id,
        dags,
        module_aliases: HashMap::new(),
    })
}

/// Resolve type annotations for a single DAG body.
///
/// Used both for the file-level root (via [`type_resolve`]) and by
/// the eval crate's per-dag-body compilation pipeline. Returns the
/// per-DAG content keyed by `dag_id`; the caller decides where to
/// install it in [`TIR::dags`].
///
/// # Errors
///
/// Returns a [`GraphcalError`] if any type annotation references an unknown
/// dimension, struct, or index.
pub fn type_resolve_single(
    ir: IR,
    dag_id: &crate::syntax::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<DagTIR, GraphcalError> {
    Ok(
        type_resolve_dag(ir.consts, ir.params, ir.nodes, &ir.registry, src, dag_id)?.with_body(
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
        ),
    )
}

/// Internal helper: resolve type annotations for the const/param/node
/// declarations of a single DAG, returning a partially-built [`DagTIR`].
fn type_resolve_dag(
    consts: Vec<crate::ir::lower::ConstEntry>,
    params: Vec<crate::ir::lower::ParamEntry>,
    nodes: Vec<crate::ir::lower::NodeEntry>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::syntax::dag_id::DagId,
) -> Result<DagTIRSeed, GraphcalError> {
    let mut resolved_decl_types = HashMap::new();
    let no_generic_params: &[GenericParamName] = &[];

    for entry in &consts {
        let resolved = resolve_type_expr(
            &entry.type_ann,
            registry,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            src,
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &params {
        let resolved = resolve_type_expr(
            &entry.type_ann,
            registry,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            src,
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }
    for entry in &nodes {
        let resolved = resolve_type_expr(
            &entry.type_ann,
            registry,
            no_generic_params,
            no_generic_params,
            no_generic_params,
            src,
        )?;
        resolved_decl_types.insert(entry.name.clone(), resolved);
    }

    Ok(DagTIRSeed {
        dag_id: dag_id.clone(),
        consts,
        params,
        nodes,
        resolved_decl_types,
    })
}

/// Partially-built [`DagTIR`] returned by [`type_resolve_dag`]; finalized
/// by [`DagTIRSeed::with_body`] which fills in the rest of the per-DAG
/// fields.
struct DagTIRSeed {
    dag_id: crate::syntax::dag_id::DagId,
    consts: Vec<crate::ir::lower::ConstEntry>,
    params: Vec<crate::ir::lower::ParamEntry>,
    nodes: Vec<crate::ir::lower::NodeEntry>,
    resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>,
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
        runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
        const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
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
    ) -> DagTIR {
        DagTIR {
            dag_id: self.dag_id,
            consts: self.consts,
            params: self.params,
            nodes: self.nodes,
            asserts,
            plots,
            figures,
            layers,
            runtime_deps,
            const_deps,
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
        }
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
    use crate::registry::declared_type::DeclaredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(DeclaredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(DeclaredType::Bool),
        ResolvedTypeExpr::Int => Ok(DeclaredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(DeclaredType::Datetime(*scale)),
        ResolvedTypeExpr::Label(index, _) => Ok(DeclaredType::Label(index.clone())),
        ResolvedTypeExpr::Scalar(dim) => Ok(DeclaredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(DeclaredType::Struct(name.clone(), vec![])),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut declared_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                declared_args.push(resolved_to_declared_type(arg, src)?);
            }
            Ok(DeclaredType::Struct(name.clone(), declared_args))
        }
        ResolvedTypeExpr::GenericDimParam(name, span) => Err(GraphcalError::EvalError {
            message: format!("cannot use generic dimension parameter `{name}` as a concrete type"),
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
                            index: name.clone(),
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
                        let idx_name = IndexName::new(
                            crate::registry::types::nat_range_index_name(form.constant()),
                        );
                        result = DeclaredType::Indexed {
                            element: Box::new(result),
                            index: idx_name,
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
            return Err(GraphcalError::IndexMismatch {
                expected: IndexName::new(crate::registry::types::nat_range_index_name(
                    form.evaluate(nat_sub).unwrap_or(0),
                )),
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
                    expected: IndexName::new(crate::registry::types::nat_range_index_name(*prev)),
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
    index_sub: &mut HashMap<GenericParamName, IndexName>,
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
                        bind_or_check(index_sub, gp.clone(), actual_idx.clone(), |prev, _| {
                            GraphcalError::IndexMismatch {
                                expected: prev.clone(),
                                found: actual_idx.clone(),
                                src: src.clone(),
                                span: span.into(),
                            }
                        })?;
                    }
                    ResolvedIndex::Concrete(name, _) => {
                        if *name != *actual_idx {
                            return Err(GraphcalError::IndexMismatch {
                                expected: name.clone(),
                                found: actual_idx.clone(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    }
                    ResolvedIndex::NatExpr(form, _) => {
                        // Extract the concrete nat value from the actual index name
                        let actual_nat =
                            crate::registry::types::parse_nat_range_index_name(actual_idx.as_str())
                                .ok_or_else(|| GraphcalError::IndexMismatch {
                                    expected: IndexName::new(format!("range({})", form.format())),
                                    found: actual_idx.clone(),
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
                    expected: format!("Label({expected_index})"),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected a label of index `{expected_index}`"),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if *expected_index != *actual_index {
                return Err(GraphcalError::IndexMismatch {
                    expected: expected_index.clone(),
                    found: actual_index.clone(),
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
            // For struct unification in function args, compare name only.
            // Type args matching is not needed here since function generics
            // don't use TypeApplication in their signatures (yet).
            let InferredType::Struct(actual_name, _) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                });
            };
            if *name != *actual_name {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.to_string(),
                    found: crate::tir::dim_check::format_inferred_type(actual, registry),
                    help: format!("expected struct type `{name}`"),
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
    index_sub: &HashMap<GenericParamName, IndexName>,
    nat_sub: &HashMap<GenericParamName, u64>,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::tir::dim_check::InferredType, GraphcalError> {
    use crate::tir::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(InferredType::Bool),
        ResolvedTypeExpr::Int => Ok(InferredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(InferredType::Datetime(*scale)),
        ResolvedTypeExpr::Label(index, _) => Ok(InferredType::Label(index.clone())),
        ResolvedTypeExpr::Scalar(dim) => Ok(InferredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(InferredType::Struct(name.clone(), vec![])),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut inferred_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                inferred_args.push(substitute_resolved_type(
                    arg, dim_sub, index_sub, nat_sub, src,
                )?);
            }
            Ok(InferredType::Struct(name.clone(), inferred_args))
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
                    ResolvedIndex::Concrete(name, _) => name.clone(),
                    ResolvedIndex::GenericParam(gp, span) => index_sub
                        .get(gp)
                        .cloned()
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!("generic index `{gp}` not bound during substitution"),
                            src: src.clone(),
                            span: (*span).into(),
                        })?,
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
                        IndexName::new(crate::registry::types::nat_range_index_name(n))
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
    match &type_ann.kind {
        TypeExprKind::Dimensionless => Ok(ResolvedTypeExpr::Dimensionless),
        TypeExprKind::Bool => Ok(ResolvedTypeExpr::Bool),
        TypeExprKind::Int => Ok(ResolvedTypeExpr::Int),
        TypeExprKind::Datetime => Ok(ResolvedTypeExpr::Datetime(TimeScale::UTC)),
        TypeExprKind::DatetimeApplication { type_args } => {
            resolve_datetime_application(type_ann, type_args, src)
        }

        TypeExprKind::Indexed { base, indexes } => {
            let resolved_base =
                resolve_type_expr(base, registry, dim_params, index_params, nat_params, src)?;
            let mut resolved_indexes = Vec::with_capacity(indexes.len());
            for idx in indexes {
                match idx {
                    crate::desugar::resolved_ast::IndexExpr::NatLiteral(n, span) => {
                        resolved_indexes.push(ResolvedIndex::NatExpr(
                            NatPolyForm::from_constant(*n),
                            *span,
                        ));
                    }
                    crate::desugar::resolved_ast::IndexExpr::NatExpr(nat_expr) => {
                        let form = normalize_nat_expr(nat_expr, nat_params, src)?;
                        resolved_indexes.push(ResolvedIndex::NatExpr(form, nat_expr.span()));
                    }
                    crate::desugar::resolved_ast::IndexExpr::Name(ident) => {
                        let idx_name = &ident.name;
                        if let Some(gp) = nat_params.iter().find(|p| p.as_str() == idx_name) {
                            // Generic nat param in index position: `D[N]` where `N: Nat`
                            resolved_indexes.push(ResolvedIndex::NatExpr(
                                NatPolyForm::from_var(gp.clone()),
                                ident.span,
                            ));
                        } else if let Some(gp) =
                            index_params.iter().find(|p| p.as_str() == idx_name)
                        {
                            resolved_indexes
                                .push(ResolvedIndex::GenericParam(gp.clone(), ident.span));
                        } else if registry.indexes.get_index(idx_name).is_some() {
                            resolved_indexes.push(ResolvedIndex::Concrete(
                                IndexName::new(idx_name),
                                ident.span,
                            ));
                        } else {
                            return Err(GraphcalError::UnknownIndex {
                                name: ident.as_index_name(),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                    }
                }
            }
            Ok(ResolvedTypeExpr::Indexed {
                base: Box::new(resolved_base),
                indexes: resolved_indexes,
            })
        }

        TypeExprKind::DimExpr(dim_expr) => resolve_dim_expr(dim_expr, registry, dim_params, src),

        TypeExprKind::TypeApplication { name, type_args } => resolve_type_application(
            type_ann,
            name,
            type_args,
            registry,
            dim_params,
            index_params,
            nat_params,
            src,
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
    dim_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    // Single-term, no power: could be struct, generic dim param, or dimension
    if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() {
        let name = &dim_expr.terms[0].term.name.name;
        let span = dim_expr.terms[0].term.span;

        // Check named index → Label type
        if let Some(idx_def) = registry.indexes.get_index(name)
            && matches!(
                idx_def.kind,
                crate::registry::types::IndexKind::Named { .. }
                    | crate::registry::types::IndexKind::RequiredNamed
            )
        {
            return Ok(ResolvedTypeExpr::Label(IndexName::new(name), span));
        }

        // Check tagged-union types first
        if registry.types.get_type(name).is_some() {
            return Ok(ResolvedTypeExpr::Struct(StructTypeName::new(name), span));
        }

        // Check generic dim param
        if let Some(gp) = dim_params.iter().find(|p| p.as_str() == name) {
            return Ok(ResolvedTypeExpr::GenericDimParam(gp.clone(), span));
        }
    }

    // Check if any term is a generic dim param
    let has_generic = dim_expr
        .terms
        .iter()
        .any(|item| dim_params.iter().any(|p| p.as_str() == item.term.name.name));

    if has_generic {
        // Build GenericDimExpr with mixed concrete/generic terms
        let mut terms = Vec::with_capacity(dim_expr.terms.len());
        for item in &dim_expr.terms {
            let name = &item.term.name.name;
            let power = item.term.power.unwrap_or(1);
            let op = item.op;

            if let Some(gp) = dim_params.iter().find(|p| p.as_str() == name) {
                terms.push(ResolvedDimTerm::GenericParam {
                    name: gp.clone(),
                    power,
                    op,
                    span: item.term.span,
                });
            } else if let Some(dim) = registry.dimensions.get_dimension(name) {
                terms.push(ResolvedDimTerm::Concrete {
                    dim: dim.clone(),
                    power,
                    op,
                });
            } else {
                return Err(GraphcalError::UnknownDimension {
                    name: DimName::new(name),
                    src: src.clone(),
                    span: item.term.span.into(),
                });
            }
        }
        Ok(ResolvedTypeExpr::GenericDimExpr {
            terms,
            span: dim_expr.span,
        })
    } else {
        // All terms are concrete dimensions — resolve to Scalar
        let mut result = Dimension::dimensionless();
        for item in &dim_expr.terms {
            let name = &item.term.name.name;
            let base = registry.dimensions.get_dimension(name).ok_or_else(|| {
                GraphcalError::UnknownDimension {
                    name: DimName::new(name),
                    src: src.clone(),
                    span: item.term.span.into(),
                }
            })?;
            let exp = item.term.power.unwrap_or(1);
            let overflow_err = || GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: item.term.span.into(),
            };
            let powered = base
                .pow(Rational::from_int(exp))
                .map_err(|_| overflow_err())?;
            result = match item.op {
                MulDivOp::Mul => (result * powered).map_err(|_| overflow_err())?,
                MulDivOp::Div => (result / powered).map_err(|_| overflow_err())?,
            };
        }
        Ok(ResolvedTypeExpr::Scalar(result))
    }
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
    let scale_name = match &arg.kind {
        TypeExprKind::DimExpr(dim_expr)
            if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() =>
        {
            &dim_expr.terms[0].term.name.name
        }
        _ => {
            return Err(GraphcalError::EvalError {
                message: "expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)".to_string(),
                src: src.clone(),
                span: arg.span.into(),
            });
        }
    };
    let scale: TimeScale = scale_name.parse().map_err(|_| GraphcalError::EvalError {
        message: format!(
            "unknown time scale `{scale_name}`; \
                     expected one of: UTC, TAI, TT, TDB, ET, GPST, GST, BDT"
        ),
        src: src.clone(),
        span: arg.span.into(),
    })?;
    Ok(ResolvedTypeExpr::Datetime(scale))
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
    name: &crate::desugar::resolved_ast::Ident,
    type_args: &[TypeExpr],
    registry: &Registry,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    nat_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let type_name = &name.name;

    // Verify this is a known generic type
    let type_def =
        registry
            .types
            .get_type(type_name)
            .ok_or_else(|| GraphcalError::UnknownStructType {
                name: StructTypeName::new(type_name),
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
                "type `{type_name}` expects {hint} type argument(s), got {}",
                type_args.len()
            ),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    // Resolve each explicit type argument, then fill in defaults
    let mut resolved_args = Vec::with_capacity(total_params);
    for arg in type_args {
        let resolved = resolve_type_expr(arg, registry, dim_params, index_params, nat_params, src)?;
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
        let resolved = resolve_type_expr(
            default_expr,
            registry,
            dim_params,
            index_params,
            nat_params,
            src,
        )?;
        resolved_args.push(resolved);
    }
    Ok(ResolvedTypeExpr::GenericStruct {
        name: StructTypeName::new(type_name),
        type_args: resolved_args,
        span: type_ann.span,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
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

    /// Create a simple dimension `TypeExpr` from a name string like `"Velocity"`.
    fn make_dim_type_expr(name: &str) -> crate::desugar::resolved_ast::TypeExpr {
        crate::desugar::resolved_ast::TypeExpr {
            kind: crate::desugar::resolved_ast::TypeExprKind::DimExpr(
                crate::desugar::resolved_ast::DimExpr {
                    terms: vec![crate::desugar::resolved_ast::DimExprItem {
                        op: crate::desugar::resolved_ast::MulDivOp::Mul,
                        term: crate::desugar::resolved_ast::DimTerm {
                            name: crate::desugar::resolved_ast::Ident {
                                name: name.to_string(),
                                span: Span::new(0, 0),
                            },
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
                    crate::syntax::names::VariantName::new("Departure"),
                    crate::syntax::names::VariantName::new("Insertion"),
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
        let mut desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        crate::syntax::ast::desugar_tuple_matches(&mut desugared);
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

    // --- type_resolve() integration tests ---

    /// Single-file integration helper: lower + type-resolve + compile each
    /// inline dag body using the dumb `lower_dag_body_to_ir` primitive
    /// directly (no self-import preprocessing — fixtures exercised here
    /// either don't use self-imports or are expected to surface errors that
    /// fall out of the unprocessed body).
    fn parse_and_type_resolve(source: &str) -> Result<TIR, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let mut desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        crate::syntax::ast::desugar_tuple_matches(&mut desugared);
        let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
        let src = NamedSource::new("test.gcl", Arc::new(source.to_string()));
        let ir = crate::ir::lower::lower(&file, &src)?;
        let parent_dag_id =
            crate::syntax::dag_id::DagId::from_relative_path(std::path::Path::new("test.gcl"))
                .unwrap();
        let mut tir = type_resolve(ir, parent_dag_id.clone(), &src)?;
        compile_inline_dag_bodies_test(&mut tir, &src, &parent_dag_id)?;
        Ok(tir)
    }

    /// Compile each inline dag body in `tir` with no self-import
    /// preprocessing. Used by compiler-side integration tests that don't
    /// have access to the eval crate's project pipeline.
    fn compile_inline_dag_bodies_test(
        tir: &mut TIR,
        src: &NamedSource<Arc<String>>,
        parent_dag_id: &crate::syntax::dag_id::DagId,
    ) -> Result<(), GraphcalError> {
        let dag_names: Vec<String> = tir
            .registry
            .dags
            .all_dags()
            .map(|(name, _)| name.clone())
            .collect();
        for name in dag_names {
            let body = tir
                .registry
                .dags
                .get(&name)
                .map(|d| d.body.clone())
                .unwrap_or_default();
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
            let mut compiled_dag = type_resolve_single(dag_body_ir, &dag_id, src)?;
            compiled_dag.populate_pub_nodes(&body);
            tir.dags.insert(dag_id, compiled_dag);
        }
        Ok(())
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

    use crate::registry::declared_type::DeclaredType;

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
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Struct(StructTypeName::new("Foo"), Span::new(0, 0)),
            &make_src(),
        )
        .unwrap();
        assert_eq!(dt, DeclaredType::Struct(StructTypeName::new("Foo"), vec![]));
    }

    #[test]
    fn convert_indexed() {
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Indexed {
                base: Box::new(ResolvedTypeExpr::Scalar(Dimension::base(
                    BaseDimId::Prelude("Length".to_string()),
                ))),
                indexes: vec![ResolvedIndex::Concrete(
                    IndexName::new("M"),
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
                index: IndexName::new("M"),
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
    // NatPolyForm::from_index_name tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_index_name_constant() {
        let form = NatPolyForm::from_index_name("__nat_range_3").unwrap();
        assert_eq!(form, NatPolyForm::from_constant(3));
    }

    #[test]
    fn parse_index_name_variable() {
        let form = NatPolyForm::from_index_name("__nat_range_N").unwrap();
        assert_eq!(form, NatPolyForm::from_var(GenericParamName::new("N")));
    }

    #[test]
    fn parse_index_name_var_plus_constant() {
        let form = NatPolyForm::from_index_name("__nat_range_N + 1").unwrap();
        let expected =
            NatPolyForm::from_var(GenericParamName::new("N")).add(&NatPolyForm::from_constant(1));
        assert_eq!(form, expected);
    }

    #[test]
    fn parse_index_name_two_vars() {
        let form = NatPolyForm::from_index_name("__nat_range_M + N + 2").unwrap();
        let expected = NatPolyForm::from_var(GenericParamName::new("M"))
            .add(&NatPolyForm::from_var(GenericParamName::new("N")))
            .add(&NatPolyForm::from_constant(2));
        assert_eq!(form, expected);
    }

    #[test]
    fn parse_index_name_no_prefix() {
        assert!(NatPolyForm::from_index_name("Phase").is_none());
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

    #[test]
    fn parse_index_name_mul() {
        let form = NatPolyForm::from_index_name("__nat_range_M * N").unwrap();
        let expected = NatPolyForm::from_var(GenericParamName::new("M"))
            .mul(&NatPolyForm::from_var(GenericParamName::new("N")));
        assert_eq!(form, expected);
    }

    #[test]
    fn parse_index_name_mul_plus_const() {
        let form = NatPolyForm::from_index_name("__nat_range_M * N + 1").unwrap();
        let expected = NatPolyForm::from_var(GenericParamName::new("M"))
            .mul(&NatPolyForm::from_var(GenericParamName::new("N")))
            .add(&NatPolyForm::from_constant(1));
        assert_eq!(form, expected);
    }
}
