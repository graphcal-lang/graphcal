use std::collections::HashMap;

use crate::desugar::desugared_ast::{DagDecl, DimExpr, TypeExpr, UnitExpr};
use crate::dimension::{BaseDimId, Dimension};
use crate::ratio::RatioError;
use crate::registry::aliased_table::{AliasCycle, AliasedTable};
use crate::registry::dimension_table::DimensionResolveError;
use crate::registry::unit::{resolve_unit_dimension_impl, resolve_unit_expr_impl};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimRef, UnitName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::type_name::{ConstructorName, StructTypeName};

use super::time_zone::TimeZoneRegistry;

pub use super::dag::DagRegistry;
pub use super::dimension_table::{BaseDimensionInfo, DimensionFormattingRegistry, DimensionTable};
pub use super::index::{
    ConcreteIndexKind, CoordinateDisplayUnit, CoordinateIndexData, CoordinateIndexError,
    CoordinateSpacing, FiniteIndex, IndexBindingCategory, IndexBindingContract,
    IndexBindingContractError, IndexBindingTarget, IndexCardinality, IndexCardinalityError,
    IndexCategory, IndexDef, IndexKind, IndexRegistry, MAX_INDEX_CARDINALITY, RequiredIndexKind,
};
pub use super::type_def::{
    StructField, TypeDef, TypeDefError, TypeDefKind, TypeGenericParam, TypeRegistry, UnionMemberDef,
};
pub(crate) use super::unit::{BaseUnitRegistrationError, UnitResolveError};
pub use super::unit::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitInfo, UnitRegistry, UnitScale,
    UnitScaleStepError, UnitScaleTerm, try_fold_unit_scale,
};

// ---------------------------------------------------------------------------
// Frozen aggregate registry
// ---------------------------------------------------------------------------

/// The frozen, read-only aggregate of all domain registries.
///
/// Produced by [`RegistryBuilder::build`]. All fields are public so that
/// consumers can access individual domain registries directly.
#[derive(Debug, Clone)]
pub struct Registry {
    pub dimensions: DimensionTable,
    pub units: UnitRegistry,
    pub types: TypeRegistry,
    pub indexes: IndexRegistry,
    /// Reproducible IANA timezone lookup backed by the bundled, pinned tzdb.
    pub time_zones: TimeZoneRegistry,
    pub(crate) dags: DagRegistry,
}

impl Registry {
    /// Consume the frontend registry after HIR lowering has replaced nominal
    /// syntax definitions with [`crate::hir::NominalTypeRegistry`].
    #[must_use]
    pub fn into_semantic(self) -> SemanticRegistry {
        SemanticRegistry {
            dimensions: self.dimensions,
            units: self.units,
            indexes: self.indexes,
            time_zones: self.time_zones,
        }
    }
}

/// Registry capabilities valid from HIR onward.
///
/// Nominal definitions are intentionally absent: canonical HIR nominal types
/// are stored separately, so later phases cannot consult syntax-backed type
/// definitions as a competing authority.
#[derive(Debug, Clone)]
pub struct SemanticRegistry {
    pub dimensions: DimensionTable,
    pub units: UnitRegistry,
    pub indexes: IndexRegistry,
    /// Reproducible IANA timezone lookup backed by the bundled, pinned tzdb.
    pub time_zones: TimeZoneRegistry,
}

impl SemanticRegistry {
    /// Discard all source-name lookup services at the TIR boundary.
    #[must_use]
    pub fn into_formatting(self) -> FormattingRegistry {
        FormattingRegistry {
            dimensions: self.dimensions.into_formatting(),
            time_zones: self.time_zones,
        }
    }
}

/// Post-resolution services retained by checked TIR and evaluation.
///
/// This registry exposes only dimension formatting and reproducible timezone
/// validation. Canonical semantic lookups live exclusively in
/// [`crate::tir::typed::ProjectTypeStore`].
#[derive(Debug, Clone)]
pub struct FormattingRegistry {
    pub dimensions: DimensionFormattingRegistry,
    /// Reproducible timezone parsing/validation data used at input boundaries.
    pub time_zones: TimeZoneRegistry,
}

// ---------------------------------------------------------------------------
// Mutable builder
// ---------------------------------------------------------------------------

/// Mutable builder for constructing a [`Registry`].
///
/// Used during IR lowering and prelude loading. Call [`build()`](Self::build)
/// to produce an immutable [`Registry`].
#[derive(Debug, Default, Clone)]
pub struct RegistryBuilder {
    dimensions: DimensionTable,
    units: AliasedTable<UnitRef, UnitInfo>,
    types: AliasedTable<StructTypeName, TypeDef>,
    ctors: HashMap<ConstructorName, StructTypeName>,
    indexes: AliasedTable<IndexBindingTarget, IndexDef>,
    dags: HashMap<DeclName, DagDecl>,
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
            dimensions: self.dimensions,
            units: UnitRegistry { units: self.units },
            types: TypeRegistry {
                types: self.types,
                ctors: self.ctors,
            },
            indexes: IndexRegistry {
                indexes: self.indexes,
            },
            time_zones: TimeZoneRegistry::bundled(),
            dags: DagRegistry { dags: self.dags },
        }
    }

    /// Register a `dag` declaration body keyed by the declaration's name.
    ///
    /// Accessed later during dim-checking of inline `@dag(args)::out`
    /// expressions.
    pub(crate) fn register_dag(&mut self, name: DeclName, decl: DagDecl) {
        self.dags.insert(name, decl);
    }

    /// Merge every entry from a frozen [`Registry`] into this builder.
    ///
    /// Used by inline-dag compilation: the dag body is lowered as a virtual
    /// file whose registry is seeded with the enclosing file's dimensions,
    /// units, indexes, types, and sibling dags so that reference resolution and
    /// type checking behave as if the dag body were declared inline at the
    /// top level.
    pub(crate) fn merge_from_registry(&mut self, parent: &Registry) {
        self.dimensions.merge_missing_from(&parent.dimensions);
        self.units.merge_missing_from(&parent.units.units);
        self.types.merge_missing_from(&parent.types.types);
        for (ctor, union_name) in &parent.types.ctors {
            self.ctors
                .entry(ctor.clone())
                .or_insert_with(|| union_name.clone());
        }
        self.indexes.merge_missing_from(&parent.indexes.indexes);
        for (name, decl) in &parent.dags.dags {
            self.dags
                .entry(name.clone())
                .or_insert_with(|| decl.clone());
        }
    }

    // -- Mutation methods (only on builder) --

    /// Mark a base dimension as affine-prone: its real-world units (e.g.
    /// Celsius/Fahrenheit on Temperature) need offset conversions that unit
    /// definitions cannot express, so user unit definitions on the bare
    /// dimension are rejected (#648 U4).
    pub(crate) fn mark_affine_prone(&mut self, id: BaseDimId) {
        self.dimensions.mark_affine_prone(id);
    }

    /// Returns `true` when `dim` is exactly an affine-prone base dimension
    /// (power 1). Compound dimensions involving the base (e.g.
    /// `Temperature / Time`) stay allowed: offsets cancel in differences.
    #[must_use]
    pub(crate) fn is_affine_prone(&self, dim: &Dimension) -> bool {
        self.dimensions.is_affine_prone(dim)
    }

    /// Register a new base dimension (`base dim Foo;`), source-visible under
    /// the leaf name carried by its [`BaseDimId`].
    pub fn register_base_dimension(&mut self, id: BaseDimId) {
        self.dimensions.register_base_dimension(id);
    }

    /// Record a base dimension without making it source-visible.
    ///
    /// Imported dimensions and units may be built from a dependency's private
    /// base dimensions; the importer tracks them but must not be able to name
    /// them.
    pub fn record_base_dimension(&mut self, id: BaseDimId) {
        self.dimensions.record_base_dimension(id);
    }

    /// Copy a dependency's metadata for one base dimension without making it
    /// source-visible, keeping metadata this builder already has.
    pub fn import_base_dimension(&mut self, id: BaseDimId, info: &BaseDimensionInfo) {
        self.dimensions.import_base_dimension(id, info);
    }

    /// Register a new base dimension together with its canonical unit symbol
    /// used for runtime display (e.g., `m` for Length).
    pub(crate) fn register_base_dimension_with_symbol(
        &mut self,
        id: BaseDimId,
        symbol: UnitName,
    ) -> BaseDimId {
        self.dimensions.register_base_dimension(id.clone());
        self.dimensions
            .import_base_dimension(id.clone(), &BaseDimensionInfo::with_canonical_unit(symbol));
        id
    }

    /// Register the one canonical unit of a base dimension.
    ///
    /// This operation updates the display metadata and unit registry atomically,
    /// so source lowering cannot install a scale-1 alias without proving that the
    /// dimension is a bare base dimension with no canonical unit yet.
    pub(crate) fn register_base_unit(
        &mut self,
        name: UnitName,
        dimension: Dimension,
    ) -> Result<(), BaseUnitRegistrationError> {
        let base_dimension = dimension
            .base_dimension_id()
            .cloned()
            .ok_or(BaseUnitRegistrationError::NotBaseDimension)?;
        self.dimensions
            .register_canonical_unit(base_dimension, name.clone())
            .map_err(|error| BaseUnitRegistrationError::AlreadyRegistered {
                existing: error.existing,
            })?;
        self.register_unit(name, dimension, PositiveFiniteScale::ONE);
        Ok(())
    }

    /// Register a named dimension under a local or module-qualified reference.
    pub fn register_dimension(&mut self, name: impl Into<DimRef>, dim: Dimension) {
        self.dimensions.register_dimension(name.into(), dim);
    }

    /// Register a source-visible dimension alias without changing identity.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCycle`] when `target` already resolves through `alias`.
    pub fn register_dimension_alias(
        &mut self,
        alias: DimRef,
        target: DimRef,
    ) -> Result<(), AliasCycle<DimRef>> {
        self.dimensions.register_dimension_alias(alias, target)
    }

    /// Register a named compile-time unit with its dimension and SI scale factor.
    pub(crate) fn register_unit(
        &mut self,
        name: impl Into<UnitRef>,
        dimension: Dimension,
        scale: PositiveFiniteScale,
    ) {
        self.register_unit_with_scale(name, dimension, UnitScale::Const(scale));
    }

    /// Register a named unit with an explicitly specified scale (which also
    /// determines its constness).
    pub fn register_unit_with_scale(
        &mut self,
        name: impl Into<UnitRef>,
        dimension: Dimension,
        scale: UnitScale,
    ) {
        self.units
            .insert(name.into(), UnitInfo { dimension, scale });
    }

    /// Register a source-visible unit alias without changing scale identity.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCycle`] when `target` already resolves through `alias`.
    pub fn register_unit_alias(
        &mut self,
        alias: UnitRef,
        target: UnitRef,
    ) -> Result<(), AliasCycle<UnitRef>> {
        self.units.insert_alias(alias, target)
    }

    /// Register a type definition.
    ///
    /// For tagged unions (the common case), also populates the
    /// constructor namespace: each variant's name resolves back to the
    /// union it belongs to. Constructor collisions are rejected upstream
    /// during declaration collection, so this low-level registry overwrites
    /// by key like the other `register_*` helpers.
    pub fn register_type(&mut self, def: TypeDef) {
        if let Some(members) = def.union_members() {
            for member in members {
                // Constructor and payload-field uniqueness were checked when
                // the TypeDef was built; registration only installs indexes.
                self.ctors.insert(member.name().clone(), def.name().clone());
            }
        }
        self.types.insert(def.name().clone(), def);
    }

    /// Register a source-visible type alias without changing nominal identity.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCycle`] when `target` already resolves through `alias`.
    pub fn register_type_alias(
        &mut self,
        alias: StructTypeName,
        target: StructTypeName,
    ) -> Result<(), AliasCycle<StructTypeName>> {
        self.types.insert_alias(alias, target)
    }

    /// Register a declared index definition.
    pub fn register_index(&mut self, name: IndexName, kind: IndexKind) {
        let target = IndexBindingTarget::Declared(name);
        self.indexes
            .insert(target.clone(), IndexDef { name: target, kind });
    }

    /// Register a source-visible index alias to a declared or structural target.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCycle`] when `target` already resolves through `alias`.
    pub fn register_index_alias(
        &mut self,
        alias: IndexName,
        target: IndexBindingTarget,
    ) -> Result<(), AliasCycle<IndexBindingTarget>> {
        self.indexes
            .insert_alias(IndexBindingTarget::Declared(alias), target)
    }

    /// Ensure that a concrete structural `Fin(N)` definition exists.
    ///
    /// If the index already exists, this is a no-op.
    pub fn ensure_finite_index(&mut self, cardinality: IndexCardinality) -> FiniteIndex {
        let finite_index = FiniteIndex::new(cardinality);
        self.indexes
            .insert_if_missing(IndexBindingTarget::Finite(finite_index), || {
                IndexDef::finite(finite_index)
            });
        finite_index
    }

    // -- Read methods (needed during mid-build reads in ir.rs) --

    /// Look up a possibly module-qualified dimension reference, following
    /// source-visible aliases.
    #[must_use]
    pub fn get_dimension(&self, reference: &DimRef) -> Option<&Dimension> {
        self.dimensions.get_dimension(reference)
    }

    /// Look up a unit by reference, following source-visible aliases.
    #[must_use]
    pub fn get_unit(&self, name: &UnitRef) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Iterate over every unit reference and its complete semantic definition.
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitRef, &UnitInfo)> {
        self.units.iter()
    }

    /// Look up a type definition by source-visible name, following aliases.
    #[must_use]
    pub fn get_type(&self, name: &StructTypeName) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Look up a declared index definition by source-visible name, following
    /// aliases.
    #[must_use]
    pub fn get_index(&self, name: &IndexName) -> Option<&IndexDef> {
        self.indexes
            .get(&IndexBindingTarget::Declared(name.clone()))
    }

    /// Look up a compiler-generated structural index by typed identity.
    #[must_use]
    pub fn get_finite_index(&self, index: FiniteIndex) -> Option<&IndexDef> {
        self.indexes.get_defined(&IndexBindingTarget::Finite(index))
    }

    /// Format a dimension as a human-readable string.
    ///
    /// Prefers a named dimension alias for compound dimensions, like
    /// [`DimensionTable::format_dimension`].
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        self.dimensions.format_dimension(dim)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if any dimension name is unknown, and `Err` if
    /// dimension exponent arithmetic overflows `i32`.
    pub fn resolve_dim_expr(&self, expr: &DimExpr) -> Result<Option<Dimension>, RatioError> {
        self.dimensions.resolve_dim_expr(expr)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`, preserving the
    /// unknown referenced dimension name in the error.
    pub fn resolve_dim_expr_detailed(
        &self,
        expr: &DimExpr,
    ) -> Result<Dimension, DimensionResolveError> {
        self.dimensions.resolve_dim_expr_detailed(expr)
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if the type references unknown dimensions, and
    /// `Err` if dimension exponent arithmetic overflows `i32`.
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Result<Option<Dimension>, RatioError> {
        self.dimensions.resolve_type_expr(type_expr)
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    ///
    /// # Errors
    ///
    /// Returns a [`UnitResolveError`] naming the unknown or dynamic-scale
    /// unit, or the exponent overflow.
    pub(crate) fn resolve_unit_expr(
        &self,
        expr: &UnitExpr,
    ) -> Result<(Dimension, PositiveFiniteScale), UnitResolveError> {
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
    use crate::dimension::{BaseDimId, Rational};
    use crate::registry::prelude::load_prelude;
    use crate::syntax::ast::{DimExprItem, DimTerm, MulDivOp, UnitExprItem};
    use crate::syntax::dimension::{DimName, UnitName};
    use crate::syntax::index_name::IndexVariantName;
    use crate::syntax::names::NamePath;
    use crate::syntax::non_empty::{NonEmpty, NonEmptyUnique};

    fn named_index_kind<const N: usize>(labels: [&str; N]) -> IndexKind {
        let labels = NonEmpty::try_from(labels.map(IndexVariantName::expect_valid)).unwrap();
        IndexKind::Concrete(ConcreteIndexKind::Named {
            variants: NonEmptyUnique::try_from_non_empty(labels).unwrap(),
        })
    }
    use crate::syntax::span::Span;
    use crate::syntax::span::Spanned;
    use crate::syntax::type_name::FieldName;

    // Well-known IDs matching prelude dimension names.
    fn length_id() -> BaseDimId {
        BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length)
    }
    fn time_id() -> BaseDimId {
        BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Time)
    }
    fn mass_id() -> BaseDimId {
        BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Mass)
    }

    fn user_dim_id(name: &str) -> BaseDimId {
        BaseDimId::UserDefined(crate::resolved_name::ResolvedDimName::from_def(
            crate::dag_id::DagId::root_in_package("test", "test"),
            DimName::expect_valid(name),
        ))
    }

    fn make_registry() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.build()
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
            r.dimensions
                .get_dimension(&DimRef::local(DimName::expect_valid("Length"))),
            Some(&Dimension::base(length_id()))
        );
        assert_eq!(
            r.dimensions
                .get_dimension(&DimRef::local(DimName::expect_valid("Time"))),
            Some(&Dimension::base(time_id()))
        );
        assert_eq!(
            r.dimensions
                .get_dimension(&DimRef::local(DimName::expect_valid("Mass"))),
            Some(&Dimension::base(mass_id()))
        );
    }

    #[test]
    fn registry_derived_dimensions() {
        let r = make_registry();
        let velocity = r
            .dimensions
            .get_dimension(&DimRef::local(DimName::expect_valid("Velocity")))
            .unwrap();
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
        assert_eq!(m.scale, UnitScale::Const(PositiveFiniteScale::ONE));
    }

    #[test]
    fn registry_derived_units() {
        let r = make_registry();
        let km = r
            .units
            .get_unit(&UnitRef::local(UnitName::expect_valid("km")))
            .unwrap();
        assert_eq!(km.dimension, Dimension::base(length_id()));
        assert_eq!(
            km.scale.static_scale().map(PositiveFiniteScale::get),
            Some(1000.0)
        );
        assert!(km.scale.constness().is_const());
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
    fn resolve_dim_expr_keys_qualified_and_aliased_references() {
        use crate::registry::dimension_table::DimensionResolveError;
        use crate::syntax::dimension::DimRef;
        use crate::syntax::names::NameAtom;
        use crate::syntax::non_empty::NonEmpty;

        let atom = |s: &str| NameAtom::parse(s).unwrap();
        let rate = DimName::expect_valid("Rate");
        let qualified =
            |owner: &str| DimRef::qualified(NonEmpty::singleton(atom(owner)), rate.clone());
        let single = |path: NamePath| DimExpr {
            terms: vec![DimExprItem {
                op: MulDivOp::Mul,
                term: DimTerm {
                    name: Spanned::new(path, Span::new(0, 0)),
                    power: None,
                    span: Span::new(0, 0),
                },
            }],
            span: Span::new(0, 0),
        };
        let velocity = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        let mass_rate = (Dimension::base(mass_id()) / Dimension::base(time_id())).unwrap();

        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.register_dimension(qualified("a"), velocity.clone());
        b.register_dimension(qualified("b"), mass_rate.clone());
        b.register_dimension(rate.clone(), Dimension::base(mass_id()));
        b.register_dimension_alias(DimRef::local(DimName::expect_valid("R")), qualified("a"))
            .unwrap();

        let member =
            |owner: &str| NamePath::qualified(NonEmpty::singleton(atom(owner)), atom("Rate"));
        assert_eq!(
            b.resolve_dim_expr_detailed(&single(member("a"))),
            Ok(velocity.clone())
        );
        assert_eq!(
            b.resolve_dim_expr_detailed(&single(member("b"))),
            Ok(mass_rate)
        );
        assert_eq!(
            b.resolve_dim_expr_detailed(&single(NamePath::expect_local("Rate"))),
            Ok(Dimension::base(mass_id()))
        );
        assert_eq!(
            b.resolve_dim_expr_detailed(&single(NamePath::expect_local("R"))),
            Ok(velocity)
        );
        assert_eq!(
            b.resolve_dim_expr_detailed(&single(member("zzz"))),
            Err(DimensionResolveError::UnknownDimension {
                name: qualified("zzz")
            })
        );
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
        assert!((scale.get() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_unit_expr_km_per_h() {
        let r = make_registry();
        // km / h
        let expr = UnitExpr {
            terms: vec![
                UnitExprItem {
                    op: MulDivOp::Mul,
                    name: make_unit_name("km"),
                    power: None,
                },
                UnitExprItem {
                    op: MulDivOp::Div,
                    name: make_unit_name("h"),
                    power: None,
                },
            ],
            span: Span::new(0, 0),
        };
        let (dim, scale) = r.units.resolve_unit_expr(&expr).unwrap();
        let expected_dim = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        assert_eq!(dim, expected_dim);
        // km/h = 1000 m / 3600 s ≈ 0.2778 m/s
        assert!((scale.get() - 1000.0 / 3600.0).abs() < 1e-10);
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
                UnitResolveError::InvalidScale(PositiveFiniteScaleError::NonFinite { value })
                    if value == f64::INFINITY
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
        let member = UnionMemberDef::try_new(
            ConstructorName::expect_valid("TransferResult"),
            vec![
                StructField::new(
                    FieldName::expect_valid("dv1"),
                    make_dim_type_expr("Velocity"),
                ),
                StructField::new(
                    FieldName::expect_valid("dv2"),
                    make_dim_type_expr("Velocity"),
                ),
            ],
        )
        .unwrap();
        b.register_type(
            TypeDef::try_union(
                StructTypeName::expect_valid("TransferResult"),
                vec![],
                vec![member],
            )
            .unwrap(),
        );
        let r = b.build();
        let velocity_dim = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        let def = r
            .types
            .get_type(&StructTypeName::expect_valid("TransferResult"))
            .unwrap();
        assert_eq!(def.name().as_str(), "TransferResult");
        assert!(def.is_union());
        let fields = def.record_fields().expect("single-variant collision");
        assert_eq!(fields.len(), 2);
        assert_eq!(
            def.record_member().map(UnionMemberDef::name),
            Some(&crate::syntax::type_name::record_constructor_name(
                def.name()
            ))
        );
        assert_eq!(fields[0].name().as_str(), "dv1");
        assert_eq!(
            r.dimensions.resolve_type_expr(fields[0].type_ann()),
            Ok(Some(velocity_dim))
        );
        assert!(
            r.types
                .get_type(&StructTypeName::expect_valid("NonExistent"))
                .is_none()
        );
    }

    #[test]
    fn registry_index_register_and_lookup() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        b.register_index(
            IndexName::expect_valid("Maneuver"),
            named_index_kind(["Departure", "Correction", "Insertion"]),
        );
        let r = b.build();
        let def = r
            .indexes
            .get_index(&IndexName::expect_valid("Maneuver"))
            .unwrap();
        assert_eq!(def.name.to_string(), "Maneuver");
        let entry_keys = def.entry_keys();
        let variant_strs: Vec<&str> = entry_keys
            .iter()
            .map(|key| {
                key.as_named()
                    .expect("named index must have named keys")
                    .as_str()
            })
            .collect();
        assert_eq!(variant_strs, vec!["Departure", "Correction", "Insertion"]);
        assert!(
            r.indexes
                .get_index(&IndexName::expect_valid("NonExistent"))
                .is_none()
        );
    }

    #[test]
    fn registry_formats_dimensions_without_registered_base_metadata() {
        let mut b = RegistryBuilder::new();
        b.register_dimension(
            DimName::expect_valid("Broken"),
            Dimension::base(length_id()),
        );

        let r = b.build();
        assert_eq!(
            r.dimensions.format_dimension(&Dimension::base(length_id())),
            "Length"
        );
    }

    #[test]
    fn register_user_defined_base_dimension() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let info_id = user_dim_id("Information");
        b.register_base_dimension(info_id.clone());
        let r = b.build();
        // Should be retrievable under its leaf name
        let dim = r
            .dimensions
            .get_dimension(&DimRef::local(DimName::expect_valid("Information")))
            .unwrap();
        assert_eq!(*dim, Dimension::base(info_id.clone()));
        assert_eq!(
            r.dimensions.base_dimension(&info_id),
            Some(&BaseDimensionInfo::default())
        );
    }

    #[test]
    fn register_base_dimension_with_symbol() {
        let mut b = RegistryBuilder::new();
        let id = b.register_base_dimension_with_symbol(
            BaseDimId::Prelude(crate::dimension::PreludeBaseDimension::Length),
            UnitName::expect_valid("m"),
        );
        let r = b.build();
        assert_eq!(
            r.dimensions.base_unit_symbols().get(&id),
            Some(&"m".to_string())
        );
    }

    #[test]
    fn register_base_unit_records_unit_and_canonical_symbol() {
        let mut b = RegistryBuilder::new();
        let info_id = user_dim_id("Information");
        b.register_base_dimension(info_id.clone());
        b.register_base_unit(
            UnitName::expect_valid("bit"),
            Dimension::base(info_id.clone()),
        )
        .unwrap();

        let registry = b.build();
        assert_eq!(
            registry.dimensions.base_unit_symbols().get(&info_id),
            Some(&"bit".to_string())
        );
        let unit = registry
            .units
            .get_unit(&UnitRef::local(UnitName::expect_valid("bit")))
            .unwrap();
        assert_eq!(unit.dimension, Dimension::base(info_id));
        assert_eq!(unit.scale, UnitScale::Const(PositiveFiniteScale::ONE));
    }

    #[test]
    fn register_base_unit_rejects_aliases_and_non_base_dimensions_atomically() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let info_id = user_dim_id("Information");
        b.register_base_dimension(info_id.clone());
        b.register_base_unit(
            UnitName::expect_valid("bit"),
            Dimension::base(info_id.clone()),
        )
        .unwrap();

        assert_eq!(
            b.register_base_unit(UnitName::expect_valid("nat"), Dimension::base(info_id),),
            Err(BaseUnitRegistrationError::AlreadyRegistered {
                existing: UnitName::expect_valid("bit"),
            })
        );
        assert!(
            b.get_unit(&UnitRef::local(UnitName::expect_valid("nat")))
                .is_none()
        );

        let velocity = (Dimension::base(length_id()) / Dimension::base(time_id())).unwrap();
        assert_eq!(
            b.register_base_unit(UnitName::expect_valid("kt"), velocity),
            Err(BaseUnitRegistrationError::NotBaseDimension)
        );
        assert!(
            b.get_unit(&UnitRef::local(UnitName::expect_valid("kt")))
                .is_none()
        );
    }

    #[test]
    fn imported_base_dimension_metadata_keeps_first_canonical_unit() {
        let mut b = RegistryBuilder::new();
        let info_id = user_dim_id("Information");
        b.record_base_dimension(info_id.clone());
        b.import_base_dimension(
            info_id.clone(),
            &BaseDimensionInfo::with_canonical_unit(UnitName::expect_valid("bit")),
        );
        // A second import does not overwrite the canonical unit.
        b.import_base_dimension(
            info_id.clone(),
            &BaseDimensionInfo::with_canonical_unit(UnitName::expect_valid("byte")),
        );
        let r = b.build();
        assert_eq!(
            r.dimensions.base_unit_symbols().get(&info_id),
            Some(&"bit".to_string())
        );
        // Recorded and imported base dimensions are never source-visible.
        assert_eq!(
            r.dimensions
                .get_dimension(&DimRef::local(DimName::expect_valid("Information"))),
            None
        );
    }

    #[test]
    fn aliases_resolve_in_every_namespace_and_reject_cycles() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let axis = IndexName::expect_valid("Axis");
        b.register_index(axis.clone(), named_index_kind(["Only"]));
        let pair = b.ensure_finite_index(IndexCardinality::try_from_u64(2).unwrap());
        let effective = IndexName::expect_valid("EffectiveAxis");
        let structural = IndexName::expect_valid("Pair");
        b.register_index_alias(
            effective.clone(),
            IndexBindingTarget::Declared(axis.clone()),
        )
        .unwrap();
        b.register_index_alias(structural.clone(), IndexBindingTarget::Finite(pair))
            .unwrap();
        assert_eq!(
            b.register_index_alias(
                axis.clone(),
                IndexBindingTarget::Declared(effective.clone())
            ),
            Err(AliasCycle {
                alias: IndexBindingTarget::Declared(axis.clone())
            })
        );

        let record = StructTypeName::expect_valid("Record");
        let member =
            UnionMemberDef::try_new(ConstructorName::expect_valid("Record"), vec![]).unwrap();
        b.register_type(TypeDef::try_union(record.clone(), vec![], vec![member]).unwrap());
        let effective_record = StructTypeName::expect_valid("EffectiveRecord");
        b.register_type_alias(effective_record.clone(), record.clone())
            .unwrap();
        assert!(
            b.register_type_alias(record.clone(), effective_record.clone())
                .is_err()
        );

        let r = b.build();
        assert_eq!(
            r.indexes.get_index(&effective).map(|d| &d.name),
            Some(&IndexBindingTarget::Declared(axis))
        );
        assert_eq!(
            r.indexes.get_index(&structural),
            r.indexes.get_finite_index(pair)
        );
        assert!(r.indexes.get_index(&structural).is_some());
        assert_eq!(r.indexes.declared_indexes().count(), 1);
        assert_eq!(r.indexes.finite_indexes().collect::<Vec<_>>(), [pair]);
        assert_eq!(
            r.types.get_type(&effective_record).map(TypeDef::name),
            Some(&record)
        );
        let (owner, ctor) = r
            .types
            .lookup_ctor(&ConstructorName::expect_valid("Record"))
            .unwrap();
        assert_eq!(owner.name(), &record);
        assert_eq!(ctor.name().as_str(), "Record");
    }

    #[test]
    fn unit_and_dimension_aliases_resolve_and_reject_cycles() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let metre = UnitRef::local(UnitName::expect_valid("m"));
        let local_metre = UnitRef::local(UnitName::expect_valid("metre"));
        b.register_unit_alias(local_metre.clone(), metre.clone())
            .unwrap();
        assert!(b.register_unit_alias(metre, local_metre.clone()).is_err());
        assert_eq!(
            b.get_unit(&local_metre).map(|info| &info.dimension),
            Some(&Dimension::base(length_id()))
        );

        let span = DimRef::local(DimName::expect_valid("Span"));
        b.register_dimension_alias(span.clone(), DimRef::local(DimName::expect_valid("Length")))
            .unwrap();
        assert!(
            b.register_dimension_alias(
                DimRef::local(DimName::expect_valid("Length")),
                span.clone()
            )
            .is_err()
        );

        let r = b.build();
        assert_eq!(
            r.units.get_unit(&local_metre).map(|info| &info.dimension),
            Some(&Dimension::base(length_id()))
        );
        assert_eq!(
            r.dimensions.get_dimension(&span),
            Some(&Dimension::base(length_id()))
        );
        // Unit expressions resolve through unit aliases too.
        let expr = UnitExpr {
            terms: vec![UnitExprItem {
                op: MulDivOp::Mul,
                name: Spanned::new(local_metre, Span::new(0, 0)),
                power: None,
            }],
            span: Span::new(0, 0),
        };
        assert_eq!(
            r.units.resolve_unit_expr(&expr).map(|(dim, _)| dim),
            Ok(Dimension::base(length_id()))
        );
    }
}
