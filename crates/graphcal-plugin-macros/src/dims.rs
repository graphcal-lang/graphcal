//! The dimension vocabulary `plugin!` signatures may name.
//!
//! Manifests carry fixed dimensions structurally, as exponent vectors over
//! the eight prelude *base* dimensions — that alphabet is the ABI's, not
//! this crate's. The derived names below are macro-time sugar so plugin
//! authors write the same vocabulary as the `.gcl` import site
//! (`Pressure`, not `Mass * Length^-1 * Time^-2`); they expand to base
//! exponents before anything reaches the manifest.
//!
//! Both tables mirror the graphcal prelude (`registry::prelude` in
//! `graphcal-compiler`). The compiler cannot be a dependency of a
//! proc-macro crate plugin authors build, so the mirror is verified from
//! the other side: `graphcal-plugin`'s integration tests compile `.gcl`
//! extern declarations spelling each name and check the loader accepts the
//! macro-produced manifest as structurally equivalent.

/// Prelude base dimension names in canonical (manifest) order.
pub const BASE_DIMENSION_NAMES: [&str; 8] = [
    "Length",
    "Time",
    "Mass",
    "Temperature",
    "ElectricCurrent",
    "Amount",
    "LuminousIntensity",
    "Angle",
];

/// Prelude derived dimension names accepted as sugar, for diagnostics.
pub const DERIVED_DIMENSION_NAMES: [&str; 9] = [
    "Velocity",
    "Acceleration",
    "Force",
    "Energy",
    "Power",
    "Frequency",
    "Pressure",
    "Area",
    "Volume",
];

const LENGTH: usize = 0;
const TIME: usize = 1;
const MASS: usize = 2;

/// Index of a prelude *base* dimension in [`BASE_DIMENSION_NAMES`].
pub fn base_dimension_index(name: &str) -> Option<usize> {
    BASE_DIMENSION_NAMES.iter().position(|base| *base == name)
}

/// Expansion of a prelude *derived* dimension as `(base index, exponent)`
/// factors.
pub fn derived_dimension_factors(name: &str) -> Option<&'static [(usize, i64)]> {
    Some(match name {
        "Velocity" => &[(LENGTH, 1), (TIME, -1)],
        "Acceleration" => &[(LENGTH, 1), (TIME, -2)],
        "Force" => &[(MASS, 1), (LENGTH, 1), (TIME, -2)],
        "Energy" => &[(MASS, 1), (LENGTH, 2), (TIME, -2)],
        "Power" => &[(MASS, 1), (LENGTH, 2), (TIME, -3)],
        "Frequency" => &[(TIME, -1)],
        "Pressure" => &[(MASS, 1), (LENGTH, -1), (TIME, -2)],
        "Area" => &[(LENGTH, 2)],
        "Volume" => &[(LENGTH, 3)],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_derived_name_expands() {
        for name in DERIVED_DIMENSION_NAMES {
            assert!(derived_dimension_factors(name).is_some(), "missing {name}");
        }
        assert!(derived_dimension_factors("Length").is_none());
        assert!(derived_dimension_factors("Density").is_none());
    }

    #[test]
    fn base_indices_follow_manifest_order() {
        assert_eq!(base_dimension_index("Length"), Some(0));
        assert_eq!(base_dimension_index("Angle"), Some(7));
        assert_eq!(base_dimension_index("Velocity"), None);
    }
}
