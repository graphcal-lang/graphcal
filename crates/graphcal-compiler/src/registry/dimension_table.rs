//! Dimension table: base-dimension metadata keyed by [`BaseDimId`] and the
//! source-visible named dimensions of one registry scope.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::desugar::desugared_ast::{DimExpr, MulDivOp, TypeExpr, TypeExprKind};
use crate::dimension::{BaseDimId, Dimension};
use crate::ratio::RatioError;
use crate::registry::aliased_table::{AliasCycle, AliasedTable};
use crate::syntax::dimension::{DimName, DimRef, UnitName};

/// Error returned when resolving a `DimExpr` to a concrete [`Dimension`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DimensionResolveError {
    /// A referenced dimension is not visible under its (possibly
    /// module-qualified) source reference.
    #[error("unknown dimension `{name}`")]
    UnknownDimension { name: DimRef },
    /// Dimension exponent arithmetic overflowed.
    #[error(transparent)]
    Overflow(#[from] RatioError),
}

/// Resolve a `DimExpr` by looking up each term's typed (possibly
/// module-qualified) reference with `lookup`.
///
/// This is the single term-folding implementation; registries supply their
/// alias-aware scope lookup, while boundary code that spans two scopes (for
/// example a dependency declaration re-read under include bindings) supplies
/// a lookup that routes each reference to its owning scope.
///
/// # Errors
///
/// Returns [`DimensionResolveError::UnknownDimension`] with the full source
/// reference when `lookup` misses, or an overflow error from exponent
/// arithmetic.
pub fn resolve_dim_expr_with<'a>(
    expr: &DimExpr,
    mut lookup: impl FnMut(&DimRef) -> Option<&'a Dimension>,
) -> Result<Dimension, DimensionResolveError> {
    expr.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, item| {
            let reference: DimRef = item.term.name.value.clone().classify_leaf();
            let Some(base) = lookup(&reference) else {
                return Err(DimensionResolveError::UnknownDimension { name: reference });
            };
            let powered = base.pow(item.term.effective_power())?;
            match item.op {
                MulDivOp::Mul => acc * powered,
                MulDivOp::Div => acc / powered,
            }
            .map_err(DimensionResolveError::from)
        })
}

/// Format a dimension, preferring a registered named alias for compound forms.
///
/// A pure base dimension (`Length`) or `Dimensionless` keeps its canonical
/// rendering. A compound dimension (`Length^2 * Mass / Time^2`) is replaced by
/// a matching named dimension (`Energy`) when one is registered; if several
/// names match, the lexicographically smallest is chosen for determinism.
fn format_dimension_preferring_alias(
    named: &AliasedTable<DimRef, Dimension>,
    dim: &Dimension,
) -> String {
    // Base dimensions and Dimensionless render as a single bare name already;
    // only compound dimensions benefit from an alias.
    if dim.is_compound()
        && let Some(alias) = named
            .iter()
            .filter(|(_, d)| *d == dim)
            .map(|(name, _)| name)
            .min()
    {
        return alias.to_string();
    }
    dim.to_string()
}

/// Display and unit-policy metadata of one base dimension.
///
/// The display name is deliberately not stored: it is always derived from
/// the [`BaseDimId`] itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseDimensionInfo {
    /// The canonical (scale-1) unit used for runtime display (`m` for
    /// `Length`), once one is registered.
    canonical_unit: Option<UnitName>,
    /// Real-world units of this dimension are affine (offset) scales, e.g.
    /// Temperature (°C, °F). User unit definitions on the bare dimension are
    /// rejected because a purely multiplicative definition would display
    /// silently wrong values (#648 U4).
    affine_prone: bool,
}

impl BaseDimensionInfo {
    /// Metadata of a (non-affine) base dimension with a canonical unit.
    #[must_use]
    pub(crate) const fn with_canonical_unit(unit: UnitName) -> Self {
        Self {
            canonical_unit: Some(unit),
            affine_prone: false,
        }
    }

    /// The canonical (scale-1) unit of this base dimension, if registered.
    #[must_use]
    pub const fn canonical_unit(&self) -> Option<&UnitName> {
        self.canonical_unit.as_ref()
    }

    /// Whether user unit definitions on the bare dimension are rejected.
    #[must_use]
    pub const fn is_affine_prone(&self) -> bool {
        self.affine_prone
    }

    /// Fill metadata this entry lacks from another view of the same base
    /// dimension. An already-registered canonical unit wins.
    fn merge_missing(&mut self, other: &Self) {
        if self.canonical_unit.is_none() {
            self.canonical_unit.clone_from(&other.canonical_unit);
        }
        self.affine_prone |= other.affine_prone;
    }
}

/// The canonical unit of a base dimension is already registered.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("base dimension already has canonical unit `{existing}`")]
pub struct CanonicalUnitAlreadyRegistered {
    pub existing: UnitName,
}

/// Dimension table: base-dimension metadata keyed by [`BaseDimId`] plus the
/// source-visible named dimensions and their aliases.
///
/// Every dimension-name lookup — direct [`Self::get_dimension`] calls and
/// `DimExpr` resolution alike — goes through the one alias-aware
/// [`AliasedTable`], so module qualifiers and source-visible aliases are
/// honoured uniformly.
#[derive(Debug, Clone, Default)]
pub struct DimensionTable {
    bases: BTreeMap<BaseDimId, BaseDimensionInfo>,
    named: AliasedTable<DimRef, Dimension>,
}

impl DimensionTable {
    pub(crate) fn into_formatting(self) -> DimensionFormattingRegistry {
        DimensionFormattingRegistry {
            bases: self.bases,
            display_aliases: self.named,
        }
    }

    // -- Mutation --

    /// Register a base dimension and make it source-visible under its leaf
    /// name (`base dim Foo;`, prelude `Length`).
    pub(crate) fn register_base_dimension(&mut self, id: BaseDimId) {
        self.named
            .insert(DimRef::local(id.source_name()), Dimension::base(id.clone()));
        self.bases.entry(id).or_default();
    }

    /// Record a base dimension without making it source-visible.
    ///
    /// Imported dimensions and units may be built from a dependency's private
    /// base dimensions; the importer tracks them but must not be able to name
    /// them.
    pub(crate) fn record_base_dimension(&mut self, id: BaseDimId) {
        self.bases.entry(id).or_default();
    }

    /// Copy a dependency's metadata for `id`, keeping what is already known.
    pub(crate) fn import_base_dimension(&mut self, id: BaseDimId, info: &BaseDimensionInfo) {
        self.bases.entry(id).or_default().merge_missing(info);
    }

    /// Record the canonical unit of a base dimension.
    pub(crate) fn register_canonical_unit(
        &mut self,
        id: BaseDimId,
        unit: UnitName,
    ) -> Result<(), CanonicalUnitAlreadyRegistered> {
        let info = self.bases.entry(id).or_default();
        if let Some(existing) = &info.canonical_unit {
            return Err(CanonicalUnitAlreadyRegistered {
                existing: existing.clone(),
            });
        }
        info.canonical_unit = Some(unit);
        Ok(())
    }

    /// Mark a base dimension as affine-prone: its real-world units (e.g.
    /// Celsius/Fahrenheit on Temperature) need offset conversions that unit
    /// definitions cannot express, so user unit definitions on the bare
    /// dimension are rejected (#648 U4).
    pub(crate) fn mark_affine_prone(&mut self, id: BaseDimId) {
        self.bases.entry(id).or_default().affine_prone = true;
    }

    /// Register a named dimension under a local or module-qualified reference.
    pub(crate) fn register_dimension(&mut self, name: DimRef, dim: Dimension) {
        self.named.insert(name, dim);
    }

    /// Register a source-visible dimension alias without changing identity.
    pub(crate) fn register_dimension_alias(
        &mut self,
        alias: DimRef,
        target: DimRef,
    ) -> Result<(), AliasCycle<DimRef>> {
        self.named.insert_alias(alias, target)
    }

    /// Merge every entry of `parent` this table does not bind yet.
    pub(crate) fn merge_missing_from(&mut self, parent: &Self) {
        for (id, info) in &parent.bases {
            self.import_base_dimension(id.clone(), info);
        }
        self.named.merge_missing_from(&parent.named);
    }

    // -- Lookup --

    /// Look up a possibly module-qualified dimension reference, following
    /// source-visible aliases.
    #[must_use]
    pub fn get_dimension(&self, reference: &DimRef) -> Option<&Dimension> {
        self.named.get(reference)
    }

    /// Iterate over all named dimensions.
    pub fn all_dimensions(&self) -> impl Iterator<Item = (&DimRef, &Dimension)> {
        self.named.iter()
    }

    /// Iterate over every known base dimension and its metadata.
    pub fn base_dimensions(&self) -> impl Iterator<Item = (&BaseDimId, &BaseDimensionInfo)> {
        self.bases.iter()
    }

    /// Metadata of one base dimension, if known.
    #[must_use]
    pub fn base_dimension(&self, id: &BaseDimId) -> Option<&BaseDimensionInfo> {
        self.bases.get(id)
    }

    /// Canonical-unit symbols of every base dimension that has one, for
    /// runtime display adapters.
    #[must_use]
    pub fn base_unit_symbols(&self) -> BTreeMap<BaseDimId, String> {
        canonical_unit_symbols(&self.bases)
    }

    /// Returns `true` when `dim` is exactly an affine-prone base dimension
    /// (power 1). Compound dimensions involving the base (e.g.
    /// `Temperature / Time`) stay allowed: offsets cancel in differences.
    #[must_use]
    pub(crate) fn is_affine_prone(&self, dim: &Dimension) -> bool {
        dim.base_dimension_id()
            .and_then(|id| self.bases.get(id))
            .is_some_and(BaseDimensionInfo::is_affine_prone)
    }

    /// Format a dimension as a human-readable string.
    ///
    /// Returns `"Dimensionless"` for dimensionless, or names like `"Length / Time"`.
    /// When a compound dimension matches a named dimension alias (e.g. `Energy`
    /// for `Length^2 * Mass / Time^2`), the alias is preferred so diagnostics
    /// speak the user's vocabulary.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format_dimension_preferring_alias(&self.named, dim)
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if any dimension name is unknown, and `Err` if
    /// dimension exponent arithmetic overflows `i32`.
    pub(crate) fn resolve_dim_expr(&self, expr: &DimExpr) -> Result<Option<Dimension>, RatioError> {
        match self.resolve_dim_expr_detailed(expr) {
            Ok(dim) => Ok(Some(dim)),
            Err(DimensionResolveError::UnknownDimension { .. }) => Ok(None),
            Err(DimensionResolveError::Overflow(err)) => Err(err),
        }
    }

    /// Resolve a `DimExpr` AST node to a concrete `Dimension`, preserving the
    /// unknown referenced dimension name in the error.
    pub fn resolve_dim_expr_detailed(
        &self,
        expr: &DimExpr,
    ) -> Result<Dimension, DimensionResolveError> {
        resolve_dim_expr_with(expr, |reference| self.get_dimension(reference))
    }

    /// Resolve a `TypeExpr` to a concrete `Dimension`.
    ///
    /// Returns `Ok(None)` if the type references unknown dimensions, and
    /// `Err` if dimension exponent arithmetic overflows `i32`.
    pub fn resolve_type_expr(&self, type_expr: &TypeExpr) -> Result<Option<Dimension>, RatioError> {
        match &type_expr.kind {
            TypeExprKind::Dimensionless => Ok(Some(Dimension::dimensionless())),
            TypeExprKind::IndexLabel { .. }
            | TypeExprKind::Bool
            | TypeExprKind::Int
            | TypeExprKind::Datetime
            | TypeExprKind::TypeApplication { .. }
            | TypeExprKind::DatetimeApplication { .. }
            | TypeExprKind::ComplexApplication { .. }
            | TypeExprKind::KeyApplication { .. } => Ok(None),
            TypeExprKind::DimExpr(dim_expr) => self.resolve_dim_expr(dim_expr),
            TypeExprKind::Indexed { base, .. } => self.resolve_type_expr(base),
        }
    }
}

/// Project base-dimension metadata to the canonical-unit symbols consumed by
/// runtime display adapters.
fn canonical_unit_symbols(
    bases: &BTreeMap<BaseDimId, BaseDimensionInfo>,
) -> BTreeMap<BaseDimId, String> {
    bases
        .iter()
        .filter_map(|(id, info)| Some((id.clone(), info.canonical_unit()?.to_string())))
        .collect()
}

/// Dimension formatting data retained after semantic name resolution.
///
/// Named dimensions are display aliases only: this type deliberately exposes
/// no source-name lookup or AST resolution API.
#[derive(Debug, Clone)]
pub struct DimensionFormattingRegistry {
    bases: BTreeMap<BaseDimId, BaseDimensionInfo>,
    display_aliases: AliasedTable<DimRef, Dimension>,
}

impl DimensionFormattingRegistry {
    /// Canonical-unit symbols of every base dimension that has one, for
    /// runtime display adapters.
    #[must_use]
    pub fn base_unit_symbols(&self) -> BTreeMap<BaseDimId, String> {
        canonical_unit_symbols(&self.bases)
    }

    /// Format a dimension without providing any semantic name-resolution API.
    #[must_use]
    pub fn format_dimension(&self, dim: &Dimension) -> String {
        format_dimension_preferring_alias(&self.display_aliases, dim)
    }

    /// Add diagnostic formatting for one synthetic rigid template dimension.
    pub(crate) fn register_rigid_dimension(
        &mut self,
        name: &crate::resolved_name::ResolvedDimName,
    ) {
        let base = BaseDimId::UserDefined(name.clone());
        self.bases.insert(
            base.clone(),
            BaseDimensionInfo {
                canonical_unit: Some(UnitName::classify(name.atom().clone())),
                affine_prone: false,
            },
        );
        self.display_aliases.insert(
            DimRef::local(DimName::classify(name.atom().clone())),
            Dimension::base(base),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::PreludeBaseDimension;

    fn length() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Length)
    }

    fn temperature() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Temperature)
    }

    fn unit(name: &str) -> UnitName {
        UnitName::expect_valid(name)
    }

    #[test]
    fn merge_missing_keeps_existing_canonical_unit_and_ors_affine_flag() {
        let mut existing = BaseDimensionInfo {
            canonical_unit: Some(unit("m")),
            affine_prone: false,
        };
        existing.merge_missing(&BaseDimensionInfo {
            canonical_unit: Some(unit("ft")),
            affine_prone: true,
        });
        assert_eq!(existing.canonical_unit(), Some(&unit("m")));
        assert!(existing.is_affine_prone());

        let mut missing = BaseDimensionInfo::default();
        missing.merge_missing(&BaseDimensionInfo {
            canonical_unit: Some(unit("K")),
            affine_prone: false,
        });
        assert_eq!(missing.canonical_unit(), Some(&unit("K")));
        assert!(!missing.is_affine_prone());
    }

    #[test]
    fn register_canonical_unit_rejects_second_unit() {
        let mut table = DimensionTable::default();
        table.register_base_dimension(length());
        assert_eq!(table.register_canonical_unit(length(), unit("m")), Ok(()));
        assert_eq!(
            table.register_canonical_unit(length(), unit("ft")),
            Err(CanonicalUnitAlreadyRegistered {
                existing: unit("m")
            })
        );
        assert_eq!(
            table.base_unit_symbols(),
            BTreeMap::from([(length(), "m".to_string())])
        );
    }

    #[test]
    fn affine_prone_applies_only_to_the_bare_base_dimension() {
        let mut table = DimensionTable::default();
        table.register_base_dimension(temperature());
        table.register_base_dimension(length());
        table.mark_affine_prone(temperature());

        let bare = Dimension::base(temperature());
        assert!(table.is_affine_prone(&bare));
        assert!(!table.is_affine_prone(&bare.pow(2).unwrap()));
        assert!(!table.is_affine_prone(&Dimension::base(length())));
        assert!(!table.is_affine_prone(&Dimension::dimensionless()));
    }

    #[test]
    fn recorded_base_dimensions_are_tracked_but_not_source_visible() {
        let mut table = DimensionTable::default();
        table.record_base_dimension(length());
        assert!(table.base_dimension(&length()).is_some());
        assert_eq!(
            table.get_dimension(&DimRef::local(DimName::expect_valid("Length"))),
            None
        );

        table.register_base_dimension(length());
        assert_eq!(
            table.get_dimension(&DimRef::local(DimName::expect_valid("Length"))),
            Some(&Dimension::base(length()))
        );
    }

    #[test]
    fn merge_missing_from_keeps_local_definitions() {
        let mut parent = DimensionTable::default();
        parent.register_base_dimension(length());
        parent.register_canonical_unit(length(), unit("m")).unwrap();
        parent.mark_affine_prone(temperature());
        let rate = DimRef::local(DimName::expect_valid("Rate"));
        parent.register_dimension(rate.clone(), Dimension::base(temperature()));

        let mut child = DimensionTable::default();
        child.register_dimension(rate.clone(), Dimension::base(length()));
        child.merge_missing_from(&parent);

        assert_eq!(child.get_dimension(&rate), Some(&Dimension::base(length())));
        assert_eq!(
            child.get_dimension(&DimRef::local(DimName::expect_valid("Length"))),
            Some(&Dimension::base(length()))
        );
        assert!(child.is_affine_prone(&Dimension::base(temperature())));
        assert_eq!(
            child.base_unit_symbols(),
            BTreeMap::from([(length(), "m".to_string())])
        );
    }

    #[test]
    fn formatting_prefers_named_alias_for_compound_dimensions_only() {
        let mut table = DimensionTable::default();
        table.register_base_dimension(length());
        let area = Dimension::base(length()).pow(2).unwrap();
        table.register_dimension(DimRef::local(DimName::expect_valid("Area")), area.clone());
        table.register_dimension(
            DimRef::local(DimName::expect_valid("Span")),
            Dimension::base(length()),
        );

        assert_eq!(table.format_dimension(&area), "Area");
        assert_eq!(table.format_dimension(&Dimension::base(length())), "Length");
        let formatting = table.into_formatting();
        assert_eq!(formatting.format_dimension(&area), "Area");
        assert_eq!(
            formatting.format_dimension(&area.pow(2).unwrap()),
            "Length^4"
        );
    }
}
