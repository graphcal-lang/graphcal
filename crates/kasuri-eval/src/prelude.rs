use kasuri_syntax::dimension::{BaseDim, Dimension};

use crate::registry::Registry;

/// Load all built-in dimensions and units into the registry.
pub fn load_prelude(registry: &mut Registry) {
    load_base_dimensions(registry);
    load_derived_dimensions(registry);
    load_base_units(registry);
    load_derived_units(registry);
}

fn load_base_dimensions(r: &mut Registry) {
    r.register_dimension("Length", Dimension::base(BaseDim::Length));
    r.register_dimension("Time", Dimension::base(BaseDim::Time));
    r.register_dimension("Mass", Dimension::base(BaseDim::Mass));
    r.register_dimension("Temperature", Dimension::base(BaseDim::Temperature));
    r.register_dimension("ElectricCurrent", Dimension::base(BaseDim::ElectricCurrent));
    r.register_dimension("Amount", Dimension::base(BaseDim::Amount));
    r.register_dimension(
        "LuminousIntensity",
        Dimension::base(BaseDim::LuminousIntensity),
    );
    r.register_dimension("Angle", Dimension::base(BaseDim::Angle));
}

fn load_derived_dimensions(r: &mut Registry) {
    let length = Dimension::base(BaseDim::Length);
    let time = Dimension::base(BaseDim::Time);
    let mass = Dimension::base(BaseDim::Mass);

    let velocity = length / time;
    let acceleration = length / time.pow_int(2);
    let force = mass * acceleration;
    let energy = force * length;
    let power = energy / time;
    let frequency = Dimension::DIMENSIONLESS / time;
    let pressure = force / length.pow_int(2);
    let area = length.pow_int(2);
    let volume = length.pow_int(3);

    r.register_dimension("Velocity", velocity);
    r.register_dimension("Acceleration", acceleration);
    r.register_dimension("Force", force);
    r.register_dimension("Energy", energy);
    r.register_dimension("Power", power);
    r.register_dimension("Frequency", frequency);
    r.register_dimension("Pressure", pressure);
    r.register_dimension("Area", area);
    r.register_dimension("Volume", volume);
}

fn load_base_units(r: &mut Registry) {
    r.register_unit("m", Dimension::base(BaseDim::Length), 1.0);
    r.register_unit("s", Dimension::base(BaseDim::Time), 1.0);
    r.register_unit("kg", Dimension::base(BaseDim::Mass), 1.0);
    r.register_unit("K", Dimension::base(BaseDim::Temperature), 1.0);
    r.register_unit("A", Dimension::base(BaseDim::ElectricCurrent), 1.0);
    r.register_unit("mol", Dimension::base(BaseDim::Amount), 1.0);
    r.register_unit("cd", Dimension::base(BaseDim::LuminousIntensity), 1.0);
    r.register_unit("rad", Dimension::base(BaseDim::Angle), 1.0);
}

fn load_derived_units(r: &mut Registry) {
    let length = Dimension::base(BaseDim::Length);
    let time = Dimension::base(BaseDim::Time);
    let mass = Dimension::base(BaseDim::Mass);
    let angle = Dimension::base(BaseDim::Angle);

    let force = mass * length / time.pow_int(2);
    let energy = force * length;
    let power = energy / time;
    let pressure = force / length.pow_int(2);
    let frequency = Dimension::DIMENSIONLESS / time;

    // Length
    r.register_unit("km", length, 1000.0);
    r.register_unit("cm", length, 0.01);
    r.register_unit("mm", length, 0.001);

    // Time
    r.register_unit("hour", time, 3600.0);
    r.register_unit("min", time, 60.0);

    // Angle
    r.register_unit("deg", angle, std::f64::consts::PI / 180.0);

    // Mass
    r.register_unit("g", mass, 0.001);

    // Force
    r.register_unit("N", force, 1.0);
    r.register_unit("kN", force, 1000.0);

    // Energy
    r.register_unit("J", energy, 1.0);
    r.register_unit("kJ", energy, 1000.0);

    // Power
    r.register_unit("W", power, 1.0);
    r.register_unit("kW", power, 1000.0);

    // Pressure
    r.register_unit("Pa", pressure, 1.0);
    r.register_unit("kPa", pressure, 1000.0);
    r.register_unit("MPa", pressure, 1_000_000.0);

    // Frequency
    r.register_unit("Hz", frequency, 1.0);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use kasuri_syntax::dimension::Rational;

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
        assert_eq!(force.exponents[BaseDim::Mass as usize], Rational::ONE);
        assert_eq!(force.exponents[BaseDim::Length as usize], Rational::ONE);
        assert_eq!(
            force.exponents[BaseDim::Time as usize],
            Rational::new(-2, 1)
        );
    }

    #[test]
    fn prelude_newton_matches_force_dim() {
        let mut r = Registry::new();
        load_prelude(&mut r);
        let force_dim = *r.get_dimension("Force").unwrap();
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
}
