use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{DimName, UnitName};

use crate::registry::Registry;

/// Load all built-in dimensions and units into the registry.
pub fn load_prelude(registry: &mut Registry) {
    load_base_dimensions(registry);
    load_derived_dimensions(registry);
    load_base_units(registry);
    load_derived_units(registry);
}

fn load_base_dimensions(r: &mut Registry) {
    r.register_base_dimension_with_symbol(DimName::new("Length"), "m".to_string());
    r.register_base_dimension_with_symbol(DimName::new("Time"), "s".to_string());
    r.register_base_dimension_with_symbol(DimName::new("Mass"), "kg".to_string());
    r.register_base_dimension_with_symbol(DimName::new("Temperature"), "K".to_string());
    r.register_base_dimension_with_symbol(DimName::new("ElectricCurrent"), "A".to_string());
    r.register_base_dimension_with_symbol(DimName::new("Amount"), "mol".to_string());
    r.register_base_dimension_with_symbol(DimName::new("LuminousIntensity"), "cd".to_string());
    r.register_base_dimension_with_symbol(DimName::new("Angle"), "rad".to_string());
}

#[expect(
    clippy::unwrap_used,
    reason = "base dimensions are always registered before derived"
)]
fn load_derived_dimensions(r: &mut Registry) {
    let length = r.get_dimension("Length").unwrap().clone();
    let time = r.get_dimension("Time").unwrap().clone();
    let mass = r.get_dimension("Mass").unwrap().clone();

    let velocity = length.clone() / time.clone();
    let acceleration = length.clone() / time.pow_int(2);
    let force = mass * acceleration.clone();
    let energy = force.clone() * length.clone();
    let power = energy.clone() / time.clone();
    let frequency = Dimension::dimensionless() / time;
    let pressure = force.clone() / length.pow_int(2);
    let area = length.pow_int(2);
    let volume = length.pow_int(3);

    r.register_dimension(DimName::new("Velocity"), velocity);
    r.register_dimension(DimName::new("Acceleration"), acceleration);
    r.register_dimension(DimName::new("Force"), force);
    r.register_dimension(DimName::new("Energy"), energy);
    r.register_dimension(DimName::new("Power"), power);
    r.register_dimension(DimName::new("Frequency"), frequency);
    r.register_dimension(DimName::new("Pressure"), pressure);
    r.register_dimension(DimName::new("Area"), area);
    r.register_dimension(DimName::new("Volume"), volume);
}

#[expect(
    clippy::unwrap_used,
    reason = "dimensions are always registered before units"
)]
fn load_base_units(r: &mut Registry) {
    let length = r.get_dimension("Length").unwrap().clone();
    let time = r.get_dimension("Time").unwrap().clone();
    let mass = r.get_dimension("Mass").unwrap().clone();
    let temperature = r.get_dimension("Temperature").unwrap().clone();
    let electric_current = r.get_dimension("ElectricCurrent").unwrap().clone();
    let amount = r.get_dimension("Amount").unwrap().clone();
    let luminous_intensity = r.get_dimension("LuminousIntensity").unwrap().clone();
    let angle = r.get_dimension("Angle").unwrap().clone();

    r.register_unit(UnitName::new("m"), length, 1.0);
    r.register_unit(UnitName::new("s"), time, 1.0);
    r.register_unit(UnitName::new("kg"), mass, 1.0);
    r.register_unit(UnitName::new("K"), temperature, 1.0);
    r.register_unit(UnitName::new("A"), electric_current, 1.0);
    r.register_unit(UnitName::new("mol"), amount, 1.0);
    r.register_unit(UnitName::new("cd"), luminous_intensity, 1.0);
    r.register_unit(UnitName::new("rad"), angle, 1.0);
}

#[expect(
    clippy::unwrap_used,
    reason = "dimensions are always registered before units"
)]
fn load_derived_units(r: &mut Registry) {
    let length = r.get_dimension("Length").unwrap().clone();
    let time = r.get_dimension("Time").unwrap().clone();
    let mass = r.get_dimension("Mass").unwrap().clone();
    let angle = r.get_dimension("Angle").unwrap().clone();

    let force = mass.clone() * length.clone() / time.pow_int(2);
    let energy = force.clone() * length.clone();
    let power = energy.clone() / time.clone();
    let pressure = force.clone() / length.pow_int(2);
    let frequency = Dimension::dimensionless() / time;

    // Length
    r.register_unit(UnitName::new("km"), length.clone(), 1000.0);
    r.register_unit(UnitName::new("cm"), length.clone(), 0.01);
    r.register_unit(UnitName::new("mm"), length, 0.001);

    // Time
    r.register_unit(
        UnitName::new("hour"),
        r.get_dimension("Time").unwrap().clone(),
        3600.0,
    );
    r.register_unit(
        UnitName::new("min"),
        r.get_dimension("Time").unwrap().clone(),
        60.0,
    );

    // Angle
    r.register_unit(UnitName::new("deg"), angle, std::f64::consts::PI / 180.0);

    // Mass
    r.register_unit(UnitName::new("g"), mass, 0.001);

    // Force
    r.register_unit(UnitName::new("N"), force.clone(), 1.0);
    r.register_unit(UnitName::new("kN"), force, 1000.0);

    // Energy
    r.register_unit(UnitName::new("J"), energy.clone(), 1.0);
    r.register_unit(UnitName::new("kJ"), energy, 1000.0);

    // Power
    r.register_unit(UnitName::new("W"), power.clone(), 1.0);
    r.register_unit(UnitName::new("kW"), power, 1000.0);

    // Pressure
    r.register_unit(UnitName::new("Pa"), pressure.clone(), 1.0);
    r.register_unit(UnitName::new("kPa"), pressure.clone(), 1000.0);
    r.register_unit(UnitName::new("MPa"), pressure, 1_000_000.0);

    // Frequency
    r.register_unit(UnitName::new("Hz"), frequency, 1.0);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use graphcal_syntax::dimension::{BaseDimId, Rational};

    // Well-known IDs matching registration order in load_base_dimensions.
    const LENGTH_ID: BaseDimId = BaseDimId(0);
    const TIME_ID: BaseDimId = BaseDimId(1);
    const MASS_ID: BaseDimId = BaseDimId(2);

    #[test]
    fn prelude_loads_all_base_dims() {
        let mut r = Registry::new();
        load_prelude(&mut r);
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
            assert!(r.get_dimension(name).is_some(), "missing dimension: {name}");
        }
    }

    #[test]
    fn prelude_loads_all_derived_dims() {
        let mut r = Registry::new();
        load_prelude(&mut r);
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
            assert!(r.get_dimension(name).is_some(), "missing dimension: {name}");
        }
    }

    #[test]
    fn prelude_force_dimension_is_correct() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let force = r.get_dimension("Force").unwrap();
        // Force = Mass * Length / Time^2
        assert_eq!(force.get_exponent(MASS_ID), Rational::ONE);
        assert_eq!(force.get_exponent(LENGTH_ID), Rational::ONE);
        assert_eq!(force.get_exponent(TIME_ID), Rational::new(-2, 1));
    }

    #[test]
    fn prelude_newton_matches_force_dim() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let force_dim = r.get_dimension("Force").unwrap().clone();
        let newton = r.get_unit("N").unwrap();
        assert_eq!(newton.dimension, force_dim);
        assert!((newton.scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prelude_km_scale_correct() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let km = r.get_unit("km").unwrap();
        assert!((km.scale - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prelude_deg_scale_correct() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let deg = r.get_unit("deg").unwrap();
        assert!((deg.scale - std::f64::consts::PI / 180.0).abs() < 1e-15);
    }

    #[test]
    fn prelude_base_dim_names_registered() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let names = r.base_dim_names();
        assert_eq!(names.len(), 8);
        assert_eq!(names.get(&LENGTH_ID), Some(&"Length".to_string()));
        assert_eq!(names.get(&TIME_ID), Some(&"Time".to_string()));
    }

    #[test]
    fn prelude_base_dim_symbols_registered() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let symbols = r.base_dim_symbols();
        assert_eq!(symbols.len(), 8);
        assert_eq!(symbols.get(&LENGTH_ID), Some(&"m".to_string()));
        assert_eq!(symbols.get(&TIME_ID), Some(&"s".to_string()));
    }
}
