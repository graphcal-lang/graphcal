use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

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
        fmt::Display::fmt(self, f)
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
    /// `1/2` — used for square-root exponents.
    pub const HALF: Self = Self { num: 1, den: 2 };
    /// `1/3` — used for cube-root exponents.
    pub const THIRD: Self = Self { num: 1, den: 3 };

    /// Try to create a new rational number, automatically reduced.
    ///
    /// Returns `Err` if `den` is zero.
    pub fn try_new(num: i32, den: i32) -> Result<Self, RationalError> {
        Self::try_new_i64(i64::from(num), i64::from(den))
    }

    /// Normalize `num / den` in `i64` with GCD reduction, then narrow back to `i32`.
    ///
    /// Returns `Err(RationalError::Overflow)` if the reduced result does not fit
    /// in `i32`, and `Err(RationalError::ZeroDenominator)` if `den` is zero.
    fn try_new_i64(num: i64, den: i64) -> Result<Self, RationalError> {
        if den == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        if num == 0 {
            return Ok(Self::ZERO);
        }
        let g = gcd64(num.unsigned_abs(), den.unsigned_abs()).cast_signed();
        let (mut n, mut d) = (num / g, den / g);
        if d < 0 {
            n = n.checked_neg().ok_or(RationalError::Overflow)?;
            d = d.checked_neg().ok_or(RationalError::Overflow)?;
        }
        let num = i32::try_from(n).map_err(|_| RationalError::Overflow)?;
        let den = i32::try_from(d).map_err(|_| RationalError::Overflow)?;
        Ok(Self { num, den })
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

    /// Negate this rational exponent, returning an error if the numerator overflows.
    pub const fn checked_neg(self) -> Result<Self, RationalError> {
        let Some(num) = self.num.checked_neg() else {
            return Err(RationalError::Overflow);
        };
        Ok(Self { num, den: self.den })
    }
}

/// Error from `Rational` construction or arithmetic.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RationalError {
    /// The denominator was zero.
    #[error("denominator must not be zero")]
    ZeroDenominator,
    /// The reduced numerator or denominator did not fit in `i32`.
    ///
    /// Dimension exponents are stored as `i32`; an operation produced a
    /// reduced value outside that range.
    #[error("dimension exponent overflowed i32")]
    Overflow,
}

impl std::ops::Add for Rational {
    type Output = Result<Self, RationalError>;
    fn add(self, rhs: Self) -> Self::Output {
        // Widen to i64 to avoid intermediate overflow
        let num =
            i64::from(self.num) * i64::from(rhs.den) + i64::from(rhs.num) * i64::from(self.den);
        let den = i64::from(self.den) * i64::from(rhs.den);
        Self::try_new_i64(num, den)
    }
}

impl std::ops::Sub for Rational {
    type Output = Result<Self, RationalError>;
    fn sub(self, rhs: Self) -> Self::Output {
        let num =
            i64::from(self.num) * i64::from(rhs.den) - i64::from(rhs.num) * i64::from(self.den);
        let den = i64::from(self.den) * i64::from(rhs.den);
        Self::try_new_i64(num, den)
    }
}

impl std::ops::Neg for Rational {
    type Output = Self;
    #[expect(
        clippy::expect_used,
        reason = "Rational values are normalized from valid i32 exponents; negation overflow is impossible"
    )]
    fn neg(self) -> Self {
        self.checked_neg()
            .expect("valid Rational exponent negation should not overflow")
    }
}

impl std::ops::Mul for Rational {
    type Output = Result<Self, RationalError>;
    fn mul(self, rhs: Self) -> Self::Output {
        let num = i64::from(self.num) * i64::from(rhs.num);
        let den = i64::from(self.den) * i64::from(rhs.den);
        Self::try_new_i64(num, den)
    }
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
    /// User-defined dimension, identified by the defining DAG's identity + name.
    UserDefined {
        dag: crate::dag_id::DagId,
        name: String,
    },
}

/// A physical dimension represented as a sparse vector of rational exponents
/// over base dimensions.
///
/// For example, Velocity = Length^1 * Time^-1 is represented as
/// `{BaseDimId::Prelude("Length"): 1, BaseDimId::Prelude("Time"): -1}`.
///
/// Only non-zero exponents are stored. An empty map represents the dimensionless
/// dimension.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Dimension {
    /// Non-zero exponents only. Sorted by `BaseDimId` for deterministic equality/hash.
    exponents: BTreeMap<BaseDimId, Rational>,
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
    ///
    /// Returns `Err(RationalError::Overflow)` if any exponent multiplication
    /// produces a reduced value outside the `i32` range.
    pub fn pow(&self, exp: Rational) -> Result<Self, RationalError> {
        if exp.is_zero() {
            return Ok(Self::dimensionless());
        }
        let mut exponents = BTreeMap::new();
        for (id, &e) in &self.exponents {
            let new_exp = (e * exp)?;
            if !new_exp.is_zero() {
                exponents.insert(id.clone(), new_exp);
            }
        }
        Ok(Self { exponents })
    }

    /// Raise a dimension to an integer power.
    ///
    /// Returns `Err(RationalError::Overflow)` if any exponent multiplication
    /// overflows `i32`.
    pub fn pow_int(&self, n: i32) -> Result<Self, RationalError> {
        self.pow(Rational::from_int(n))
    }

    /// Format this dimension using named base dimensions for display.
    ///
    /// The `names` map must provide a `BaseDimId → name` mapping for every
    /// base dimension in `self`.
    ///
    /// # Errors
    ///
    /// Returns [`MissingBaseDimensionName`] when `names` does not contain an
    /// entry for a base dimension referenced by `self`.
    pub fn try_format_with(
        &self,
        names: &BTreeMap<BaseDimId, String>,
    ) -> Result<String, MissingBaseDimensionName> {
        if self.is_dimensionless() {
            return Ok("Dimensionless".to_string());
        }
        self.format_exponents(names, " * ", " / ")
    }

    /// Format the dimension's exponents.
    ///
    /// `mul_sep` is placed between positive-exponent terms (e.g., `"*"` or `" * "`).
    /// `div_sep` is placed before each negative-exponent term when positive terms exist
    /// (e.g., `"/"` or `" / "`).
    fn format_exponents(
        &self,
        names: &BTreeMap<BaseDimId, String>,
        mul_sep: &str,
        div_sep: &str,
    ) -> Result<String, MissingBaseDimensionName> {
        let mut out = String::new();
        let mut first = true;

        // Positive exponents (numerator)
        for (id, &exp) in &self.exponents {
            if exp.num() <= 0 {
                continue;
            }
            if !first {
                out.push_str(mul_sep);
            }
            first = false;
            push_dim_factor(&mut out, registered_base_dim_name(names, id)?, exp);
        }

        // Negative exponents (denominator)
        for (id, &exp) in &self.exponents {
            if exp.num() >= 0 {
                continue;
            }
            let name = registered_base_dim_name(names, id)?;
            if first {
                // Only negative exponents (e.g., Frequency = s^-1)
                push_dim_factor(&mut out, name, exp);
                first = false;
            } else {
                out.push_str(div_sep);
                let pos_exp = -exp;
                push_dim_factor(&mut out, name, pos_exp);
            }
        }

        Ok(out)
    }
}

fn registered_base_dim_name<'a>(
    names: &'a BTreeMap<BaseDimId, String>,
    id: &BaseDimId,
) -> Result<&'a str, MissingBaseDimensionName> {
    names
        .get(id)
        .map(String::as_str)
        .ok_or_else(|| MissingBaseDimensionName { id: id.clone() })
}

fn push_dim_factor(out: &mut String, name: &str, exp: Rational) {
    out.push_str(name);
    if exp != Rational::ONE {
        out.push('^');
        out.push_str(&exp.to_string());
    }
}

/// A base dimension referenced by a [`Dimension`] had no registered display name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("missing display name for base dimension {id:?}")]
pub struct MissingBaseDimensionName {
    pub id: BaseDimId,
}

/// Whether to add or subtract exponents when combining dimensions.
#[derive(Clone, Copy)]
enum CombineOp {
    /// Add exponents (dimension multiplication).
    Add,
    /// Subtract exponents (dimension division).
    Sub,
}

impl Dimension {
    /// Multiply two dimensions, returning an error if exponent arithmetic overflows.
    pub fn checked_mul(self, other: &Self) -> Result<Self, RationalError> {
        self.combine(other, CombineOp::Add)
    }

    /// Divide two dimensions, returning an error if exponent arithmetic overflows.
    pub fn checked_div(self, other: &Self) -> Result<Self, RationalError> {
        self.combine(other, CombineOp::Sub)
    }

    /// Combine two dimensions by adding or subtracting exponents.
    fn combine(self, other: &Self, op: CombineOp) -> Result<Self, RationalError> {
        let mut exponents = self.exponents;
        for (id, exp) in &other.exponents {
            let entry = exponents.entry(id.clone()).or_insert(Rational::ZERO);
            *entry = match op {
                CombineOp::Add => (*entry + *exp)?,
                CombineOp::Sub => (*entry - *exp)?,
            };
            if entry.is_zero() {
                exponents.remove(id);
            }
        }
        Ok(Self { exponents })
    }
}

impl std::ops::Mul for Dimension {
    type Output = Result<Self, RationalError>;
    /// Multiply two dimensions (add exponents).
    fn mul(self, other: Self) -> Self::Output {
        self.checked_mul(&other)
    }
}

impl std::ops::Div for Dimension {
    type Output = Result<Self, RationalError>;
    /// Divide two dimensions (subtract exponents).
    fn div(self, other: Self) -> Self::Output {
        self.checked_div(&other)
    }
}

impl std::ops::Mul for &Dimension {
    type Output = Result<Dimension, RationalError>;
    fn mul(self, other: Self) -> Self::Output {
        self.clone().checked_mul(other)
    }
}

impl std::ops::Div for &Dimension {
    type Output = Result<Dimension, RationalError>;
    fn div(self, other: Self) -> Self::Output {
        self.clone().checked_div(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: build a `Rational` from integer literals, panicking on
    /// zero denominator. Tests are the only place panicking is acceptable.
    fn r(num: i32, den: i32) -> Rational {
        Rational::try_new(num, den).expect("non-zero denominator")
    }

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
        assert_eq!(r(2, 4), r(1, 2));
        assert_eq!(r(-3, 6), r(-1, 2));
        assert_eq!(r(6, -4), r(-3, 2));
        assert_eq!(r(0, 5), Rational::ZERO);
    }

    #[test]
    fn rational_arithmetic() {
        let half = r(1, 2);
        let third = r(1, 3);

        // 1/2 + 1/3 = 5/6
        let sum = (half + third).unwrap();
        assert_eq!(sum, r(5, 6));

        // 1/2 - 1/3 = 1/6
        let diff = (half - third).unwrap();
        assert_eq!(diff, r(1, 6));

        // 1/2 * 1/3 = 1/6
        let prod = (half * third).unwrap();
        assert_eq!(prod, r(1, 6));

        // -1/2
        assert_eq!(-half, r(-1, 2));
    }

    #[test]
    fn rational_from_int() {
        assert_eq!(Rational::from_int(3), r(3, 1));
        assert_eq!(Rational::from_int(0), Rational::ZERO);
        assert_eq!(Rational::from_int(-2), r(-2, 1));
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
        let velocity = (l / t).unwrap();

        assert_eq!(velocity.get_exponent(&length()), Rational::ONE);
        assert_eq!(velocity.get_exponent(&time()), Rational::from_int(-1));
    }

    #[test]
    fn dimension_acceleration() {
        // Acceleration = Length / Time^2
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let accel = (l / t.pow_int(2).unwrap()).unwrap();

        assert_eq!(accel.get_exponent(&length()), Rational::ONE);
        assert_eq!(accel.get_exponent(&time()), Rational::from_int(-2));
    }

    #[test]
    fn dimension_force() {
        // Force = Mass * Length / Time^2
        let m = Dimension::base(mass());
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let force = ((m * l).unwrap() / t.pow_int(2).unwrap()).unwrap();

        assert_eq!(force.get_exponent(&mass()), Rational::ONE);
        assert_eq!(force.get_exponent(&length()), Rational::ONE);
        assert_eq!(force.get_exponent(&time()), Rational::from_int(-2));
    }

    #[test]
    fn dimension_sqrt() {
        // sqrt(Area) = sqrt(Length^2) = Length
        let area = Dimension::base(length()).pow_int(2).unwrap();
        let sqrt_area = area.pow(Rational::HALF).unwrap();
        assert_eq!(sqrt_area, Dimension::base(length()));
    }

    #[test]
    fn dimension_mul_div_inverse() {
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let velocity = (l.clone() / t.clone()).unwrap();

        // velocity * time = length
        assert_eq!((velocity.clone() * t.clone()).unwrap(), l);

        // length / velocity = time
        assert_eq!((l / velocity).unwrap(), t);
    }

    #[test]
    fn dimension_dimensionless_mul() {
        let l = Dimension::base(length());
        assert_eq!((Dimension::dimensionless() * l.clone()).unwrap(), l);
        assert_eq!((l.clone() * Dimension::dimensionless()).unwrap(), l);
    }

    #[test]
    fn dimension_display_simple() {
        let names = test_names();
        assert_eq!(
            Dimension::dimensionless().try_format_with(&names).unwrap(),
            "Dimensionless"
        );
        assert_eq!(
            Dimension::base(length()).try_format_with(&names).unwrap(),
            "Length"
        );
    }

    #[test]
    fn dimension_display_reports_missing_base_name() {
        let names = BTreeMap::new();

        assert_eq!(
            Dimension::base(length()).try_format_with(&names),
            Err(MissingBaseDimensionName { id: length() })
        );
    }

    #[test]
    fn dimension_display_velocity() {
        let names = test_names();
        let velocity = (Dimension::base(length()) / Dimension::base(time())).unwrap();
        assert_eq!(velocity.try_format_with(&names).unwrap(), "Length / Time");
    }

    #[test]
    fn dimension_display_force() {
        let names = test_names();
        let force = ((Dimension::base(mass()) * Dimension::base(length())).unwrap()
            / Dimension::base(time()).pow_int(2).unwrap())
        .unwrap();
        assert_eq!(
            force.try_format_with(&names).unwrap(),
            "Length * Mass / Time^2"
        );
    }

    #[test]
    fn dimension_display_area() {
        let names = test_names();
        let area = Dimension::base(length()).pow_int(2).unwrap();
        assert_eq!(area.try_format_with(&names).unwrap(), "Length^2");
    }

    #[test]
    fn dimension_display_frequency() {
        let names = test_names();
        // Frequency = Time^-1 (only negative exponent)
        let freq = (Dimension::dimensionless() / Dimension::base(time())).unwrap();
        assert_eq!(freq.try_format_with(&names).unwrap(), "Time^-1");
    }

    #[test]
    fn dimension_user_defined_base() {
        // User-defined base dimension gets a new ID
        let info_id = BaseDimId::UserDefined {
            dag: crate::dag_id::DagId::root_in_package("test", "test"),
            name: "Information".to_string(),
        };
        let information = Dimension::base(info_id.clone());
        let t = Dimension::base(time());
        let bandwidth = (information / t).unwrap();

        assert_eq!(bandwidth.get_exponent(&info_id), Rational::ONE);
        assert_eq!(bandwidth.get_exponent(&time()), Rational::from_int(-1));

        // Display with names
        let mut names = test_names();
        names.insert(info_id, "Information".to_string());
        assert_eq!(
            bandwidth.try_format_with(&names).unwrap(),
            "Information / Time"
        );
    }

    #[test]
    fn dimension_hash_consistency() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let a = (Dimension::base(length()) / Dimension::base(time())).unwrap();
        let b = (Dimension::base(length()) / Dimension::base(time())).unwrap();
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
                .prop_map(|(n, d)| Rational::try_new(n, d).expect("filtered d != 0"))
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
                let r = Rational::try_new(n, d).expect("d != 0 by prop_assume");
                // den is always positive
                prop_assert!(r.den() > 0, "den must be positive, got {}", r.den());
                // gcd(|num|, den) == 1 (reduced form)
                if r.num() != 0 {
                    let g = gcd64(
                        u64::from(r.num().unsigned_abs()),
                        u64::from(r.den().unsigned_abs()),
                    );
                    prop_assert_eq!(g, 1, "not reduced: {}/{}", r.num(), r.den());
                } else {
                    prop_assert_eq!(r.den(), 1, "zero should have den=1, got {}", r.den());
                }
            }

            #[test]
            fn rational_add_commutative(a in arb_rational(), b in arb_rational()) {
                prop_assert_eq!((a + b).unwrap(), (b + a).unwrap());
            }

            #[test]
            fn rational_mul_commutative(a in arb_rational(), b in arb_rational()) {
                prop_assert_eq!((a * b).unwrap(), (b * a).unwrap());
            }

            #[test]
            fn rational_additive_identity(a in arb_rational()) {
                prop_assert_eq!((a + Rational::ZERO).unwrap(), a);
            }

            #[test]
            fn rational_multiplicative_identity(a in arb_rational()) {
                prop_assert_eq!((a * Rational::ONE).unwrap(), a);
            }

            #[test]
            fn rational_additive_inverse(a in arb_rational()) {
                prop_assert_eq!((a + (-a)).unwrap(), Rational::ZERO);
            }

            #[test]
            fn rational_sub_self_is_zero(a in arb_rational()) {
                prop_assert_eq!((a - a).unwrap(), Rational::ZERO);
            }

            // --- Dimension invariants ---

            #[test]
            fn dimension_mul_commutative(a in arb_dimension(), b in arb_dimension()) {
                prop_assert_eq!((a.clone() * b.clone()).unwrap(), (b * a).unwrap());
            }

            #[test]
            fn dimension_dimensionless_is_mul_identity(a in arb_dimension()) {
                prop_assert_eq!((a.clone() * Dimension::dimensionless()).unwrap(), a);
            }

            #[test]
            fn dimension_self_div_is_dimensionless(a in arb_dimension()) {
                prop_assert_eq!((a.clone() / a).unwrap(), Dimension::dimensionless());
            }

            #[test]
            fn dimension_div_inverse(a in arb_dimension(), b in arb_dimension()) {
                // (a / b) * b == a
                prop_assert_eq!(((a.clone() / b.clone()).unwrap() * b).unwrap(), a);
            }

            #[test]
            fn dimension_pow_int_consistent_with_pow(a in arb_dimension(), n in -3i32..=3) {
                prop_assert_eq!(a.pow_int(n).unwrap(), a.pow(Rational::from_int(n)).unwrap());
            }

            #[test]
            fn dimension_pow_distributes_over_mul(
                a in arb_dimension(),
                b in arb_dimension(),
                r in arb_rational(),
            ) {
                // (a * b).pow(r) == a.pow(r) * b.pow(r)
                prop_assert_eq!(
                    (a.clone() * b.clone()).unwrap().pow(r).unwrap(),
                    (a.pow(r).unwrap() * b.pow(r).unwrap()).unwrap(),
                );
            }
        }
    }
}
