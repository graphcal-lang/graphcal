use crate::dag_id::DagId;
use crate::syntax::dimension::{Dimension, RationalError};
use crate::syntax::names::{DimName, UnitName};

use crate::registry::types::{PositiveFiniteScale, RegistryBuilder};

/// Canonical synthetic owner for Graphcal prelude type-system symbols.
///
/// Prelude names are implicitly in scope, so they do not have a source module
/// alias. HIR still needs a canonical owner for resolved names; this synthetic
/// [`DagId`] is that owner at the compiler boundary.
pub const PRELUDE_DAG_ID_SEGMENT: &str = "__graphcal_prelude__";

/// Dimension names provided by the Graphcal prelude.
pub const PRELUDE_DIMENSION_NAMES: &[&str] = &[
    "Length",
    "Time",
    "Mass",
    "Temperature",
    "ElectricCurrent",
    "Amount",
    "LuminousIntensity",
    "Angle",
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

/// Non-dimension type names provided by the Graphcal prelude.
pub const PRELUDE_BUILTIN_TYPE_NAMES: &[&str] = &["Dimensionless", "Bool", "Int", "Datetime"];

/// Unit names provided by the Graphcal prelude.
pub const PRELUDE_UNIT_NAMES: &[&str] = &[
    "m", "s", "kg", "K", "A", "mol", "cd", "rad", "km", "cm", "mm", "hour", "min", "deg", "g", "N",
    "kN", "J", "kJ", "W", "kW", "Pa", "kPa", "MPa", "Hz",
];

/// Canonical synthetic owner for Graphcal prelude symbols.
#[must_use]
pub fn prelude_dag_id() -> DagId {
    DagId::root_in_package(PRELUDE_DAG_ID_SEGMENT, PRELUDE_DAG_ID_SEGMENT)
}

/// Base dimension IDs returned by `load_base_dimensions`.
///
/// Thread these through the prelude loading pipeline to avoid
/// re-looking up dimensions by name.
struct BaseDimIds {
    length: Dimension,
    time: Dimension,
    mass: Dimension,
    temperature: Dimension,
    electric_current: Dimension,
    amount: Dimension,
    luminous_intensity: Dimension,
    angle: Dimension,
}

/// Load all built-in dimensions and units into the registry builder.
pub(crate) fn load_prelude(builder: &mut RegistryBuilder) -> Result<(), RationalError> {
    let ids = load_base_dimensions(builder);
    load_derived_dimensions(builder, &ids)?;
    load_base_units(builder, &ids);
    load_derived_units(builder, &ids)?;
    Ok(())
}

fn load_base_dimensions(r: &mut RegistryBuilder) -> BaseDimIds {
    use crate::syntax::dimension::BaseDimId;

    let length_id = r.register_base_dimension_with_symbol(
        DimName::new("Length"),
        BaseDimId::Prelude("Length".to_string()),
        "m".to_string(),
    );
    let time_id = r.register_base_dimension_with_symbol(
        DimName::new("Time"),
        BaseDimId::Prelude("Time".to_string()),
        "s".to_string(),
    );
    let mass_id = r.register_base_dimension_with_symbol(
        DimName::new("Mass"),
        BaseDimId::Prelude("Mass".to_string()),
        "kg".to_string(),
    );
    let temperature_id = r.register_base_dimension_with_symbol(
        DimName::new("Temperature"),
        BaseDimId::Prelude("Temperature".to_string()),
        "K".to_string(),
    );
    // Real-world temperature units (°C, °F) are affine scales; reject user
    // unit definitions on bare Temperature so a linear definition cannot
    // silently display wrong values (#648 U4).
    r.mark_affine_prone(temperature_id.clone());
    let electric_current_id = r.register_base_dimension_with_symbol(
        DimName::new("ElectricCurrent"),
        BaseDimId::Prelude("ElectricCurrent".to_string()),
        "A".to_string(),
    );
    let amount_id = r.register_base_dimension_with_symbol(
        DimName::new("Amount"),
        BaseDimId::Prelude("Amount".to_string()),
        "mol".to_string(),
    );
    let luminous_intensity_id = r.register_base_dimension_with_symbol(
        DimName::new("LuminousIntensity"),
        BaseDimId::Prelude("LuminousIntensity".to_string()),
        "cd".to_string(),
    );
    let angle_id = r.register_base_dimension_with_symbol(
        DimName::new("Angle"),
        BaseDimId::Prelude("Angle".to_string()),
        "rad".to_string(),
    );

    BaseDimIds {
        length: Dimension::base(length_id),
        time: Dimension::base(time_id),
        mass: Dimension::base(mass_id),
        temperature: Dimension::base(temperature_id),
        electric_current: Dimension::base(electric_current_id),
        amount: Dimension::base(amount_id),
        luminous_intensity: Dimension::base(luminous_intensity_id),
        angle: Dimension::base(angle_id),
    }
}

fn load_derived_dimensions(r: &mut RegistryBuilder, ids: &BaseDimIds) -> Result<(), RationalError> {
    let velocity = (&ids.length / &ids.time)?;
    let time_squared = ids.time.pow_int(2)?;
    let acceleration = (&ids.length / &time_squared)?;
    let force = (&ids.mass * &acceleration)?;
    let energy = (&force * &ids.length)?;
    let power = (&energy / &ids.time)?;
    let frequency = (Dimension::dimensionless() / ids.time.clone())?;
    let length_squared = ids.length.pow_int(2)?;
    let pressure = (&force / &length_squared)?;
    let area = ids.length.pow_int(2)?;
    let volume = ids.length.pow_int(3)?;

    r.register_dimension(DimName::new("Velocity"), velocity);
    r.register_dimension(DimName::new("Acceleration"), acceleration);
    r.register_dimension(DimName::new("Force"), force);
    r.register_dimension(DimName::new("Energy"), energy);
    r.register_dimension(DimName::new("Power"), power);
    r.register_dimension(DimName::new("Frequency"), frequency);
    r.register_dimension(DimName::new("Pressure"), pressure);
    r.register_dimension(DimName::new("Area"), area);
    r.register_dimension(DimName::new("Volume"), volume);
    Ok(())
}

const fn prelude_scale(value: f64) -> PositiveFiniteScale {
    PositiveFiniteScale::new_unchecked(value)
}

fn load_base_units(r: &mut RegistryBuilder, ids: &BaseDimIds) {
    r.register_unit(UnitName::new("m"), ids.length.clone(), prelude_scale(1.0));
    r.register_unit(UnitName::new("s"), ids.time.clone(), prelude_scale(1.0));
    r.register_unit(UnitName::new("kg"), ids.mass.clone(), prelude_scale(1.0));
    r.register_unit(
        UnitName::new("K"),
        ids.temperature.clone(),
        prelude_scale(1.0),
    );
    r.register_unit(
        UnitName::new("A"),
        ids.electric_current.clone(),
        prelude_scale(1.0),
    );
    r.register_unit(UnitName::new("mol"), ids.amount.clone(), prelude_scale(1.0));
    r.register_unit(
        UnitName::new("cd"),
        ids.luminous_intensity.clone(),
        prelude_scale(1.0),
    );
    r.register_unit(UnitName::new("rad"), ids.angle.clone(), prelude_scale(1.0));
}

fn load_derived_units(r: &mut RegistryBuilder, ids: &BaseDimIds) -> Result<(), RationalError> {
    let mass_length = (&ids.mass * &ids.length)?;
    let time_squared = ids.time.pow_int(2)?;
    let force = (mass_length / time_squared)?;
    let energy = (&force * &ids.length)?;
    let power = (&energy / &ids.time)?;
    let length_squared = ids.length.pow_int(2)?;
    let pressure = (&force / &length_squared)?;
    let frequency = (Dimension::dimensionless() / ids.time.clone())?;

    // Length
    r.register_unit(
        UnitName::new("km"),
        ids.length.clone(),
        prelude_scale(1000.0),
    );
    r.register_unit(UnitName::new("cm"), ids.length.clone(), prelude_scale(0.01));
    r.register_unit(
        UnitName::new("mm"),
        ids.length.clone(),
        prelude_scale(0.001),
    );

    // Time
    r.register_unit(
        UnitName::new("hour"),
        ids.time.clone(),
        prelude_scale(3600.0),
    );
    r.register_unit(UnitName::new("min"), ids.time.clone(), prelude_scale(60.0));

    // Angle
    r.register_unit(
        UnitName::new("deg"),
        ids.angle.clone(),
        prelude_scale(std::f64::consts::PI / 180.0),
    );

    // Mass
    r.register_unit(UnitName::new("g"), ids.mass.clone(), prelude_scale(0.001));

    // Force
    r.register_unit(UnitName::new("N"), force.clone(), prelude_scale(1.0));
    r.register_unit(UnitName::new("kN"), force, prelude_scale(1000.0));

    // Energy
    r.register_unit(UnitName::new("J"), energy.clone(), prelude_scale(1.0));
    r.register_unit(UnitName::new("kJ"), energy, prelude_scale(1000.0));

    // Power
    r.register_unit(UnitName::new("W"), power.clone(), prelude_scale(1.0));
    r.register_unit(UnitName::new("kW"), power, prelude_scale(1000.0));

    // Pressure
    r.register_unit(UnitName::new("Pa"), pressure.clone(), prelude_scale(1.0));
    r.register_unit(
        UnitName::new("kPa"),
        pressure.clone(),
        prelude_scale(1000.0),
    );
    r.register_unit(UnitName::new("MPa"), pressure, prelude_scale(1_000_000.0));

    // Frequency
    r.register_unit(UnitName::new("Hz"), frequency, prelude_scale(1.0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types::RegistryBuilder;
    use crate::syntax::dimension::{BaseDimId, Rational};

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
                r.dimensions.get_dimension(name).is_some(),
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
                r.dimensions.get_dimension(name).is_some(),
                "missing dimension: {name}"
            );
        }
    }

    #[test]
    fn prelude_force_dimension_is_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let force = r.dimensions.get_dimension("Force").unwrap();
        // Force = Mass * Length / Time^2
        assert_eq!(force.get_exponent(&mass_id()), Rational::ONE);
        assert_eq!(force.get_exponent(&length_id()), Rational::ONE);
        assert_eq!(force.get_exponent(&time_id()), Rational::from_int(-2));
    }

    #[test]
    fn prelude_newton_matches_force_dim() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let force_dim = r.dimensions.get_dimension("Force").unwrap().clone();
        let newton = r
            .units
            .get_unit(&crate::syntax::names::UnitRef::local("N"))
            .unwrap();
        assert_eq!(newton.dimension, force_dim);
        assert!((newton.scale.as_static().unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prelude_km_scale_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let km = r
            .units
            .get_unit(&crate::syntax::names::UnitRef::local("km"))
            .unwrap();
        assert!((km.scale.as_static().unwrap() - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prelude_deg_scale_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let deg = r
            .units
            .get_unit(&crate::syntax::names::UnitRef::local("deg"))
            .unwrap();
        assert!((deg.scale.as_static().unwrap() - std::f64::consts::PI / 180.0).abs() < 1e-15);
    }

    #[test]
    fn prelude_base_dim_names_registered() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let names = r.dimensions.base_dim_names();
        assert_eq!(names.len(), 8);
        assert_eq!(names.get(&length_id()), Some(&"Length".to_string()));
        assert_eq!(names.get(&time_id()), Some(&"Time".to_string()));
    }

    #[test]
    fn prelude_base_dim_symbols_registered() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b).unwrap();
        let r = b.build();
        let symbols = r.dimensions.base_dim_symbols();
        assert_eq!(symbols.len(), 8);
        assert_eq!(symbols.get(&length_id()), Some(&"m".to_string()));
        assert_eq!(symbols.get(&time_id()), Some(&"s".to_string()));
    }
}
