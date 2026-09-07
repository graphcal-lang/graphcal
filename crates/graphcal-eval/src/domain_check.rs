//! Pure runtime-value validation against independently owned domain contracts.

use graphcal_compiler::registry::runtime_value::RuntimeValue;

use crate::domain_constraint::{
    DomainInstant, ResolvedDomainBounds, ResolvedDomainConstraint, ResolvedDomainConstraintRef,
};

/// A domain-constraint violation with a human-readable message.
///
/// The message includes the relevant bound with display units substituted and
/// is safe to embed verbatim in a diagnostic.
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

/// Check every scalar sub-value against a resolved domain constraint.
///
/// # Errors
///
/// Returns the first violation, including its indexed variant prefix, or an
/// error when the value/constraint family invariant is violated.
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
        RuntimeValue::Quantity(value) => match constraint.as_ref() {
            ResolvedDomainConstraintRef::Quantity(bounds) => {
                check_quantity_bounds(value.get(), bounds)
            }
            other => Err(constraint_kind_mismatch("Quantity", other)),
        },
        RuntimeValue::Int(value) => match constraint.as_ref() {
            ResolvedDomainConstraintRef::Int(bounds) => check_bounds(value, bounds),
            other => Err(constraint_kind_mismatch("Int", other)),
        },
        RuntimeValue::Datetime(epoch) => match constraint.as_ref() {
            ResolvedDomainConstraintRef::Datetime { scale, bounds } => {
                let instant = DomainInstant::from_epoch(*epoch, scale).map_err(|error| {
                    DomainViolation::new(format!("has incompatible datetime domain: {error}"))
                })?;
                check_bounds(&instant, bounds)
            }
            other => Err(constraint_kind_mismatch("Datetime", other)),
        },
        other => Err(constraint_kind_mismatch(
            &other.kind().to_string(),
            constraint.as_ref(),
        )),
    }
}

fn constraint_kind_mismatch(
    value_kind: &str,
    constraint: ResolvedDomainConstraintRef<'_>,
) -> DomainViolation {
    let constraint_kind = match constraint {
        ResolvedDomainConstraintRef::Quantity(_) => "Quantity",
        ResolvedDomainConstraintRef::Int(_) => "Int",
        ResolvedDomainConstraintRef::Datetime { .. } => "Datetime",
    };
    DomainViolation::new(format!(
        "internal domain kind mismatch: {value_kind} value with {constraint_kind} bounds"
    ))
}

fn check_quantity_bounds(
    value: f64,
    bounds: &ResolvedDomainBounds<f64>,
) -> Result<(), DomainViolation> {
    // Guard the PartialOrd boundary: NaN would otherwise look in-bounds.
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
    use crate::domain_constraint::ResolvedDomainBound;

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
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(RuntimeValue::quantity(value).is_err());
        }
    }

    #[test]
    fn non_finite_internal_quantity_bound_never_passes() {
        let constraint = ResolvedDomainConstraint::quantity(ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(f64::NAN, "NaN".to_string())),
            None,
        ));
        assert!(
            check_domain_constraint(&RuntimeValue::quantity(0.5).unwrap(), &constraint).is_err()
        );
    }
}
