//! Core physical-dimension algebra.

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

use crate::ratio::{ExponentStyle, Ratio, RatioError};

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
}

/// A physical dimension represented as a sparse vector of rational exponents
/// over base dimensions.
///
/// For example, Velocity = Length^1 * Time^-1 is represented as
/// `{BaseDimId::Prelude(Length): 1, BaseDimId::Prelude(Time): -1}`.
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

    /// Return the base-dimension identity when this dimension consists of
    /// exactly one base dimension to the first power.
    #[must_use]
    pub(crate) fn base_dimension_id(&self) -> Option<&BaseDimId> {
        match self.exponents.iter().next() {
            Some((id, &Rational::ONE)) if self.exponents.len() == 1 => Some(id),
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
        self.exponents.get(id).copied().unwrap_or(Rational::ZERO)
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
        let exp = exp.into();
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

    /// Format this dimension using named base dimensions for display.
    ///
    /// The `names` map must provide a `BaseDimId → name` mapping for every
    /// base dimension in `self`.
    ///
    /// # Errors
    ///
    /// Returns [`MissingBaseDimensionName`] when `names` does not contain an
    /// entry for a base dimension referenced by `self`.
    pub(crate) fn try_format_with(
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
            if !exp.is_positive() {
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
            if !exp.is_negative() {
                continue;
            }
            let name = registered_base_dim_name(names, id)?;
            if first {
                // Only negative exponents (e.g., Frequency = s^-1).
                push_dim_factor(&mut out, name, exp);
                first = false;
            } else {
                out.push_str(div_sep);
                push_dim_factor(&mut out, name, -exp);
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
        out.push_str(&exp.fmt_exponent(ExponentStyle::Compact).to_string());
    }
}

/// A base dimension referenced by a [`Dimension`] had no registered display name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("missing display name for base dimension {id:?}")]
pub struct MissingBaseDimensionName {
    id: BaseDimId,
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
    pub fn checked_mul(self, other: &Self) -> Result<Self, RatioError> {
        self.combine(other, CombineOp::Add)
    }

    /// Divide two dimensions, returning an error if exponent arithmetic overflows.
    pub fn checked_div(self, other: &Self) -> Result<Self, RatioError> {
        self.combine(other, CombineOp::Sub)
    }

    /// Combine two dimensions by adding or subtracting exponents.
    fn combine(self, other: &Self, op: CombineOp) -> Result<Self, RatioError> {
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

    /// Build a names map for display tests.
    fn test_names() -> BTreeMap<BaseDimId, String> {
        PreludeBaseDimension::ALL
            .into_iter()
            .map(|dimension| {
                (
                    BaseDimId::Prelude(dimension),
                    dimension.as_str().to_string(),
                )
            })
            .collect()
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
            / Dimension::base(time()).pow(2).unwrap())
        .unwrap();
        assert_eq!(
            force.try_format_with(&names).unwrap(),
            "Length * Mass / Time^2"
        );
    }

    #[test]
    fn dimension_display_renders_extreme_denominator_magnitude() {
        let dimension = Dimension {
            exponents: BTreeMap::from([
                (length(), Rational::integer(-i32::MAX).unwrap()),
                (mass(), Rational::ONE),
            ]),
        };

        assert_eq!(
            dimension.try_format_with(&test_names()).unwrap(),
            "Mass / Length^2147483647"
        );
    }

    #[test]
    fn dimension_display_area() {
        let names = test_names();
        let area = Dimension::base(length()).pow(2).unwrap();
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
        let info_id = BaseDimId::UserDefined(crate::syntax::dimension::ResolvedDimName::from_def(
            crate::dag_id::DagId::root_in_package("test", "test"),
            crate::syntax::dimension::DimName::expect_valid("Information"),
        ));
        let information = Dimension::base(info_id.clone());
        let t = Dimension::base(time());
        let bandwidth = (information / t).unwrap();

        assert_eq!(bandwidth.get_exponent(&info_id), Rational::ONE);
        assert_eq!(bandwidth.get_exponent(&time()), Rational::from(-1));

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

        /// Strategy for generating Dimension values with small exponents.
        /// Uses a fixed set of prelude base dimension IDs.
        fn arb_dimension() -> impl Strategy<Value = Dimension> {
            proptest::collection::btree_map(0usize..8, arb_rational(), 0..=8).prop_map(|map| {
                let exponents = map
                    .into_iter()
                    .filter(|(_, r)| !r.is_zero())
                    .map(|(idx, r)| (BaseDimId::Prelude(PreludeBaseDimension::ALL[idx]), r))
                    .collect();
                Dimension { exponents }
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
