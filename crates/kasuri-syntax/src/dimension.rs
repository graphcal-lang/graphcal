use std::fmt;

/// A rational number for dimension exponents (e.g., 1/2 for sqrt).
///
/// Always stored in reduced form with `den > 0`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    pub num: i32,
    pub den: i32,
}

impl fmt::Debug for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl Rational {
    pub const ZERO: Self = Self { num: 0, den: 1 };
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// Create a new rational number, automatically reduced.
    ///
    /// # Panics
    ///
    /// Panics if `den` is zero.
    #[must_use]
    pub fn new(num: i32, den: i32) -> Self {
        assert!(den != 0, "denominator must not be zero");
        if num == 0 {
            return Self::ZERO;
        }
        let g = gcd(num.unsigned_abs(), den.unsigned_abs()).cast_signed();
        let (n, d) = (num / g, den / g);
        // Normalize sign: denominator is always positive
        if d < 0 {
            Self { num: -n, den: -d }
        } else {
            Self { num: n, den: d }
        }
    }

    /// Create a rational from an integer.
    #[must_use]
    pub const fn from_int(n: i32) -> Self {
        if n == 0 {
            Self::ZERO
        } else {
            Self { num: n, den: 1 }
        }
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    #[must_use]
    pub const fn is_integer(self) -> bool {
        self.den == 1
    }
}

impl std::ops::Add for Rational {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.num * rhs.den + rhs.num * self.den, self.den * rhs.den)
    }
}

impl std::ops::Sub for Rational {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.num * rhs.den - rhs.num * self.den, self.den * rhs.den)
    }
}

impl std::ops::Neg for Rational {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            num: -self.num,
            den: self.den,
        }
    }
}

impl std::ops::Mul for Rational {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self::new(self.num * rhs.num, self.den * rhs.den)
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The 8 base dimension indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BaseDim {
    Length = 0,
    Time = 1,
    Mass = 2,
    Temperature = 3,
    ElectricCurrent = 4,
    Amount = 5,
    LuminousIntensity = 6,
    Angle = 7,
}

impl BaseDim {
    pub const ALL: [Self; 8] = [
        Self::Length,
        Self::Time,
        Self::Mass,
        Self::Temperature,
        Self::ElectricCurrent,
        Self::Amount,
        Self::LuminousIntensity,
        Self::Angle,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Length => "Length",
            Self::Time => "Time",
            Self::Mass => "Mass",
            Self::Temperature => "Temperature",
            Self::ElectricCurrent => "ElectricCurrent",
            Self::Amount => "Amount",
            Self::LuminousIntensity => "LuminousIntensity",
            Self::Angle => "Angle",
        }
    }

    /// The SI base unit symbol for this dimension.
    #[must_use]
    pub const fn si_symbol(self) -> &'static str {
        match self {
            Self::Length => "m",
            Self::Time => "s",
            Self::Mass => "kg",
            Self::Temperature => "K",
            Self::ElectricCurrent => "A",
            Self::Amount => "mol",
            Self::LuminousIntensity => "cd",
            Self::Angle => "rad",
        }
    }
}

/// A physical dimension represented as a vector of 8 rational exponents
/// over the base dimensions.
///
/// For example, Velocity = Length^1 * Time^-1 is represented as
/// `[1, -1, 0, 0, 0, 0, 0, 0]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dimension {
    pub exponents: [Rational; 8],
}

impl fmt::Debug for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Dimension({self})")
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_dimensionless() {
            return write!(f, "Dimensionless");
        }

        let mut first = true;
        // Positive exponents first (numerator part)
        for (i, base) in BaseDim::ALL.iter().enumerate() {
            let exp = self.exponents[i];
            if exp.is_zero() || exp.num < 0 {
                continue;
            }
            if !first {
                write!(f, " * ")?;
            }
            first = false;
            write!(f, "{}", base.name())?;
            if exp != Rational::ONE {
                write!(f, "^{exp}")?;
            }
        }

        // Negative exponents (denominator part)
        for (i, base) in BaseDim::ALL.iter().enumerate() {
            let exp = self.exponents[i];
            if exp.is_zero() || exp.num > 0 {
                continue;
            }
            if first {
                // Only negative exponents exist (e.g., Frequency = Time^-1)
                write!(f, "{}", base.name())?;
                write!(f, "^{exp}")?;
                first = false;
            } else {
                write!(f, " / {}", base.name())?;
                let pos_exp = -exp;
                if pos_exp != Rational::ONE {
                    write!(f, "^{pos_exp}")?;
                }
            }
        }

        Ok(())
    }
}

impl Dimension {
    /// The dimensionless dimension (all exponents zero).
    pub const DIMENSIONLESS: Self = Self {
        exponents: [Rational::ZERO; 8],
    };

    /// A dimension with a single base dimension at exponent 1.
    #[must_use]
    pub const fn base(dim: BaseDim) -> Self {
        let mut exponents = [Rational::ZERO; 8];
        exponents[dim as usize] = Rational::ONE;
        Self { exponents }
    }

    #[must_use]
    pub fn is_dimensionless(self) -> bool {
        self.exponents.iter().all(|e| e.is_zero())
    }

    /// Raise a dimension to a rational power (multiply all exponents).
    #[must_use]
    pub fn pow(self, exp: Rational) -> Self {
        let mut exponents = [Rational::ZERO; 8];
        for (i, e) in exponents.iter_mut().enumerate() {
            *e = self.exponents[i] * exp;
        }
        Self { exponents }
    }

    /// Raise a dimension to an integer power.
    #[must_use]
    pub fn pow_int(self, n: i32) -> Self {
        self.pow(Rational::from_int(n))
    }

    /// Format this dimension as an SI unit string (e.g., `m/s`, `kg*m/s^2`).
    ///
    /// Returns `None` for dimensionless.
    #[must_use]
    pub fn si_unit_string(self) -> Option<String> {
        if self.is_dimensionless() {
            return None;
        }

        let mut result = String::new();
        let mut first = true;

        // Positive exponents (numerator)
        for (i, base) in BaseDim::ALL.iter().enumerate() {
            let exp = self.exponents[i];
            if exp.is_zero() || exp.num < 0 {
                continue;
            }
            if !first {
                result.push('*');
            }
            first = false;
            result.push_str(base.si_symbol());
            if exp != Rational::ONE {
                result.push('^');
                result.push_str(&exp.to_string());
            }
        }

        // Negative exponents (denominator)
        for (i, base) in BaseDim::ALL.iter().enumerate() {
            let exp = self.exponents[i];
            if exp.is_zero() || exp.num > 0 {
                continue;
            }
            if first {
                // Only negative exponents (e.g., Frequency = s^-1)
                result.push_str(base.si_symbol());
                result.push('^');
                result.push_str(&exp.to_string());
                first = false;
            } else {
                result.push('/');
                result.push_str(base.si_symbol());
                let pos_exp = -exp;
                if pos_exp != Rational::ONE {
                    result.push('^');
                    result.push_str(&pos_exp.to_string());
                }
            }
        }

        Some(result)
    }
}

impl std::ops::Mul for Dimension {
    type Output = Self;
    /// Multiply two dimensions (add exponents).
    #[expect(clippy::suspicious_arithmetic_impl)]
    fn mul(self, other: Self) -> Self {
        let mut exponents = [Rational::ZERO; 8];
        for (i, e) in exponents.iter_mut().enumerate() {
            *e = self.exponents[i] + other.exponents[i];
        }
        Self { exponents }
    }
}

impl std::ops::Div for Dimension {
    type Output = Self;
    /// Divide two dimensions (subtract exponents).
    #[expect(clippy::suspicious_arithmetic_impl)]
    fn div(self, other: Self) -> Self {
        let mut exponents = [Rational::ZERO; 8];
        for (i, e) in exponents.iter_mut().enumerate() {
            *e = self.exponents[i] - other.exponents[i];
        }
        Self { exponents }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_creation_and_reduction() {
        assert_eq!(Rational::new(2, 4), Rational::new(1, 2));
        assert_eq!(Rational::new(-3, 6), Rational::new(-1, 2));
        assert_eq!(Rational::new(6, -4), Rational::new(-3, 2));
        assert_eq!(Rational::new(0, 5), Rational::ZERO);
    }

    #[test]
    fn rational_arithmetic() {
        let half = Rational::new(1, 2);
        let third = Rational::new(1, 3);

        // 1/2 + 1/3 = 5/6
        let sum = half + third;
        assert_eq!(sum, Rational::new(5, 6));

        // 1/2 - 1/3 = 1/6
        let diff = half - third;
        assert_eq!(diff, Rational::new(1, 6));

        // 1/2 * 1/3 = 1/6
        let prod = half * third;
        assert_eq!(prod, Rational::new(1, 6));

        // -1/2
        assert_eq!(-half, Rational::new(-1, 2));
    }

    #[test]
    fn rational_from_int() {
        assert_eq!(Rational::from_int(3), Rational::new(3, 1));
        assert_eq!(Rational::from_int(0), Rational::ZERO);
        assert_eq!(Rational::from_int(-2), Rational::new(-2, 1));
    }

    #[test]
    fn dimension_base() {
        let length = Dimension::base(BaseDim::Length);
        assert_eq!(length.exponents[0], Rational::ONE);
        for i in 1..8 {
            assert!(length.exponents[i].is_zero());
        }
    }

    #[test]
    fn dimension_dimensionless() {
        assert!(Dimension::DIMENSIONLESS.is_dimensionless());
        assert!(!Dimension::base(BaseDim::Length).is_dimensionless());
    }

    #[test]
    fn dimension_velocity() {
        // Velocity = Length / Time
        let length = Dimension::base(BaseDim::Length);
        let time = Dimension::base(BaseDim::Time);
        let velocity = length / time;

        assert_eq!(velocity.exponents[BaseDim::Length as usize], Rational::ONE);
        assert_eq!(
            velocity.exponents[BaseDim::Time as usize],
            Rational::from_int(-1)
        );
    }

    #[test]
    fn dimension_acceleration() {
        // Acceleration = Length / Time^2
        let length = Dimension::base(BaseDim::Length);
        let time = Dimension::base(BaseDim::Time);
        let accel = length / time.pow_int(2);

        assert_eq!(accel.exponents[BaseDim::Length as usize], Rational::ONE);
        assert_eq!(
            accel.exponents[BaseDim::Time as usize],
            Rational::from_int(-2)
        );
    }

    #[test]
    fn dimension_force() {
        // Force = Mass * Length / Time^2
        let mass = Dimension::base(BaseDim::Mass);
        let length = Dimension::base(BaseDim::Length);
        let time = Dimension::base(BaseDim::Time);
        let force = mass * length / time.pow_int(2);

        assert_eq!(force.exponents[BaseDim::Mass as usize], Rational::ONE);
        assert_eq!(force.exponents[BaseDim::Length as usize], Rational::ONE);
        assert_eq!(
            force.exponents[BaseDim::Time as usize],
            Rational::from_int(-2)
        );
    }

    #[test]
    fn dimension_sqrt() {
        // sqrt(Area) = sqrt(Length^2) = Length
        let area = Dimension::base(BaseDim::Length).pow_int(2);
        let sqrt_area = area.pow(Rational::new(1, 2));
        assert_eq!(sqrt_area, Dimension::base(BaseDim::Length));
    }

    #[test]
    fn dimension_mul_div_inverse() {
        let length = Dimension::base(BaseDim::Length);
        let time = Dimension::base(BaseDim::Time);
        let velocity = length / time;

        // velocity * time = length
        assert_eq!(velocity * time, length);

        // length / velocity = time
        assert_eq!(length / velocity, time);
    }

    #[test]
    fn dimension_dimensionless_mul() {
        let length = Dimension::base(BaseDim::Length);
        assert_eq!(Dimension::DIMENSIONLESS * length, length);
        assert_eq!(length * Dimension::DIMENSIONLESS, length);
    }

    #[test]
    fn dimension_display_simple() {
        assert_eq!(format!("{}", Dimension::DIMENSIONLESS), "Dimensionless");
        assert_eq!(format!("{}", Dimension::base(BaseDim::Length)), "Length");
    }

    #[test]
    fn dimension_display_velocity() {
        let velocity = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        assert_eq!(format!("{velocity}"), "Length / Time");
    }

    #[test]
    fn dimension_display_force() {
        let force = Dimension::base(BaseDim::Mass) * Dimension::base(BaseDim::Length)
            / Dimension::base(BaseDim::Time).pow_int(2);
        assert_eq!(format!("{force}"), "Length * Mass / Time^2");
    }

    #[test]
    fn dimension_display_area() {
        let area = Dimension::base(BaseDim::Length).pow_int(2);
        assert_eq!(format!("{area}"), "Length^2");
    }

    #[test]
    fn dimension_display_frequency() {
        // Frequency = Time^-1 (only negative exponent)
        let freq = Dimension::DIMENSIONLESS / Dimension::base(BaseDim::Time);
        assert_eq!(format!("{freq}"), "Time^-1");
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating Rational values with small numerators/denominators
        /// to avoid i32 overflow in intermediate calculations.
        fn arb_rational() -> impl Strategy<Value = Rational> {
            (-50i32..=50, -50i32..=50)
                .prop_filter("denominator must be non-zero", |&(_, d)| d != 0)
                .prop_map(|(n, d)| Rational::new(n, d))
        }

        /// Strategy for generating Dimension values with small exponents.
        fn arb_dimension() -> impl Strategy<Value = Dimension> {
            proptest::array::uniform8(arb_rational()).prop_map(|exponents| Dimension { exponents })
        }

        proptest! {
            // --- Rational invariants ---

            #[test]
            fn rational_always_reduced(n in -100i32..=100, d in -100i32..=100) {
                prop_assume!(d != 0);
                let r = Rational::new(n, d);
                // den is always positive
                prop_assert!(r.den > 0, "den must be positive, got {}", r.den);
                // gcd(|num|, den) == 1 (reduced form)
                if r.num != 0 {
                    let g = gcd(r.num.unsigned_abs(), r.den.unsigned_abs());
                    prop_assert_eq!(g, 1, "not reduced: {}/{}", r.num, r.den);
                } else {
                    prop_assert_eq!(r.den, 1, "zero should have den=1, got {}", r.den);
                }
            }

            #[test]
            fn rational_add_commutative(a in arb_rational(), b in arb_rational()) {
                prop_assert_eq!(a + b, b + a);
            }

            #[test]
            fn rational_mul_commutative(a in arb_rational(), b in arb_rational()) {
                prop_assert_eq!(a * b, b * a);
            }

            #[test]
            fn rational_additive_identity(a in arb_rational()) {
                prop_assert_eq!(a + Rational::ZERO, a);
            }

            #[test]
            fn rational_multiplicative_identity(a in arb_rational()) {
                prop_assert_eq!(a * Rational::ONE, a);
            }

            #[test]
            fn rational_additive_inverse(a in arb_rational()) {
                prop_assert_eq!(a + (-a), Rational::ZERO);
            }

            #[test]
            fn rational_sub_self_is_zero(a in arb_rational()) {
                prop_assert_eq!(a - a, Rational::ZERO);
            }

            // --- Dimension invariants ---

            #[test]
            fn dimension_mul_commutative(a in arb_dimension(), b in arb_dimension()) {
                prop_assert_eq!(a * b, b * a);
            }

            #[test]
            fn dimension_dimensionless_is_mul_identity(a in arb_dimension()) {
                prop_assert_eq!(a * Dimension::DIMENSIONLESS, a);
            }

            #[test]
            fn dimension_self_div_is_dimensionless(a in arb_dimension()) {
                prop_assert_eq!(a / a, Dimension::DIMENSIONLESS);
            }

            #[test]
            fn dimension_div_inverse(a in arb_dimension(), b in arb_dimension()) {
                // (a / b) * b == a
                prop_assert_eq!((a / b) * b, a);
            }

            #[test]
            fn dimension_pow_int_consistent_with_pow(a in arb_dimension(), n in -3i32..=3) {
                prop_assert_eq!(a.pow_int(n), a.pow(Rational::from_int(n)));
            }

            #[test]
            fn dimension_pow_distributes_over_mul(
                a in arb_dimension(),
                b in arb_dimension(),
                r in arb_rational(),
            ) {
                // (a * b).pow(r) == a.pow(r) * b.pow(r)
                prop_assert_eq!((a * b).pow(r), a.pow(r) * b.pow(r));
            }
        }
    }
}
