//! The Graphcal prelude as owner-qualified static definitions.
//!
//! The prelude catalog ([`crate::semantic::prelude`]) lists the dimensions
//! and units; the resolver's prelude scope names them. This module owns the
//! only place the two meet.

use crate::dimension::BaseDimId;
use crate::ir::module_definitions::{ForeignDefinitionError, StaticDefinitions};
use crate::ratio::RatioError;
use crate::resolve::prelude::prelude_type_scope;
use crate::semantic::prelude::{
    prelude_base_dimension_infos, prelude_dag_id, prelude_dimensions, prelude_units,
};
use crate::syntax::dimension::{DimName, UnitName};

/// Failure to construct the built-in prelude definitions (a compiler bug).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreludeDefinitionError {
    #[error(transparent)]
    Dimension(#[from] RatioError),
    #[error(transparent)]
    Owner(#[from] ForeignDefinitionError),
    #[error("the prelude does not declare `{0}`")]
    Undeclared(&'static str),
}

/// The Graphcal prelude's dimensions, units, and base-dimension metadata,
/// owned by the synthetic [`prelude_dag_id`].
///
/// # Errors
///
/// Returns an error only if the built-in declarations are inconsistent,
/// which would be a compiler bug.
pub fn prelude_definitions() -> Result<StaticDefinitions, PreludeDefinitionError> {
    let scope = prelude_type_scope();
    let mut definitions = StaticDefinitions::new(prelude_dag_id());
    for (base, info) in prelude_base_dimension_infos() {
        definitions.insert_base_dimension(BaseDimId::Prelude(base), info);
    }
    for (name, dimension) in prelude_dimensions()? {
        let identity = scope
            .dimension(&DimName::expect_valid(name))
            .ok_or(PreludeDefinitionError::Undeclared(name))?;
        definitions.insert_dimension(identity, dimension)?;
    }
    for (name, info) in prelude_units()? {
        let identity = scope
            .unit(&UnitName::expect_valid(name))
            .ok_or(PreludeDefinitionError::Undeclared(name))?;
        definitions.insert_unit(identity, info)?;
    }
    Ok(definitions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::{Dimension, PreludeBaseDimension, Rational};
    use crate::semantic::prelude::{
        prelude_base_dimension, prelude_dimension_names, prelude_unit_names,
    };
    use crate::semantic::unit_scale::{PositiveFiniteScale, UnitScale};
    use crate::syntax::dimension::{DimRef, UnitRef};

    /// The prelude as source-spelled lookups, mirroring the implicit scope.
    struct Prelude {
        definitions: crate::ir::module_definitions::StaticDefinitions,
    }

    impl Prelude {
        fn load() -> Self {
            Self {
                definitions: prelude_definitions().unwrap(),
            }
        }

        fn dimension(&self, reference: &DimRef) -> Option<&Dimension> {
            self.definitions
                .dimensions()
                .find(|(identity, _)| {
                    !reference.is_qualified() && identity.atom() == reference.leaf().atom()
                })
                .map(|(_, dimension)| dimension)
        }

        fn unit(&self, reference: &UnitRef) -> Option<&crate::semantic::unit_scale::UnitInfo> {
            self.definitions
                .units()
                .find(|(identity, _)| {
                    !reference.is_qualified() && identity.atom() == reference.leaf().atom()
                })
                .map(|(_, info)| info)
        }
    }

    // Well-known IDs matching prelude dimension names.
    fn length_id() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Length)
    }
    fn time_id() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Time)
    }
    fn mass_id() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Mass)
    }

    #[test]
    fn prelude_loads_all_base_dims() {
        let r = Prelude::load();
        for name in [
            "Length",
            "Time",
            "Mass",
            "Temperature",
            "ElectricCurrent",
            "Amount",
            "LuminousIntensity",
            "Angle",
        ] {
            assert!(
                r.dimension(&crate::syntax::dimension::DimRef::local(
                    DimName::expect_valid(name)
                ))
                .is_some(),
                "missing dimension: {name}"
            );
        }
    }

    #[test]
    fn prelude_loads_all_derived_dims() {
        let r = Prelude::load();
        for name in [
            "Velocity",
            "Acceleration",
            "Force",
            "Energy",
            "Power",
            "Frequency",
            "Pressure",
            "Area",
            "Volume",
        ] {
            assert!(
                r.dimension(&crate::syntax::dimension::DimRef::local(
                    DimName::expect_valid(name)
                ))
                .is_some(),
                "missing dimension: {name}"
            );
        }
    }

    #[test]
    fn prelude_name_lists_match_loaded_registry() {
        use std::collections::BTreeSet;

        let r = Prelude::load();

        let listed_dims = prelude_dimension_names().collect::<BTreeSet<_>>();
        let loaded_dims = r
            .definitions
            .dimensions()
            .map(|(name, _)| name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(listed_dims, loaded_dims);

        let listed_units = prelude_unit_names().collect::<BTreeSet<_>>();
        let loaded_units = r
            .definitions
            .units()
            .map(|(unit, _)| unit.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(listed_units, loaded_units);
    }

    #[test]
    fn prelude_force_dimension_is_correct() {
        let r = Prelude::load();
        let force = r
            .dimension(&crate::syntax::dimension::DimRef::local(
                DimName::expect_valid("Force"),
            ))
            .unwrap();
        // Force = Mass * Length / Time^2
        assert_eq!(force.get_exponent(&mass_id()), Rational::ONE);
        assert_eq!(force.get_exponent(&length_id()), Rational::ONE);
        assert_eq!(force.get_exponent(&time_id()), Rational::from(-2));
    }

    #[test]
    fn prelude_newton_matches_force_dim() {
        let r = Prelude::load();
        let force_dim = r
            .dimension(&crate::syntax::dimension::DimRef::local(
                DimName::expect_valid("Force"),
            ))
            .unwrap()
            .clone();
        let newton = r
            .unit(&crate::syntax::dimension::UnitRef::local(
                crate::syntax::dimension::UnitName::expect_valid("N"),
            ))
            .unwrap();
        assert_eq!(newton.dimension, force_dim);
        assert_eq!(newton.scale, UnitScale::Const(PositiveFiniteScale::ONE));
    }

    #[test]
    fn prelude_km_scale_correct() {
        let r = Prelude::load();
        let km = r
            .unit(&crate::syntax::dimension::UnitRef::local(
                UnitName::expect_valid("km"),
            ))
            .unwrap();
        assert_eq!(
            km.scale.static_scale().map(PositiveFiniteScale::get),
            Some(1000.0)
        );
    }

    #[test]
    fn prelude_deg_scale_correct() {
        let r = Prelude::load();
        let deg = r
            .unit(&crate::syntax::dimension::UnitRef::local(
                crate::syntax::dimension::UnitName::expect_valid("deg"),
            ))
            .unwrap();
        assert_eq!(
            deg.scale.static_scale().map(PositiveFiniteScale::get),
            Some(std::f64::consts::PI / 180.0)
        );
    }

    #[test]
    fn prelude_base_dimensions_registered() {
        let r = Prelude::load();
        let bases: Vec<_> = r.definitions.base_dimensions().map(|(id, _)| id).collect();
        assert_eq!(bases.len(), 8);
        assert!(bases.contains(&&length_id()));
        assert!(bases.contains(&&time_id()));
        let affine: Vec<_> = r
            .definitions
            .base_dimensions()
            .filter(|(_, info)| info.is_affine_prone())
            .map(|(id, _)| id.name())
            .collect();
        assert_eq!(affine, ["Temperature"]);
    }

    #[test]
    fn base_dimension_names_const_matches_registrations() {
        assert!(
            prelude_dimension_names()
                .take(PreludeBaseDimension::ALL.len())
                .eq(PreludeBaseDimension::ALL_NAMES)
        );
        let r = Prelude::load();
        for name in PreludeBaseDimension::ALL_NAMES {
            let expected = prelude_base_dimension(name).unwrap();
            assert_eq!(
                r.dimension(&crate::syntax::dimension::DimRef::local(
                    DimName::expect_valid(name)
                )),
                Some(&expected)
            );
        }
        assert_eq!(prelude_base_dimension("Velocity"), None);
        assert_eq!(prelude_base_dimension("NotADimension"), None);
    }

    #[test]
    fn prelude_base_dim_symbols_registered() {
        let r = Prelude::load();
        let symbols = r
            .definitions
            .base_dimensions()
            .filter_map(|(id, info)| Some((id.clone(), info.canonical_unit()?.to_string())))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(symbols.len(), 8);
        assert_eq!(symbols.get(&length_id()), Some(&"m".to_string()));
        assert_eq!(symbols.get(&time_id()), Some(&"s".to_string()));
    }

    #[test]
    fn prelude_time_unit_spellings_are_canonical() {
        use crate::syntax::dimension::UnitRef;

        let registry = Prelude::load();
        let unit = |name| UnitRef::local(UnitName::expect_valid(name));

        assert_eq!(
            registry
                .unit(&unit("h"))
                .unwrap()
                .scale
                .static_scale()
                .map(PositiveFiniteScale::get),
            Some(3600.0)
        );
        assert_eq!(
            registry
                .unit(&unit("min"))
                .unwrap()
                .scale
                .static_scale()
                .map(PositiveFiniteScale::get),
            Some(60.0)
        );
        assert!(registry.unit(&unit("hour")).is_none());
    }

    /// The declaration table must reproduce the dimensions the prelude used to
    /// build with explicit products and quotients.
    #[test]
    fn declared_dimensions_match_their_defining_products() {
        let registry = Prelude::load();
        let base = |base| Dimension::base(BaseDimId::Prelude(base));
        let length = base(PreludeBaseDimension::Length);
        let time = base(PreludeBaseDimension::Time);
        let mass = base(PreludeBaseDimension::Mass);
        let velocity = (&length / &time).unwrap();
        let acceleration = (&velocity / &time).unwrap();
        let force = (&mass * &acceleration).unwrap();
        let energy = (&force * &length).unwrap();
        let power = (&energy / &time).unwrap();
        let area = length.pow(2).unwrap();
        let expected = [
            ("Velocity", velocity),
            ("Acceleration", acceleration),
            ("Force", force.clone()),
            ("Energy", energy.clone()),
            ("Power", power.clone()),
            ("Frequency", (Dimension::dimensionless() / time).unwrap()),
            ("Pressure", (&force / &area).unwrap()),
            ("Area", area),
            ("Volume", length.pow(3).unwrap()),
        ];
        for (name, dimension) in &expected {
            assert_eq!(
                registry.dimension(&crate::syntax::dimension::DimRef::local(
                    DimName::expect_valid(*name)
                )),
                Some(dimension),
                "{name}"
            );
        }
        let unit = |name| {
            registry
                .unit(&crate::syntax::dimension::UnitRef::local(
                    UnitName::expect_valid(name),
                ))
                .unwrap()
        };
        for (name, dimension, scale) in [
            ("m", &length, 1.0),
            ("rad", &base(PreludeBaseDimension::Angle), 1.0),
            ("mm", &length, 0.001),
            ("g", &mass, 0.001),
            ("kJ", &energy, 1000.0),
            ("kW", &power, 1000.0),
            ("MPa", &expected[6].1, 1_000_000.0),
            ("Hz", &expected[5].1, 1.0),
        ] {
            assert_eq!(&unit(name).dimension, dimension, "{name}");
            assert_eq!(
                unit(name)
                    .scale
                    .static_scale()
                    .map(PositiveFiniteScale::get),
                Some(scale),
                "{name}"
            );
        }
    }

    #[test]
    fn only_bare_temperature_is_affine_prone() {
        let prelude = Prelude::load();
        for base in PreludeBaseDimension::ALL {
            assert_eq!(
                prelude
                    .definitions
                    .base_dimensions()
                    .find(|(id, _)| **id == BaseDimId::Prelude(base))
                    .is_some_and(|(_, info)| info.is_affine_prone()),
                base == PreludeBaseDimension::Temperature,
                "{base}"
            );
        }
    }

    #[test]
    fn generated_name_lists_follow_registration_order() {
        let dimensions = prelude_dimension_names().collect::<Vec<_>>();
        assert_eq!(dimensions.len(), 17);
        assert_eq!(dimensions[..2], ["Length", "Time"]);
        assert_eq!(dimensions[8..10], ["Velocity", "Acceleration"]);
        let units = prelude_unit_names().collect::<Vec<_>>();
        assert_eq!(
            units,
            [
                "m", "s", "kg", "K", "A", "mol", "cd", "rad", "km", "cm", "mm", "h", "min", "deg",
                "g", "N", "kN", "J", "kJ", "W", "kW", "Pa", "kPa", "MPa", "Hz",
            ]
        );
    }
}
