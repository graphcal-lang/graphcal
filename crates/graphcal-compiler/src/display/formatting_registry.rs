//! Post-resolution formatting services.
//!
//! Canonical dimensions, units, indexes, and nominal types are keyed by
//! [`ResolvedName`](crate::resolved_name::ResolvedName) identities in
//! [`ModuleDefinitions`](crate::ir::module_definitions::ModuleDefinitions) and
//! the project type store; no source-name keyed registry exists.

use std::collections::BTreeMap;

use crate::dimension::{BaseDimId, Dimension};
use crate::syntax::dimension::{DimName, DimRef};

use crate::ratio::RatioError;
use crate::semantic::dimension_table::{BaseDimensionInfo, DimensionFormattingRegistry};
use crate::semantic::prelude::{prelude_base_dimension_infos, prelude_dimensions};
use crate::semantic::time_zone::TimeZoneRegistry;

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

impl FormattingRegistry {
    /// Formatting services from base-dimension metadata and the named
    /// dimensions visible to one module.
    #[must_use]
    pub fn new(
        bases: BTreeMap<BaseDimId, BaseDimensionInfo>,
        display_dimensions: impl IntoIterator<Item = (DimRef, Dimension)>,
    ) -> Self {
        Self {
            dimensions: DimensionFormattingRegistry::new(bases, display_dimensions),
            time_zones: TimeZoneRegistry::bundled(),
        }
    }

    /// Formatting services for a module that sees only the Graphcal prelude.
    ///
    /// # Errors
    ///
    /// Returns an error only if the built-in prelude is inconsistent.
    pub fn graphcal_prelude() -> Result<Self, RatioError> {
        Ok(Self::new(
            prelude_base_dimension_infos()
                .map(|(base, info)| (BaseDimId::Prelude(base), info))
                .collect(),
            prelude_dimensions()?
                .into_iter()
                .map(|(name, dimension)| (DimRef::local(DimName::expect_valid(name)), dimension)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::PreludeBaseDimension;

    #[test]
    fn prelude_formatting_prefers_named_compound_dimensions() {
        let formatting = FormattingRegistry::graphcal_prelude().unwrap();
        let length = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length));
        let time = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));
        let velocity = (length.clone() / time).unwrap();

        assert_eq!(
            formatting.dimensions.format_dimension(&velocity),
            "Velocity"
        );
        assert_eq!(formatting.dimensions.format_dimension(&length), "Length");
        assert_eq!(
            formatting
                .dimensions
                .base_unit_symbols()
                .get(&BaseDimId::Prelude(PreludeBaseDimension::Length)),
            Some(&"m".to_string())
        );
    }
}
