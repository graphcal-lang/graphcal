use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroUsize;

use crate::desugar::desugared_ast::{DagDecl, DimExpr, TypeExpr, UnitExpr};
use crate::dimension::{BaseDimId, Dimension, Rational, RationalError};
use crate::registry::dimension_registry::{
    DimensionResolveError, assert_base_dim_names_cover,
    format_dimension_preferring_alias_after_validation, resolve_dim_expr_detailed_impl,
    resolve_dim_expr_impl, resolve_type_expr_impl,
};
use crate::registry::unit::{resolve_unit_dimension_impl, resolve_unit_expr_impl};
use crate::syntax::ast::UnitConstness;
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::type_name::{ConstructorName, StructTypeName};

pub use super::dag::DagRegistry;
pub use super::dimension_registry::{DimensionRegistry, RegistryBuildError};
pub use super::index::{
    IndexDef, IndexKind, IndexRegistry, NatRangeIndex, NatRangeIndexError, RangeIndexData,
};
pub use super::type_def::{
    StructField, TypeDef, TypeDefKind, TypeGenericConstraint, TypeGenericParam, TypeRegistry,
    UnionMemberDef,
};
pub use super::unit::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitInfo, UnitRegistry, UnitResolveError,
    UnitScale, pow_scale,
};

// ---------------------------------------------------------------------------
// Frozen aggregate registry
// ---------------------------------------------------------------------------

/// The frozen, read-only aggregate of all domain registries.
///
/// Produced by [`RegistryBuilder::try_build`]. All fields are public so that
/// consumers can access individual domain registries directly.
#[derive(Debug, Clone)]
pub struct Registry {
    pub dimensions: DimensionRegistry,
    pub units: UnitRegistry,
    pub types: TypeRegistry,
    pub indexes: IndexRegistry,
    pub dags: DagRegistry,
}

// ---------------------------------------------------------------------------
// Mutable builder
// ---------------------------------------------------------------------------

/// Mutable builder for constructing a [`Registry`].
///
/// Used during IR lowering and prelude loading. Call [`try_build()`](Self::try_build)
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
    /// # Errors
    ///
    /// Returns [`RegistryBuildError`] if a registered dimension or unit
    /// references a base dimension that has no registered display name.
    pub fn try_build(self) -> Result<Registry, RegistryBuildError> {
        self.assert_base_dim_name_invariant()?;
        Ok(Registry {
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
        })
    }

    fn assert_base_dim_name_invariant(&self) -> Result<(), RegistryBuildError> {
        for (name, dim) in &self.dimensions {
            assert_base_dim_names_cover(&self.base_dim_names, dim, format!("dimension `{name}`"))?;
        }
        for (name, info) in &self.units {
            assert_base_dim_names_cover(
                &self.base_dim_names,
                &info.dimension,
                format!("unit `{name}`"),
            )?;
        }
        Ok(())
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
    /// union it belongs to. Constructor collisions are rejected upstream
    /// during declaration collection, so this low-level registry overwrites
    /// by key like the other `register_*` helpers.
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
        format_dimension_preferring_alias_after_validation(
            &self.dimensions,
            &self.base_dim_names,
            dim,
        )
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if any dimension name is unknown, and `Err` if
    /// dimension exponent arithmetic overflows `i32`.
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Result<Option<Dimension>, RationalError> {
        resolve_dim_expr_impl(&self.dimensions, expr)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`, preserving the
    /// unknown referenced dimension name in the error.
    pub fn resolve_dim_expr_detailed(
        &self,
        expr: &DimExpr,
    ) -> Result<Dimension, DimensionResolveError> {
        resolve_dim_expr_detailed_impl(&self.dimensions, expr)
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
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::dimension::BaseDimId;
    use crate::registry::prelude::load_prelude;
    use crate::syntax::ast::{DimExprItem, DimTerm, MulDivOp, UnitExprItem};
    use crate::syntax::dimension::UnitName;
    use crate::syntax::index_name::IndexVariantName;
    use crate::syntax::names::NamePath;
    use crate::syntax::span::Span;
    use crate::syntax::span::Spanned;
    use crate::syntax::type_name::FieldName;

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
        b.try_build().unwrap()
    }

    fn make_dim_term_name(name: &str) -> Spanned<NamePath> {
        Spanned::new(NamePath::expect_local(name), Span::new(0, 0))
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
        Spanned::new(
            UnitRef::local(UnitName::expect_valid(name)),
            Span::new(0, 0),
        )
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
        let m = r
            .units
            .get_unit(&UnitRef::local(UnitName::expect_valid("m")))
            .unwrap();
        assert_eq!(m.dimension, Dimension::base(length_id()));
        assert!((m.scale.as_static().unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_derived_units() {
        let r = make_registry();
        let km = r
            .units
            .get_unit(&UnitRef::local(UnitName::expect_valid("km")))
            .unwrap();
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
                    power: Some(Rational::from(2)),
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.units.resolve_unit_expr(&expr).unwrap();
        let expected_dim =
            (Dimension::base(length_id()) / Dimension::base(time_id()).pow(2).unwrap()).unwrap();
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
    fn resolve_unit_expr_rejects_non_finite_compound_scale() {
        let r = make_registry();
        let expr = UnitExpr {
            terms: vec![UnitExprItem {
                op: MulDivOp::Mul,
                name: make_unit_name("km"),
                power: Some(Rational::from(400)),
            }],
            span: Span::new(0, 0),
        };
        let err = r.units.resolve_unit_expr(&expr).unwrap_err();
        assert!(
            matches!(
                err,
                UnitResolveError::InvalidScale {
                    reason: PositiveFiniteScaleError::NonFinite,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn registry_type_register_and_lookup() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        // Record-shaped types are single-variant unions whose sole
        // constructor's name matches the type's name.
        b.register_type(TypeDef {
            name: StructTypeName::expect_valid("TransferResult"),
            generic_params: vec![],
            kind: TypeDefKind::Union {
                members: vec![UnionMemberDef {
                    name: ConstructorName::expect_valid("TransferResult"),
                    fields: vec![
                        StructField {
                            name: FieldName::expect_valid("dv1"),
                            type_ann: make_dim_type_expr("Velocity"),
                        },
                        StructField {
                            name: FieldName::expect_valid("dv2"),
                            type_ann: make_dim_type_expr("Velocity"),
                        },
                    ],
                }],
            },
        });
        let r = b.try_build().unwrap();
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
            name: IndexName::expect_valid("Maneuver"),
            kind: IndexKind::Named {
                variants: vec![
                    IndexVariantName::expect_valid("Departure"),
                    IndexVariantName::expect_valid("Correction"),
                    IndexVariantName::expect_valid("Insertion"),
                ],
            },
        });
        let r = b.try_build().unwrap();
        let def = r.indexes.get_index("Maneuver").unwrap();
        assert_eq!(def.name.as_str(), "Maneuver");
        let variants = def.variants();
        let variant_strs: Vec<&str> = variants.iter().map(IndexVariantName::as_str).collect();
        assert_eq!(variant_strs, vec!["Departure", "Correction", "Insertion"]);
        assert!(r.indexes.get_index("NonExistent").is_none());
    }

    #[test]
    fn registry_try_build_reports_missing_dimension_base_name() {
        let mut b = RegistryBuilder::new();
        b.register_dimension(
            DimName::expect_valid("Broken"),
            Dimension::base(length_id()),
        );

        let err = b.try_build().unwrap_err();
        assert_eq!(
            err,
            RegistryBuildError::MissingBaseDimensionName {
                context: "dimension `Broken`".to_string(),
                id: length_id(),
            }
        );
    }

    #[test]
    fn register_user_defined_base_dimension() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let info_id = BaseDimId::UserDefined {
            dag: crate::dag_id::DagId::root_in_package("test", "test"),
            name: "Information".to_string(),
        };
        let id = b.register_base_dimension(DimName::expect_valid("Information"), info_id.clone());
        assert_eq!(id, info_id);
        let r = b.try_build().unwrap();
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
            DimName::expect_valid("Length"),
            BaseDimId::Prelude("Length".to_string()),
            "m".to_string(),
        );
        let r = b.try_build().unwrap();
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
        let id = b.register_base_dimension(DimName::expect_valid("Information"), info_id);
        b.set_base_dim_symbol(id.clone(), "bit".to_string());
        // Second call should not overwrite
        b.set_base_dim_symbol(id.clone(), "byte".to_string());
        let r = b.try_build().unwrap();
        assert_eq!(
            r.dimensions.base_dim_symbols().get(&id),
            Some(&"bit".to_string())
        );
    }
}
