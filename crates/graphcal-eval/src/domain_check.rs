//! Typed domain constraints and pure runtime-value checks.
//!
//! Used both at compile time (validating const values in `exec_plan`) and at
//! evaluation time (validating param/node values in `eval::runtime`).

use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::time_scale::TimeScale;

/// One evaluated inclusive domain bound plus its user-facing diagnostic text.
#[derive(Debug, Clone)]
pub struct ResolvedDomainBound<T> {
    value: T,
    display: String,
}

impl<T> ResolvedDomainBound<T> {
    pub const fn new(value: T, display: String) -> Self {
        Self { value, display }
    }

    pub const fn value(&self) -> &T {
        &self.value
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    fn try_map<U, E>(
        self,
        map: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<ResolvedDomainBound<U>, E> {
        Ok(ResolvedDomainBound {
            value: map(self.value)?,
            display: self.display,
        })
    }
}

/// Evaluated inclusive lower and upper bounds for one constraint family.
#[derive(Debug, Clone)]
pub struct ResolvedDomainBounds<T> {
    min: Option<ResolvedDomainBound<T>>,
    max: Option<ResolvedDomainBound<T>>,
}

impl<T> ResolvedDomainBounds<T> {
    pub const fn new(
        min: Option<ResolvedDomainBound<T>>,
        max: Option<ResolvedDomainBound<T>>,
    ) -> Self {
        Self { min, max }
    }

    pub const fn min(&self) -> Option<&ResolvedDomainBound<T>> {
        self.min.as_ref()
    }

    pub const fn max(&self) -> Option<&ResolvedDomainBound<T>> {
        self.max.as_ref()
    }
}

/// A canonical physical instant admitted only from an epoch in the target scale.
///
/// Storing TAI duration makes ordering independent of hifitime's display scale,
/// while the private constructor prevents a cross-scale bound from entering a
/// datetime constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DomainInstant(hifitime::Duration);

impl DomainInstant {
    pub(crate) const fn duration(self) -> hifitime::Duration {
        self.0
    }

    fn from_epoch(
        epoch: hifitime::Epoch,
        expected_scale: TimeScale,
    ) -> Result<Self, DomainInstantError> {
        if epoch.time_scale != expected_scale.to_hifitime() {
            return Err(DomainInstantError::ScaleMismatch {
                expected: expected_scale,
                actual: epoch.time_scale,
            });
        }
        Ok(Self(epoch.to_tai_duration()))
    }
}

/// Failure to canonicalize a datetime value for a same-scale domain constraint.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DomainInstantError {
    #[error("expected time scale {expected}, got {actual:?}")]
    ScaleMismatch {
        expected: TimeScale,
        actual: hifitime::TimeScale,
    },
}

#[derive(Debug, Clone)]
enum ResolvedDomainConstraintKind {
    Quantity(ResolvedDomainBounds<f64>),
    Int(ResolvedDomainBounds<i64>),
    Datetime {
        scale: TimeScale,
        bounds: ResolvedDomainBounds<DomainInstant>,
    },
}

/// An evaluated domain constraint whose bound representation matches its value family.
///
/// The private variant carrier prevents quantity, integer, and datetime bounds
/// from being mixed after constraint resolution. Integer bounds remain `i64`
/// throughout and datetime bounds are canonicalized only after their declared
/// scale has been verified.
#[derive(Debug, Clone)]
pub struct ResolvedDomainConstraint {
    kind: ResolvedDomainConstraintKind,
}

/// Borrowed typed view used by model-interface projections.
pub enum ResolvedDomainConstraintRef<'constraint> {
    Quantity(&'constraint ResolvedDomainBounds<f64>),
    Int(&'constraint ResolvedDomainBounds<i64>),
    Datetime {
        scale: TimeScale,
        bounds: &'constraint ResolvedDomainBounds<DomainInstant>,
    },
}

impl ResolvedDomainConstraint {
    pub const fn quantity(bounds: ResolvedDomainBounds<f64>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Quantity(bounds),
        }
    }

    pub const fn int(bounds: ResolvedDomainBounds<i64>) -> Self {
        Self {
            kind: ResolvedDomainConstraintKind::Int(bounds),
        }
    }

    pub(crate) const fn as_ref(&self) -> ResolvedDomainConstraintRef<'_> {
        match &self.kind {
            ResolvedDomainConstraintKind::Quantity(bounds) => {
                ResolvedDomainConstraintRef::Quantity(bounds)
            }
            ResolvedDomainConstraintKind::Int(bounds) => ResolvedDomainConstraintRef::Int(bounds),
            ResolvedDomainConstraintKind::Datetime { scale, bounds } => {
                ResolvedDomainConstraintRef::Datetime {
                    scale: *scale,
                    bounds,
                }
            }
        }
    }

    pub fn datetime(
        scale: TimeScale,
        bounds: ResolvedDomainBounds<hifitime::Epoch>,
    ) -> Result<Self, DomainInstantError> {
        let min = bounds
            .min
            .map(|bound| bound.try_map(|epoch| DomainInstant::from_epoch(epoch, scale)))
            .transpose()?;
        let max = bounds
            .max
            .map(|bound| bound.try_map(|epoch| DomainInstant::from_epoch(epoch, scale)))
            .transpose()?;
        Ok(Self {
            kind: ResolvedDomainConstraintKind::Datetime {
                scale,
                bounds: ResolvedDomainBounds::new(min, max),
            },
        })
    }
}

/// A domain-constraint violation with a human-readable message.
///
/// The `message` field is safe to embed into a diagnostic verbatim. It includes
/// the relevant bound (min/max) with display units already substituted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct DomainViolation {
    pub message: String,
}

impl DomainViolation {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Check a runtime value against a resolved domain constraint.
///
/// Returns `Ok(())` if the value satisfies the constraint, or a
/// [`DomainViolation`] describing the first violation (including the variant
/// prefix for indexed values).
///
/// # Errors
///
/// Returns [`DomainViolation`] when any scalar sub-value falls outside the
/// declared bounds or when an internal value/constraint family invariant is
/// violated.
pub fn check_domain_constraint(
    value: &RuntimeValue,
    constraint: &ResolvedDomainConstraint,
) -> Result<(), DomainViolation> {
    match value {
        RuntimeValue::Indexed { entries, .. } => entries.iter().try_for_each(|(variant, entry)| {
            check_domain_constraint(entry, constraint).map_err(|violation| {
                DomainViolation::new(format!("at {variant}: {}", violation.message))
            })
        }),
        RuntimeValue::Quantity(value) => match &constraint.kind {
            ResolvedDomainConstraintKind::Quantity(bounds) => check_quantity_bounds(*value, bounds),
            other => Err(constraint_kind_mismatch("Quantity", other)),
        },
        RuntimeValue::Int(value) => match &constraint.kind {
            ResolvedDomainConstraintKind::Int(bounds) => check_bounds(value, bounds),
            other => Err(constraint_kind_mismatch("Int", other)),
        },
        RuntimeValue::Datetime(epoch) => match &constraint.kind {
            ResolvedDomainConstraintKind::Datetime { scale, bounds } => {
                let instant = DomainInstant::from_epoch(*epoch, *scale).map_err(|error| {
                    DomainViolation::new(format!("has incompatible datetime domain: {error}"))
                })?;
                check_bounds(&instant, bounds)
            }
            other => Err(constraint_kind_mismatch("Datetime", other)),
        },
        other => Err(constraint_kind_mismatch(
            &other.kind().to_string(),
            &constraint.kind,
        )),
    }
}

fn constraint_kind_mismatch(
    value_kind: &str,
    constraint: &ResolvedDomainConstraintKind,
) -> DomainViolation {
    let constraint_kind = match constraint {
        ResolvedDomainConstraintKind::Quantity(_) => "Quantity",
        ResolvedDomainConstraintKind::Int(_) => "Int",
        ResolvedDomainConstraintKind::Datetime { .. } => "Datetime",
    };
    DomainViolation::new(format!(
        "internal domain kind mismatch: {value_kind} value with {constraint_kind} bounds"
    ))
}

fn check_quantity_bounds(
    value: f64,
    bounds: &ResolvedDomainBounds<f64>,
) -> Result<(), DomainViolation> {
    // Runtime-value construction owns the public finite-number policy. Check
    // that invariant again only at this PartialOrd boundary: NaN would make
    // both `< min` and `> max` false and otherwise look spuriously in-bounds.
    let all_finite = value.is_finite()
        && bounds.min().is_none_or(|bound| bound.value().is_finite())
        && bounds.max().is_none_or(|bound| bound.value().is_finite());
    if !all_finite {
        return Err(DomainViolation::new(
            "internal domain invariant: quantity values and bounds must be finite",
        ));
    }
    check_bounds(&value, bounds)
}

fn check_bounds<T: PartialOrd>(
    value: &T,
    bounds: &ResolvedDomainBounds<T>,
) -> Result<(), DomainViolation> {
    if let Some(min) = bounds.min()
        && value < min.value()
    {
        return Err(DomainViolation::new(format!(
            "below minimum ({})",
            min.display()
        )));
    }
    if let Some(max) = bounds.max()
        && value > max.value()
    {
        return Err(DomainViolation::new(format!(
            "above maximum ({})",
            max.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int_constraint(min: i64, max: i64) -> ResolvedDomainConstraint {
        ResolvedDomainConstraint::int(ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(min, min.to_string())),
            Some(ResolvedDomainBound::new(max, max.to_string())),
        ))
    }

    #[test]
    fn full_range_integer_bounds_are_compared_without_float_conversion() {
        let constraint = int_constraint(i64::MIN, i64::MAX);
        check_domain_constraint(&RuntimeValue::Int(i64::MIN), &constraint).unwrap();
        check_domain_constraint(&RuntimeValue::Int(i64::MAX), &constraint).unwrap();
    }

    #[test]
    fn non_finite_internal_quantity_never_passes_bounds() {
        let constraint = ResolvedDomainConstraint::quantity(ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(0.0, "0".to_string())),
            Some(ResolvedDomainBound::new(1.0, "1".to_string())),
        ));

        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = check_domain_constraint(&RuntimeValue::Quantity(value), &constraint)
                .expect_err("non-finite internal quantities must fail closed");
            assert!(error.message.contains("must be finite"));
        }
    }

    #[test]
    fn non_finite_internal_quantity_bound_never_passes() {
        let constraint = ResolvedDomainConstraint::quantity(ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(f64::NAN, "NaN".to_string())),
            None,
        ));
        assert!(check_domain_constraint(&RuntimeValue::Quantity(0.5), &constraint).is_err());
    }

    #[test]
    fn datetime_constraint_constructor_rejects_a_cross_scale_epoch() {
        let tt_epoch =
            hifitime::Epoch::maybe_from_gregorian(2024, 1, 1, 0, 0, 0, 0, hifitime::TimeScale::TT)
                .unwrap();
        let bounds = ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(tt_epoch, tt_epoch.to_string())),
            None,
        );
        let error = ResolvedDomainConstraint::datetime(TimeScale::UTC, bounds).unwrap_err();
        assert!(matches!(error, DomainInstantError::ScaleMismatch { .. }));
    }
}
