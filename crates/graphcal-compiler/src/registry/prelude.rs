use crate::dag_id::DagId;
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension};
use crate::ratio::RatioError;
use crate::syntax::dimension::{DimName, UnitName};

use crate::registry::types::{PositiveFiniteScale, RegistryBuilder};

/// Canonical synthetic owner for Graphcal prelude type-system symbols.
///
/// Prelude names are implicitly in scope, so they do not have a source module
/// alias. HIR still needs a canonical owner for resolved names; this synthetic
/// [`DagId`] is that owner at the compiler boundary.
const PRELUDE_DAG_ID_SEGMENT: &str = "__graphcal_prelude__";

/// Look up a prelude *base* dimension by name.
///
/// Returns the single-factor [`Dimension`] for one of
/// [`PreludeBaseDimension::ALL_NAMES`], without needing a loaded registry, and
/// `None` for every other name (including prelude-derived dimensions, which
/// exist only through registry resolution).
///
/// The base dimensions are the dimension alphabet of boundaries that carry
/// dimensions structurally as base-dimension exponent vectors — in particular
/// WASM plugin manifests (ABI v1, #25), which may reference these names and
/// nothing else. Every other dimension (prelude-derived or user-defined
/// derived) reduces to exponents over base dimensions; user-defined *base*
/// dimensions are scoped to their defining module and never cross such
/// boundaries.
#[must_use]
pub fn prelude_base_dimension(name: &str) -> Option<Dimension> {
    PreludeBaseDimension::parse(name).map(|base| Dimension::base(BaseDimId::Prelude(base)))
}

/// Non-dimension type names provided by the Graphcal prelude.
pub(crate) const PRELUDE_BUILTIN_TYPE_NAMES: &[&str] =
    &["Dimensionless", "Bool", "Int", "Datetime", "Complex", "Key"];

/// Canonical synthetic owner for Graphcal prelude symbols.
#[must_use]
pub fn prelude_dag_id() -> DagId {
    DagId::root_in_package(PRELUDE_DAG_ID_SEGMENT, PRELUDE_DAG_ID_SEGMENT)
}

// Declaration table. Registration and the public name lists are generated
// from these declarations, so they cannot drift apart.

/// A prelude dimension as integer exponents over prelude base dimensions.
type BaseFactors = &'static [(PreludeBaseDimension, i16)];

const LENGTH: BaseFactors = &[(PreludeBaseDimension::Length, 1)];
const TIME: BaseFactors = &[(PreludeBaseDimension::Time, 1)];
const MASS: BaseFactors = &[(PreludeBaseDimension::Mass, 1)];
const ANGLE: BaseFactors = &[(PreludeBaseDimension::Angle, 1)];
const FORCE: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, 1),
    (PreludeBaseDimension::Time, -2),
];
const ENERGY: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, 2),
    (PreludeBaseDimension::Time, -2),
];
const POWER: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, 2),
    (PreludeBaseDimension::Time, -3),
];
const PRESSURE: BaseFactors = &[
    (PreludeBaseDimension::Mass, 1),
    (PreludeBaseDimension::Length, -1),
    (PreludeBaseDimension::Time, -2),
];
const FREQUENCY: BaseFactors = &[(PreludeBaseDimension::Time, -1)];

/// The display symbol of a base dimension, which is also the spelling of its
/// coherent base unit (scale 1).
const fn base_symbol(base: PreludeBaseDimension) -> &'static str {
    match base {
        PreludeBaseDimension::Length => "m",
        PreludeBaseDimension::Time => "s",
        PreludeBaseDimension::Mass => "kg",
        PreludeBaseDimension::Temperature => "K",
        PreludeBaseDimension::ElectricCurrent => "A",
        PreludeBaseDimension::Amount => "mol",
        PreludeBaseDimension::LuminousIntensity => "cd",
        PreludeBaseDimension::Angle => "rad",
    }
}

/// Whether real-world units of this base dimension are affine scales.
///
/// Temperature units (°C, °F) are affine, so user unit definitions on bare
/// Temperature are rejected: a linear definition would silently display wrong
/// values (#648 U4).
const fn is_affine_prone(base: PreludeBaseDimension) -> bool {
    matches!(base, PreludeBaseDimension::Temperature)
}

/// A named prelude dimension derived from the base dimensions.
struct DerivedDimensionDecl {
    name: &'static str,
    factors: BaseFactors,
}

const fn dimension(name: &'static str, factors: BaseFactors) -> DerivedDimensionDecl {
    DerivedDimensionDecl { name, factors }
}

/// Derived prelude dimensions, in registration order.
const DERIVED_DIMENSIONS: &[DerivedDimensionDecl] = &[
    dimension(
        "Velocity",
        &[
            (PreludeBaseDimension::Length, 1),
            (PreludeBaseDimension::Time, -1),
        ],
    ),
    dimension(
        "Acceleration",
        &[
            (PreludeBaseDimension::Length, 1),
            (PreludeBaseDimension::Time, -2),
        ],
    ),
    dimension("Force", FORCE),
    dimension("Energy", ENERGY),
    dimension("Power", POWER),
    dimension("Frequency", FREQUENCY),
    dimension("Pressure", PRESSURE),
    dimension("Area", &[(PreludeBaseDimension::Length, 2)]),
    dimension("Volume", &[(PreludeBaseDimension::Length, 3)]),
];

/// A prelude unit other than a coherent base unit.
struct DerivedUnitDecl {
    name: &'static str,
    factors: BaseFactors,
    scale: PositiveFiniteScale,
}

/// Evaluated only in the `DERIVED_UNITS` const initializer, so an invalid
/// built-in scale fails the build.
const fn unit(name: &'static str, factors: BaseFactors, scale: f64) -> DerivedUnitDecl {
    DerivedUnitDecl {
        name,
        factors,
        scale: PositiveFiniteScale::from_const(scale),
    }
}

/// Prelude units other than the coherent base units, in registration order.
const DERIVED_UNITS: &[DerivedUnitDecl] = &[
    unit("km", LENGTH, 1000.0),
    unit("cm", LENGTH, 0.01),
    unit("mm", LENGTH, 0.001),
    unit("h", TIME, 3600.0),
    unit("min", TIME, 60.0),
    unit("deg", ANGLE, std::f64::consts::PI / 180.0),
    unit("g", MASS, 0.001),
    unit("N", FORCE, 1.0),
    unit("kN", FORCE, 1000.0),
    unit("J", ENERGY, 1.0),
    unit("kJ", ENERGY, 1000.0),
    unit("W", POWER, 1.0),
    unit("kW", POWER, 1000.0),
    unit("Pa", PRESSURE, 1.0),
    unit("kPa", PRESSURE, 1000.0),
    unit("MPa", PRESSURE, 1_000_000.0),
    unit("Hz", FREQUENCY, 1.0),
];

/// Dimension names provided by the Graphcal prelude: base dimensions first,
/// then derived dimensions, in registration order.
pub(crate) fn prelude_dimension_names() -> impl Iterator<Item = &'static str> + Clone {
    PreludeBaseDimension::ALL_NAMES
        .into_iter()
        .chain(DERIVED_DIMENSIONS.iter().map(|decl| decl.name))
}

/// Unit names provided by the Graphcal prelude: coherent base units first,
/// then the other units, in registration order.
pub fn prelude_unit_names() -> impl Iterator<Item = &'static str> + Clone {
    PreludeBaseDimension::ALL
        .into_iter()
        .map(base_symbol)
        .chain(DERIVED_UNITS.iter().map(|decl| decl.name))
}

fn dimension_of(factors: BaseFactors) -> Result<Dimension, RatioError> {
    factors
        .iter()
        .try_fold(Dimension::dimensionless(), |product, &(base, exponent)| {
            product * Dimension::base(BaseDimId::Prelude(base)).pow(exponent)?
        })
}

/// Load all built-in dimensions and units into the registry builder.
pub(crate) fn load_prelude(builder: &mut RegistryBuilder) -> Result<(), RatioError> {
    for base in PreludeBaseDimension::ALL {
        let id = builder.register_base_dimension_with_symbol(
            BaseDimId::Prelude(base),
            UnitName::expect_valid(base_symbol(base)),
        );
        if is_affine_prone(base) {
            builder.mark_affine_prone(id);
        }
    }
    for decl in DERIVED_DIMENSIONS {
        builder.register_dimension(
            DimName::expect_valid(decl.name),
            dimension_of(decl.factors)?,
        );
    }
    for base in PreludeBaseDimension::ALL {
        builder.register_unit(
            UnitName::expect_valid(base_symbol(base)),
            Dimension::base(BaseDimId::Prelude(base)),
            PositiveFiniteScale::ONE,
        );
    }
    for decl in DERIVED_UNITS {
        builder.register_unit(
            UnitName::expect_valid(decl.name),
            dimension_of(decl.factors)?,
            decl.scale,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::Rational;
    use crate::registry::types::{RegistryBuilder, UnitScale};

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
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
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
                r.dimensions
                    .get_dimension(&crate::syntax::dimension::DimRef::local(
                        DimName::expect_valid(name)
                    ))
                    .is_some(),
                "missing dimension: {name}"
            );
        }
    }

    #[test]
    fn prelude_loads_all_derived_dims() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
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
                r.dimensions
                    .get_dimension(&crate::syntax::dimension::DimRef::local(
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

        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();

        let listed_dims = prelude_dimension_names().collect::<BTreeSet<_>>();
        let loaded_dims = r
            .dimensions
            .all_dimensions()
            .map(|(name, _)| name.leaf().as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(listed_dims, loaded_dims);

        let listed_units = prelude_unit_names().collect::<BTreeSet<_>>();
        let loaded_units = r
            .units
            .all_units()
            .map(|(unit_ref, _)| unit_ref.leaf().as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(listed_units, loaded_units);
    }

    #[test]
    fn prelude_force_dimension_is_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let force = r
            .dimensions
            .get_dimension(&crate::syntax::dimension::DimRef::local(
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
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let force_dim = r
            .dimensions
            .get_dimension(&crate::syntax::dimension::DimRef::local(
                DimName::expect_valid("Force"),
            ))
            .unwrap()
            .clone();
        let newton = r
            .units
            .get_unit(&crate::syntax::dimension::UnitRef::local(
                crate::syntax::dimension::UnitName::expect_valid("N"),
            ))
            .unwrap();
        assert_eq!(newton.dimension, force_dim);
        assert_eq!(newton.scale, UnitScale::Const(PositiveFiniteScale::ONE));
    }

    #[test]
    fn prelude_km_scale_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let km = r
            .units
            .get_unit(&crate::syntax::dimension::UnitRef::local(
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
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let deg = r
            .units
            .get_unit(&crate::syntax::dimension::UnitRef::local(
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
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let bases: Vec<_> = r.dimensions.base_dimensions().map(|(id, _)| id).collect();
        assert_eq!(bases.len(), 8);
        assert!(bases.contains(&&length_id()));
        assert!(bases.contains(&&time_id()));
        let affine: Vec<_> = r
            .dimensions
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
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        for name in PreludeBaseDimension::ALL_NAMES {
            let expected = prelude_base_dimension(name).unwrap();
            assert_eq!(
                r.dimensions
                    .get_dimension(&crate::syntax::dimension::DimRef::local(
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
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let symbols = r.dimensions.base_unit_symbols();
        assert_eq!(symbols.len(), 8);
        assert_eq!(symbols.get(&length_id()), Some(&"m".to_string()));
        assert_eq!(symbols.get(&time_id()), Some(&"s".to_string()));
    }

    #[test]
    fn prelude_time_unit_spellings_are_canonical() {
        use crate::syntax::dimension::UnitRef;

        let mut builder = RegistryBuilder::new();
        load_prelude(&mut builder).unwrap();
        let registry = builder.build();
        let unit = |name| UnitRef::local(UnitName::expect_valid(name));

        assert_eq!(
            registry
                .units
                .get_unit(&unit("h"))
                .unwrap()
                .scale
                .static_scale()
                .map(PositiveFiniteScale::get),
            Some(3600.0)
        );
        assert_eq!(
            registry
                .units
                .get_unit(&unit("min"))
                .unwrap()
                .scale
                .static_scale()
                .map(PositiveFiniteScale::get),
            Some(60.0)
        );
        assert!(registry.units.get_unit(&unit("hour")).is_none());
    }

    /// The declaration table must reproduce the dimensions the prelude used to
    /// build with explicit products and quotients.
    #[test]
    fn declared_dimensions_match_their_defining_products() {
        let mut builder = RegistryBuilder::new();
        load_prelude(&mut builder).unwrap();
        let registry = builder.build();
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
                registry
                    .dimensions
                    .get_dimension(&crate::syntax::dimension::DimRef::local(
                        DimName::expect_valid(*name)
                    )),
                Some(dimension),
                "{name}"
            );
        }
        let unit = |name| {
            registry
                .units
                .get_unit(&crate::syntax::dimension::UnitRef::local(
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
        let mut builder = RegistryBuilder::new();
        load_prelude(&mut builder).unwrap();
        for base in PreludeBaseDimension::ALL {
            assert_eq!(
                builder.is_affine_prone(&Dimension::base(BaseDimId::Prelude(base))),
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
