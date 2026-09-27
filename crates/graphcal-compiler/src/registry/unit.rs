use std::collections::HashMap;

use thiserror::Error;

use crate::desugar::desugared_ast::{MulDivOp, UnitExpr};
use crate::dimension::{Dimension, Rational};
use crate::ratio::RatioError;
use crate::syntax::ast::UnitConstness;
use crate::syntax::dimension::UnitRef;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum BaseUnitRegistrationError {
    #[error("the declared dimension is not a base dimension")]
    NotBaseDimension,
    #[error("the dimension already has canonical base unit `{existing}`")]
    AlreadyRegistered { existing: String },
}

/// A raw value that is not a valid [`PositiveFiniteScale`].
///
/// The rejected value is kept so diagnostics can report it; the `Display`
/// output is a predicate phrase meant to follow a context noun
/// (e.g. `"unit scale must be finite, got inf"`).
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum PositiveFiniteScaleError {
    #[error("must be finite, got {value}")]
    NonFinite { value: f64 },
    #[error("must be greater than zero, got {value}")]
    NonPositive { value: f64 },
}

/// A unit scale factor that is guaranteed to be positive and finite.
///
/// Arithmetic stays inside the type through the `checked_*` operations, so a
/// proof of positivity is never dropped to a raw `f64` and re-validated.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct PositiveFiniteScale(f64);

impl PositiveFiniteScale {
    /// The identity scale (`1.0`).
    pub const ONE: Self = Self(1.0);

    /// Validate a raw scale factor.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is `NaN`, infinite, zero, or negative.
    pub const fn new(value: f64) -> Result<Self, PositiveFiniteScaleError> {
        if !value.is_finite() {
            Err(PositiveFiniteScaleError::NonFinite { value })
        } else if value <= 0.0 {
            Err(PositiveFiniteScaleError::NonPositive { value })
        } else {
            Ok(Self(value))
        }
    }

    /// Construct a scale from a compile-time constant.
    ///
    /// Intended for `const { PositiveFiniteScale::from_const(..) }` so an
    /// invalid built-in constant is rejected when the crate is compiled.
    ///
    /// # Panics
    ///
    /// Panics (at compile time in a const context) when `value` is not
    /// positive and finite.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "evaluated in const blocks, so an invalid built-in scale fails the build"
    )]
    pub const fn from_const(value: f64) -> Self {
        match Self::new(value) {
            Ok(scale) => scale,
            Err(_) => panic!("constant unit scale must be positive and finite"),
        }
    }

    /// Return the wrapped raw scale factor.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Multiply two scales.
    ///
    /// # Errors
    ///
    /// Returns an error when the product overflows to infinity or underflows
    /// to zero.
    pub const fn checked_mul(self, rhs: Self) -> Result<Self, PositiveFiniteScaleError> {
        Self::new(self.0 * rhs.0)
    }

    /// Divide two scales.
    ///
    /// # Errors
    ///
    /// Returns an error when the quotient overflows to infinity or underflows
    /// to zero.
    pub const fn checked_div(self, rhs: Self) -> Result<Self, PositiveFiniteScaleError> {
        Self::new(self.0 / rhs.0)
    }

    /// Raise the scale to a rational power.
    ///
    /// Integer powers use `powi` for exactness; fractional powers use `powf`,
    /// which is well-defined because the base is positive.
    ///
    /// # Errors
    ///
    /// Returns an error when the power overflows to infinity or underflows to
    /// zero.
    pub fn checked_pow(self, exp: Rational) -> Result<Self, PositiveFiniteScaleError> {
        let powered = if exp.is_integer() {
            self.0.powi(exp.num())
        } else {
            self.0.powf(f64::from(exp.num()) / f64::from(exp.den()))
        };
        Self::new(powered)
    }
}

impl std::fmt::Display for PositiveFiniteScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One `op unit^power` factor of a compound unit scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitScaleTerm {
    pub op: MulDivOp,
    pub scale: PositiveFiniteScale,
    pub power: Rational,
}

/// Which step of a compound unit-scale fold left the positive finite range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnitScaleStepError {
    /// Raising one term's scale to its power failed.
    Power(PositiveFiniteScaleError),
    /// Combining the powered term into the running compound scale failed.
    Compound(PositiveFiniteScaleError),
}

impl UnitScaleTerm {
    /// Apply this term to a running compound scale: `acc * scale^power` or
    /// `acc / scale^power`.
    ///
    /// # Errors
    ///
    /// Reports whether the power or the compound product left the positive
    /// finite range.
    pub fn apply_to(
        self,
        acc: PositiveFiniteScale,
    ) -> Result<PositiveFiniteScale, UnitScaleStepError> {
        let powered = self
            .scale
            .checked_pow(self.power)
            .map_err(UnitScaleStepError::Power)?;
        match self.op {
            MulDivOp::Mul => acc.checked_mul(powered),
            MulDivOp::Div => acc.checked_div(powered),
        }
        .map_err(UnitScaleStepError::Compound)
    }
}

/// Fold the terms of a unit expression into one compound scale.
///
/// This is the single unit-scale fold shared by compile-time resolution and
/// runtime (dynamic-scale) evaluation. `resolve` produces each term's scale
/// (and may fail, e.g. for unknown or dynamic units); `step_error` converts
/// an arithmetic failure of the fold at that term into the caller's error.
///
/// # Errors
///
/// Returns the first error from `resolve` or `step_error`.
pub fn try_fold_unit_scale<T, E>(
    terms: impl IntoIterator<Item = T>,
    mut resolve: impl FnMut(&T) -> Result<UnitScaleTerm, E>,
    mut step_error: impl FnMut(&T, UnitScaleStepError) -> E,
) -> Result<PositiveFiniteScale, E> {
    terms
        .into_iter()
        .try_fold(PositiveFiniteScale::ONE, |acc, item| {
            resolve(&item)?
                .apply_to(acc)
                .map_err(|error| step_error(&item, error))
        })
}

/// How a unit's scale factor is determined, together with whether the unit
/// may appear in compile-time (`const`) contexts.
///
/// Constness is folded into this enum so a `const` unit with a
/// runtime-dependent scale is unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitScale {
    /// A compile-time unit with a static scale: prelude units, `base unit`,
    /// and `const unit km: Length = 1000 m;`.
    Const(PositiveFiniteScale),
    /// A runtime unit (plain `unit`) whose scale happens to be static, e.g.
    /// `unit mile: Length = 1609.344 m;`. It is not usable in `const` bodies.
    Runtime(PositiveFiniteScale),
    /// A runtime unit whose scale depends on a strictly typed HIR expression
    /// retained by IR/TIR (e.g., `unit EUR: Money = (@rate) USD;`).
    Dynamic {
        /// The scale factor of the base unit in the definition (resolved at compile time).
        /// For `(@rate) USD` where USD has scale 1.0, this is 1.0.
        base_unit_scale: PositiveFiniteScale,
    },
}

impl UnitScale {
    /// Returns the compile-time scale factor, or `None` if the scale is dynamic.
    #[must_use]
    pub const fn static_scale(&self) -> Option<PositiveFiniteScale> {
        match self {
            Self::Const(scale) | Self::Runtime(scale) => Some(*scale),
            Self::Dynamic { .. } => None,
        }
    }

    /// Whether the unit may appear in compile-time (`const`) contexts.
    #[must_use]
    pub const fn constness(&self) -> UnitConstness {
        match self {
            Self::Const(_) => UnitConstness::Const,
            Self::Runtime(_) | Self::Dynamic { .. } => UnitConstness::Dynamic,
        }
    }
}

/// Information about a registered unit.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitInfo {
    /// The dimension this unit measures.
    pub dimension: Dimension,
    /// Scale factor to convert 1 of this unit to base SI units, together
    /// with whether the unit is compile-time.
    /// e.g., km -> `Const(1000.0)` (1 km = 1000 m)
    pub scale: UnitScale,
}

/// Why a unit expression could not be resolved.
///
/// Carries the failing unit name so callers can produce a precise
/// diagnostic instead of re-scanning the expression to find it (the old
/// `Ok(None)` return conflated unknown names with dynamic scales).
#[derive(Debug, Clone, PartialEq)]
pub enum UnitResolveError {
    /// A unit name in the expression is not registered.
    UnknownUnit(UnitRef),
    /// A unit in the expression has a runtime-dependent scale.
    DynamicScale(UnitRef),
    /// The compound scale was zero, negative, NaN, or infinite.
    InvalidScale(PositiveFiniteScaleError),
    /// Dimension exponent arithmetic overflowed.
    Overflow(RatioError),
}

impl From<RatioError> for UnitResolveError {
    fn from(err: RatioError) -> Self {
        Self::Overflow(err)
    }
}

/// Shared implementation for resolving a `UnitExpr` to its dimension and static scale factor.
pub(crate) fn resolve_unit_expr_impl(
    units: &HashMap<UnitRef, UnitInfo>,
    expr: &UnitExpr,
) -> Result<(Dimension, PositiveFiniteScale), UnitResolveError> {
    let mut dim = Dimension::dimensionless();
    let scale = try_fold_unit_scale(
        &expr.terms,
        |item| {
            let Some(info) = units.get(&item.name.value) else {
                return Err(UnitResolveError::UnknownUnit(item.name.value.clone()));
            };
            let power = item.effective_power();
            let powered_dim = info.dimension.pow(power)?;
            let Some(scale) = info.scale.static_scale() else {
                return Err(UnitResolveError::DynamicScale(item.name.value.clone()));
            };
            dim = match item.op {
                MulDivOp::Mul => (&dim * &powered_dim)?,
                MulDivOp::Div => (&dim / &powered_dim)?,
            };
            Ok(UnitScaleTerm {
                op: item.op,
                scale,
                power,
            })
        },
        |_, (UnitScaleStepError::Power(error) | UnitScaleStepError::Compound(error))| {
            UnitResolveError::InvalidScale(error)
        },
    )?;
    Ok((dim, scale))
}

/// Shared implementation for resolving a `UnitExpr` to its dimension only (ignoring scales).
///
/// Works for both static and dynamic units.
pub(crate) fn resolve_unit_dimension_impl(
    units: &HashMap<UnitRef, UnitInfo>,
    expr: &UnitExpr,
) -> Result<Dimension, UnitResolveError> {
    let mut dim = Dimension::dimensionless();
    for item in &expr.terms {
        let Some(info) = units.get(&item.name.value) else {
            return Err(UnitResolveError::UnknownUnit(item.name.value.clone()));
        };
        let exp = item.effective_power();
        let powered_dim = info.dimension.pow(exp)?;
        dim = match item.op {
            MulDivOp::Mul => (dim * powered_dim)?,
            MulDivOp::Div => (dim / powered_dim)?,
        };
    }
    Ok(dim)
}

/// Unit registry: maps unit names to `UnitInfo` (dimension + scale).
#[derive(Debug, Clone)]
pub struct UnitRegistry {
    pub(crate) units: HashMap<UnitRef, UnitInfo>,
    pub(crate) aliases: HashMap<UnitRef, UnitRef>,
}

impl UnitRegistry {
    /// Look up a unit by reference (bare or module-alias-qualified).
    #[must_use]
    pub fn get_unit(&self, name: &UnitRef) -> Option<&UnitInfo> {
        let mut current = name.clone();
        let mut remaining = self.aliases.len() + 1;
        loop {
            if let Some(info) = self.units.get(&current) {
                return Some(info);
            }
            current = self.aliases.get(&current)?.clone();
            remaining = remaining.checked_sub(1)?;
        }
    }

    /// Iterate over every unit reference and its complete semantic definition.
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitRef, &UnitInfo)> {
        self.units.iter()
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    #[cfg(test)]
    pub(crate) fn resolve_unit_expr(
        &self,
        expr: &UnitExpr,
    ) -> Result<(Dimension, PositiveFiniteScale), UnitResolveError> {
        resolve_unit_expr_impl(&self.units, expr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scale(value: f64) -> PositiveFiniteScale {
        PositiveFiniteScale::new(value).unwrap()
    }

    fn ratio(num: i32, den: i32) -> Rational {
        Rational::try_new(num, den).unwrap()
    }

    fn unit_term(op: MulDivOp, value: f64) -> UnitScaleTerm {
        UnitScaleTerm {
            op,
            scale: scale(value),
            power: Rational::from(1),
        }
    }

    #[test]
    fn new_rejects_non_finite_and_non_positive_values() {
        assert!(matches!(
            PositiveFiniteScale::new(f64::NAN),
            Err(PositiveFiniteScaleError::NonFinite { value }) if value.is_nan()
        ));
        assert_eq!(
            PositiveFiniteScale::new(f64::INFINITY),
            Err(PositiveFiniteScaleError::NonFinite {
                value: f64::INFINITY
            })
        );
        assert_eq!(
            PositiveFiniteScale::new(f64::NEG_INFINITY),
            Err(PositiveFiniteScaleError::NonFinite {
                value: f64::NEG_INFINITY
            })
        );
        assert_eq!(
            PositiveFiniteScale::new(0.0),
            Err(PositiveFiniteScaleError::NonPositive { value: 0.0 })
        );
        assert_eq!(
            PositiveFiniteScale::new(-2.0),
            Err(PositiveFiniteScaleError::NonPositive { value: -2.0 })
        );
        assert_eq!(
            scale(f64::MIN_POSITIVE).get().to_bits(),
            f64::MIN_POSITIVE.to_bits()
        );
        assert_eq!(PositiveFiniteScale::ONE.get().to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn error_display_is_a_predicate_for_a_context_noun() {
        assert_eq!(
            PositiveFiniteScaleError::NonFinite {
                value: f64::INFINITY
            }
            .to_string(),
            "must be finite, got inf"
        );
        assert_eq!(
            PositiveFiniteScaleError::NonPositive { value: 0.0 }.to_string(),
            "must be greater than zero, got 0"
        );
    }

    #[test]
    fn from_const_accepts_valid_constants() {
        const KILO: PositiveFiniteScale = PositiveFiniteScale::from_const(1000.0);
        assert_eq!(KILO.get().to_bits(), 1000.0_f64.to_bits());
    }

    #[test]
    #[should_panic(expected = "constant unit scale must be positive and finite")]
    fn from_const_rejects_invalid_constants() {
        let _ = PositiveFiniteScale::from_const(0.0);
    }

    #[test]
    fn checked_mul_and_div_stay_in_range() {
        assert_eq!(scale(1000.0).checked_mul(scale(0.001)), Ok(scale(1.0)));
        assert_eq!(
            scale(1000.0).checked_div(scale(3600.0)),
            Ok(scale(1000.0 / 3600.0))
        );
        assert_eq!(
            scale(1e300).checked_mul(scale(1e300)),
            Err(PositiveFiniteScaleError::NonFinite {
                value: f64::INFINITY
            })
        );
        assert_eq!(
            scale(1e-300).checked_mul(scale(1e-300)),
            Err(PositiveFiniteScaleError::NonPositive { value: 0.0 })
        );
        assert_eq!(
            scale(1e300).checked_div(scale(1e-300)),
            Err(PositiveFiniteScaleError::NonFinite {
                value: f64::INFINITY
            })
        );
        assert_eq!(
            scale(1e-300).checked_div(scale(1e300)),
            Err(PositiveFiniteScaleError::NonPositive { value: 0.0 })
        );
    }

    #[test]
    fn checked_pow_handles_integer_fractional_and_out_of_range_powers() {
        assert_eq!(
            scale(10.0).checked_pow(Rational::from(3)),
            Ok(scale(1000.0))
        );
        assert_eq!(scale(10.0).checked_pow(Rational::from(-2)), Ok(scale(0.01)));
        assert_eq!(scale(4.0).checked_pow(ratio(1, 2)), Ok(scale(2.0)));
        assert_eq!(scale(8.0).checked_pow(ratio(-2, 3)), Ok(scale(0.25)));
        assert_eq!(scale(7.0).checked_pow(Rational::from(0)), Ok(scale(1.0)));
        assert_eq!(
            scale(1000.0).checked_pow(Rational::from(400)),
            Err(PositiveFiniteScaleError::NonFinite {
                value: f64::INFINITY
            })
        );
        assert_eq!(
            scale(1000.0).checked_pow(Rational::from(-400)),
            Err(PositiveFiniteScaleError::NonPositive { value: 0.0 })
        );
    }

    #[test]
    fn term_application_multiplies_or_divides_the_powered_scale() {
        let term = |op| UnitScaleTerm {
            op,
            scale: scale(10.0),
            power: Rational::from(2),
        };
        assert_eq!(term(MulDivOp::Mul).apply_to(scale(3.0)), Ok(scale(300.0)));
        assert_eq!(term(MulDivOp::Div).apply_to(scale(300.0)), Ok(scale(3.0)));
    }

    #[test]
    fn term_application_distinguishes_power_and_compound_failures() {
        let power_overflow = UnitScaleTerm {
            op: MulDivOp::Mul,
            scale: scale(1000.0),
            power: Rational::from(400),
        };
        assert_eq!(
            power_overflow.apply_to(PositiveFiniteScale::ONE),
            Err(UnitScaleStepError::Power(
                PositiveFiniteScaleError::NonFinite {
                    value: f64::INFINITY
                }
            ))
        );
        assert_eq!(
            unit_term(MulDivOp::Div, 1e-300).apply_to(scale(1e300)),
            Err(UnitScaleStepError::Compound(
                PositiveFiniteScaleError::NonFinite {
                    value: f64::INFINITY
                }
            ))
        );
    }

    #[test]
    fn fold_starts_at_one_and_applies_terms_in_order() {
        let empty: [(MulDivOp, f64); 0] = [];
        assert_eq!(
            try_fold_unit_scale(
                empty,
                |&(op, value)| Ok::<_, ()>(unit_term(op, value)),
                |_, _| ()
            ),
            Ok(PositiveFiniteScale::ONE)
        );
        // km / h = 1000 / 3600
        let terms = [(MulDivOp::Mul, 1000.0), (MulDivOp::Div, 3600.0)];
        assert_eq!(
            try_fold_unit_scale(
                terms,
                |&(op, value)| Ok::<_, ()>(unit_term(op, value)),
                |_, _| ()
            ),
            Ok(scale(1000.0 / 3600.0))
        );
    }

    #[test]
    fn fold_reports_resolve_errors_and_step_errors_with_the_failing_term() {
        let resolve = |&(name, value): &(&str, f64)| {
            if name == "c" {
                Err(format!("resolve {name}"))
            } else {
                Ok(unit_term(MulDivOp::Mul, value))
            }
        };
        let result = try_fold_unit_scale(
            [("a", 1e200), ("b", 1e200), ("c", 1.0)],
            resolve,
            |&(name, _), error| format!("step {name}: {error:?}"),
        );
        assert_eq!(
            result,
            Err("step b: Compound(NonFinite { value: inf })".to_string())
        );
        let result =
            try_fold_unit_scale([("a", 2.0), ("c", 1.0)], resolve, |_, _| "step".to_string());
        assert_eq!(result, Err("resolve c".to_string()));
    }

    #[test]
    fn unit_scale_carries_constness_and_static_scale() {
        let kilo = scale(1000.0);
        assert_eq!(UnitScale::Const(kilo).static_scale(), Some(kilo));
        assert_eq!(UnitScale::Const(kilo).constness(), UnitConstness::Const);
        assert_eq!(UnitScale::Runtime(kilo).static_scale(), Some(kilo));
        assert_eq!(UnitScale::Runtime(kilo).constness(), UnitConstness::Dynamic);
        let dynamic = UnitScale::Dynamic {
            base_unit_scale: kilo,
        };
        assert_eq!(dynamic.static_scale(), None);
        assert_eq!(dynamic.constness(), UnitConstness::Dynamic);
    }
}
