use std::collections::HashMap;

use thiserror::Error;

use crate::desugar::desugared_ast::{Expr, MulDivOp, UnitExpr};
use crate::dimension::{Dimension, Rational, RationalError};
use crate::syntax::ast::UnitConstness;
use crate::syntax::dimension::UnitRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PositiveFiniteScaleError {
    #[error("scale must be finite")]
    NonFinite,
    #[error("scale must be greater than zero")]
    NonPositive,
}

/// A unit scale factor that is guaranteed to be positive and finite.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct PositiveFiniteScale(f64);

impl PositiveFiniteScale {
    /// Validate a raw scale factor.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is `NaN`, infinite, zero, or negative.
    pub fn new(value: f64) -> Result<Self, PositiveFiniteScaleError> {
        if !value.is_finite() {
            Err(PositiveFiniteScaleError::NonFinite)
        } else if value <= 0.0 {
            Err(PositiveFiniteScaleError::NonPositive)
        } else {
            Ok(Self(value))
        }
    }

    /// Construct a scale from trusted internal constants.
    ///
    /// Callers must ensure `value` is positive and finite. This is restricted
    /// to the compiler crate so external code must use [`Self::new`].
    #[must_use]
    pub(crate) const fn new_unchecked(value: f64) -> Self {
        Self(value)
    }

    /// Return the wrapped raw scale factor.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl std::fmt::Display for PositiveFiniteScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// How a unit's scale factor is determined.
#[derive(Debug, Clone)]
pub enum UnitScale {
    /// Scale factor known at compile time (e.g., `const unit km: Length = 1000 m;`).
    Static(PositiveFiniteScale),
    /// Scale factor depends on runtime values (e.g., `unit EUR: Money = (@rate) USD;`).
    ///
    /// The final SI scale = `eval(scale_expr) * base_unit_scale`.
    Dynamic {
        /// The unevaluated scale expression containing `@`-references.
        scale_expr: Expr,
        /// The scale factor of the base unit in the definition (resolved at compile time).
        /// For `(@rate) USD` where USD has scale 1.0, this is 1.0.
        base_unit_scale: PositiveFiniteScale,
    },
}

impl UnitScale {
    /// Returns the static scale factor, or `None` if the scale is dynamic.
    #[must_use]
    pub(crate) const fn as_static(&self) -> Option<f64> {
        match self {
            Self::Static(s) => Some(s.get()),
            Self::Dynamic { .. } => None,
        }
    }

    /// Returns `true` if the scale is resolved at compile time.
    #[must_use]
    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }

    /// Returns `true` if the scale depends on runtime values.
    #[must_use]
    pub const fn is_dynamic(&self) -> bool {
        matches!(self, Self::Dynamic { .. })
    }
}

/// Information about a registered unit.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    /// The dimension this unit measures.
    pub dimension: Dimension,
    /// Whether this unit may appear in compile-time (`const`) contexts.
    pub constness: UnitConstness,
    /// Scale factor to convert 1 of this unit to base SI units.
    /// e.g., km -> `Static(1000.0)` (1 km = 1000 m)
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
    InvalidScale {
        value: f64,
        reason: PositiveFiniteScaleError,
    },
    /// Dimension exponent arithmetic overflowed.
    Overflow(RationalError),
}

impl From<RationalError> for UnitResolveError {
    fn from(err: RationalError) -> Self {
        Self::Overflow(err)
    }
}

/// Raise a positive unit scale to a rational power.
///
/// Integer powers use `powi` for exactness; fractional powers fall back to
/// `powf`, which is well-defined because unit scales are always positive.
#[must_use]
pub fn pow_scale(scale: f64, exp: Rational) -> f64 {
    if exp.is_integer() {
        scale.powi(exp.num())
    } else {
        scale.powf(f64::from(exp.num()) / f64::from(exp.den()))
    }
}

/// Shared implementation for resolving a `UnitExpr` to its dimension and static scale factor.
pub(crate) fn resolve_unit_expr_impl(
    units: &HashMap<UnitRef, UnitInfo>,
    expr: &UnitExpr,
) -> Result<(Dimension, f64), UnitResolveError> {
    let mut dim = Dimension::dimensionless();
    let mut scale = 1.0_f64;
    for item in &expr.terms {
        let Some(info) = units.get(&item.name.value) else {
            return Err(UnitResolveError::UnknownUnit(item.name.value.clone()));
        };
        let exp = item.power.unwrap_or(Rational::ONE);
        let powered_dim = info.dimension.pow(exp)?;
        let Some(static_scale) = info.scale.as_static() else {
            return Err(UnitResolveError::DynamicScale(item.name.value.clone()));
        };
        let powered_scale = pow_scale(static_scale, exp);
        match item.op {
            MulDivOp::Mul => {
                dim = (dim * powered_dim)?;
                scale *= powered_scale;
            }
            MulDivOp::Div => {
                dim = (dim / powered_dim)?;
                scale /= powered_scale;
            }
        }
        PositiveFiniteScale::new(scale).map_err(|reason| UnitResolveError::InvalidScale {
            value: scale,
            reason,
        })?;
    }
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
        let exp = item.power.unwrap_or(Rational::ONE);
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
}

impl UnitRegistry {
    /// Look up a unit by reference (bare or module-alias-qualified).
    #[must_use]
    pub fn get_unit(&self, name: &UnitRef) -> Option<&UnitInfo> {
        self.units.get(name)
    }

    /// Iterate over all units: (reference, dimension, scale).
    pub fn all_units(&self) -> impl Iterator<Item = (&UnitRef, &Dimension, &UnitScale)> {
        self.units
            .iter()
            .map(|(name, info)| (name, &info.dimension, &info.scale))
    }

    /// Resolve a `UnitExpr` to its dimension and compound static scale factor.
    #[cfg(test)]
    pub(crate) fn resolve_unit_expr(
        &self,
        expr: &UnitExpr,
    ) -> Result<(Dimension, f64), UnitResolveError> {
        resolve_unit_expr_impl(&self.units, expr)
    }

    /// Resolve a `UnitExpr` to its dimension only (ignoring scales).
    ///
    /// Works for both static and dynamic units.
    ///
    /// # Errors
    ///
    /// Returns a [`UnitResolveError`] naming the unknown unit, or the
    /// exponent overflow.
    pub(crate) fn resolve_unit_dimension(
        &self,
        expr: &UnitExpr,
    ) -> Result<Dimension, UnitResolveError> {
        resolve_unit_dimension_impl(&self.units, expr)
    }
}
