//! Sparse monomials: finite products of keys raised to non-zero exponents.
//!
//! [`SparseMonomial<K, E>`] is the single monomial algebra shared by physical
//! dimensions (base dimensions with rational exponents), dimension monomials
//! in function signatures (signature binders with rational exponents), and
//! type-level Nat polynomials (generic parameters with natural exponents).
//!
//! Invariants, enforced by every constructor and operation:
//!
//! - factors are kept sorted by key (so equality, hashing, and ordering are
//!   structural and deterministic);
//! - no factor carries a zero exponent (the empty product is the unit).

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::ops::Neg;

use thiserror::Error;

/// Exponent arithmetic required by [`SparseMonomial`].
pub trait MonomialExponent: Copy + Eq {
    /// Error reported when exponent arithmetic leaves the representable range.
    type Error;

    /// Whether the exponent is zero (such factors are never stored).
    fn is_zero(self) -> bool;

    /// Exponent sum (multiplying monomials).
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] when the sum is not representable.
    fn checked_add(self, rhs: Self) -> Result<Self, Self::Error>;

    /// Exponent product (raising a monomial to a power).
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] when the product is not representable.
    fn checked_mul(self, rhs: Self) -> Result<Self, Self::Error>;
}

impl<T: crate::ratio::RatioInt> MonomialExponent for crate::ratio::Ratio<T> {
    type Error = crate::ratio::RatioError;

    fn is_zero(self) -> bool {
        Self::is_zero(self)
    }

    fn checked_add(self, rhs: Self) -> Result<Self, Self::Error> {
        self + rhs
    }

    fn checked_mul(self, rhs: Self) -> Result<Self, Self::Error> {
        self * rhs
    }
}

/// A product of distinct keys raised to non-zero exponents.
///
/// The empty monomial is the multiplicative unit (the dimensionless
/// dimension, the constant Nat monomial, …).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SparseMonomial<K, E> {
    factors: BTreeMap<K, E>,
}

/// A factor list could not form a [`SparseMonomial`] as written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MonomialFactorError<K> {
    /// A factor carried a zero exponent.
    #[error("a factor has a zero exponent")]
    ZeroExponent(K),
    /// The same key appeared in more than one factor.
    #[error("a key appears in more than one factor")]
    DuplicateKey(K),
}

impl<K> MonomialFactorError<K> {
    /// Translate the offending key (for example, from a resolved binder back
    /// to its written name).
    #[must_use]
    pub fn map_key<L>(self, f: impl FnOnce(K) -> L) -> MonomialFactorError<L> {
        match self {
            Self::ZeroExponent(key) => MonomialFactorError::ZeroExponent(f(key)),
            Self::DuplicateKey(key) => MonomialFactorError::DuplicateKey(f(key)),
        }
    }
}

impl<K, E> Default for SparseMonomial<K, E> {
    fn default() -> Self {
        Self::one()
    }
}

impl<K, E> SparseMonomial<K, E> {
    /// The unit monomial (empty product).
    #[must_use]
    pub const fn one() -> Self {
        Self {
            factors: BTreeMap::new(),
        }
    }

    /// Number of factors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.factors.len()
    }

    /// Whether this is the unit monomial (no factors).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.factors.is_empty()
    }

    /// Factors in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &E)> {
        self.factors.iter()
    }

    /// Keys in order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.factors.keys()
    }

    /// The only factor, when there is exactly one.
    #[must_use]
    pub fn as_single(&self) -> Option<(&K, &E)> {
        let mut factors = self.factors.iter();
        match (factors.next(), factors.next()) {
            (Some(factor), None) => Some(factor),
            _ => None,
        }
    }
}

impl<K: Ord, E: MonomialExponent> SparseMonomial<K, E> {
    /// A single factor `key^exponent` (the unit when `exponent` is zero).
    #[must_use]
    pub fn single(key: K, exponent: E) -> Self {
        let mut factors = BTreeMap::new();
        if !exponent.is_zero() {
            factors.insert(key, exponent);
        }
        Self { factors }
    }

    /// Build from factors written out explicitly, as a signature or manifest
    /// spells them: every key at most once, no zero exponent.
    ///
    /// # Errors
    ///
    /// Returns [`MonomialFactorError`] naming the first offending key.
    pub fn try_from_factors(
        factors: impl IntoIterator<Item = (K, E)>,
    ) -> Result<Self, MonomialFactorError<K>> {
        let mut map = BTreeMap::new();
        for (key, exponent) in factors {
            if exponent.is_zero() {
                return Err(MonomialFactorError::ZeroExponent(key));
            }
            match map.entry(key) {
                Entry::Vacant(slot) => {
                    slot.insert(exponent);
                }
                Entry::Occupied(slot) => {
                    return Err(MonomialFactorError::DuplicateKey(slot.remove_entry().0));
                }
            }
        }
        Ok(Self { factors: map })
    }

    /// The exponent of `key`, or `None` when the key does not occur.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<E> {
        self.factors.get(key).copied()
    }

    /// Product of two monomials (exponents add; cancelled factors vanish).
    ///
    /// # Errors
    ///
    /// Returns the exponent error when a sum is not representable.
    pub fn try_mul(&self, other: &Self) -> Result<Self, E::Error>
    where
        K: Clone,
    {
        let mut factors = self.factors.clone();
        for (key, &exponent) in &other.factors {
            match factors.entry(key.clone()) {
                Entry::Vacant(slot) => {
                    slot.insert(exponent);
                }
                Entry::Occupied(mut slot) => {
                    let sum = slot.get().checked_add(exponent)?;
                    if sum.is_zero() {
                        slot.remove();
                    } else {
                        slot.insert(sum);
                    }
                }
            }
        }
        Ok(Self { factors })
    }

    /// Raise every factor to `exponent` (the unit for a zero exponent).
    ///
    /// # Errors
    ///
    /// Returns the exponent error when a product is not representable.
    pub fn try_pow(&self, exponent: E) -> Result<Self, E::Error>
    where
        K: Clone,
    {
        if exponent.is_zero() {
            return Ok(Self::one());
        }
        let mut factors = BTreeMap::new();
        for (key, &power) in &self.factors {
            let product = power.checked_mul(exponent)?;
            if !product.is_zero() {
                factors.insert(key.clone(), product);
            }
        }
        Ok(Self { factors })
    }
}

impl<K: Ord + Clone, E: MonomialExponent + Neg<Output = E>> SparseMonomial<K, E> {
    /// The multiplicative inverse (every exponent negated).
    #[must_use]
    pub fn inverse(&self) -> Self {
        Self {
            factors: self
                .factors
                .iter()
                .map(|(key, &exponent)| (key.clone(), -exponent))
                .collect(),
        }
    }

    /// Quotient of two monomials (exponents subtract).
    ///
    /// # Errors
    ///
    /// Returns the exponent error when a difference is not representable.
    pub fn try_div(&self, other: &Self) -> Result<Self, E::Error> {
        self.try_mul(&other.inverse())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal signed exponent for exercising the algebra.
    impl MonomialExponent for i8 {
        type Error = ();

        fn is_zero(self) -> bool {
            self == 0
        }

        fn checked_add(self, rhs: Self) -> Result<Self, Self::Error> {
            Self::checked_add(self, rhs).ok_or(())
        }

        fn checked_mul(self, rhs: Self) -> Result<Self, Self::Error> {
            Self::checked_mul(self, rhs).ok_or(())
        }
    }

    fn monomial(factors: &[(char, i8)]) -> SparseMonomial<char, i8> {
        SparseMonomial::try_from_factors(factors.iter().copied()).unwrap()
    }

    #[test]
    fn factors_are_sorted_and_zero_free() {
        let value = monomial(&[('b', 2), ('a', -1)]);
        assert_eq!(
            value.iter().collect::<Vec<_>>(),
            vec![(&'a', &-1), (&'b', &2)]
        );
        assert!(SparseMonomial::<char, i8>::single('a', 0).is_empty());
        assert_eq!(
            SparseMonomial::try_from_factors([('a', 0_i8)]),
            Err(MonomialFactorError::ZeroExponent('a'))
        );
        assert_eq!(
            SparseMonomial::try_from_factors([('a', 1_i8), ('a', 2)]),
            Err(MonomialFactorError::DuplicateKey('a'))
        );
    }

    #[test]
    fn multiplication_cancels_and_division_inverts() {
        let a = monomial(&[('a', 1), ('b', 2)]);
        let b = monomial(&[('a', -1), ('c', 1)]);
        assert_eq!(a.try_mul(&b), Ok(monomial(&[('b', 2), ('c', 1)])));
        assert_eq!(a.try_div(&a), Ok(SparseMonomial::one()));
        assert_eq!(a.try_mul(&b).unwrap().try_div(&b), Ok(a));
        assert_eq!(
            monomial(&[('a', 127)]).try_mul(&monomial(&[('a', 1)])),
            Err(())
        );
    }

    #[test]
    fn powers_scale_every_exponent() {
        let a = monomial(&[('a', 1), ('b', -2)]);
        assert_eq!(a.try_pow(3), Ok(monomial(&[('a', 3), ('b', -6)])));
        assert_eq!(a.try_pow(0), Ok(SparseMonomial::one()));
        assert_eq!(a.try_pow(100), Err(()));
    }

    #[test]
    fn single_factor_projection() {
        assert_eq!(monomial(&[('a', 2)]).as_single(), Some((&'a', &2)));
        assert_eq!(monomial(&[('a', 1), ('b', 1)]).as_single(), None);
        assert_eq!(SparseMonomial::<char, i8>::one().as_single(), None);
    }
}
