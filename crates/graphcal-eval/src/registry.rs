use std::collections::{BTreeMap, HashMap};

use graphcal_syntax::ast::{
    DimExpr, FnBody, GenericConstraint, MulDivOp, TypeExpr, TypeExprKind, UnitExpr,
};
use graphcal_syntax::dimension::{BaseDimId, Dimension, Rational};
use graphcal_syntax::names::{
    DimName, FieldName, FnName, GenericParamName, IndexName, StructTypeName, UnitName, VariantName,
};
use graphcal_syntax::span::Span;

// ---------------------------------------------------------------------------
// Data types (unchanged)
// ---------------------------------------------------------------------------

/// Information about a registered unit.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    /// The dimension this unit measures.
    pub dimension: Dimension,
    /// Scale factor to convert 1 of this unit to base SI units.
    /// e.g., km -> 1000.0 (1 km = 1000 m)
    pub scale: f64,
}

/// A field in a type variant definition.
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: FieldName,
    pub type_ann: TypeExpr,
}

/// A variant within a type definition.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: VariantName,
    pub fields: Vec<StructField>,
}

/// The constraint on a generic parameter of a type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeGenericConstraint {
    /// `D: Dim` — the generic stands for a dimension.
    Dim,
    /// `I: Index` — the generic stands for an index.
    Index,
    /// `F: Type` — unconstrained phantom type parameter.
    Unconstrained,
}

impl From<GenericConstraint> for TypeGenericConstraint {
    fn from(c: GenericConstraint) -> Self {
        match c {
            GenericConstraint::Dim => Self::Dim,
            GenericConstraint::Index => Self::Index,
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
    pub default: Option<graphcal_syntax::ast::TypeExpr>,
}

/// A type definition: may have zero variants (empty/marker type),
/// one variant (struct sugar), or multiple variants (tagged union).
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: StructTypeName,
    pub generic_params: Vec<TypeGenericParam>,
    pub derives: Vec<graphcal_syntax::ast::DeriveOp>,
    pub variants: Vec<VariantDef>,
}

impl TypeDef {
    /// Returns true if this is a single-variant type (struct sugar).
    #[must_use]
    pub const fn is_single_variant(&self) -> bool {
        self.variants.len() == 1
    }

    /// Look up a variant by name.
    #[must_use]
    pub fn get_variant(&self, name: &str) -> Option<&VariantDef> {
        self.variants.iter().find(|v| v.name.as_str() == name)
    }
}

/// A user-defined function parameter.
#[derive(Debug, Clone)]
pub struct FnParamDef {
    pub name: String,
    pub type_expr: TypeExpr,
}

/// A generic parameter on a user-defined function, with its constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnGenericConstraint {
    /// `D: Dim` — the generic stands for a dimension.
    Dim,
    /// `I: Index` — the generic stands for an index.
    Index,
}

/// A generic parameter with name and constraint.
#[derive(Debug, Clone)]
pub struct FnGenericParam {
    pub name: GenericParamName,
    pub constraint: FnGenericConstraint,
}

/// A user-defined function stored in the registry.
#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: FnName,
    pub generic_params: Vec<FnGenericParam>,
    pub params: Vec<FnParamDef>,
    pub return_type_expr: TypeExpr,
    pub body: FnBody,
    pub span: Span,
}

/// The kind of an index: either named variants or a numeric range.
#[derive(Debug, Clone)]
pub enum IndexKind {
    /// A named label set, e.g. `index Maneuver = { Departure, Correction, Insertion };`
    Named { variants: Vec<VariantName> },
    /// A numeric range, e.g. `index T = range(0.0 s, 100.0 s, step: 0.1 s);`
    Range {
        start: f64,
        end: f64,
        step: f64,
        dimension: Dimension,
        /// Display unit label (e.g., `"s"`) for formatting step values.
        display_label: Option<String>,
        /// Scale factor from SI to display unit: `display_value = si_value / scale`.
        display_scale: f64,
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
    #[must_use]
    pub fn variants(&self) -> Vec<VariantName> {
        match &self.kind {
            IndexKind::Named { variants } => variants.clone(),
            IndexKind::Range { .. } => {
                let count = self.step_count();
                (0..count)
                    .map(|i| VariantName::new(format!("#{i}")))
                    .collect()
            }
        }
    }

    /// Returns the number of steps/variants in this index.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range is validated: start <= end, step > 0"
    )]
    pub fn step_count(&self) -> usize {
        match &self.kind {
            IndexKind::Named { variants } => variants.len(),
            IndexKind::Range {
                start, end, step, ..
            } => (((end - start) / step).round() as usize) + 1,
        }
    }

    /// Returns the SI value at step `i` for a range index.
    ///
    /// # Errors
    ///
    /// Returns an error message if this is a named (non-range) index.
    #[expect(
        clippy::cast_precision_loss,
        reason = "range step indices are small enough for exact f64 representation"
    )]
    pub fn step_value(&self, i: usize) -> Result<f64, String> {
        match &self.kind {
            IndexKind::Named { .. } => Err(format!(
                "step_value() called on named index `{}`",
                self.name
            )),
            IndexKind::Range { start, step, .. } => Ok(start + (i as f64) * step),
        }
    }

    /// Returns true if this is a range index.
    #[must_use]
    pub const fn is_range(&self) -> bool {
        matches!(self.kind, IndexKind::Range { .. })
    }

    /// Returns true if this is a named index.
    #[must_use]
    pub const fn is_named(&self) -> bool {
        matches!(self.kind, IndexKind::Named { .. })
    }
}

// ---------------------------------------------------------------------------
// Domain-specific registries (frozen / read-only)
// ---------------------------------------------------------------------------

/// Dimension registry: maps dimension names to `Dimension` values and tracks
/// base dimension metadata (ID assignment, names, SI symbols).
#[derive(Debug)]
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
        let mut result = Dimension::dimensionless();
        for item in &expr.terms {
            let base = self.dimensions.get(item.term.name.name.as_str())?;
            let exp = item.term.power.unwrap_or(1);
            let powered = base.pow(Rational::from_int(exp));
            result = match item.op {
                MulDivOp::Mul => result * powered,
                MulDivOp::Div => result / powered,
            };
        }
        Some(result)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `None` if the type references unknown dimensions.
    #[must_use]
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Option<Dimension> {
        match &type_expr.kind {
            TypeExprKind::Dimensionless => Some(Dimension::dimensionless()),
            TypeExprKind::Bool | TypeExprKind::Int | TypeExprKind::TypeApplication { .. } => None,
            TypeExprKind::DimExpr(dim_expr) => self.resolve_dim_expr(dim_expr),
            TypeExprKind::Indexed { base, .. } => self.resolve_type_expr(base),
        }
    }
}

/// Unit registry: maps unit names to `UnitInfo` (dimension + scale).
#[derive(Debug)]
pub struct UnitRegistry {
    units: HashMap<UnitName, UnitInfo>,
}

impl UnitRegistry {
    /// Look up a unit by name.
    #[must_use]
    pub fn get_unit(&self, name: &str) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Resolve a `UnitExpr` to its dimension and compound scale factor.
    ///
    /// Returns `None` if any unit name is unknown.
    #[must_use]
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Option<(Dimension, f64)> {
        let mut dim = Dimension::dimensionless();
        let mut scale = 1.0;
        for item in &expr.terms {
            let info = self.units.get(item.name.value.as_str())?;
            let exp = item.power.unwrap_or(1);
            let powered_dim = info.dimension.pow(Rational::from_int(exp));
            let powered_scale = info.scale.powi(exp);
            match item.op {
                MulDivOp::Mul => {
                    dim = dim * powered_dim;
                    scale *= powered_scale;
                }
                MulDivOp::Div => {
                    dim = dim / powered_dim;
                    scale /= powered_scale;
                }
            }
        }
        Some((dim, scale))
    }
}

/// Type registry: maps type names to `TypeDef` and provides variant reverse lookup.
#[derive(Debug)]
pub struct TypeRegistry {
    types: HashMap<StructTypeName, TypeDef>,
    /// Reverse lookup: variant name → owning type name.
    variant_to_type: HashMap<VariantName, StructTypeName>,
}

impl TypeRegistry {
    /// Look up a type definition by type name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Look up a type definition and specific variant by variant name.
    #[must_use]
    pub fn get_type_by_variant(&self, variant_name: &str) -> Option<(&TypeDef, &VariantDef)> {
        let type_name = self.variant_to_type.get(variant_name)?;
        let type_def = self.types.get(type_name.as_str())?;
        let variant_def = type_def.get_variant(variant_name)?;
        Some((type_def, variant_def))
    }

    /// Iterate over all registered type definitions.
    pub fn all_types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values()
    }
}

/// Function registry: maps function names to `FnDef`.
#[derive(Debug)]
pub struct FunctionRegistry {
    functions: HashMap<FnName, FnDef>,
}

impl FunctionRegistry {
    /// Look up a user-defined function by name.
    #[must_use]
    pub fn get_function(&self, name: &str) -> Option<&FnDef> {
        self.functions.get(name)
    }

    /// Iterate over all user-defined functions.
    pub fn all_functions(&self) -> impl Iterator<Item = &FnDef> {
        self.functions.values()
    }
}

/// Index registry: maps index names to `IndexDef`.
#[derive(Debug)]
pub struct IndexRegistry {
    indexes: HashMap<IndexName, IndexDef>,
}

impl IndexRegistry {
    /// Look up an index definition by name.
    #[must_use]
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(name)
    }
}

// ---------------------------------------------------------------------------
// Frozen aggregate registry
// ---------------------------------------------------------------------------

/// The frozen, read-only aggregate of all domain registries.
///
/// Produced by [`RegistryBuilder::build`]. All fields are public so that
/// consumers can access individual domain registries directly.
#[derive(Debug)]
pub struct Registry {
    pub dimensions: DimensionRegistry,
    pub units: UnitRegistry,
    pub types: TypeRegistry,
    pub functions: FunctionRegistry,
    pub indexes: IndexRegistry,
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
    /// Counter for assigning unique `BaseDimId` values.
    next_base_dim_id: u32,
    base_dim_names: BTreeMap<BaseDimId, String>,
    base_dim_symbols: BTreeMap<BaseDimId, String>,

    dimensions: HashMap<DimName, Dimension>,
    units: HashMap<UnitName, UnitInfo>,
    types: HashMap<StructTypeName, TypeDef>,
    variant_to_type: HashMap<VariantName, StructTypeName>,
    functions: HashMap<FnName, FnDef>,
    indexes: HashMap<IndexName, IndexDef>,
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
                variant_to_type: self.variant_to_type,
            },
            functions: FunctionRegistry {
                functions: self.functions,
            },
            indexes: IndexRegistry {
                indexes: self.indexes,
            },
        }
    }

    // -- Mutation methods (only on builder) --

    /// Register a new base dimension (bodyless `dimension Foo;`).
    ///
    /// Assigns the next available `BaseDimId`, creates a singleton `Dimension`,
    /// and records the name for display purposes.
    pub fn register_base_dimension(&mut self, name: DimName) -> BaseDimId {
        let id = BaseDimId(self.next_base_dim_id);
        self.next_base_dim_id += 1;
        let dim = Dimension::base(id);
        self.base_dim_names.insert(id, name.to_string());
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
        symbol: String,
    ) -> BaseDimId {
        let id = self.register_base_dimension(name);
        self.base_dim_symbols.insert(id, symbol);
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
        self.units.insert(name, UnitInfo { dimension, scale });
    }

    /// Register a type definition (single-variant struct sugar or multi-variant tagged union).
    pub fn register_type(&mut self, def: TypeDef) {
        for variant in &def.variants {
            self.variant_to_type
                .insert(variant.name.clone(), def.name.clone());
        }
        self.types.insert(def.name.clone(), def);
    }

    /// Register a user-defined function.
    pub fn register_function(&mut self, def: FnDef) {
        self.functions.insert(def.name.clone(), def);
    }

    /// Register an index definition.
    pub fn register_index(&mut self, def: IndexDef) {
        self.indexes.insert(def.name.clone(), def);
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
        let mut result = Dimension::dimensionless();
        for item in &expr.terms {
            let base = self.dimensions.get(item.term.name.name.as_str())?;
            let exp = item.term.power.unwrap_or(1);
            let powered = base.pow(Rational::from_int(exp));
            result = match item.op {
                MulDivOp::Mul => result * powered,
                MulDivOp::Div => result / powered,
            };
        }
        Some(result)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `None` if the type references unknown dimensions.
    #[must_use]
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Option<Dimension> {
        match &type_expr.kind {
            TypeExprKind::Dimensionless => Some(Dimension::dimensionless()),
            TypeExprKind::Bool | TypeExprKind::Int | TypeExprKind::TypeApplication { .. } => None,
            TypeExprKind::DimExpr(dim_expr) => self.resolve_dim_expr(dim_expr),
            TypeExprKind::Indexed { base, .. } => self.resolve_type_expr(base),
        }
    }

    /// Resolve a `UnitExpr` to its dimension and compound scale factor.
    ///
    /// Returns `None` if any unit name is unknown.
    #[must_use]
    pub fn resolve_unit_expr(&self, expr: &UnitExpr) -> Option<(Dimension, f64)> {
        let mut dim = Dimension::dimensionless();
        let mut scale = 1.0;
        for item in &expr.terms {
            let info = self.units.get(item.name.value.as_str())?;
            let exp = item.power.unwrap_or(1);
            let powered_dim = info.dimension.pow(Rational::from_int(exp));
            let powered_scale = info.scale.powi(exp);
            match item.op {
                MulDivOp::Mul => {
                    dim = dim * powered_dim;
                    scale *= powered_scale;
                }
                MulDivOp::Div => {
                    dim = dim / powered_dim;
                    scale /= powered_scale;
                }
            }
        }
        Some((dim, scale))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use crate::prelude::load_prelude;
    use graphcal_syntax::ast::{DimExprItem, DimTerm, Ident, UnitExprItem};
    use graphcal_syntax::dimension::BaseDimId;
    use graphcal_syntax::names::Spanned;
    use graphcal_syntax::span::Span;

    // Well-known IDs matching prelude registration order.
    const LENGTH_ID: BaseDimId = BaseDimId(0);
    const TIME_ID: BaseDimId = BaseDimId(1);
    const MASS_ID: BaseDimId = BaseDimId(2);

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
        use graphcal_syntax::ast::{DimExpr, DimExprItem, DimTerm};
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
            Some(&Dimension::base(LENGTH_ID))
        );
        assert_eq!(
            r.dimensions.get_dimension("Time"),
            Some(&Dimension::base(TIME_ID))
        );
        assert_eq!(
            r.dimensions.get_dimension("Mass"),
            Some(&Dimension::base(MASS_ID))
        );
    }

    #[test]
    fn registry_derived_dimensions() {
        let r = make_registry();
        let velocity = r.dimensions.get_dimension("Velocity").unwrap();
        let expected = Dimension::base(LENGTH_ID) / Dimension::base(TIME_ID);
        assert_eq!(*velocity, expected);
    }

    #[test]
    fn registry_base_units() {
        let r = make_registry();
        let m = r.units.get_unit("m").unwrap();
        assert_eq!(m.dimension, Dimension::base(LENGTH_ID));
        assert!((m.scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_derived_units() {
        let r = make_registry();
        let km = r.units.get_unit("km").unwrap();
        assert_eq!(km.dimension, Dimension::base(LENGTH_ID));
        assert!((km.scale - 1000.0).abs() < f64::EPSILON);
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
        let expected = Dimension::base(LENGTH_ID) / Dimension::base(TIME_ID);
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
        let expected_dim = Dimension::base(LENGTH_ID) / Dimension::base(TIME_ID).pow_int(2);
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
        let expected_dim = Dimension::base(LENGTH_ID) / Dimension::base(TIME_ID);
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
            derives: vec![],
            variants: vec![VariantDef {
                name: VariantName::new("TransferResult"),
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
        });
        let r = b.build();
        let velocity_dim = Dimension::base(LENGTH_ID) / Dimension::base(TIME_ID);
        let def = r.types.get_type("TransferResult").unwrap();
        assert_eq!(def.name.as_str(), "TransferResult");
        assert!(def.is_single_variant());
        let variant = def.get_variant("TransferResult").unwrap();
        assert_eq!(variant.fields.len(), 2);
        assert_eq!(variant.fields[0].name.as_str(), "dv1");
        assert_eq!(
            r.dimensions.resolve_type_expr(&variant.fields[0].type_ann),
            Some(velocity_dim)
        );
        assert!(r.types.get_type("NonExistent").is_none());

        // variant_to_type reverse lookup
        let (type_def, variant_def) = r.types.get_type_by_variant("TransferResult").unwrap();
        assert_eq!(type_def.name.as_str(), "TransferResult");
        assert_eq!(variant_def.name.as_str(), "TransferResult");
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
        let id = b.register_base_dimension(DimName::new("Information"));
        // Should get the next ID after the 8 prelude base dimensions
        assert_eq!(id, BaseDimId(8));
        let r = b.build();
        // Should be retrievable
        let dim = r.dimensions.get_dimension("Information").unwrap();
        assert_eq!(*dim, Dimension::base(id));
        // Name should be recorded
        assert_eq!(
            r.dimensions.base_dim_names().get(&id),
            Some(&"Information".to_string())
        );
    }

    #[test]
    fn register_base_dimension_with_symbol() {
        let mut b = RegistryBuilder::new();
        let id = b.register_base_dimension_with_symbol(DimName::new("Length"), "m".to_string());
        let r = b.build();
        assert_eq!(
            r.dimensions.base_dim_symbols().get(&id),
            Some(&"m".to_string())
        );
    }

    #[test]
    fn set_base_dim_symbol_only_first() {
        let mut b = RegistryBuilder::new();
        let id = b.register_base_dimension(DimName::new("Information"));
        b.set_base_dim_symbol(id, "bit".to_string());
        // Second call should not overwrite
        b.set_base_dim_symbol(id, "byte".to_string());
        let r = b.build();
        assert_eq!(
            r.dimensions.base_dim_symbols().get(&id),
            Some(&"bit".to_string())
        );
    }
}
