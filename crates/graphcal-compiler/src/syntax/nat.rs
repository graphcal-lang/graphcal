//! Typed representation of type-level Nat arithmetic.
//!
//! Nat forms are used by both type resolution and declared type references, so
//! they live in the syntax layer rather than in TIR. Rendering a form to a
//! string is a display operation only; semantic comparisons use the normalized
//! polynomial structure directly.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::syntax::type_name::GenericParamName;

/// Arithmetic overflow while combining type-level Nat forms.
///
/// Coefficients and exponents are stored as `u64`; combining forms whose
/// values exceed that range must fail loudly instead of wrapping, since a
/// wrapped form could spuriously unify with an unrelated type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NatOverflowError;

impl std::fmt::Display for NatOverflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "type-level Nat arithmetic overflow (values are stored as `u64`)"
        )
    }
}

impl std::error::Error for NatOverflowError {}

/// A monomial: product of variables raised to natural number exponents.
///
/// Represented as a sorted map from variable name to exponent. The empty map
/// represents the constant monomial (= 1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Monomial(pub(crate) BTreeMap<GenericParamName, u64>);

impl Monomial {
    /// The constant monomial (empty product = 1).
    #[must_use]
    pub(crate) const fn constant() -> Self {
        Self(BTreeMap::new())
    }

    /// A single-variable monomial with exponent 1.
    #[must_use]
    pub(crate) fn var(name: GenericParamName) -> Self {
        let mut m = BTreeMap::new();
        m.insert(name, 1);
        Self(m)
    }

    /// Returns `true` if this is the constant monomial (no variables).
    #[must_use]
    pub(crate) fn is_constant(&self) -> bool {
        self.0.is_empty()
    }

    /// Multiply two monomials: add exponents of each variable.
    ///
    /// Returns an error if an exponent overflows.
    pub(crate) fn mul(&self, other: &Self) -> Result<Self, NatOverflowError> {
        let mut result = self.0.clone();
        for (var, exp) in &other.0 {
            let entry = result.entry(var.clone()).or_insert(0);
            *entry = entry.checked_add(*exp).ok_or(NatOverflowError)?;
        }
        Ok(Self(result))
    }

    /// Evaluate the monomial given variable bindings.
    ///
    /// Returns `None` if any variable is unbound or arithmetic overflows.
    #[must_use]
    pub(crate) fn evaluate(&self, bindings: &HashMap<GenericParamName, u64>) -> Option<u64> {
        let mut result: u64 = 1;
        for (var, exp) in &self.0 {
            let val = bindings.get(var)?;
            result = result.checked_mul(val.checked_pow(u32::try_from(*exp).ok()?)?)?;
        }
        Some(result)
    }

    /// Substitute bound variables, returning a new monomial with only unbound
    /// variables and the multiplicative factor contributed by bound variables.
    ///
    /// Returns `None` if arithmetic overflows.
    #[must_use]
    pub(crate) fn substitute(
        &self,
        bindings: &HashMap<GenericParamName, u64>,
    ) -> Option<(Self, u64)> {
        let mut remaining = BTreeMap::new();
        let mut factor: u64 = 1;
        for (var, exp) in &self.0 {
            if let Some(val) = bindings.get(var) {
                factor = factor.checked_mul(val.checked_pow(u32::try_from(*exp).ok()?)?)?;
            } else {
                remaining.insert(var.clone(), *exp);
            }
        }
        Some((Self(remaining), factor))
    }

    /// Format as a human-readable string, e.g. `""`, `"N"`, `"M * N"`, `"N^2"`.
    #[must_use]
    pub(crate) fn format(&self) -> String {
        let mut parts = Vec::new();
        for (var, exp) in &self.0 {
            if *exp == 1 {
                parts.push(var.to_string());
            } else {
                parts.push(format!("{var}^{exp}"));
            }
        }
        parts.join(" * ")
    }
}

impl PartialOrd for Monomial {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Monomial {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare by iterating entries in sorted order (BTreeMap guarantees this).
        let a: Vec<_> = self.0.iter().collect();
        let b: Vec<_> = other.0.iter().collect();
        a.cmp(&b)
    }
}

/// A normalized polynomial form for Nat expressions.
///
/// This is the canonical representation for Nat arithmetic (Level 1 addition +
/// Level 2 multiplication). Each term is a monomial mapped to its coefficient.
/// Two `NatPolyForm`s are equal iff their normalized terms match.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NatPolyForm {
    /// Monomial → coefficient mapping (only non-zero coefficients).
    pub(crate) terms: BTreeMap<Monomial, u64>,
}

/// Backward-compatible alias for code that still speaks in linear Nat forms.
pub type NatLinearForm = NatPolyForm;

impl NatPolyForm {
    /// Create a polynomial from a constant.
    #[must_use]
    pub fn from_constant(c: u64) -> Self {
        let mut terms = BTreeMap::new();
        if c != 0 {
            terms.insert(Monomial::constant(), c);
        }
        Self { terms }
    }

    /// Create a polynomial from a single variable with coefficient 1.
    #[must_use]
    pub fn from_var(name: GenericParamName) -> Self {
        let mut terms = BTreeMap::new();
        terms.insert(Monomial::var(name), 1);
        Self { terms }
    }

    /// Add two polynomials.
    ///
    /// Returns an error if a coefficient overflows.
    pub fn add(&self, other: &Self) -> Result<Self, NatOverflowError> {
        let mut terms = self.terms.clone();
        for (mono, coeff) in &other.terms {
            let entry = terms.entry(mono.clone()).or_insert(0);
            *entry = entry.checked_add(*coeff).ok_or(NatOverflowError)?;
        }
        terms.retain(|_, c| *c != 0);
        Ok(Self { terms })
    }

    /// Multiply two polynomials (distributive law).
    ///
    /// Returns an error if a coefficient or exponent overflows.
    pub fn mul(&self, other: &Self) -> Result<Self, NatOverflowError> {
        let mut terms: BTreeMap<Monomial, u64> = BTreeMap::new();
        for (m1, c1) in &self.terms {
            for (m2, c2) in &other.terms {
                let mono = m1.mul(m2)?;
                let term = c1.checked_mul(*c2).ok_or(NatOverflowError)?;
                let entry = terms.entry(mono).or_insert(0);
                *entry = entry.checked_add(term).ok_or(NatOverflowError)?;
            }
        }
        terms.retain(|_, c| *c != 0);
        Ok(Self { terms })
    }

    /// Returns the constant term (coefficient of the empty monomial).
    #[must_use]
    pub fn constant(&self) -> u64 {
        self.terms.get(&Monomial::constant()).copied().unwrap_or(0)
    }

    /// Returns `true` if this form has no variables (is a constant).
    #[must_use]
    pub fn is_constant(&self) -> bool {
        self.terms.iter().all(|(m, _)| m.is_constant())
    }

    /// Evaluate to a concrete value given variable bindings.
    ///
    /// Returns `None` if any variable is unbound or arithmetic overflows.
    #[must_use]
    pub fn evaluate(&self, bindings: &HashMap<GenericParamName, u64>) -> Option<u64> {
        let mut result: u64 = 0;
        for (mono, coeff) in &self.terms {
            result = result.checked_add(coeff.checked_mul(mono.evaluate(bindings)?)?)?;
        }
        Some(result)
    }

    /// Format as a human-readable string.
    ///
    /// Examples: `"3"`, `"N"`, `"N + 1"`, `"M * N"`, `"2 * N^2 + N + 1"`.
    #[must_use]
    pub fn format(&self) -> String {
        if self.terms.is_empty() {
            return "0".to_string();
        }
        let mut parts = Vec::new();
        // Non-constant terms first (sorted by monomial), then constant.
        for (mono, coeff) in &self.terms {
            if mono.is_constant() {
                continue;
            }
            let mono_str = mono.format();
            if *coeff == 1 {
                parts.push(mono_str);
            } else {
                parts.push(format!("{coeff} * {mono_str}"));
            }
        }
        if let Some(&c) = self.terms.get(&Monomial::constant())
            && (c > 0 || parts.is_empty())
        {
            parts.push(c.to_string());
        }
        if parts.is_empty() {
            "0".to_string()
        } else {
            parts.join(" + ")
        }
    }

    /// Check if `self <= other` for all non-negative variable assignments.
    ///
    /// Returns `true` iff for every monomial, the coefficient in `self` is <=
    /// the coefficient in `other`. This is sound because all `Nat` variables
    /// are non-negative, so each monomial evaluates to a non-negative value.
    #[must_use]
    pub fn is_leq(&self, other: &Self) -> bool {
        self.terms.iter().all(|(mono, &coeff)| {
            let other_coeff = other.terms.get(mono).copied().unwrap_or(0);
            coeff <= other_coeff
        })
    }

    /// Collect all variable names that appear in any monomial of this polynomial.
    #[must_use]
    pub fn variables(&self) -> BTreeSet<GenericParamName> {
        self.terms
            .keys()
            .flat_map(|mono| mono.0.keys().cloned())
            .collect()
    }
}

impl std::fmt::Display for NatPolyForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format())
    }
}
