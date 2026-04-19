use std::collections::{BTreeMap, HashMap};

use crate::syntax::ast::{
    DagDecl, DimExpr, Expr, GenericConstraint, MulDivOp, TypeExpr, TypeExprKind, UnitExpr,
};
use crate::syntax::dimension::{BaseDimId, Dimension, Rational};
use crate::syntax::names::{
    DimName, FieldName, GenericParamName, IndexName, StructTypeName, UnitName, VariantName,
};
// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// How a unit's scale factor is determined.
#[derive(Debug, Clone)]
pub enum UnitScale {
    /// Scale factor known at compile time (e.g., `unit km: Length = 1000 m;`).
    Static(f64),
    /// Scale factor depends on runtime values (e.g., `unit EUR: Money = (@rate) USD;`).
    ///
    /// The final SI scale = `eval(scale_expr) * base_unit_scale`.
    Dynamic {
        /// The unevaluated scale expression containing `@`-references.
        scale_expr: Expr,
        /// The scale factor of the base unit in the definition (resolved at compile time).
        /// For `(@rate) USD` where USD has scale 1.0, this is 1.0.
        base_unit_scale: f64,
    },
}

impl UnitScale {
    /// Returns the static scale factor, or `None` if the scale is dynamic.
    #[must_use]
    pub const fn as_static(&self) -> Option<f64> {
        match self {
            Self::Static(s) => Some(*s),
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

/// A member of a union type, with optional type arguments for generic members.
#[derive(Debug, Clone)]
pub struct UnionMemberDef {
    pub name: StructTypeName,
    pub type_args: Vec<TypeExpr>,
}

/// The kind of a type definition.
#[derive(Debug, Clone)]
pub enum TypeDefKind {
    /// A unit type with no fields: `type Coasting;`
    Unit,
    /// A record type with fields: `type TransferResult { dv1: Velocity, dv2: Velocity }`
    Record { fields: Vec<StructField> },
    /// A union type: `type ManeuverKind = Impulsive | Coasting;`
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
    pub default: Option<crate::syntax::ast::TypeExpr>,
}

/// A type definition: unit type, record type, or union type.
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: StructTypeName,
    pub generic_params: Vec<TypeGenericParam>,
    pub kind: TypeDefKind,
}

impl TypeDef {
    /// Returns the fields if this is a record type.
    #[must_use]
    pub fn fields(&self) -> &[StructField] {
        match &self.kind {
            TypeDefKind::Record { fields } => fields,
            TypeDefKind::Unit | TypeDefKind::Union { .. } => &[],
        }
    }

    /// Returns the union members if this is a union type.
    #[must_use]
    pub fn union_members(&self) -> Option<&[UnionMemberDef]> {
        match &self.kind {
            TypeDefKind::Union { members } => Some(members),
            TypeDefKind::Unit | TypeDefKind::Record { .. } => None,
        }
    }

    /// Returns true if this is a union type.
    #[must_use]
    pub const fn is_union(&self) -> bool {
        matches!(self.kind, TypeDefKind::Union { .. })
    }

    /// Returns true if this is a record type (has fields).
    #[must_use]
    pub const fn is_record(&self) -> bool {
        matches!(self.kind, TypeDefKind::Record { .. })
    }

    /// Returns true if this is a unit type (no fields, no members).
    #[must_use]
    pub const fn is_unit(&self) -> bool {
        matches!(self.kind, TypeDefKind::Unit)
    }
}

/// Data for a concrete numeric range index (e.g., `linspace(0.0 s, 100.0 s, step: 0.1 s)`).
#[derive(Debug, Clone)]
pub struct RangeIndexData {
    pub start: f64,
    pub end: f64,
    pub step: f64,
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range is validated: start <= end, step > 0, so n >= 0"
    )]
    pub fn step_count(&self) -> usize {
        let n = (self.end - self.start) / self.step;
        // Use round() to avoid off-by-one from floating-point imprecision
        // (e.g., 0.3 / 0.1 = 2.9999... should give 3, not 2).
        n.round() as usize + 1
    }
}

/// The kind of an index: either named variants or a numeric range.
#[derive(Debug, Clone)]
pub enum IndexKind {
    /// A named label set, e.g. `index Maneuver = { Departure, Correction, Insertion };`
    Named { variants: Vec<VariantName> },
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
        /// The size of the range (number of elements).
        size: u64,
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
    pub fn variants(&self) -> Vec<VariantName> {
        match &self.kind {
            IndexKind::Named { variants } => variants.clone(),
            IndexKind::Range(data) => {
                let count = data.step_count();
                (0..count)
                    .map(|i| VariantName::new(format!("#{i}")))
                    .collect()
            }
            IndexKind::NatRange { size } => (0..*size)
                .map(|i| VariantName::new(format!("#{i}")))
                .collect(),
            IndexKind::RequiredNamed | IndexKind::RequiredRange { .. } => vec![],
        }
    }

    /// Returns the number of steps/variants in this index.
    ///
    /// Returns 0 for required indexes (no variants until bound).
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "NatRange size u64 -> usize: practically bounded by registry validation"
    )]
    pub fn step_count(&self) -> usize {
        match &self.kind {
            IndexKind::Named { variants } => variants.len(),
            IndexKind::Range(data) => data.step_count(),
            IndexKind::NatRange { size } => *size as usize,
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
            IndexKind::NatRange { size } => Some(*size),
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

/// Generate the synthetic index name for a nat range of given size.
///
/// E.g., `nat_range_index_name(3)` → `"__nat_range_3"`.
#[must_use]
pub fn nat_range_index_name(size: u64) -> String {
    format!("__nat_range_{size}")
}

/// Check if an index name is a synthetic nat range index and extract its size.
#[must_use]
pub fn parse_nat_range_index_name(name: &str) -> Option<u64> {
    name.strip_prefix("__nat_range_")
        .and_then(|s| s.parse().ok())
}

// ---------------------------------------------------------------------------
// Private helper functions for resolution logic
// ---------------------------------------------------------------------------

/// Shared implementation for resolving a `DimExpr` to a concrete `Dimension`.
fn resolve_dim_expr_impl(
    dimensions: &HashMap<DimName, Dimension>,
    expr: &DimExpr,
) -> Option<Dimension> {
    expr.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |result, item| {
            let base = dimensions.get(item.term.name.name.as_str())?;
            let exp = item.term.power.unwrap_or(1);
            let powered = base.pow(Rational::from_int(exp));
            Some(match item.op {
                MulDivOp::Mul => result * powered,
                MulDivOp::Div => result / powered,
            })
        })
}

/// Shared implementation for resolving a `TypeExpr` to a concrete `Dimension`.
fn resolve_type_expr_impl(
    dimensions: &HashMap<DimName, Dimension>,
    type_expr: &TypeExpr,
) -> Option<Dimension> {
    match &type_expr.kind {
        TypeExprKind::Dimensionless => Some(Dimension::dimensionless()),
        TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::TypeApplication { .. } => None,
        TypeExprKind::DimExpr(dim_expr) => resolve_dim_expr_impl(dimensions, dim_expr),
        TypeExprKind::Indexed { base, .. } => resolve_type_expr_impl(dimensions, base),
    }
}

/// Shared implementation for resolving a `UnitExpr` to its dimension and static scale factor.
///
/// Returns `None` if any unit name is unknown or if any unit has a dynamic scale.
fn resolve_unit_expr_impl(
    units: &HashMap<UnitName, UnitInfo>,
    expr: &UnitExpr,
) -> Option<(Dimension, f64)> {
    expr.terms
        .iter()
        .try_fold((Dimension::dimensionless(), 1.0), |(dim, scale), item| {
            let info = units.get(item.name.value.as_str())?;
            let exp = item.power.unwrap_or(1);
            let powered_dim = info.dimension.pow(Rational::from_int(exp));
            let static_scale = info.scale.as_static()?;
            let powered_scale = static_scale.powi(exp);
            Some(match item.op {
                MulDivOp::Mul => (dim * powered_dim, scale * powered_scale),
                MulDivOp::Div => (dim / powered_dim, scale / powered_scale),
            })
        })
}

/// Shared implementation for resolving a `UnitExpr` to its dimension only (ignoring scales).
///
/// Works for both static and dynamic units. Returns `None` if any unit name is unknown.
fn resolve_unit_dimension_impl(
    units: &HashMap<UnitName, UnitInfo>,
    expr: &UnitExpr,
) -> Option<Dimension> {
    expr.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |dim, item| {
            let info = units.get(item.name.value.as_str())?;
            let exp = item.power.unwrap_or(1);
            let powered_dim = info.dimension.pow(Rational::from_int(exp));
            Some(match item.op {
                MulDivOp::Mul => dim * powered_dim,
                MulDivOp::Div => dim / powered_dim,
            })
        })
}

// ---------------------------------------------------------------------------
// Domain-specific registries (frozen / read-only)
// ---------------------------------------------------------------------------

/// Dimension registry: maps dimension names to `Dimension` values and tracks
/// base dimension metadata (ID assignment, names, SI symbols).
#[derive(Debug, Clone)]
pub struct DimensionRegistry {
    /// Base dimension ID → dimension name (for display).
    base_dim_names: BTreeMap<BaseDimId, String>,
    /// Base dimension ID → SI unit symbol (for `si_unit_string()`).
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

    /// Get the base dimension symbols map (for SI unit string formatting).
    #[must_use]
    pub const fn base_dim_symbols(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_symbols
    }

    /// Format a dimension as a human-readable string using registered base dimension names.
    ///
    /// Returns `"Dimensionless"` for dimensionless, or names like `"Length / Time"`.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format!("{}", dim.display_with(&self.base_dim_names))
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `None` if any dimension name is unknown.
    #[must_use]
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Option<Dimension> {
        resolve_dim_expr_impl(&self.dimensions, expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `None` if the type references unknown dimensions.
    #[must_use]
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Option<Dimension> {
        resolve_type_expr_impl(&self.dimensions, type_expr)
    }
}

/// Unit registry: maps unit names to `UnitInfo` (dimension + scale).
#[derive(Debug, Clone)]
pub struct UnitRegistry {
    units: HashMap<UnitName, UnitInfo>,
}

impl UnitRegistry {
    /// Look up a unit by name.
    #[must_use]
    pub fn get_unit(&self, name: &str) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Iterate over all units: (name, dimension, scale).
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitName, &Dimension, &UnitScale)> {
        self.units
            .iter()
            .map(|(name, info)| (name, &info.dimension, &info.scale))
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    ///
    /// Returns `None` if any unit name is unknown or if any unit has a dynamic scale.
    #[must_use]
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Option<(Dimension, f64)> {
        resolve_unit_expr_impl(&self.units, expr)
    }

    /// Resolve a `UnitExpr` to its dimension only (ignoring scales).
    ///
    /// Works for both static and dynamic units. Returns `None` if any unit name is unknown.
    #[must_use]
    pub fn resolve_unit_dimension(&self, expr: &UnitExpr) -> Option<Dimension> {
        resolve_unit_dimension_impl(&self.units, expr)
    }
}

/// Type registry: maps type names to `TypeDef` and provides union membership lookup.
#[derive(Debug, Clone)]
pub struct TypeRegistry {
    types: HashMap<StructTypeName, TypeDef>,
    /// Reverse lookup: member type name → union type names it belongs to.
    type_to_unions: HashMap<StructTypeName, Vec<StructTypeName>>,
}

impl TypeRegistry {
    /// Look up a type definition by type name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Check if `member_name` is a member of the union type `union_name`.
    #[must_use]
    pub fn is_member_of_union(&self, member_name: &str, union_name: &str) -> bool {
        self.type_to_unions
            .get(member_name)
            .is_some_and(|unions| unions.iter().any(|u| u.as_str() == union_name))
    }

    /// Get the union types that `member_name` belongs to.
    #[must_use]
    pub fn get_unions_for_type(&self, member_name: &str) -> &[StructTypeName] {
        self.type_to_unions
            .get(member_name)
            .map_or(&[], Vec::as_slice)
    }

    /// Iterate over all registered type definitions.
    pub fn all_types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values()
    }
}

/// Index registry: maps index names to `IndexDef`.
#[derive(Debug, Clone)]
pub struct IndexRegistry {
    indexes: HashMap<IndexName, IndexDef>,
}

impl IndexRegistry {
    /// Look up an index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    /// Iterate over all index definitions.
    pub fn all_indexes(&self) -> impl Iterator<Item = &IndexDef> {
        self.indexes.values()
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
/// invocations `@dag(args)::out` against the called `dag`'s `pub param` and
/// `pub node` signatures.
#[derive(Debug, Default, Clone)]
pub struct DagRegistry {
    dags: HashMap<String, DagDecl>,
}

impl DagRegistry {
    /// Return the AST body of the named `dag`, if one is declared in this file.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DagDecl> {
        self.dags.get(name)
    }

    /// Iterate over all registered dags.
    pub fn all_dags(&self) -> impl Iterator<Item = (&String, &DagDecl)> {
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
    units: HashMap<UnitName, UnitInfo>,
    types: HashMap<StructTypeName, TypeDef>,
    type_to_unions: HashMap<StructTypeName, Vec<StructTypeName>>,
    indexes: HashMap<IndexName, IndexDef>,
    dags: HashMap<String, DagDecl>,
}

impl RegistryBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Freeze the builder into an immutable [`Registry`].
    #[must_use]
    pub fn build(self) -> Registry {
        Registry {
            dimensions: DimensionRegistry {
                base_dim_names: self.base_dim_names,
                base_dim_symbols: self.base_dim_symbols,
                dimensions: self.dimensions,
            },
            units: UnitRegistry { units: self.units },
            types: TypeRegistry {
                types: self.types,
                type_to_unions: self.type_to_unions,
            },
            indexes: IndexRegistry {
                indexes: self.indexes,
            },
            dags: DagRegistry { dags: self.dags },
        }
    }

    /// Register a `dag` declaration body keyed by the declaration's name.
    ///
    /// Accessed later during dim-checking of inline `@dag(args)::out`
    /// expressions.
    pub fn register_dag(&mut self, name: String, decl: DagDecl) {
        self.dags.insert(name, decl);
    }

    /// Merge every entry from a frozen [`Registry`] into this builder.
    ///
    /// Used by inline-dag compilation: the dag body is lowered as a virtual
    /// file whose registry is seeded with the enclosing file's dimensions,
    /// units, indexes, types, and sibling dags so that name resolution and
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
        for (member, unions) in &parent.types.type_to_unions {
            self.type_to_unions
                .entry(member.clone())
                .or_insert_with(|| unions.clone());
        }
        for (name, def) in &parent.indexes.indexes {
            self.indexes
                .entry(name.clone())
                .or_insert_with(|| def.clone());
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
    pub fn register_base_dimension(&mut self, name: DimName, id: BaseDimId) -> BaseDimId {
        let dim = Dimension::base(id.clone());
        self.base_dim_names.insert(id.clone(), name.to_string());
        self.dimensions.insert(name, dim);
        id
    }

    /// Register a new base dimension with an SI symbol.
    ///
    /// Same as `register_base_dimension` but also records the unit symbol
    /// used in `si_unit_string()` output (e.g., `"m"` for Length).
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
    /// (e.g., `unit bit: Information;` records `"bit"` as the symbol).
    pub fn set_base_dim_symbol(&mut self, id: BaseDimId, symbol: String) {
        self.base_dim_symbols.entry(id).or_insert(symbol);
    }

    /// Register a named dimension.
    pub fn register_dimension(&mut self, name: DimName, dim: Dimension) {
        self.dimensions.insert(name, dim);
    }

    /// Register a named unit with its dimension and SI scale factor.
    pub fn register_unit(&mut self, name: UnitName, dimension: Dimension, scale: f64) {
        self.units.insert(
            name,
            UnitInfo {
                dimension,
                scale: UnitScale::Static(scale),
            },
        );
    }

    /// Register a named unit with a dynamic scale factor.
    pub fn register_unit_dynamic(
        &mut self,
        name: UnitName,
        dimension: Dimension,
        scale: UnitScale,
    ) {
        self.units.insert(name, UnitInfo { dimension, scale });
    }

    /// Register a type definition (single-variant struct sugar or multi-variant tagged union).
    pub fn register_type(&mut self, def: TypeDef) {
        if let TypeDefKind::Union { ref members } = def.kind {
            for member in members {
                self.type_to_unions
                    .entry(member.name.clone())
                    .or_default()
                    .push(def.name.clone());
            }
        }
        self.types.insert(def.name.clone(), def);
    }

    /// Register an index definition.
    pub fn register_index(&mut self, def: IndexDef) {
        self.indexes.insert(def.name.clone(), def);
    }

    /// Ensure a synthetic nat range index of the given size is registered.
    ///
    /// Returns the synthetic index name (e.g., `__nat_range_3`).
    /// If the index already exists, this is a no-op.
    pub fn ensure_nat_range_index(&mut self, size: u64) -> IndexName {
        let name = IndexName::new(nat_range_index_name(size));
        self.indexes
            .entry(name.clone())
            .or_insert_with(|| IndexDef {
                name: name.clone(),
                kind: IndexKind::NatRange { size },
            });
        name
    }

    // -- Read methods (needed during mid-build reads in ir.rs) --

    /// Look up a dimension by name.
    #[must_use]
    pub fn get_dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    /// Look up a unit by name.
    #[must_use]
    pub fn get_unit(&self, name: &str) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Iterate over all units: (name, dimension, scale).
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitName, &Dimension, &UnitScale)> {
        self.units
            .iter()
            .map(|(name, info)| (name, &info.dimension, &info.scale))
    }

    /// Look up a type definition by type name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Look up an index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }

    /// Get the base dimension names map (for display purposes).
    #[must_use]
    pub const fn base_dim_names(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_names
    }

    /// Get the base dimension symbols map (for SI unit string formatting).
    #[must_use]
    pub const fn base_dim_symbols(&self) -> &BTreeMap<BaseDimId, String> {
        &self.base_dim_symbols
    }

    /// Format a dimension as a human-readable string using registered base dimension names.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format!("{}", dim.display_with(&self.base_dim_names))
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `None` if any dimension name is unknown.
    #[must_use]
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Option<Dimension> {
        resolve_dim_expr_impl(&self.dimensions, expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `None` if the type references unknown dimensions.
    #[must_use]
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Option<Dimension> {
        resolve_type_expr_impl(&self.dimensions, type_expr)
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    ///
    /// Returns `None` if any unit name is unknown or if any unit has a dynamic scale.
    #[must_use]
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Option<(Dimension, f64)> {
        resolve_unit_expr_impl(&self.units, expr)
    }

    /// Resolve a `UnitExpr` to its dimension only (ignoring scales).
    ///
    /// Works for both static and dynamic units. Returns `None` if any unit name is unknown.
    #[must_use]
    pub fn resolve_unit_dimension(&self, expr: &UnitExpr) -> Option<Dimension> {
        resolve_unit_dimension_impl(&self.units, expr)
    }
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
    use crate::syntax::ast::{DimExprItem, DimTerm, Ident, UnitExprItem};
    use crate::syntax::dimension::BaseDimId;
    use crate::syntax::names::Spanned;
    use crate::syntax::span::Span;

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
        load_prelude(&mut b);
        b.build()
    }

    fn make_ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        }
    }

    /// Create a simple dimension `TypeExpr` from a name string.
    fn make_dim_type_expr(name: &str) -> TypeExpr {
        use crate::syntax::ast::{DimExpr, DimExprItem, DimTerm};
        TypeExpr {
            kind: TypeExprKind::DimExpr(DimExpr {
                terms: vec![DimExprItem {
                    op: MulDivOp::Mul,
                    term: DimTerm {
                        name: make_ident(name),
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

    fn make_unit_name(name: &str) -> Spanned<UnitName> {
        Spanned::new(UnitName::new(name), Span::new(0, 0))
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
        let expected = Dimension::base(length_id()) / Dimension::base(time_id());
        assert_eq!(*velocity, expected);
    }

    #[test]
    fn registry_base_units() {
        let r = make_registry();
        let m = r.units.get_unit("m").unwrap();
        assert_eq!(m.dimension, Dimension::base(length_id()));
        assert!((m.scale.as_static().unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_derived_units() {
        let r = make_registry();
        let km = r.units.get_unit("km").unwrap();
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
                        name: make_ident("Length"),
                        power: None,
                        span: Span::new(0, 0),
                    },
                },
                DimExprItem {
                    op: MulDivOp::Div,
                    term: DimTerm {
                        name: make_ident("Time"),
                        power: None,
                        span: Span::new(0, 0),
                    },
                },
            ],
            span: Span::new(0, 0),
        };
        let dim = r.dimensions.resolve_dim_expr(&expr).unwrap();
        let expected = Dimension::base(length_id()) / Dimension::base(time_id());
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
                    power: Some(2),
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.units.resolve_unit_expr(&expr).unwrap();
        let expected_dim = Dimension::base(length_id()) / Dimension::base(time_id()).pow_int(2);
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
        let expected_dim = Dimension::base(length_id()) / Dimension::base(time_id());
        assert_eq!(dim, expected_dim);
        // km/hour = 1000 m / 3600 s ≈ 0.2778 m/s
        assert!((scale - 1000.0 / 3600.0).abs() < 1e-10);
    }

    #[test]
    fn registry_type_register_and_lookup() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        b.register_type(TypeDef {
            name: StructTypeName::new("TransferResult"),
            generic_params: vec![],
            kind: TypeDefKind::Record {
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
            },
        });
        let r = b.build();
        let velocity_dim = Dimension::base(length_id()) / Dimension::base(time_id());
        let def = r.types.get_type("TransferResult").unwrap();
        assert_eq!(def.name.as_str(), "TransferResult");
        assert!(def.is_record());
        let fields = def.fields();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name.as_str(), "dv1");
        assert_eq!(
            r.dimensions.resolve_type_expr(&fields[0].type_ann),
            Some(velocity_dim)
        );
        assert!(r.types.get_type("NonExistent").is_none());
    }

    #[test]
    fn registry_index_register_and_lookup() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        b.register_index(IndexDef {
            name: IndexName::new("Maneuver"),
            kind: IndexKind::Named {
                variants: vec![
                    VariantName::new("Departure"),
                    VariantName::new("Correction"),
                    VariantName::new("Insertion"),
                ],
            },
        });
        let r = b.build();
        let def = r.indexes.get_index("Maneuver").unwrap();
        assert_eq!(def.name.as_str(), "Maneuver");
        let variants = def.variants();
        let variant_strs: Vec<&str> = variants.iter().map(VariantName::as_str).collect();
        assert_eq!(variant_strs, vec!["Departure", "Correction", "Insertion"]);
        assert!(r.indexes.get_index("NonExistent").is_none());
    }

    #[test]
    fn register_user_defined_base_dimension() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        let info_id = BaseDimId::UserDefined {
            dag: crate::syntax::dag_id::DagId::new(["test"]),
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
            dag: crate::syntax::dag_id::DagId::new(["test"]),
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
