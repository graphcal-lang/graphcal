//! Base-dimension metadata keyed by [`BaseDimId`] and the dimension
//! formatting services built from it.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::dimension::{BaseDimId, Dimension};
use crate::syntax::dimension::{DimName, DimRef, UnitName};

/// Format a dimension, preferring a registered named alias for compound forms.
///
/// A pure base dimension (`Length`) or `Dimensionless` keeps its canonical
/// rendering. A compound dimension (`Length^2 * Mass / Time^2`) is replaced by
/// a matching named dimension (`Energy`) when one is registered; if several
/// names match, the lexicographically smallest is chosen for determinism.
fn format_dimension_preferring_alias(
    named: &BTreeMap<DimRef, Dimension>,
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
    /// Metadata of a base dimension with the given canonical unit and affine policy.
    #[must_use]
    pub(crate) const fn new(canonical_unit: Option<UnitName>, affine_prone: bool) -> Self {
        Self {
            canonical_unit,
            affine_prone,
        }
    }

    /// Record the canonical unit of this base dimension.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalUnitAlreadyRegistered`] when a canonical unit is
    /// already recorded.
    pub(crate) fn register_canonical_unit(
        &mut self,
        unit: UnitName,
    ) -> Result<(), CanonicalUnitAlreadyRegistered> {
        if let Some(existing) = &self.canonical_unit {
            return Err(CanonicalUnitAlreadyRegistered {
                existing: existing.clone(),
            });
        }
        self.canonical_unit = Some(unit);
        Ok(())
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
    pub(crate) fn merge_missing(&mut self, other: &Self) {
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

/// How a diagnostic spells a dimension.
///
/// It is constructed only by
/// [`DimensionFormattingRegistry::dimension_spelling`], so a payload holding a
/// `DimensionSpelling` always names a dimension, never free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionSpelling(String);

impl std::fmt::Display for DimensionSpelling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Dimension formatting data retained after semantic name resolution.
///
/// Named dimensions are display aliases only: this type deliberately exposes
/// no source-name lookup or AST resolution API.
#[derive(Debug, Clone)]
pub struct DimensionFormattingRegistry {
    bases: BTreeMap<BaseDimId, BaseDimensionInfo>,
    display_aliases: BTreeMap<DimRef, Dimension>,
}

impl DimensionFormattingRegistry {
    /// Formatting data from base-dimension metadata and the named dimensions
    /// a diagnostic may prefer over a base-dimension expansion.
    #[must_use]
    pub fn new(
        bases: BTreeMap<BaseDimId, BaseDimensionInfo>,
        display_aliases: impl IntoIterator<Item = (DimRef, Dimension)>,
    ) -> Self {
        Self {
            bases,
            display_aliases: display_aliases.into_iter().collect(),
        }
    }

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

    /// Spell a dimension for a diagnostic payload.
    #[must_use]
    pub fn dimension_spelling(&self, dim: &Dimension) -> DimensionSpelling {
        DimensionSpelling(self.format_dimension(dim))
    }

    /// Add diagnostic formatting for one rigid template dimension port.
    ///
    /// Like a required port (`pub(bind) dim Q;`), a rigid port is an opaque
    /// base dimension without a canonical unit.
    pub(crate) fn register_rigid_dimension(
        &mut self,
        name: &crate::resolved_name::ResolvedDimName,
    ) {
        let base = BaseDimId::UserDefined(name.clone());
        self.bases
            .insert(base.clone(), BaseDimensionInfo::default());
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
        let mut info = BaseDimensionInfo::default();
        assert_eq!(info.register_canonical_unit(unit("m")), Ok(()));
        assert_eq!(
            info.register_canonical_unit(unit("ft")),
            Err(CanonicalUnitAlreadyRegistered {
                existing: unit("m")
            })
        );
        let formatting =
            DimensionFormattingRegistry::new(BTreeMap::from([(length(), info)]), Vec::new());
        assert_eq!(
            formatting.base_unit_symbols(),
            BTreeMap::from([(length(), "m".to_string())])
        );
    }

    #[test]
    fn formatting_prefers_named_alias_for_compound_dimensions_only() {
        let area = Dimension::base(length()).pow(2).unwrap();
        let formatting = DimensionFormattingRegistry::new(
            BTreeMap::new(),
            [
                (DimRef::local(DimName::expect_valid("Area")), area.clone()),
                (
                    DimRef::local(DimName::expect_valid("Span")),
                    Dimension::base(length()),
                ),
            ],
        );

        assert_eq!(formatting.format_dimension(&area), "Area");
        assert_eq!(
            formatting.format_dimension(&Dimension::base(length())),
            "Length"
        );
        assert_eq!(
            formatting.format_dimension(&area.pow(2).unwrap()),
            "Length^4"
        );
    }
}
