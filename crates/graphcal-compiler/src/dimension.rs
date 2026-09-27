//! Core physical-dimension algebra.

use std::fmt;

use crate::ratio::{ExponentStyle, Ratio, RatioError};
use crate::sparse_monomial::SparseMonomial;

/// A dimension exponent (e.g., `1/2` for sqrt), in the symmetric `i32` range.
pub type Rational = Ratio<i32>;

macro_rules! define_prelude_base_dimensions {
    (@unit $_variant:ident) => { () };
    (@count $($variant:ident),+ $(,)?) => {
        <[()]>::len(&[$(define_prelude_base_dimensions!(@unit $variant)),+])
    };
    (
        $(
            $(#[$variant_meta:meta])*
            $variant:ident => $name:literal
        ),+ $(,)?
    ) => {
        /// Closed vocabulary of independent physical dimensions built into the prelude.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum PreludeBaseDimension {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl PreludeBaseDimension {
            /// Every prelude base dimension in canonical registration order.
            pub const ALL: [Self; define_prelude_base_dimensions!(@count $($variant),+)] = [
                $(Self::$variant),+
            ];

            /// Canonical source spellings in the same order as [`Self::ALL`].
            pub const ALL_NAMES: [&'static str; Self::ALL.len()] = [$($name),+];

            /// Parse a source spelling at the text-to-semantic boundary.
            #[must_use]
            pub fn parse(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Canonical source spelling.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }

        impl PartialOrd for PreludeBaseDimension {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for PreludeBaseDimension {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.as_str().cmp(other.as_str())
            }
        }

        impl fmt::Display for PreludeBaseDimension {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

define_prelude_base_dimensions! {
    /// Distance.
    Length => "Length",
    /// Duration.
    Time => "Time",
    /// Inertial mass.
    Mass => "Mass",
    /// Thermodynamic temperature.
    Temperature => "Temperature",
    /// Electric current.
    ElectricCurrent => "ElectricCurrent",
    /// Amount of substance.
    Amount => "Amount",
    /// Luminous intensity.
    LuminousIntensity => "LuminousIntensity",
    /// Plane angle.
    Angle => "Angle",
}

/// A unique identifier for a base dimension.
///
/// Identity is name-based rather than auto-incremented, ensuring consistency
/// across per-file compilation units (important for diamond imports).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BaseDimId {
    /// Built-in prelude dimension.
    Prelude(PreludeBaseDimension),
    /// User-defined dimension with its canonical defining DAG and typed leaf.
    UserDefined(crate::syntax::dimension::ResolvedDimName),
}

impl BaseDimId {
    /// Canonical leaf spelling used only at display/serialization boundaries.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Prelude(dimension) => dimension.as_str(),
            Self::UserDefined(name) => name.as_str(),
        }
    }

    /// The unqualified source name that makes this base dimension visible in
    /// its defining scope (`Length`, or the user-defined leaf).
    #[must_use]
    pub fn source_name(&self) -> crate::syntax::dimension::DimName {
        match self {
            Self::Prelude(dimension) => {
                crate::syntax::dimension::DimName::expect_valid(dimension.as_str())
            }
            Self::UserDefined(name) => name.to_unowned_def_name(),
        }
    }
}

/// A physical dimension represented as a sparse vector of rational exponents
/// over base dimensions.
///
/// For example, Velocity = Length^1 * Time^-1 is represented as
/// `{BaseDimId::Prelude(Length): 1, BaseDimId::Prelude(Time): -1}`.
///
/// Only non-zero exponents are stored. The unit monomial represents the
/// dimensionless dimension.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Dimension {
    /// Non-zero exponents only. Sorted by `BaseDimId` for deterministic equality/hash.
    exponents: SparseMonomial<BaseDimId, Rational>,
}

impl fmt::Debug for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_dimensionless() {
            write!(f, "Dimension(Dimensionless)")
        } else {
            write!(f, "Dimension(")?;
            let mut first = true;
            for (id, exp) in self.exponents.iter() {
                if !first {
                    write!(f, " * ")?;
                }
                first = false;
                write!(f, "{}", id.name())?;
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
            exponents: SparseMonomial::one(),
        }
    }

    /// A dimension with a single base dimension at exponent 1.
    #[must_use]
    pub fn base(id: BaseDimId) -> Self {
        Self {
            exponents: SparseMonomial::single(id, Rational::ONE),
        }
    }

    #[must_use]
    pub fn is_dimensionless(&self) -> bool {
        self.exponents.is_empty()
    }

    /// Return the base-dimension identity when this dimension consists of
    /// exactly one base dimension to the first power.
    #[must_use]
    pub(crate) fn base_dimension_id(&self) -> Option<&BaseDimId> {
        match self.exponents.as_single() {
            Some((id, &Rational::ONE)) => Some(id),
            _ => None,
        }
    }

    /// Returns true when this dimension cannot be represented by a single base
    /// dimension to the first power.
    #[must_use]
    pub(crate) fn is_compound(&self) -> bool {
        !self.exponents.is_empty() && self.base_dimension_id().is_none()
    }

    /// Get the exponent for a specific base dimension (zero if absent).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn get_exponent(&self, id: &BaseDimId) -> Rational {
        self.exponents.get(id).unwrap_or(Rational::ZERO)
    }

    /// Returns an iterator over the non-zero `(BaseDimId, Rational)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&BaseDimId, &Rational)> {
        self.exponents.iter()
    }

    /// Raise a dimension to a power (multiply all exponents).
    ///
    /// Small integer literals (`i16`) are accepted directly and converted to [`Rational`].
    ///
    /// Returns `Err(RatioError::Overflow)` if any exponent multiplication
    /// produces a reduced value outside the symmetric `i32` range.
    pub fn pow(&self, exp: impl Into<Rational>) -> Result<Self, RatioError> {
        Ok(Self {
            exponents: self.exponents.try_pow(exp.into())?,
        })
    }

    /// Render user-defined base dimensions with their canonical owner
    /// (`pkg.mod.Rate`) instead of the bare leaf.
    ///
    /// Used only when two unequal dimensions would otherwise render with the
    /// same leaf-only spelling.
    #[must_use]
    pub fn owner_qualified(&self) -> impl fmt::Display + '_ {
        OwnerQualifiedDimension(self)
    }

    /// Write the exponents as `num * num / den / den`, naming each base
    /// dimension with `name`.
    fn fmt_exponents(
        &self,
        f: &mut fmt::Formatter<'_>,
        name: impl Fn(&BaseDimId, &mut fmt::Formatter<'_>) -> fmt::Result,
    ) -> fmt::Result {
        if self.is_dimensionless() {
            return f.write_str("Dimensionless");
        }
        let factor = |f: &mut fmt::Formatter<'_>, id: &BaseDimId, exp: Rational| {
            name(id, f)?;
            if exp == Rational::ONE {
                Ok(())
            } else {
                write!(f, "{}", exp.fmt_exponent(ExponentStyle::Compact))
            }
        };
        let mut first = true;

        // Positive exponents (numerator)
        for (id, &exp) in self.exponents.iter().filter(|(_, exp)| exp.is_positive()) {
            if !first {
                f.write_str(" * ")?;
            }
            first = false;
            factor(f, id, exp)?;
        }

        // Negative exponents (denominator)
        for (id, &exp) in self.exponents.iter().filter(|(_, exp)| exp.is_negative()) {
            if first {
                // Only negative exponents (e.g., Frequency = Time^-1).
                first = false;
                factor(f, id, exp)?;
            } else {
                f.write_str(" / ")?;
                factor(f, id, -exp)?;
            }
        }
        Ok(())
    }
}

/// Canonical rendering: every base dimension by its leaf spelling
/// (`Length * Mass / Time^2`, `Dimensionless`).
impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_exponents(f, |id, f| f.write_str(id.name()))
    }
}

struct OwnerQualifiedDimension<'a>(&'a Dimension);

impl fmt::Display for OwnerQualifiedDimension<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt_exponents(f, |id, f| match id {
            BaseDimId::Prelude(dimension) => f.write_str(dimension.as_str()),
            BaseDimId::UserDefined(resolved) => write!(f, "{resolved}"),
        })
    }
}

impl Dimension {
    /// Multiply two dimensions, returning an error if exponent arithmetic overflows.
    pub fn checked_mul(self, other: &Self) -> Result<Self, RatioError> {
        Ok(Self {
            exponents: self.exponents.try_mul(&other.exponents)?,
        })
    }

    /// Divide two dimensions, returning an error if exponent arithmetic overflows.
    pub fn checked_div(self, other: &Self) -> Result<Self, RatioError> {
        Ok(Self {
            exponents: self.exponents.try_div(&other.exponents)?,
        })
    }
}

impl std::ops::Mul for Dimension {
    type Output = Result<Self, RatioError>;
    /// Multiply two dimensions (add exponents).
    fn mul(self, other: Self) -> Self::Output {
        self.checked_mul(&other)
    }
}

impl std::ops::Div for Dimension {
    type Output = Result<Self, RatioError>;
    /// Divide two dimensions (subtract exponents).
    fn div(self, other: Self) -> Self::Output {
        self.checked_div(&other)
    }
}

impl std::ops::Mul for &Dimension {
    type Output = Result<Dimension, RatioError>;
    fn mul(self, other: Self) -> Self::Output {
        self.clone().checked_mul(other)
    }
}

impl std::ops::Div for &Dimension {
    type Output = Result<Dimension, RatioError>;
    fn div(self, other: Self) -> Self::Output {
        self.clone().checked_div(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: well-known base dimension IDs matching prelude dimensions.
    fn length() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Length)
    }
    fn time() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Time)
    }
    fn mass() -> BaseDimId {
        BaseDimId::Prelude(PreludeBaseDimension::Mass)
    }

    #[test]
    fn prelude_base_dimension_vocabulary_is_closed_and_roundtrips() {
        assert_eq!(
            PreludeBaseDimension::ALL.map(PreludeBaseDimension::as_str),
            PreludeBaseDimension::ALL_NAMES
        );
        for dimension in PreludeBaseDimension::ALL {
            assert_eq!(
                PreludeBaseDimension::parse(dimension.as_str()),
                Some(dimension)
            );
        }
        assert_eq!(PreludeBaseDimension::parse("length"), None);
        assert_eq!(PreludeBaseDimension::parse("Velocity"), None);
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
    fn base_dimension_id_requires_exactly_one_first_power_term() {
        let length_id = length();
        assert_eq!(
            Dimension::base(length_id.clone()).base_dimension_id(),
            Some(&length_id)
        );
        assert!(Dimension::dimensionless().base_dimension_id().is_none());
        assert!(
            Dimension::base(length())
                .pow(2)
                .unwrap()
                .base_dimension_id()
                .is_none()
        );
        assert!(
            (Dimension::base(length()) / Dimension::base(time()))
                .unwrap()
                .base_dimension_id()
                .is_none()
        );
    }

    #[test]
    fn dimension_velocity() {
        // Velocity = Length / Time
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let velocity = (l / t).unwrap();

        assert_eq!(velocity.get_exponent(&length()), Rational::ONE);
        assert_eq!(velocity.get_exponent(&time()), Rational::from(-1));
    }

    #[test]
    fn dimension_acceleration() {
        // Acceleration = Length / Time^2
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let accel = (l / t.pow(2).unwrap()).unwrap();

        assert_eq!(accel.get_exponent(&length()), Rational::ONE);
        assert_eq!(accel.get_exponent(&time()), Rational::from(-2));
    }

    #[test]
    fn dimension_force() {
        // Force = Mass * Length / Time^2
        let m = Dimension::base(mass());
        let l = Dimension::base(length());
        let t = Dimension::base(time());
        let force = ((m * l).unwrap() / t.pow(2).unwrap()).unwrap();

        assert_eq!(force.get_exponent(&mass()), Rational::ONE);
        assert_eq!(force.get_exponent(&length()), Rational::ONE);
        assert_eq!(force.get_exponent(&time()), Rational::from(-2));
    }

    #[test]
    fn dimension_sqrt() {
        // sqrt(Area) = sqrt(Length^2) = Length
        let area = Dimension::base(length()).pow(2).unwrap();
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
        assert_eq!(Dimension::dimensionless().to_string(), "Dimensionless");
        assert_eq!(Dimension::base(length()).to_string(), "Length");
    }

    #[test]
    fn dimension_display_velocity() {
        let velocity = (Dimension::base(length()) / Dimension::base(time())).unwrap();
        assert_eq!(velocity.to_string(), "Length / Time");
    }

    #[test]
    fn dimension_display_force() {
        let force = ((Dimension::base(mass()) * Dimension::base(length())).unwrap()
            / Dimension::base(time()).pow(2).unwrap())
        .unwrap();
        assert_eq!(force.to_string(), "Length * Mass / Time^2");
    }

    #[test]
    fn dimension_display_renders_extreme_denominator_magnitude() {
        let dimension = Dimension {
            exponents: SparseMonomial::try_from_factors([
                (length(), Rational::integer(-i32::MAX).unwrap()),
                (mass(), Rational::ONE),
            ])
            .unwrap(),
        };

        assert_eq!(dimension.to_string(), "Mass / Length^2147483647");
    }

    #[test]
    fn dimension_display_area() {
        let area = Dimension::base(length()).pow(2).unwrap();
        assert_eq!(area.to_string(), "Length^2");
    }

    #[test]
    fn dimension_display_frequency() {
        // Frequency = Time^-1 (only negative exponent)
        let freq = (Dimension::dimensionless() / Dimension::base(time())).unwrap();
        assert_eq!(freq.to_string(), "Time^-1");
    }

    #[test]
    fn dimension_user_defined_base() {
        // User-defined base dimension gets a new ID
        let resolved = crate::syntax::dimension::ResolvedDimName::from_def(
            crate::dag_id::DagId::root_in_package("test", "test"),
            crate::syntax::dimension::DimName::expect_valid("Information"),
        );
        let info_id = BaseDimId::UserDefined(resolved.clone());
        let information = Dimension::base(info_id.clone());
        let t = Dimension::base(time());
        let bandwidth = (information / t).unwrap();

        assert_eq!(bandwidth.get_exponent(&info_id), Rational::ONE);
        assert_eq!(bandwidth.get_exponent(&time()), Rational::from(-1));

        assert_eq!(bandwidth.to_string(), "Information / Time");
        assert_eq!(
            bandwidth.owner_qualified().to_string(),
            format!("{resolved} / Time")
        );
        assert_eq!(
            Dimension::base(length()).owner_qualified().to_string(),
            "Length"
        );
        assert_eq!(info_id.source_name().as_str(), "Information");
        assert_eq!(length().source_name().as_str(), "Length");
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

        /// Strategy for generating Dimension values with small exponents.
        /// Uses a fixed set of prelude base dimension IDs.
        fn arb_dimension() -> impl Strategy<Value = Dimension> {
            proptest::collection::btree_map(0usize..8, arb_rational(), 0..=8).prop_map(|map| {
                let exponents = map
                    .into_iter()
                    .filter(|(_, r)| !r.is_zero())
                    .map(|(idx, r)| (BaseDimId::Prelude(PreludeBaseDimension::ALL[idx]), r));
                Dimension {
                    exponents: SparseMonomial::try_from_factors(exponents).unwrap(),
                }
            })
        }

        proptest! {
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
            fn dimension_pow_accepts_integer_exponents(a in arb_dimension(), n in -3i16..=3) {
                prop_assert_eq!(a.pow(n).unwrap(), a.pow(Rational::from(n)).unwrap());
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
