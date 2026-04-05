use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};

/// A rational number for dimension exponents (e.g., 1/2 for sqrt).
///
/// Always stored in reduced form with `den > 0`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i32,
    den: i32,
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
    #[expect(
        clippy::panic,
        reason = "panicking variant for known-safe literal denominators"
    )]
    pub fn new(num: i32, den: i32) -> Self {
        match Self::try_new(num, den) {
            Ok(r) => r,
            Err(RationalError::ZeroDenominator) => panic!("denominator must not be zero"),
        }
    }

    /// Try to create a new rational number, automatically reduced.
    ///
    /// Returns `Err` if `den` is zero.
    pub fn try_new(num: i32, den: i32) -> Result<Self, RationalError> {
        if den == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        if num == 0 {
            return Ok(Self::ZERO);
        }
        let g = gcd(num.unsigned_abs(), den.unsigned_abs()).cast_signed();
        let (n, d) = (num / g, den / g);
        // Normalize sign: denominator is always positive
        if d < 0 {
            Ok(Self { num: -n, den: -d })
        } else {
            Ok(Self { num: n, den: d })
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

    /// Returns the numerator.
    #[must_use]
    pub const fn num(self) -> i32 {
        self.num
    }

    /// Returns the denominator (always positive).
    #[must_use]
    pub const fn den(self) -> i32 {
        self.den
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

/// Error from `Rational` construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RationalError {
    ZeroDenominator,
}

impl fmt::Display for RationalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDenominator => write!(f, "denominator must not be zero"),
        }
    }
}

impl std::error::Error for RationalError {}

/// Compute `num / den` in `i64` with GCD reduction, then narrow back to `i32`.
///
/// # Panics
///
/// Panics if the reduced result does not fit in `i32` (extremely unlikely for
/// dimension exponents) or if `den` is zero.
fn reduce_i64(num: i64, den: i64) -> (i32, i32) {
    assert!(den != 0, "denominator must not be zero in reduce_i64");
    if num == 0 {
        return (0, 1);
    }
    let g = gcd64(num.unsigned_abs(), den.unsigned_abs()).cast_signed();
    let (mut n, mut d) = (num / g, den / g);
    if d < 0 {
        n = -n;
        d = -d;
    }
    #[expect(
        clippy::expect_used,
        reason = "overflow of dimension exponents after GCD reduction is practically impossible"
    )]
    (
        i32::try_from(n).expect("dimension exponent numerator overflow"),
        i32::try_from(d).expect("dimension exponent denominator overflow"),
    )
}

impl std::ops::Add for Rational {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        // Widen to i64 to avoid intermediate overflow
        let num =
            i64::from(self.num) * i64::from(rhs.den) + i64::from(rhs.num) * i64::from(self.den);
        let den = i64::from(self.den) * i64::from(rhs.den);
        let (n, d) = reduce_i64(num, den);
        Self { num: n, den: d }
    }
}

impl std::ops::Sub for Rational {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        let num =
            i64::from(self.num) * i64::from(rhs.den) - i64::from(rhs.num) * i64::from(self.den);
        let den = i64::from(self.den) * i64::from(rhs.den);
        let (n, d) = reduce_i64(num, den);
        Self { num: n, den: d }
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
        let num = i64::from(self.num) * i64::from(rhs.num);
        let den = i64::from(self.den) * i64::from(rhs.den);
        let (n, d) = reduce_i64(num, den);
        Self { num: n, den: d }
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn gcd64(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd64(b, a % b) }
}

/// A unique identifier for a base dimension.
///
/// Identity is name-based rather than auto-incremented, ensuring consistency
/// across per-file compilation units (important for diamond imports).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BaseDimId {
    /// Built-in prelude dimension (e.g., "Length", "Time", "Mass").
    Prelude(String),
    /// User-defined dimension, identified by defining file path + name.
    /// The file path is relative to the project root for cross-machine consistency.
    UserDefined {
        file: std::path::PathBuf,
        name: String,
    },
}

impl BaseDimId {
    /// A human-readable fallback name when no symbol/name map is available.
    #[must_use]
    pub fn fallback_symbol(&self) -> String {
        match self {
            Self::Prelude(name) | Self::UserDefined { name, .. } => name.clone(),
        }
    }
}

/// A physical dimension represented as a sparse vector of rational exponents
/// over base dimensions.
///
/// For example, Velocity = Length^1 * Time^-1 is represented as
/// `{BaseDimId::Prelude("Length"): 1, BaseDimId::Prelude("Time"): -1}`.
///
/// Only non-zero exponents are stored. An empty map represents the dimensionless
/// dimension.
#[derive(Clone, PartialEq, Eq)]
pub struct Dimension {
    /// Non-zero exponents only. Sorted by `BaseDimId` for deterministic equality/hash.
    exponents: BTreeMap<BaseDimId, Rational>,
}

// Manual Hash impl because BTreeMap doesn't derive Hash,
// but its iteration order is deterministic (sorted by key).
impl Hash for Dimension {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.exponents.len());
        for (id, exp) in &self.exponents {
            id.hash(state);
            exp.hash(state);
        }
    }
}

impl fmt::Debug for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_dimensionless() {
            write!(f, "Dimension(Dimensionless)")
        } else {
            write!(f, "Dimension(")?;
            let mut first = true;
            for (id, exp) in &self.exponents {
                if !first {
                    write!(f, " * ")?;
                }
                first = false;
                match id {
                    BaseDimId::Prelude(name) | BaseDimId::UserDefined { name, .. } => {
                        write!(f, "{name}")?;
                    }
                }
                if *exp != Rational::ONE {
                    write!(f, "^{exp}")?;
                }
            }
            write!(f, ")")
        }
    }
}

impl Dimension {
    /// The dimensionless dimension (empty exponent map).
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self {
            exponents: BTreeMap::new(),
        }
    }

    /// A dimension with a single base dimension at exponent 1.
    #[must_use]
    pub fn base(id: BaseDimId) -> Self {
        let mut exponents = BTreeMap::new();
        exponents.insert(id, Rational::ONE);
        Self { exponents }
    }

    #[must_use]
    pub fn is_dimensionless(&self) -> bool {
        self.exponents.is_empty()
    }

    /// Get the exponent for a specific base dimension (zero if absent).
    #[must_use]
    pub fn get_exponent(&self, id: &BaseDimId) -> Rational {
        self.exponents.get(id).copied().unwrap_or(Rational::ZERO)
    }

    /// Returns an iterator over the non-zero `(BaseDimId, Rational)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&BaseDimId, &Rational)> {
        self.exponents.iter()
    }

    /// Raise a dimension to a rational power (multiply all exponents).
    #[must_use]
    pub fn pow(&self, exp: Rational) -> Self {
        if exp.is_zero() {
            return Self::dimensionless();
        }
        let exponents = self
            .exponents
            .iter()
            .map(|(id, &e)| (id.clone(), e * exp))
            .filter(|(_, e)| !e.is_zero())
            .collect();
        Self { exponents }
    }

    /// Raise a dimension to an integer power.
    #[must_use]
    pub fn pow_int(&self, n: i32) -> Self {
        self.pow(Rational::from_int(n))
    }

    /// Format this dimension using named base dimensions for display.
    ///
    /// The `names` map provides `BaseDimId → name` mappings.
    /// Unknown IDs are displayed as `D{id}`.
    #[must_use]
    pub const fn display_with<'a>(
        &'a self,
        names: &'a BTreeMap<BaseDimId, String>,
    ) -> DimensionDisplay<'a> {
        DimensionDisplay { dim: self, names }
    }

    /// Format this dimension as an SI unit string (e.g., `m/s`, `kg*m/s^2`).
    ///
    /// The `symbols` map provides `BaseDimId → unit symbol` mappings.
    /// Returns `None` for dimensionless.
    #[must_use]
    pub fn si_unit_string(&self, symbols: &BTreeMap<BaseDimId, String>) -> Option<String> {
        if self.is_dimensionless() {
            return None;
        }

        let mut result = String::new();
        let mut first = true;

        // Positive exponents (numerator)
        for (id, &exp) in &self.exponents {
            if exp.num() <= 0 {
                continue;
            }
            if !first {
                result.push('*');
            }
            first = false;
            let symbol = symbols
                .get(id)
                .map_or_else(|| id.fallback_symbol(), String::clone);
            result.push_str(&symbol);
            if exp != Rational::ONE {
                result.push('^');
                result.push_str(&exp.to_string());
            }
        }

        // Negative exponents (denominator)
        for (id, &exp) in &self.exponents {
            if exp.num() >= 0 {
                continue;
            }
            let symbol = symbols
                .get(id)
                .map_or_else(|| id.fallback_symbol(), String::clone);
            if first {
                // Only negative exponents (e.g., Frequency = s^-1)
                result.push_str(&symbol);
                result.push('^');
                result.push_str(&exp.to_string());
                first = false;
            } else {
                result.push('/');
                result.push_str(&symbol);
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

/// A wrapper for displaying a `Dimension` with named base dimensions.
pub struct DimensionDisplay<'a> {
    dim: &'a Dimension,
    names: &'a BTreeMap<BaseDimId, String>,
}

impl fmt::Display for DimensionDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.dim.is_dimensionless() {
            return write!(f, "Dimensionless");
        }

        let mut first = true;

        // Positive exponents first (numerator part)
        for (id, &exp) in &self.dim.exponents {
            if exp.num() <= 0 {
                continue;
            }
            if !first {
                write!(f, " * ")?;
            }
            first = false;
            let name = self
                .names
                .get(id)
                .map_or_else(|| id.fallback_symbol(), String::clone);
            write!(f, "{name}")?;
            if exp != Rational::ONE {
                write!(f, "^{exp}")?;
            }
        }

        // Negative exponents (denominator part)
        for (id, &exp) in &self.dim.exponents {
            if exp.num() >= 0 {
                continue;
            }
            let name = self
                .names
                .get(id)
                .map_or_else(|| id.fallback_symbol(), String::clone);
            if first {
                // Only negative exponents exist (e.g., Frequency = Time^-1)
                write!(f, "{name}")?;
                write!(f, "^{exp}")?;
                first = false;
            } else {
                write!(f, " / {name}")?;
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
    /// Combine two dimensions by adding or subtracting exponents.
    ///
    /// If `negate` is false, exponents are added (multiplication).
    /// If `negate` is true, exponents are subtracted (division).
    fn combine(self, other: &Self, negate: bool) -> Self {
        let mut exponents = self.exponents;
        for (id, exp) in &other.exponents {
            let entry = exponents.entry(id.clone()).or_insert(Rational::ZERO);
            *entry = if negate { *entry - *exp } else { *entry + *exp };
            if entry.is_zero() {
                exponents.remove(id);
            }
        }
        Self { exponents }
    }
}

impl std::ops::Mul for Dimension {
    type Output = Self;
    /// Multiply two dimensions (add exponents).
    fn mul(self, other: Self) -> Self {
        self.combine(&other, false)
    }
}

impl std::ops::Div for Dimension {
    type Output = Self;
    /// Divide two dimensions (subtract exponents).
    fn div(self, other: Self) -> Self {
        self.combine(&other, true)
    }
}

impl std::ops::Mul for &Dimension {
    type Output = Dimension;
    fn mul(self, other: Self) -> Dimension {
        self.clone() * other.clone()
    }
}

impl std::ops::Div for &Dimension {
    type Output = Dimension;
    fn div(self, other: Self) -> Dimension {
        self.clone() / other.clone()
    }
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

    // Helper: well-known base dimension IDs matching prelude dimensions.
    fn length() -> BaseDimId {
        BaseDimId::Prelude("Length".to_string())
    }
    fn time() -> BaseDimId {
        BaseDimId::Prelude("Time".to_string())
    }
    fn mass() -> BaseDimId {
        BaseDimId::Prelude("Mass".to_string())
    }

    /// Build a names map for display tests.
    fn test_names() -> BTreeMap<BaseDimId, String> {
        let mut m = BTreeMap::new();
        m.insert(
            BaseDimId::Prelude("Length".to_string()),
            "Length".to_string(),
        );
        m.insert(BaseDimId::Prelude("Time".to_string()), "Time".to_string());
        m.insert(BaseDimId::Prelude("Mass".to_string()), "Mass".to_string());
        m.insert(
            BaseDimId::Prelude("Temperature".to_string()),
            "Temperature".to_string(),
        );
        m.insert(
            BaseDimId::Prelude("ElectricCurrent".to_string()),
            "ElectricCurrent".to_string(),
        );
        m.insert(
            BaseDimId::Prelude("Amount".to_string()),
            "Amount".to_string(),
        );
        m.insert(
            BaseDimId::Prelude("LuminousIntensity".to_string()),
            "LuminousIntensity".to_string(),
        );
        m.insert(BaseDimId::Prelude("Angle".to_string()), "Angle".to_string());
        m
    }

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
        let len = Dimension::base(length());
        assert_eq!(len.get_exponent(&length()), Rational::ONE);
        assert!(len.get_exponent(&time()).is_zero());
        assert!(len.get_exponent(&mass()).is_zero());
    }

    #[test]
    fn dimension_dimensionless() {
        assert!(Dimension::dimensionless().is_dimensionless());
        assert!(!Dimension::base(length()).is_dimensionless());
    }

    #[test]
    fn dimension_velocity() {
        // Velocity = Length / Time
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let velocity = l / t;

        assert_eq!(velocity.get_exponent(&length()), Rational::ONE);
        assert_eq!(velocity.get_exponent(&time()), Rational::from_int(-1));
    }

    #[test]
    fn dimension_acceleration() {
        // Acceleration = Length / Time^2
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let accel = l / t.pow_int(2);

        assert_eq!(accel.get_exponent(&length()), Rational::ONE);
        assert_eq!(accel.get_exponent(&time()), Rational::from_int(-2));
    }

    #[test]
    fn dimension_force() {
        // Force = Mass * Length / Time^2
        let m = Dimension::base(mass());
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let force = m * l / t.pow_int(2);

        assert_eq!(force.get_exponent(&mass()), Rational::ONE);
        assert_eq!(force.get_exponent(&length()), Rational::ONE);
        assert_eq!(force.get_exponent(&time()), Rational::from_int(-2));
    }

    #[test]
    fn dimension_sqrt() {
        // sqrt(Area) = sqrt(Length^2) = Length
        let area = Dimension::base(length()).pow_int(2);
        let sqrt_area = area.pow(Rational::new(1, 2));
        assert_eq!(sqrt_area, Dimension::base(length()));
    }

    #[test]
    fn dimension_mul_div_inverse() {
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let velocity = l.clone() / t.clone();

        // velocity * time = length
        assert_eq!(velocity.clone() * t.clone(), l);

        // length / velocity = time
        assert_eq!(l / velocity, t);
    }

    #[test]
    fn dimension_dimensionless_mul() {
        let l = Dimension::base(length());
        assert_eq!(Dimension::dimensionless() * l.clone(), l);
        assert_eq!(l.clone() * Dimension::dimensionless(), l);
    }

    #[test]
    fn dimension_display_simple() {
        let names = test_names();
        assert_eq!(
            format!("{}", Dimension::dimensionless().display_with(&names)),
            "Dimensionless"
        );
        assert_eq!(
            format!("{}", Dimension::base(length()).display_with(&names)),
            "Length"
        );
    }

    #[test]
    fn dimension_display_velocity() {
        let names = test_names();
        let velocity = Dimension::base(length()) / Dimension::base(time());
        assert_eq!(
            format!("{}", velocity.display_with(&names)),
            "Length / Time"
        );
    }

    #[test]
    fn dimension_display_force() {
        let names = test_names();
        let force = Dimension::base(mass()) * Dimension::base(length())
            / Dimension::base(time()).pow_int(2);
        assert_eq!(
            format!("{}", force.display_with(&names)),
            "Length * Mass / Time^2"
        );
    }

    #[test]
    fn dimension_display_area() {
        let names = test_names();
        let area = Dimension::base(length()).pow_int(2);
        assert_eq!(format!("{}", area.display_with(&names)), "Length^2");
    }

    #[test]
    fn dimension_display_frequency() {
        let names = test_names();
        // Frequency = Time^-1 (only negative exponent)
        let freq = Dimension::dimensionless() / Dimension::base(time());
        assert_eq!(format!("{}", freq.display_with(&names)), "Time^-1");
    }

    #[test]
    fn dimension_user_defined_base() {
        // User-defined base dimension gets a new ID
        let info_id = BaseDimId::UserDefined {
            file: std::path::PathBuf::from("test.gcl"),
            name: "Information".to_string(),
        };
        let information = Dimension::base(info_id.clone());
        let t = Dimension::base(time());
        let bandwidth = information / t;

        assert_eq!(bandwidth.get_exponent(&info_id), Rational::ONE);
        assert_eq!(bandwidth.get_exponent(&time()), Rational::from_int(-1));

        // Display with names
        let mut names = test_names();
        names.insert(info_id, "Information".to_string());
        assert_eq!(
            format!("{}", bandwidth.display_with(&names)),
            "Information / Time"
        );
    }

    #[test]
    fn dimension_hash_consistency() {
        use std::collections::hash_map::DefaultHasher;

        let a = Dimension::base(length()) / Dimension::base(time());
        let b = Dimension::base(length()) / Dimension::base(time());
        assert_eq!(a, b);

        let mut ha = DefaultHasher::new();
        a.hash(&mut ha);
        let mut hb = DefaultHasher::new();
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
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

        /// The 8 prelude dimension names for property testing.
        const PRELUDE_DIMS: [&str; 8] = [
            "Length",
            "Time",
            "Mass",
            "Temperature",
            "ElectricCurrent",
            "Amount",
            "LuminousIntensity",
            "Angle",
        ];

        /// Strategy for generating Dimension values with small exponents.
        /// Uses a fixed set of prelude base dimension IDs.
        fn arb_dimension() -> impl Strategy<Value = Dimension> {
            proptest::collection::btree_map(0usize..8, arb_rational(), 0..=8).prop_map(|map| {
                let exponents = map
                    .into_iter()
                    .filter(|(_, r)| !r.is_zero())
                    .map(|(idx, r)| (BaseDimId::Prelude(PRELUDE_DIMS[idx].to_string()), r))
                    .collect();
                Dimension { exponents }
            })
        }

        proptest! {
            // --- Rational invariants ---

            #[test]
            fn rational_always_reduced(n in -100i32..=100, d in -100i32..=100) {
                prop_assume!(d != 0);
                let r = Rational::new(n, d);
                // den is always positive
                prop_assert!(r.den() > 0, "den must be positive, got {}", r.den());
                // gcd(|num|, den) == 1 (reduced form)
                if r.num() != 0 {
                    let g = gcd(r.num().unsigned_abs(), r.den().unsigned_abs());
                    prop_assert_eq!(g, 1, "not reduced: {}/{}", r.num(), r.den());
                } else {
                    prop_assert_eq!(r.den(), 1, "zero should have den=1, got {}", r.den());
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
                prop_assert_eq!(a.clone() * b.clone(), b * a);
            }

            #[test]
            fn dimension_dimensionless_is_mul_identity(a in arb_dimension()) {
                prop_assert_eq!(a.clone() * Dimension::dimensionless(), a);
            }

            #[test]
            fn dimension_self_div_is_dimensionless(a in arb_dimension()) {
                prop_assert_eq!(a.clone() / a, Dimension::dimensionless());
            }

            #[test]
            fn dimension_div_inverse(a in arb_dimension(), b in arb_dimension()) {
                // (a / b) * b == a
                prop_assert_eq!((a.clone() / b.clone()) * b, a);
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
                prop_assert_eq!((a.clone() * b.clone()).pow(r), a.pow(r) * b.pow(r));
            }
        }
    }
}
