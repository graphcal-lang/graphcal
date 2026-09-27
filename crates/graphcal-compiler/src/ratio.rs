//! Reduced rational numbers over a symmetric fixed-width integer range.
//!
//! [`Ratio<T>`] is the single rational implementation shared by dimension
//! exponents (`Ratio<i32>`, see [`crate::dimension::Rational`]) and exact
//! source-level power exponents (`Ratio<i64>`, see
//! [`crate::exact_rational::ExactRational`]).
//!
//! Invariants, enforced by every constructor:
//!
//! - the denominator is positive;
//! - numerator and denominator are coprime (zero is `0/1`);
//! - both components lie in the *symmetric* range `-T::MAX..=T::MAX`.
//!
//! Excluding `T::MIN` makes negation total, so no caller has to handle an
//! "unrepresentable magnitude" case when moving a factor across a fraction bar.

use std::fmt;
use std::hash::Hash;
use std::ops::Neg;

use thiserror::Error;

use crate::sparse_monomial::MonomialExponent;

mod sealed {
    pub trait Sealed {}
}

/// Fixed-width signed integers that can carry a [`Ratio`].
///
/// Arithmetic is performed in `i128` and narrowed back, so implementors must
/// be at most 64 bits wide: products of two in-range values then always fit.
pub trait RatioInt:
    sealed::Sealed + Copy + Eq + Hash + Neg<Output = Self> + fmt::Display + fmt::Debug
{
    /// Additive identity.
    const ZERO: Self;
    /// Multiplicative identity.
    const ONE: Self;

    /// Widen losslessly to the arithmetic representation.
    fn widen(self) -> i128;

    /// Narrow a non-negative magnitude, rejecting values above `Self::MAX`.
    fn narrow_magnitude(magnitude: u128) -> Option<Self>;
}

macro_rules! impl_ratio_int {
    ($($int:ty),+ $(,)?) => {
        $(
            impl sealed::Sealed for $int {}

            impl RatioInt for $int {
                const ZERO: Self = 0;
                const ONE: Self = 1;

                fn widen(self) -> i128 {
                    i128::from(self)
                }

                fn narrow_magnitude(magnitude: u128) -> Option<Self> {
                    Self::try_from(magnitude).ok()
                }
            }
        )+
    };
}

impl_ratio_int!(i32, i64);

/// A reduced rational number with a positive denominator and components in
/// the symmetric range of `T`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio<T> {
    num: T,
    den: T,
}

/// Failure to construct a [`Ratio`] or to compute with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RatioError {
    /// The denominator was zero.
    #[error("denominator must not be zero")]
    ZeroDenominator,
    /// The reduced numerator or denominator fell outside the symmetric range
    /// of the component integer type.
    #[error("rational value overflowed its integer range")]
    Overflow,
}

impl<T: RatioInt> Ratio<T> {
    /// Zero (`0/1`).
    pub const ZERO: Self = Self {
        num: T::ZERO,
        den: T::ONE,
    };
    /// One (`1/1`).
    pub const ONE: Self = Self {
        num: T::ONE,
        den: T::ONE,
    };

    /// Construct `num / den`, reduced.
    ///
    /// # Errors
    ///
    /// Returns [`RatioError::ZeroDenominator`] for a zero denominator and
    /// [`RatioError::Overflow`] when the reduced value lies outside the
    /// symmetric range (only possible when a component is `T::MIN`).
    pub fn try_new(num: T, den: T) -> Result<Self, RatioError> {
        Self::try_from_wide(num.widen(), den.widen())
    }

    /// Construct an integer value.
    ///
    /// # Errors
    ///
    /// Returns [`RatioError::Overflow`] for `T::MIN`.
    pub fn integer(value: T) -> Result<Self, RatioError> {
        Self::try_new(value, T::ONE)
    }

    /// Construct from widened arithmetic, reducing before narrowing.
    ///
    /// # Errors
    ///
    /// See [`Self::try_new`].
    pub(crate) fn try_from_wide(num: i128, den: i128) -> Result<Self, RatioError> {
        if den == 0 {
            return Err(RatioError::ZeroDenominator);
        }
        if num == 0 {
            return Ok(Self::ZERO);
        }
        let (num_magnitude, den_magnitude) = (num.unsigned_abs(), den.unsigned_abs());
        let divisor = gcd(num_magnitude, den_magnitude);
        let num_magnitude =
            T::narrow_magnitude(num_magnitude / divisor).ok_or(RatioError::Overflow)?;
        let den_reduced =
            T::narrow_magnitude(den_magnitude / divisor).ok_or(RatioError::Overflow)?;
        let negative = (num < 0) != (den < 0);
        let num = if negative {
            -num_magnitude
        } else {
            num_magnitude
        };
        Ok(Self {
            num,
            den: den_reduced,
        })
    }

    /// Numerator of the reduced value (carries the sign).
    #[must_use]
    pub const fn num(self) -> T {
        self.num
    }

    /// Denominator of the reduced value (always positive).
    #[must_use]
    pub const fn den(self) -> T {
        self.den
    }

    /// Whether the value is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.num == T::ZERO
    }

    /// Whether the value is an integer.
    #[must_use]
    pub fn is_integer(self) -> bool {
        self.den == T::ONE
    }

    /// Whether the value is strictly positive.
    #[must_use]
    pub fn is_positive(self) -> bool {
        self.num.widen() > 0
    }

    /// Whether the value is strictly negative.
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.num.widen() < 0
    }

    /// The reciprocal `den / num`.
    ///
    /// # Errors
    ///
    /// Returns [`RatioError::ZeroDenominator`] for zero.
    pub fn recip(self) -> Result<Self, RatioError> {
        Self::try_new(self.den, self.num)
    }

    /// Render the value as it is spelled in an exponent position of source
    /// syntax: `2`, `-3`, or the parenthesized `(1/2)`.
    #[must_use]
    pub fn source_syntax(self) -> impl fmt::Display {
        Exponent {
            value: self,
            style: ExponentStyle::Source,
            caret: false,
        }
    }

    /// Render an exponent suffix: `^2`, and `^(1/2)` or `^1/2` for fractions
    /// depending on `style`. This is the single exponent renderer; callers
    /// decide whether an exponent of one is shown at all.
    #[must_use]
    pub fn fmt_exponent(self, style: ExponentStyle) -> impl fmt::Display {
        Exponent {
            value: self,
            style,
            caret: true,
        }
    }

    fn checked_combine(
        self,
        rhs: Self,
        combine: impl FnOnce(i128, i128) -> Option<i128>,
    ) -> Result<Self, RatioError> {
        let lhs_scaled = self.num.widen().checked_mul(rhs.den.widen());
        let rhs_scaled = rhs.num.widen().checked_mul(self.den.widen());
        let den = self.den.widen().checked_mul(rhs.den.widen());
        match (lhs_scaled, rhs_scaled, den) {
            (Some(lhs), Some(rhs), Some(den)) => {
                let num = combine(lhs, rhs).ok_or(RatioError::Overflow)?;
                Self::try_from_wide(num, den)
            }
            _ => Err(RatioError::Overflow),
        }
    }
}

impl<T: RatioInt> MonomialExponent for Ratio<T> {
    type Error = RatioError;

    fn is_zero(self) -> bool {
        Self::is_zero(self)
    }

    fn checked_add(self, rhs: Self) -> Result<Self, RatioError> {
        self + rhs
    }

    fn checked_mul(self, rhs: Self) -> Result<Self, RatioError> {
        self * rhs
    }
}

impl Ratio<i32> {
    /// `1/2` — used for square-root exponents.
    pub const HALF: Self = Self { num: 1, den: 2 };
    /// `1/3` — used for cube-root exponents.
    pub const THIRD: Self = Self { num: 1, den: 3 };
}

/// Small integer literals always fit the symmetric `i32` range.
impl From<i16> for Ratio<i32> {
    fn from(value: i16) -> Self {
        Self {
            num: i32::from(value),
            den: 1,
        }
    }
}

impl From<Ratio<i32>> for Ratio<i64> {
    fn from(value: Ratio<i32>) -> Self {
        Self {
            num: i64::from(value.num),
            den: i64::from(value.den),
        }
    }
}

impl TryFrom<Ratio<i64>> for Ratio<i32> {
    type Error = RatioError;

    fn try_from(value: Ratio<i64>) -> Result<Self, Self::Error> {
        Self::try_from_wide(i128::from(value.num), i128::from(value.den))
    }
}

impl<T: RatioInt> Neg for Ratio<T> {
    type Output = Self;

    /// Total: the symmetric range guarantees the negated numerator fits.
    fn neg(self) -> Self {
        Self {
            num: -self.num,
            den: self.den,
        }
    }
}

impl<T: RatioInt> std::ops::Add for Ratio<T> {
    type Output = Result<Self, RatioError>;

    fn add(self, rhs: Self) -> Self::Output {
        self.checked_combine(rhs, i128::checked_add)
    }
}

impl<T: RatioInt> std::ops::Sub for Ratio<T> {
    type Output = Result<Self, RatioError>;

    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_combine(rhs, i128::checked_sub)
    }
}

impl<T: RatioInt> std::ops::Mul for Ratio<T> {
    type Output = Result<Self, RatioError>;

    fn mul(self, rhs: Self) -> Self::Output {
        let num = self.num.widen().checked_mul(rhs.num.widen());
        let den = self.den.widen().checked_mul(rhs.den.widen());
        match (num, den) {
            (Some(num), Some(den)) => Self::try_from_wide(num, den),
            _ => Err(RatioError::Overflow),
        }
    }
}

/// Compact rendering: `3`, `-1/2`.
impl<T: RatioInt> fmt::Display for Ratio<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_integer() {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl<T: RatioInt> fmt::Debug for Ratio<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// How a fractional exponent is spelled after `^`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExponentStyle {
    /// `^(1/2)`: the re-parseable source-syntax form.
    Source,
    /// `^1/2`: the compact form used in dimension names and default unit
    /// labels.
    Compact,
}

struct Exponent<T> {
    value: Ratio<T>,
    style: ExponentStyle,
    caret: bool,
}

impl<T: RatioInt> fmt::Display for Exponent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.caret {
            f.write_str("^")?;
        }
        match self.style {
            ExponentStyle::Source if !self.value.is_integer() => {
                write!(f, "({}/{})", self.value.num, self.value.den)
            }
            ExponentStyle::Source | ExponentStyle::Compact => fmt::Display::fmt(&self.value, f),
        }
    }
}

const fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(num: i32, den: i32) -> Ratio<i32> {
        Ratio::try_new(num, den).expect("valid test rational")
    }

    #[test]
    fn construction_reduces_and_normalizes_sign() {
        assert_eq!(r(2, 4), r(1, 2));
        assert_eq!(r(-3, 6), r(-1, 2));
        assert_eq!(r(6, -4), r(-3, 2));
        assert_eq!(r(-6, -4), r(3, 2));
        assert_eq!(r(0, -5), Ratio::ZERO);
        assert_eq!(
            Ratio::<i32>::try_new(1, 0),
            Err(RatioError::ZeroDenominator)
        );
    }

    #[test]
    fn range_is_symmetric() {
        assert_eq!(Ratio::integer(i32::MIN), Err(RatioError::Overflow));
        assert_eq!(
            Ratio::<i32>::try_new(1, i32::MIN),
            Err(RatioError::Overflow)
        );
        // `MIN / 2` reduces into range.
        assert_eq!(Ratio::try_new(i32::MIN, 2), Ratio::integer(-(1 << 30)));
        let max = Ratio::integer(i32::MAX).unwrap();
        assert_eq!((-max).num(), -i32::MAX);
        assert_eq!(-(-max), max);
    }

    #[test]
    fn arithmetic_is_exact_and_checked() {
        let half = r(1, 2);
        let third = r(1, 3);
        assert_eq!((half + third).unwrap(), r(5, 6));
        assert_eq!((half - third).unwrap(), r(1, 6));
        assert_eq!((half * third).unwrap(), r(1, 6));
        assert_eq!(-half, r(-1, 2));
        let max = Ratio::integer(i32::MAX).unwrap();
        assert_eq!(max + Ratio::ONE, Err(RatioError::Overflow));
        assert_eq!(max * max, Err(RatioError::Overflow));
        let wide_max = Ratio::integer(i64::MAX).unwrap();
        assert_eq!((wide_max * wide_max), Err(RatioError::Overflow));
        assert_eq!((wide_max + (-wide_max)).unwrap(), Ratio::ZERO);
    }

    #[test]
    fn widening_and_narrowing_preserve_value() {
        let wide = Ratio::<i64>::from(r(-3, 2));
        assert_eq!(wide, Ratio::try_new(-3, 2).unwrap());
        assert_eq!(Ratio::<i32>::try_from(wide), Ok(r(-3, 2)));
        assert_eq!(
            Ratio::<i32>::try_from(Ratio::<i64>::integer(i64::from(i32::MAX) + 1).unwrap()),
            Err(RatioError::Overflow)
        );
    }

    #[test]
    fn exponent_rendering_styles() {
        assert_eq!(r(1, 2).to_string(), "1/2");
        assert_eq!(r(-3, 1).to_string(), "-3");
        assert_eq!(
            r(2, 1).fmt_exponent(ExponentStyle::Source).to_string(),
            "^2"
        );
        assert_eq!(
            r(-1, 2).fmt_exponent(ExponentStyle::Source).to_string(),
            "^(-1/2)"
        );
        assert_eq!(
            r(-1, 2).fmt_exponent(ExponentStyle::Compact).to_string(),
            "^-1/2"
        );
        assert_eq!(r(1, 2).source_syntax().to_string(), "(1/2)");
        assert_eq!(r(4, 1).source_syntax().to_string(), "4");
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        fn arb_ratio() -> impl Strategy<Value = Ratio<i32>> {
            (-50i32..=50, -50i32..=50)
                .prop_filter("denominator must be non-zero", |&(_, d)| d != 0)
                .prop_map(|(n, d)| Ratio::try_new(n, d).expect("filtered d != 0"))
        }

        proptest! {
            #[test]
            fn always_reduced(n in any::<i32>(), d in any::<i32>()) {
                prop_assume!(d != 0);
                if let Ok(value) = Ratio::try_new(n, d) {
                    prop_assert!(value.den() > 0);
                    prop_assert!(value.num() != i32::MIN);
                    let g = gcd(
                        u128::from(value.num().unsigned_abs()),
                        u128::from(value.den().unsigned_abs()),
                    );
                    prop_assert_eq!(g, 1);
                }
            }

            #[test]
            fn add_commutative(a in arb_ratio(), b in arb_ratio()) {
                prop_assert_eq!((a + b).unwrap(), (b + a).unwrap());
            }

            #[test]
            fn mul_commutative(a in arb_ratio(), b in arb_ratio()) {
                prop_assert_eq!((a * b).unwrap(), (b * a).unwrap());
            }

            #[test]
            fn identities(a in arb_ratio()) {
                prop_assert_eq!((a + Ratio::ZERO).unwrap(), a);
                prop_assert_eq!((a * Ratio::ONE).unwrap(), a);
                prop_assert_eq!((a - a).unwrap(), Ratio::ZERO);
            }

            #[test]
            fn negation_is_total_additive_inverse(n in any::<i32>(), d in 1i32..) {
                if let Ok(value) = Ratio::try_new(n, d) {
                    prop_assert_eq!(-(-value), value);
                    prop_assert_eq!((value + (-value)).unwrap(), Ratio::ZERO);
                }
            }
        }
    }
}
