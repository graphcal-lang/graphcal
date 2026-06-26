use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroUsize;

use thiserror::Error;

use crate::desugar::desugared_ast::{
    DagDecl, DimExpr, Expr, GenericConstraint, MulDivOp, TypeExpr, TypeExprKind, UnitExpr,
};
use crate::syntax::ast::UnitConstness;
use crate::syntax::dimension::{BaseDimId, Dimension, Rational, RationalError};
use crate::syntax::names::{
    ConstructorName, DeclName, DimName, FieldName, GenericParamName, IndexName, IndexVariantName,
    StructTypeName, UnitRef,
};
// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Error returned when a unit scale is not a positive finite scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PositiveFiniteScaleError {
    #[error("scale must be finite")]
    NonFinite,
    #[error("scale must be greater than zero")]
    NonPositive,
}

/// A unit scale factor that is guaranteed to be positive and finite.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct PositiveFiniteScale(f64);

impl PositiveFiniteScale {
    /// Validate a raw scale factor.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is `NaN`, infinite, zero, or negative.
    pub fn new(value: f64) -> Result<Self, PositiveFiniteScaleError> {
        if !value.is_finite() {
            Err(PositiveFiniteScaleError::NonFinite)
        } else if value <= 0.0 {
            Err(PositiveFiniteScaleError::NonPositive)
        } else {
            Ok(Self(value))
        }
    }

    /// Construct a scale from trusted internal constants.
    ///
    /// Callers must ensure `value` is positive and finite. This is restricted
    /// to the compiler crate so external code must use [`Self::new`].
    #[must_use]
    pub(crate) const fn new_unchecked(value: f64) -> Self {
        Self(value)
    }

    /// Return the wrapped raw scale factor.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl std::fmt::Display for PositiveFiniteScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// How a unit's scale factor is determined.
#[derive(Debug, Clone)]
pub enum UnitScale {
    /// Scale factor known at compile time (e.g., `const unit km: Length = 1000 m;`).
    Static(PositiveFiniteScale),
    /// Scale factor depends on runtime values (e.g., `unit EUR: Money = (@rate) USD;`).
    ///
    /// The final SI scale = `eval(scale_expr) * base_unit_scale`.
    Dynamic {
        /// The unevaluated scale expression containing `@`-references.
        scale_expr: Expr,
        /// The scale factor of the base unit in the definition (resolved at compile time).
        /// For `(@rate) USD` where USD has scale 1.0, this is 1.0.
        base_unit_scale: PositiveFiniteScale,
    },
}

impl UnitScale {
    /// Returns the static scale factor, or `None` if the scale is dynamic.
    #[must_use]
    pub const fn as_static(&self) -> Option<f64> {
        match self {
            Self::Static(s) => Some(s.get()),
            Self::Dynamic { .. } => None,
        }
    }

    /// Returns `true` if the scale is resolved at compile time.
    #[must_use]
    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }

    /// Returns `true` if the scale depends on runtime values.
    #[must_use]
    pub const fn is_dynamic(&self) -> bool {
        matches!(self, Self::Dynamic { .. })
    }
}

/// Information about a registered unit.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    /// The dimension this unit measures.
    pub dimension: Dimension,
    /// Whether this unit may appear in compile-time (`const`) contexts.
    pub constness: UnitConstness,
    /// Scale factor to convert 1 of this unit to base SI units.
    /// e.g., km -> `Static(1000.0)` (1 km = 1000 m)
    pub scale: UnitScale,
}

/// A field in a record type definition.
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: FieldName,
    pub type_ann: TypeExpr,
}

/// A member (constructor) of a tagged-union type.
///
/// The compiler treats every `type T { ... }` declaration as an n-variant
/// tagged union — including single-variant cases. Each variant carries
/// its payload fields inline; there are no per-variant standalone types.
#[derive(Debug, Clone)]
pub struct UnionMemberDef {
    /// Constructor name.
    pub name: ConstructorName,
    /// Payload fields for this constructor. An empty `Vec` means a unit
    /// constructor (`Coast`).
    pub fields: Vec<StructField>,
}

/// The kind of a type definition.
///
/// The functional core only distinguishes two shapes: a *required* type
/// stub (no body, awaits binding via include) and an *n-variant union*
/// — single-variant or multi-variant alike. Record-shaped types are
/// represented as a single-variant union whose sole constructor's name
/// matches the type's name (e.g.,
/// `type Position { Position(x: Length, y: Length) }`).
#[derive(Debug, Clone)]
pub enum TypeDefKind {
    /// A required type with no body: `type Element;`. Bound from outside
    /// via parameterized include.
    Required,
    /// A tagged union: `type Maneuver { Impulsive(delta_v: Velocity), Coast }`
    /// or, as a single-variant special case,
    /// `type Position { Position(x: Length, y: Length) }`.
    Union { members: Vec<UnionMemberDef> },
}

/// The constraint on a generic parameter of a type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeGenericConstraint {
    /// `D: Dim` — the generic stands for a dimension.
    Dim,
    /// `I: Index` — the generic stands for an index.
    Index,
    /// `N: Nat` — the generic stands for a natural number (type-level).
    Nat,
    /// `F: Type` — unconstrained phantom type parameter.
    Unconstrained,
}

impl From<GenericConstraint> for TypeGenericConstraint {
    fn from(c: GenericConstraint) -> Self {
        match c {
            GenericConstraint::Dim => Self::Dim,
            GenericConstraint::Index => Self::Index,
            GenericConstraint::Nat => Self::Nat,
            GenericConstraint::Type => Self::Unconstrained,
        }
    }
}

/// A generic parameter on a type definition.
#[derive(Debug, Clone)]
pub struct TypeGenericParam {
    pub name: GenericParamName,
    pub constraint: TypeGenericConstraint,
    /// Optional default type expression, e.g. `F: Type = Unframed`.
    pub default: Option<crate::desugar::desugared_ast::TypeExpr>,
}

/// A registered type definition: either a required type stub or a tagged union.
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: StructTypeName,
    pub generic_params: Vec<TypeGenericParam>,
    pub kind: TypeDefKind,
}

impl TypeDef {
    /// Returns the union members if this is a tagged union.
    ///
    /// Returns `None` only for a required (unbound) type stub.
    #[must_use]
    pub fn union_members(&self) -> Option<&[UnionMemberDef]> {
        match &self.kind {
            TypeDefKind::Union { members } => Some(members),
            TypeDefKind::Required => None,
        }
    }

    /// Returns `true` if this is a tagged union — single-variant or
    /// multi-variant.
    #[must_use]
    pub const fn is_union(&self) -> bool {
        matches!(self.kind, TypeDefKind::Union { .. })
    }

    /// Returns `true` if this is a required type stub awaiting binding.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self.kind, TypeDefKind::Required)
    }

    /// If this is a single-variant union whose sole constructor's name
    /// equals the type's name, returns that variant's payload fields.
    /// This is the record-like shape: field access and brace
    /// construction work directly on it.
    ///
    /// For multi-variant unions or single-variant unions whose
    /// constructor name differs from the type name, returns `None` —
    /// callers must dispatch through the constructor namespace and / or
    /// `match`.
    #[must_use]
    pub fn record_fields(&self) -> Option<&[StructField]> {
        let TypeDefKind::Union { members } = &self.kind else {
            return None;
        };
        let [only] = members.as_slice() else {
            return None;
        };
        (only.name.as_str() == self.name.as_str()).then_some(only.fields.as_slice())
    }

    /// Backward-compatible accessor that returns the record-shaped
    /// fields (empty when the type is multi-variant or a required
    /// stub). Prefer [`record_fields`](Self::record_fields) at new call
    /// sites — it makes the single-variant precondition explicit.
    #[must_use]
    pub fn fields(&self) -> &[StructField] {
        self.record_fields().unwrap_or(&[])
    }
}

/// Data for a concrete numeric range index (e.g., `linspace(0.0 s, 100.0 s, step: 0.1 s)`).
#[derive(Debug, Clone)]
pub struct RangeIndexData {
    pub start: f64,
    pub end: f64,
    pub step: f64,
    /// Validated number of inclusive range steps.
    pub step_count: NonZeroUsize,
    pub dimension: Dimension,
    /// Display unit label (e.g., `"s"`) for formatting step values.
    pub display_label: Option<String>,
    /// Scale factor from SI to display unit: `display_value = si_value / scale`.
    pub display_scale: f64,
}

impl RangeIndexData {
    /// Returns the SI value at step `i`.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "range step indices are small enough for exact f64 representation"
    )]
    pub fn step_value(&self, i: usize) -> f64 {
        (i as f64).mul_add(self.step, self.start)
    }

    /// Returns the number of steps in this range.
    #[must_use]
    pub const fn step_count(&self) -> usize {
        self.step_count.get()
    }
}

/// The kind of an index: either named variants or a numeric range.
#[derive(Debug, Clone)]
pub enum IndexKind {
    /// A named label set, e.g. `index Maneuver = { Departure, Correction, Insertion };`
    Named { variants: Vec<IndexVariantName> },
    /// A numeric range, e.g. `index T = linspace(0.0 s, 100.0 s, step: 0.1 s);`
    Range(RangeIndexData),
    /// Required named index (no variants): must be bound via parameterized import.
    RequiredNamed,
    /// Required range index with dimension constraint: must be bound via parameterized import.
    RequiredRange { dimension: Dimension },
    /// A Nat-parameterized range: `range(N)` with elements `{0, 1, ..., N-1}`.
    ///
    /// Created synthetically for integer literals in index position (e.g., `D[3]`).
    NatRange {
        /// The non-zero size of the range (number of elements). Stored as
        /// `usize` because it bounds in-memory variant tables; AST-level Nat
        /// literals are converted at the registry boundary.
        size: NonZeroUsize,
    },
}

/// A declared index with its ordered variants.
#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: IndexName,
    pub kind: IndexKind,
}

impl IndexDef {
    /// Returns the ordered variant names for this index.
    ///
    /// For named indexes, returns the declared variants.
    /// For range indexes, generates synthetic names like `"#0"`, `"#1"`, etc.
    /// For nat range indexes, generates synthetic names like `"#0"`, `"#1"`, etc.
    /// For required indexes, returns an empty vec (no variants until bound).
    #[must_use]
    pub fn variants(&self) -> Vec<IndexVariantName> {
        match &self.kind {
            IndexKind::Named { variants } => variants.clone(),
            IndexKind::Range(data) => {
                let count = data.step_count();
                (0..count).map(IndexVariantName::range_step).collect()
            }
            IndexKind::NatRange { size } => {
                (0..size.get()).map(IndexVariantName::range_step).collect()
            }
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. } => vec![],
        }
    }

    /// Returns the number of steps/variants in this index.
    ///
    /// Returns 0 for required indexes (no variants until bound).
    #[must_use]
    pub const fn step_count(&self) -> usize {
        match &self.kind {
            IndexKind::Named { variants } => variants.len(),
            IndexKind::Range(data) => data.step_count(),
            IndexKind::NatRange { size } => size.get(),
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. } => 0,
        }
    }

    /// Returns the range data if this is a concrete range index.
    #[must_use]
    pub const fn range_data(&self) -> Option<&RangeIndexData> {
        match &self.kind {
            IndexKind::Range(data) => Some(data),
            _ => None,
        }
    }

    /// Returns true if this is a range index (concrete or required, not nat range).
    #[must_use]
    pub const fn is_range(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::Range(_) | IndexKind::RequiredRange { .. }
        )
    }

    /// Returns true if this is a named index (concrete or required).
    #[must_use]
    pub const fn is_named(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::Named { .. } | IndexKind::RequiredNamed
        )
    }

    /// Returns true if this is a nat range index.
    #[must_use]
    pub const fn is_nat_range(&self) -> bool {
        matches!(self.kind, IndexKind::NatRange { .. })
    }

    /// Returns the nat range size, if this is a nat range index.
    #[must_use]
    pub const fn nat_range_size(&self) -> Option<u64> {
        match &self.kind {
            IndexKind::NatRange { size } => Some(size.get() as u64),
            _ => None,
        }
    }

    /// Returns true if this is a required index (must be bound via parameterized import).
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(
            self.kind,
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Nat range helpers
// ---------------------------------------------------------------------------

/// Error returned when an AST/runtime Nat range size cannot become a concrete index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NatRangeIndexError {
    /// Empty Nat ranges are deliberately not representable.
    #[error("range(0) is not allowed; indexes must contain at least one element")]
    Empty,
    /// The source-level `u64` size does not fit in this target's in-memory index size.
    #[error("nat range size {size} does not fit in usize on this target")]
    DoesNotFitUsize { size: u64 },
}

/// Typed identity for a concrete compiler-generated Nat range index.
///
/// The core carries this non-zero size directly; display names are derived only
/// for diagnostics and compatibility with APIs that still need an [`IndexName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NatRangeIndex {
    size: NonZeroUsize,
}

impl NatRangeIndex {
    /// Create an identity for a non-empty Nat range index.
    #[must_use]
    pub const fn new(size: NonZeroUsize) -> Self {
        Self { size }
    }

    /// Try to create an identity from an AST/runtime `u64` size.
    ///
    /// # Errors
    ///
    /// Returns an error when `size` is zero or cannot fit in `usize` on this target.
    pub fn try_from_u64(size: u64) -> Result<Self, NatRangeIndexError> {
        if size == 0 {
            return Err(NatRangeIndexError::Empty);
        }
        let size =
            usize::try_from(size).map_err(|_| NatRangeIndexError::DoesNotFitUsize { size })?;
        let size = NonZeroUsize::new(size).ok_or(NatRangeIndexError::Empty)?;
        Ok(Self::new(size))
    }

    /// Return the non-zero in-memory size.
    #[must_use]
    pub const fn size(self) -> NonZeroUsize {
        self.size
    }

    /// Return the size as a `u64` for Nat-expression comparisons and display.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "Graphcal currently supports targets where usize fits in u64"
    )]
    pub fn size_u64(self) -> u64 {
        u64::try_from(self.size.get()).expect("usize fits in u64 on supported targets")
    }

    /// Render this identity for diagnostics as source-level `range(N)` syntax.
    #[must_use]
    pub fn display_name(self) -> IndexName {
        IndexName::new(format!("range({})", self.size_u64()))
    }
}

// ---------------------------------------------------------------------------
// Private helper functions for resolution logic
// ---------------------------------------------------------------------------

/// Why a unit expression could not be resolved.
///
/// Carries the failing unit name so callers can produce a precise
/// diagnostic instead of re-scanning the expression to find it (the old
/// `Ok(None)` return conflated unknown names with dynamic scales).
#[derive(Debug, Clone, PartialEq)]
pub enum UnitResolveError {
    /// A unit name in the expression is not registered.
    UnknownUnit(UnitRef),
    /// A unit in the expression has a runtime-dependent scale.
    DynamicScale(UnitRef),
    /// Dimension exponent arithmetic overflowed.
    Overflow(RationalError),
}

impl From<RationalError> for UnitResolveError {
    fn from(err: RationalError) -> Self {
        Self::Overflow(err)
    }
}

/// Shared implementation for resolving a `DimExpr` to a concrete `Dimension`.
fn resolve_dim_expr_impl(
    dimensions: &HashMap<DimName, Dimension>,
    expr: &DimExpr,
) -> Result<Option<Dimension>, RationalError> {
    expr.terms
        .iter()
        .try_fold(Some(Dimension::dimensionless()), |acc, item| {
            let Some(acc) = acc else {
                return Ok(None);
            };
            let Some(atom) = item.term.name.value.as_bare() else {
                return Ok(None);
            };
            let Some(base) = dimensions.get(atom.as_str()) else {
                return Ok(None);
            };
            let exp = item.term.power.unwrap_or(Rational::ONE);
            let powered = base.pow(exp)?;
            match item.op {
                MulDivOp::Mul => acc * powered,
                MulDivOp::Div => acc / powered,
            }
            .map(Some)
        })
}

/// Shared implementation for resolving a `TypeExpr` to a concrete `Dimension`.
fn resolve_type_expr_impl(
    dimensions: &HashMap<DimName, Dimension>,
    type_expr: &TypeExpr,
) -> Result<Option<Dimension>, RationalError> {
    match &type_expr.kind {
        TypeExprKind::Dimensionless => Ok(Some(Dimension::dimensionless())),
        TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::TypeApplication { .. }
        | TypeExprKind::DatetimeApplication { .. } => Ok(None),
        TypeExprKind::DimExpr(dim_expr) => resolve_dim_expr_impl(dimensions, dim_expr),
        TypeExprKind::Indexed { base, .. } => resolve_type_expr_impl(dimensions, base),
    }
}

/// Raise a positive unit scale to a rational power.
///
/// Integer powers use `powi` for exactness; fractional powers fall back to
/// `powf`, which is well-defined because unit scales are always positive.
#[must_use]
pub fn pow_scale(scale: f64, exp: Rational) -> f64 {
    if exp.is_integer() {
        scale.powi(exp.num())
    } else {
        scale.powf(f64::from(exp.num()) / f64::from(exp.den()))
    }
}

/// Shared implementation for resolving a `UnitExpr` to its dimension and static scale factor.
fn resolve_unit_expr_impl(
    units: &HashMap<UnitRef, UnitInfo>,
    expr: &UnitExpr,
) -> Result<(Dimension, f64), UnitResolveError> {
    let mut dim = Dimension::dimensionless();
    let mut scale = 1.0_f64;
    for item in &expr.terms {
        let Some(info) = units.get(&item.name.value) else {
            return Err(UnitResolveError::UnknownUnit(item.name.value.clone()));
        };
        let exp = item.power.unwrap_or(Rational::ONE);
        let powered_dim = info.dimension.pow(exp)?;
        let Some(static_scale) = info.scale.as_static() else {
            return Err(UnitResolveError::DynamicScale(item.name.value.clone()));
        };
        let powered_scale = pow_scale(static_scale, exp);
        match item.op {
            MulDivOp::Mul => {
                dim = (dim * powered_dim)?;
                scale *= powered_scale;
            }
            MulDivOp::Div => {
                dim = (dim / powered_dim)?;
                scale /= powered_scale;
            }
        }
    }
    Ok((dim, scale))
}

/// Shared implementation for resolving a `UnitExpr` to its dimension only (ignoring scales).
///
/// Works for both static and dynamic units.
fn resolve_unit_dimension_impl(
    units: &HashMap<UnitRef, UnitInfo>,
    expr: &UnitExpr,
) -> Result<Dimension, UnitResolveError> {
    let mut dim = Dimension::dimensionless();
    for item in &expr.terms {
        let Some(info) = units.get(&item.name.value) else {
            return Err(UnitResolveError::UnknownUnit(item.name.value.clone()));
        };
        let exp = item.power.unwrap_or(Rational::ONE);
        let powered_dim = info.dimension.pow(exp)?;
        dim = match item.op {
            MulDivOp::Mul => (dim * powered_dim)?,
            MulDivOp::Div => (dim / powered_dim)?,
        };
    }
    Ok(dim)
}

/// Format a dimension, preferring a registered named alias for compound forms.
///
/// A pure base dimension (`Length`) or `Dimensionless` keeps its canonical
/// rendering. A compound dimension (`Length^2 * Mass / Time^2`) is replaced by
/// a matching named dimension (`Energy`) when one is registered; if several
/// names match, the lexicographically smallest is chosen for determinism.
fn format_dimension_preferring_alias(
    dimensions: &HashMap<DimName, Dimension>,
    base_dim_names: &BTreeMap<BaseDimId, String>,
    dim: &Dimension,
) -> String {
    let canonical = format!("{}", dim.display_with(base_dim_names));
    // Base dimensions and Dimensionless render as a single bare name already;
    // only compound renderings benefit from an alias.
    let is_compound = canonical.contains([' ', '^', '*', '/']);
    if is_compound
        && let Some(alias) = dimensions
            .iter()
            .filter(|(_, d)| *d == dim)
            .map(|(name, _)| name)
            .min()
    {
        return alias.to_string();
    }
    canonical
}

fn assert_base_dim_names_cover(
    base_dim_names: &BTreeMap<BaseDimId, String>,
    dim: &Dimension,
    context: &str,
) {
    for (id, _) in dim.iter() {
        assert!(
            base_dim_names.contains_key(id),
            "registry invariant violation: {context} references base dimension {id:?} without a registered display name"
        );
    }
}

// ---------------------------------------------------------------------------
// Domain-specific registries (frozen / read-only)
// ---------------------------------------------------------------------------

/// Dimension registry: maps dimension names to `Dimension` values and tracks
/// base dimension metadata (ID assignment, names, default unit symbols).
#[derive(Debug, Clone)]
pub struct DimensionRegistry {
    /// Base dimension ID → dimension name (for display).
    base_dim_names: BTreeMap<BaseDimId, String>,
    /// Base dimension ID → default unit symbol for runtime display.
    base_dim_symbols: BTreeMap<BaseDimId, String>,
    dimensions: HashMap<DimName, Dimension>,
}

impl DimensionRegistry {
    /// Look up a dimension by name.
    #[must_use]
    pub fn get_dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    /// Iterate over all named dimensions.
    pub fn all_dimensions(&self) -> impl Iterator<Item = (&DimName, &Dimension)> {
        self.dimensions.iter()
    }

    /// Get the base dimension names map (for display purposes).
    #[must_use]
    pub const fn base_dim_names(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_names
    }

    /// Get the base dimension symbols map for runtime display.
    #[must_use]
    pub const fn base_dim_symbols(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_symbols
    }

    /// Format a dimension as a human-readable string using registered base dimension names.
    ///
    /// Returns `"Dimensionless"` for dimensionless, or names like `"Length / Time"`.
    /// When a compound dimension matches a named dimension alias (e.g. `Energy`
    /// for `Length^2 * Mass / Time^2`), the alias is preferred so diagnostics
    /// speak the user's vocabulary.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format_dimension_preferring_alias(&self.dimensions, &self.base_dim_names, dim)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if any dimension name is unknown, and `Err` if
    /// dimension exponent arithmetic overflows `i32`.
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Result<Option<Dimension>, RationalError> {
        resolve_dim_expr_impl(&self.dimensions, expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if the type references unknown dimensions, and
    /// `Err` if dimension exponent arithmetic overflows `i32`.
    pub fn resolve_type_expr(
        &self,
        type_expr: &TypeExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        resolve_type_expr_impl(&self.dimensions, type_expr)
    }
}

/// Unit registry: maps unit names to `UnitInfo` (dimension + scale).
#[derive(Debug, Clone)]
pub struct UnitRegistry {
    units: HashMap<UnitRef, UnitInfo>,
}

impl UnitRegistry {
    /// Look up a unit by reference (bare or module-alias-qualified).
    #[must_use]
    pub fn get_unit(&self, name: &UnitRef) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Iterate over all units: (reference, dimension, scale).
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitRef, &Dimension, &UnitScale)> {
        self.units
            .iter()
            .map(|(name, info)| (name, &info.dimension, &info.scale))
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    ///
    /// # Errors
    ///
    /// Returns a [`UnitResolveError`] naming the unknown or dynamic-scale
    /// unit, or the exponent overflow.
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Result<(Dimension, f64), UnitResolveError> {
        resolve_unit_expr_impl(&self.units, expr)
    }

    /// Resolve a `UnitExpr` to its dimension only (ignoring scales).
    ///
    /// Works for both static and dynamic units.
    ///
    /// # Errors
    ///
    /// Returns a [`UnitResolveError`] naming the unknown unit, or the
    /// exponent overflow.
    pub fn resolve_unit_dimension(&self, expr: &UnitExpr) -> Result<Dimension, UnitResolveError> {
        resolve_unit_dimension_impl(&self.units, expr)
    }
}

/// Type registry: maps type names to `TypeDef` and provides
/// constructor-namespace lookup.
///
/// The constructor namespace is *separate from* the type namespace: a
/// single lexeme can name both a type (`Position` — the n-variant
/// union) and a constructor (`Position` — the sole constructor of that
/// union). [`lookup_ctor`](Self::lookup_ctor) walks the constructor
/// side; [`get_type`](Self::get_type) walks the type side.
#[derive(Debug, Clone)]
pub struct TypeRegistry {
    types: HashMap<StructTypeName, TypeDef>,
    /// Constructor namespace: each constructor name resolves to the
    /// union it belongs to. With no module system, the namespace is
    /// flat. Duplicate names are rejected upstream during name
    /// resolution; like every `register_*` entry point, insertion here
    /// is last-wins defense-in-depth, not a validation layer.
    ctors: HashMap<ConstructorName, StructTypeName>,
}

impl TypeRegistry {
    /// Look up a type definition by type name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Look up the union that owns a constructor name, plus the
    /// constructor's payload fields. Returns `None` if the name is not
    /// a registered constructor.
    #[must_use]
    pub fn lookup_ctor(&self, ctor: &ConstructorName) -> Option<(&TypeDef, &UnionMemberDef)> {
        let union_name = self.ctors.get(ctor)?;
        let td = self.types.get(union_name)?;
        let members = td.union_members()?;
        let member = members.iter().find(|m| m.name == *ctor)?;
        Some((td, member))
    }

    /// Iterate over all registered type definitions.
    pub fn all_types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values()
    }
}

/// Index registry: maps declared index names and typed Nat-range identities to `IndexDef`.
#[derive(Debug, Clone)]
pub struct IndexRegistry {
    indexes: HashMap<IndexName, IndexDef>,
    nat_ranges: HashMap<NatRangeIndex, IndexDef>,
}

impl IndexRegistry {
    /// Look up a declared index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    /// Look up a compiler-generated Nat range index by typed identity.
    #[must_use]
    pub fn get_nat_range(&self, index: NatRangeIndex) -> Option<&IndexDef> {
        self.nat_ranges.get(&index)
    }

    /// Iterate over all index definitions.
    pub fn all_indexes(&self) -> impl Iterator<Item = &IndexDef> {
        self.indexes.values().chain(self.nat_ranges.values())
    }
}

// ---------------------------------------------------------------------------
// Frozen aggregate registry
// ---------------------------------------------------------------------------

/// The frozen, read-only aggregate of all domain registries.
///
/// Produced by [`RegistryBuilder::build`]. All fields are public so that
/// consumers can access individual domain registries directly.
#[derive(Debug, Clone)]
pub struct Registry {
    pub dimensions: DimensionRegistry,
    pub units: UnitRegistry,
    pub types: TypeRegistry,
    pub indexes: IndexRegistry,
    pub dags: DagRegistry,
}

/// Registry of `dag` declaration bodies accessible by name within a file.
///
/// Populated at IR lowering time with the raw AST body for each declared `dag`.
/// Used during dim-checking (and later, evaluation) to resolve inline DAG
/// invocations `@dag(args).out` against the called `dag`'s `pub param` and
/// `pub node` signatures.
#[derive(Debug, Default, Clone)]
pub struct DagRegistry {
    /// Dag bodies keyed by their declaration name. Dags live in the
    /// declaration namespace, so the key is the typed [`DeclName`] like
    /// every other registry — not a bare `String`.
    dags: HashMap<DeclName, DagDecl>,
}

impl DagRegistry {
    /// Return the AST body of the named `dag`, if one is declared in this file.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DagDecl> {
        self.dags.get(name)
    }

    /// Iterate over all registered dags.
    pub fn all_dags(&self) -> impl Iterator<Item = (&DeclName, &DagDecl)> {
        self.dags.iter()
    }
}

// ---------------------------------------------------------------------------
// Mutable builder
// ---------------------------------------------------------------------------

/// Mutable builder for constructing a [`Registry`].
///
/// Used during IR lowering and prelude loading. Call [`build()`](Self::build)
/// to produce an immutable [`Registry`].
#[derive(Debug, Default)]
pub struct RegistryBuilder {
    base_dim_names: BTreeMap<BaseDimId, String>,
    base_dim_symbols: BTreeMap<BaseDimId, String>,

    dimensions: HashMap<DimName, Dimension>,
    units: HashMap<UnitRef, UnitInfo>,
    types: HashMap<StructTypeName, TypeDef>,
    ctors: HashMap<ConstructorName, StructTypeName>,
    indexes: HashMap<IndexName, IndexDef>,
    nat_ranges: HashMap<NatRangeIndex, IndexDef>,
    dags: HashMap<DeclName, DagDecl>,
    /// Base dimensions whose real-world units are affine (offset) scales,
    /// e.g. Temperature (°C, °F). User unit definitions on these dimensions
    /// are rejected because a purely multiplicative definition would display
    /// silently wrong values (#648 U4).
    affine_prone_dims: BTreeSet<BaseDimId>,
}

impl RegistryBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Freeze the builder into an immutable [`Registry`].
    ///
    /// # Panics
    ///
    /// Panics if a registered dimension or unit references a base dimension that
    /// has no registered display name. This indicates an internal registry
    /// construction bug: all semantic base-dimension IDs must be paired with
    /// explicit presentation metadata before formatting is possible.
    #[must_use]
    pub fn build(self) -> Registry {
        self.assert_base_dim_name_invariant();
        Registry {
            dimensions: DimensionRegistry {
                base_dim_names: self.base_dim_names,
                base_dim_symbols: self.base_dim_symbols,
                dimensions: self.dimensions,
            },
            units: UnitRegistry { units: self.units },
            types: TypeRegistry {
                types: self.types,
                ctors: self.ctors,
            },
            indexes: IndexRegistry {
                indexes: self.indexes,
                nat_ranges: self.nat_ranges,
            },
            dags: DagRegistry { dags: self.dags },
        }
    }

    fn assert_base_dim_name_invariant(&self) {
        for (name, dim) in &self.dimensions {
            assert_base_dim_names_cover(&self.base_dim_names, dim, &format!("dimension `{name}`"));
        }
        for (name, info) in &self.units {
            assert_base_dim_names_cover(
                &self.base_dim_names,
                &info.dimension,
                &format!("unit `{name}`"),
            );
        }
    }

    /// Register a `dag` declaration body keyed by the declaration's name.
    ///
    /// Accessed later during dim-checking of inline `@dag(args).out`
    /// expressions.
    pub fn register_dag(&mut self, name: DeclName, decl: DagDecl) {
        self.dags.insert(name, decl);
    }

    /// Merge every entry from a frozen [`Registry`] into this builder.
    ///
    /// Used by inline-dag compilation: the dag body is lowered as a virtual
    /// file whose registry is seeded with the enclosing file's dimensions,
    /// units, indexes, types, and sibling dags so that reference resolution and
    /// type checking behave as if the dag body were declared inline at the
    /// top level.
    pub fn merge_from_registry(&mut self, parent: &Registry) {
        for (id, name) in &parent.dimensions.base_dim_names {
            self.base_dim_names
                .entry(id.clone())
                .or_insert_with(|| name.clone());
        }
        for (id, symbol) in &parent.dimensions.base_dim_symbols {
            self.base_dim_symbols
                .entry(id.clone())
                .or_insert_with(|| symbol.clone());
        }
        for (name, dim) in &parent.dimensions.dimensions {
            self.dimensions
                .entry(name.clone())
                .or_insert_with(|| dim.clone());
        }
        for (name, info) in &parent.units.units {
            self.units
                .entry(name.clone())
                .or_insert_with(|| info.clone());
        }
        for (name, def) in &parent.types.types {
            self.types
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
        for (ctor, union_name) in &parent.types.ctors {
            self.ctors
                .entry(ctor.clone())
                .or_insert_with(|| union_name.clone());
        }
        for (name, def) in &parent.indexes.indexes {
            self.indexes
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
        for (index, def) in &parent.indexes.nat_ranges {
            self.nat_ranges.entry(*index).or_insert_with(|| def.clone());
        }
        for (name, decl) in &parent.dags.dags {
            self.dags
                .entry(name.clone())
                .or_insert_with(|| decl.clone());
        }
    }

    // -- Mutation methods (only on builder) --

    /// Register a new base dimension (`base dim Foo;`).
    ///
    /// The caller provides the [`BaseDimId`] which encodes the dimension's
    /// identity (prelude name or user-defined file+name).
    /// Mark a base dimension as affine-prone: its real-world units (e.g.
    /// Celsius/Fahrenheit on Temperature) need offset conversions that unit
    /// definitions cannot express, so user unit definitions on the bare
    /// dimension are rejected (#648 U4).
    pub fn mark_affine_prone(&mut self, id: BaseDimId) {
        self.affine_prone_dims.insert(id);
    }

    /// Returns `true` when `dim` is exactly an affine-prone base dimension
    /// (power 1). Compound dimensions involving the base (e.g.
    /// `Temperature / Time`) stay allowed: offsets cancel in differences.
    #[must_use]
    pub fn is_affine_prone(&self, dim: &Dimension) -> bool {
        let mut iter = dim.iter();
        let Some((id, &exp)) = iter.next() else {
            return false;
        };
        iter.next().is_none() && exp == Rational::ONE && self.affine_prone_dims.contains(id)
    }

    pub fn register_base_dimension(&mut self, name: DimName, id: BaseDimId) -> BaseDimId {
        let dim = Dimension::base(id.clone());
        self.base_dim_names.insert(id.clone(), name.to_string());
        self.dimensions.insert(name, dim);
        id
    }

    /// Register a new base dimension with an SI symbol.
    ///
    /// Same as `register_base_dimension` but also records the default unit symbol
    /// used for runtime display (e.g., `"m"` for Length).
    pub fn register_base_dimension_with_symbol(
        &mut self,
        name: DimName,
        id: BaseDimId,
        symbol: String,
    ) -> BaseDimId {
        let id = self.register_base_dimension(name, id);
        self.base_dim_symbols.insert(id.clone(), symbol);
        id
    }

    /// Record an SI symbol for an existing base dimension.
    ///
    /// Used when the first base unit for a user-defined dimension is declared
    /// (e.g., `base unit bit: Information;` records `"bit"` as the symbol).
    pub fn set_base_dim_symbol(&mut self, id: BaseDimId, symbol: String) {
        self.base_dim_symbols.entry(id).or_insert(symbol);
    }

    /// Register a named dimension.
    pub fn register_dimension(&mut self, name: DimName, dim: Dimension) {
        self.dimensions.insert(name, dim);
    }

    /// Register a named unit with its dimension and SI scale factor.
    pub fn register_unit(
        &mut self,
        name: impl Into<UnitRef>,
        dimension: Dimension,
        scale: PositiveFiniteScale,
    ) {
        self.units.insert(
            name.into(),
            UnitInfo {
                dimension,
                constness: UnitConstness::Const,
                scale: UnitScale::Static(scale),
            },
        );
    }

    /// Register a named unit with an explicitly specified scale and constness.
    pub fn register_unit_with_scale(
        &mut self,
        name: impl Into<UnitRef>,
        dimension: Dimension,
        scale: UnitScale,
        constness: UnitConstness,
    ) {
        self.units.insert(
            name.into(),
            UnitInfo {
                dimension,
                constness,
                scale,
            },
        );
    }

    /// Register a named runtime unit with a static or dynamic scale factor.
    pub fn register_unit_dynamic(
        &mut self,
        name: impl Into<UnitRef>,
        dimension: Dimension,
        scale: UnitScale,
    ) {
        self.register_unit_with_scale(name, dimension, scale, UnitConstness::Dynamic);
    }

    /// Register a type definition.
    ///
    /// For tagged unions (the common case), also populates the
    /// constructor namespace: each variant's name resolves back to the
    /// union it belongs to. Constructor collisions are detected here —
    /// the prelude is loaded first, so any later user-defined
    /// constructor that collides with a prelude or sibling type's
    /// constructor is silently ignored on the *second* registration
    /// (consistent with the type-name "first wins" behavior).
    pub fn register_type(&mut self, def: TypeDef) {
        if let TypeDefKind::Union { ref members } = def.kind {
            for member in members {
                // Last-wins, like every other register_* entry point —
                // duplicates are rejected upstream during declaration collection.
                self.ctors.insert(member.name.clone(), def.name.clone());
            }
        }
        self.types.insert(def.name.clone(), def);
    }

    /// Register an index definition.
    pub fn register_index(&mut self, def: IndexDef) {
        self.indexes.insert(def.name.clone(), def);
    }

    /// Ensure a typed Nat range index of the given size is registered.
    ///
    /// If the index already exists, this is a no-op.
    ///
    /// `size` is `NonZeroUsize` because empty indexes are not representable.
    /// AST-level `u64` literals must be checked at the boundary before
    /// reaching this entry point.
    pub fn ensure_nat_range_index(&mut self, size: NonZeroUsize) -> NatRangeIndex {
        let nat_range = NatRangeIndex::new(size);
        self.nat_ranges
            .entry(nat_range)
            .or_insert_with(|| IndexDef {
                name: nat_range.display_name(),
                kind: IndexKind::NatRange { size },
            });
        nat_range
    }

    // -- Read methods (needed during mid-build reads in ir.rs) --

    /// Look up a dimension by name.
    #[must_use]
    pub fn get_dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    /// Look up a unit by name.
    #[must_use]
    pub fn get_unit(&self, name: &UnitRef) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Iterate over all units: (reference, dimension, scale).
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitRef, &Dimension, &UnitScale)> {
        self.units
            .iter()
            .map(|(name, info)| (name, &info.dimension, &info.scale))
    }

    /// Look up a type definition by type name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Look up a declared index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    /// Look up a compiler-generated Nat range index by typed identity.
    #[must_use]
    pub fn get_nat_range(&self, index: NatRangeIndex) -> Option<&IndexDef> {
        self.nat_ranges.get(&index)
    }

    /// Get the base dimension names map (for display purposes).
    #[must_use]
    pub const fn base_dim_names(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_names
    }

    /// Get the base dimension symbols map for runtime display.
    #[must_use]
    pub const fn base_dim_symbols(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_symbols
    }

    /// Format a dimension as a human-readable string using registered base dimension names.
    ///
    /// Prefers a named dimension alias for compound dimensions, like
    /// [`DimensionRegistry::format_dimension`].
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format_dimension_preferring_alias(&self.dimensions, &self.base_dim_names, dim)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if any dimension name is unknown, and `Err` if
    /// dimension exponent arithmetic overflows `i32`.
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Result<Option<Dimension>, RationalError> {
        resolve_dim_expr_impl(&self.dimensions, expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if the type references unknown dimensions, and
    /// `Err` if dimension exponent arithmetic overflows `i32`.
    pub fn resolve_type_expr(
        &self,
        type_expr: &TypeExpr,
    ) -> Result<Option<Dimension>, RationalError> {
        resolve_type_expr_impl(&self.dimensions, type_expr)
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    ///
    /// # Errors
    ///
    /// Returns a [`UnitResolveError`] naming the unknown or dynamic-scale
    /// unit, or the exponent overflow.
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Result<(Dimension, f64), UnitResolveError> {
        resolve_unit_expr_impl(&self.units, expr)
    }

    /// Resolve a `UnitExpr` to its dimension only (ignoring scales).
    ///
    /// Works for both static and dynamic units.
    ///
    /// # Errors
    ///
    /// Returns a [`UnitResolveError`] naming the unknown unit, or the
    /// exponent overflow.
    pub fn resolve_unit_dimension(&self, expr: &UnitExpr) -> Result<Dimension, UnitResolveError> {
        resolve_unit_dimension_impl(&self.units, expr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::prelude::load_prelude;
    use crate::syntax::ast::{DimExprItem, DimTerm, UnitExprItem};
    use crate::syntax::dimension::BaseDimId;
    use crate::syntax::names::{NamePath, UnitName};
    use crate::syntax::span::Span;
    use crate::syntax::span::Spanned;

    // Well-known IDs matching prelude dimension names.
    fn length_id() -> BaseDimId {
        BaseDimId::Prelude("Length".to_string())
    }
    fn time_id() -> BaseDimId {
        BaseDimId::Prelude("Time".to_string())
    }
    fn mass_id() -> BaseDimId {
        BaseDimId::Prelude("Mass".to_string())
    }

    fn make_registry() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.build()
    }

    fn make_dim_term_name(name: &str) -> Spanned<NamePath> {
        Spanned::new(NamePath::from(name), Span::new(0, 0))
    }

    /// Create a simple dimension `TypeExpr` from a name string.
    fn make_dim_type_expr(name: &str) -> TypeExpr {
        use crate::syntax::ast::{DimExpr, DimExprItem, DimTerm};
        TypeExpr {
            kind: TypeExprKind::DimExpr(DimExpr {
                terms: vec![DimExprItem {
                    op: MulDivOp::Mul,
                    term: DimTerm {
                        name: make_dim_term_name(name),
                        power: None,
                        span: Span::new(0, 0),
                    },
                }],
                span: Span::new(0, 0),
            }),
            constraints: vec![],
            span: Span::new(0, 0),
        }
    }

    fn make_unit_name(name: &str) -> Spanned<UnitRef> {
        Spanned::new(UnitRef::local(UnitName::new(name)), Span::new(0, 0))
    }

    #[test]
    fn registry_base_dimensions() {
        let r = make_registry();
        assert_eq!(
            r.dimensions.get_dimension("Length"),
            Some(&Dimension::base(length_id()))
        );
        assert_eq!(
            r.dimensions.get_dimension("Time"),
            Some(&Dimension::base(time_id()))
        );
        assert_eq!(
            r.dimensions.get_dimension("Mass"),
            Some(&Dimension::base(mass_id()))
        );
    }

    #[test]
    fn registry_derived_dimensions() {
        let r = make_registry();
        let velocity = r.dimensions.get_dimension("Velocity").unwrap();
        let expected = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        assert_eq!(*velocity, expected);
    }

    #[test]
    fn registry_base_units() {
        let r = make_registry();
        let m = r.units.get_unit(&UnitRef::local("m")).unwrap();
        assert_eq!(m.dimension, Dimension::base(length_id()));
        assert!((m.scale.as_static().unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_derived_units() {
        let r = make_registry();
        let km = r.units.get_unit(&UnitRef::local("km")).unwrap();
        assert_eq!(km.dimension, Dimension::base(length_id()));
        assert!((km.scale.as_static().unwrap() - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_dim_expr_velocity() {
        let r = make_registry();
        // Length / Time
        let expr = DimExpr {
            terms: vec![
                DimExprItem {
                    op: MulDivOp::Mul,
                    term: DimTerm {
                        name: make_dim_term_name("Length"),
                        power: None,
                        span: Span::new(0, 0),
                    },
                },
                DimExprItem {
                    op: MulDivOp::Div,
                    term: DimTerm {
                        name: make_dim_term_name("Time"),
                        power: None,
                        span: Span::new(0, 0),
                    },
                },
            ],
            span: Span::new(0, 0),
        };
        let dim = r.dimensions.resolve_dim_expr(&expr).unwrap().unwrap();
        let expected = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        assert_eq!(dim, expected);
    }

    #[test]
    fn resolve_unit_expr_m_per_s_squared() {
        let r = make_registry();
        // m / s^2
        let expr = UnitExpr {
            terms: vec![
                UnitExprItem {
                    op: MulDivOp::Mul,
                    name: make_unit_name("m"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_unit_name("s"),
                    power: Some(Rational::from_int(2)),
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.units.resolve_unit_expr(&expr).unwrap();
        let expected_dim = (Dimension::base(length_id())
            / Dimension::base(time_id()).pow_int(2).unwrap())
        .unwrap();
        assert_eq!(dim, expected_dim);
        assert!((scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_unit_expr_km_per_hour() {
        let r = make_registry();
        // km / hour
        let expr = UnitExpr {
            terms: vec![
                UnitExprItem {
                    op: MulDivOp::Mul,
                    name: make_unit_name("km"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_unit_name("hour"),
                    power: None,
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.units.resolve_unit_expr(&expr).unwrap();
        let expected_dim = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        assert_eq!(dim, expected_dim);
        // km/hour = 1000 m / 3600 s ≈ 0.2778 m/s
        assert!((scale - 1000.0 / 3600.0).abs() < 1e-10);
    }

    #[test]
    fn registry_type_register_and_lookup() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        // Record-shaped types are single-variant unions whose sole
        // constructor's name matches the type's name.
        b.register_type(TypeDef {
            name: StructTypeName::new("TransferResult"),
            generic_params: vec![],
            kind: TypeDefKind::Union {
                members: vec![UnionMemberDef {
                    name: ConstructorName::new("TransferResult"),
                    fields: vec![
                        StructField {
                            name: FieldName::new("dv1"),
                            type_ann: make_dim_type_expr("Velocity"),
                        },
                        StructField {
                            name: FieldName::new("dv2"),
                            type_ann: make_dim_type_expr("Velocity"),
                        },
                    ],
                }],
            },
        });
        let r = b.build();
        let velocity_dim = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        let def = r.types.get_type("TransferResult").unwrap();
        assert_eq!(def.name.as_str(), "TransferResult");
        assert!(def.is_union());
        let fields = def.record_fields().expect("single-variant collision");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name.as_str(), "dv1");
        assert_eq!(
            r.dimensions.resolve_type_expr(&fields[0].type_ann),
            Ok(Some(velocity_dim))
        );
        assert!(r.types.get_type("NonExistent").is_none());
    }

    #[test]
    fn registry_index_register_and_lookup() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.register_index(IndexDef {
            name: IndexName::new("Maneuver"),
            kind: IndexKind::Named {
                variants: vec![
                    IndexVariantName::new("Departure"),
                    IndexVariantName::new("Correction"),
                    IndexVariantName::new("Insertion"),
                ],
            },
        });
        let r = b.build();
        let def = r.indexes.get_index("Maneuver").unwrap();
        assert_eq!(def.name.as_str(), "Maneuver");
        let variants = def.variants();
        let variant_strs: Vec<&str> = variants.iter().map(IndexVariantName::as_str).collect();
        assert_eq!(variant_strs, vec!["Departure", "Correction", "Insertion"]);
        assert!(r.indexes.get_index("NonExistent").is_none());
    }

    #[test]
    #[should_panic(expected = "registry invariant violation")]
    fn registry_build_panics_when_dimension_base_name_is_missing() {
        let mut b = RegistryBuilder::new();
        b.register_dimension(DimName::new("Broken"), Dimension::base(length_id()));

        let _ = b.build();
    }

    #[test]
    fn register_user_defined_base_dimension() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let info_id = BaseDimId::UserDefined {
            dag: crate::dag_id::DagId::root_in_package("test", "test"),
            name: "Information".to_string(),
        };
        let id = b.register_base_dimension(DimName::new("Information"), info_id.clone());
        assert_eq!(id, info_id);
        let r = b.build();
        // Should be retrievable
        let dim = r.dimensions.get_dimension("Information").unwrap();
        assert_eq!(*dim, Dimension::base(id.clone()));
        // Name should be recorded
        assert_eq!(
            r.dimensions.base_dim_names().get(&id),
            Some(&"Information".to_string())
        );
    }

    #[test]
    fn register_base_dimension_with_symbol() {
        let mut b = RegistryBuilder::new();
        let id = b.register_base_dimension_with_symbol(
            DimName::new("Length"),
            BaseDimId::Prelude("Length".to_string()),
            "m".to_string(),
        );
        let r = b.build();
        assert_eq!(
            r.dimensions.base_dim_symbols().get(&id),
            Some(&"m".to_string())
        );
    }

    #[test]
    fn set_base_dim_symbol_only_first() {
        let mut b = RegistryBuilder::new();
        let info_id = BaseDimId::UserDefined {
            dag: crate::dag_id::DagId::root_in_package("test", "test"),
            name: "Information".to_string(),
        };
        let id = b.register_base_dimension(DimName::new("Information"), info_id);
        b.set_base_dim_symbol(id.clone(), "bit".to_string());
        // Second call should not overwrite
        b.set_base_dim_symbol(id.clone(), "byte".to_string());
        let r = b.build();
        assert_eq!(
            r.dimensions.base_dim_symbols().get(&id),
            Some(&"bit".to_string())
        );
    }
}
