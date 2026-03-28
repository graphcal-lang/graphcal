use crate::syntax::dimension::Dimension;
use crate::syntax::names::{DimName, UnitName};

use crate::registry::registry::RegistryBuilder;

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
pub fn load_prelude(builder: &mut RegistryBuilder) {
    let ids = load_base_dimensions(builder);
    load_derived_dimensions(builder, &ids);
    load_base_units(builder, &ids);
    load_derived_units(builder, &ids);
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

fn load_derived_dimensions(r: &mut RegistryBuilder, ids: &BaseDimIds) {
    let velocity = ids.length.clone() / ids.time.clone();
    let acceleration = ids.length.clone() / ids.time.pow_int(2);
    let force = ids.mass.clone() * acceleration.clone();
    let energy = force.clone() * ids.length.clone();
    let power = energy.clone() / ids.time.clone();
    let frequency = Dimension::dimensionless() / ids.time.clone();
    let pressure = force.clone() / ids.length.pow_int(2);
    let area = ids.length.pow_int(2);
    let volume = ids.length.pow_int(3);

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

fn load_base_units(r: &mut RegistryBuilder, ids: &BaseDimIds) {
    r.register_unit(UnitName::new("m"), ids.length.clone(), 1.0);
    r.register_unit(UnitName::new("s"), ids.time.clone(), 1.0);
    r.register_unit(UnitName::new("kg"), ids.mass.clone(), 1.0);
    r.register_unit(UnitName::new("K"), ids.temperature.clone(), 1.0);
    r.register_unit(UnitName::new("A"), ids.electric_current.clone(), 1.0);
    r.register_unit(UnitName::new("mol"), ids.amount.clone(), 1.0);
    r.register_unit(UnitName::new("cd"), ids.luminous_intensity.clone(), 1.0);
    r.register_unit(UnitName::new("rad"), ids.angle.clone(), 1.0);
}

fn load_derived_units(r: &mut RegistryBuilder, ids: &BaseDimIds) {
    let force = ids.mass.clone() * ids.length.clone() / ids.time.pow_int(2);
    let energy = force.clone() * ids.length.clone();
    let power = energy.clone() / ids.time.clone();
    let pressure = force.clone() / ids.length.pow_int(2);
    let frequency = Dimension::dimensionless() / ids.time.clone();

    // Length
    r.register_unit(UnitName::new("km"), ids.length.clone(), 1000.0);
    r.register_unit(UnitName::new("cm"), ids.length.clone(), 0.01);
    r.register_unit(UnitName::new("mm"), ids.length.clone(), 0.001);

    // Time
    r.register_unit(UnitName::new("hour"), ids.time.clone(), 3600.0);
    r.register_unit(UnitName::new("min"), ids.time.clone(), 60.0);

    // Angle
    r.register_unit(
        UnitName::new("deg"),
        ids.angle.clone(),
        std::f64::consts::PI / 180.0,
    );

    // Mass
    r.register_unit(UnitName::new("g"), ids.mass.clone(), 0.001);

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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
    use super::*;
    use crate::registry::registry::RegistryBuilder;
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
        load_prelude(&mut b);
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
        load_prelude(&mut b);
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
        load_prelude(&mut b);
        let r = b.build();
        let force = r.dimensions.get_dimension("Force").unwrap();
        // Force = Mass * Length / Time^2
        assert_eq!(force.get_exponent(&mass_id()), Rational::ONE);
        assert_eq!(force.get_exponent(&length_id()), Rational::ONE);
        assert_eq!(force.get_exponent(&time_id()), Rational::new(-2, 1));
    }

    #[test]
    fn prelude_newton_matches_force_dim() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        let r = b.build();
        let force_dim = r.dimensions.get_dimension("Force").unwrap().clone();
        let newton = r.units.get_unit("N").unwrap();
        assert_eq!(newton.dimension, force_dim);
        assert!((newton.scale.as_static().unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prelude_km_scale_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        let r = b.build();
        let km = r.units.get_unit("km").unwrap();
        assert!((km.scale.as_static().unwrap() - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prelude_deg_scale_correct() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        let r = b.build();
        let deg = r.units.get_unit("deg").unwrap();
        assert!((deg.scale.as_static().unwrap() - std::f64::consts::PI / 180.0).abs() < 1e-15);
    }

    #[test]
    fn prelude_base_dim_names_registered() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        let r = b.build();
        let names = r.dimensions.base_dim_names();
        assert_eq!(names.len(), 8);
        assert_eq!(names.get(&length_id()), Some(&"Length".to_string()));
        assert_eq!(names.get(&time_id()), Some(&"Time".to_string()));
    }

    #[test]
    fn prelude_base_dim_symbols_registered() {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        let r = b.build();
        let symbols = r.dimensions.base_dim_symbols();
        assert_eq!(symbols.len(), 8);
        assert_eq!(symbols.get(&length_id()), Some(&"m".to_string()));
        assert_eq!(symbols.get(&time_id()), Some(&"s".to_string()));
    }
}
