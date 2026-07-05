//! Exact rational arithmetic for dimension exponents at macro-expansion
//! time.
//!
//! Wider than the wire form on purpose: intermediate products of user
//! exponents are computed in `i64` and reduced, and only the final,
//! reduced values must fit the manifest's `i32` fields (checked when the
//! manifest is built).

/// A reduced rational with a positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    num: i64,
    den: i64,
}

impl Default for Rational {
    /// Zero, so exponent accumulators start from the empty product.
    fn default() -> Self {
        Self::ZERO
    }
}

impl Rational {
    pub const ZERO: Self = Self { num: 0, den: 1 };
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// Build a reduced rational. Returns `None` for a zero denominator or
    /// when sign normalization overflows.
    pub fn new(num: i64, den: i64) -> Option<Self> {
        if den == 0 {
            return None;
        }
        let (num, den) = if den < 0 {
            (num.checked_neg()?, den.checked_neg()?)
        } else {
            (num, den)
        };
        let divisor = gcd(num.unsigned_abs(), den.unsigned_abs());
        // `divisor` is at least 1 here: den != 0 implies its abs is >= 1.
        let divisor = i64::try_from(divisor).ok()?;
        Some(Self {
            num: num / divisor,
            den: den / divisor,
        })
    }

    pub const fn num(self) -> i64 {
        self.num
    }

    pub const fn den(self) -> i64 {
        self.den
    }

    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    /// `self + other`, or `None` on overflow.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        let num = self
            .num
            .checked_mul(other.den)?
            .checked_add(other.num.checked_mul(self.den)?)?;
        Self::new(num, self.den.checked_mul(other.den)?)
    }

    /// `self * other`, or `None` on overflow.
    pub fn checked_mul(self, other: Self) -> Option<Self> {
        Self::new(
            self.num.checked_mul(other.num)?,
            self.den.checked_mul(other.den)?,
        )
    }

    /// `-self`, or `None` on overflow (`num == i64::MIN`).
    pub fn checked_neg(self) -> Option<Self> {
        Some(Self {
            num: self.num.checked_neg()?,
            den: self.den,
        })
    }
}

const fn gcd(a: u64, b: u64) -> u64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_normalizes_sign() {
        let r = Rational::new(2, 4).unwrap();
        assert_eq!((r.num(), r.den()), (1, 2));
        let r = Rational::new(1, -2).unwrap();
        assert_eq!((r.num(), r.den()), (-1, 2));
        let r = Rational::new(-6, -4).unwrap();
        assert_eq!((r.num(), r.den()), (3, 2));
        assert!(Rational::new(1, 0).is_none());
    }

    #[test]
    fn adds_and_multiplies_exactly() {
        let half = Rational::new(1, 2).unwrap();
        let third = Rational::new(1, 3).unwrap();
        let sum = half.checked_add(third).unwrap();
        assert_eq!((sum.num(), sum.den()), (5, 6));
        let product = half.checked_mul(third).unwrap();
        assert_eq!((product.num(), product.den()), (1, 6));
        let cancelled = half.checked_add(half.checked_neg().unwrap()).unwrap();
        assert!(cancelled.is_zero());
    }

    #[test]
    fn overflow_is_reported() {
        let big = Rational::new(i64::MAX, 1).unwrap();
        assert!(big.checked_mul(Rational::new(2, 1).unwrap()).is_none());
        assert!(big.checked_add(Rational::ONE).is_none());
        assert!(Rational::new(i64::MIN, 1).unwrap().checked_neg().is_none());
    }
}
