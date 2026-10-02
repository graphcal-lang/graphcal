//! Pure runtime-value validation against independently owned domain contracts.

use graphcal_compiler::semantic_error::domain::{
    DomainBoundSpelling, DomainBoundViolation, DomainViolationDetail,
};

use crate::domain_constraint::{
    DomainInstant, ResolvedDomainBounds, ResolvedDomainConstraint, ResolvedDomainConstraintRef,
};
use crate::invariant::{Failure, Invariant};
use crate::runtime_value::RuntimeValue;

/// A domain-constraint violation: the bound a value is outside of, at the
/// indexed entry it is found in.
pub type DomainViolation = DomainViolationDetail;

/// Check every scalar sub-value against a resolved domain constraint.
///
/// # Errors
///
/// Returns the first violation, at its indexed entry, or a violated
/// [`Invariant`] when the value is not of the family the constraint
/// constrains: the constraint is resolved from the value's checked type.
pub fn check_domain_constraint(
    value: &RuntimeValue,
    constraint: &ResolvedDomainConstraint,
) -> Result<(), Failure<DomainViolation>> {
    match (value, constraint.as_ref()) {
        (RuntimeValue::Indexed(entries), _) => entries.iter().try_for_each(|(entry, value)| {
            check_domain_constraint(value, constraint)
                .map_err(|failure| failure.map_error(|violation| violation.at_entry(entry.clone())))
        }),
        (RuntimeValue::Quantity(value), ResolvedDomainConstraintRef::Quantity(bounds)) => {
            check_bounds(value, bounds).map_err(Failure::Error)
        }
        (RuntimeValue::Int(value), ResolvedDomainConstraintRef::Int(bounds)) => {
            check_bounds(value, bounds).map_err(Failure::Error)
        }
        (
            RuntimeValue::Datetime(epoch),
            ResolvedDomainConstraintRef::Datetime { scale, bounds },
        ) => {
            let instant = DomainInstant::from_epoch(*epoch, scale).map_err(|error| {
                Invariant::violated(format_args!(
                    "a datetime checked as Datetime<{scale}> is outside its scale: {error}"
                ))
            })?;
            check_bounds(&instant, bounds).map_err(Failure::Error)
        }
        (value, constraint) => Err(Invariant::violated(format_args!(
            "a {} value was checked against {} bounds",
            value.describe(),
            match constraint {
                ResolvedDomainConstraintRef::Quantity(_) => "Quantity",
                ResolvedDomainConstraintRef::Int(_) => "Int",
                ResolvedDomainConstraintRef::Datetime { .. } => "Datetime",
            }
        ))
        .into()),
    }
}

fn check_bounds<T: PartialOrd>(
    value: &T,
    bounds: &ResolvedDomainBounds<T>,
) -> Result<(), DomainViolation> {
    let spelling =
        |bound: &crate::domain_constraint::ResolvedDomainBound<T>| -> DomainBoundSpelling {
            bound.display().clone()
        };
    if let Some(min) = bounds.min()
        && value < min.value()
    {
        return Err(DomainViolation::new(DomainBoundViolation::BelowMinimum(
            spelling(min),
        )));
    }
    if let Some(max) = bounds.max()
        && value > max.value()
    {
        return Err(DomainViolation::new(DomainBoundViolation::AboveMaximum(
            spelling(max),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::finite_value::FiniteQuantity;
    use graphcal_compiler::semantic::index_def::FiniteIndex;
    use graphcal_compiler::syntax::index_name::IndexEntryKey;

    use super::*;
    use crate::domain_constraint::ResolvedDomainBound;
    use crate::runtime_value::{IndexAxis, IndexedValue};

    fn int_constraint(min: i64, max: i64) -> ResolvedDomainConstraint {
        ResolvedDomainConstraint::int(ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(
                min,
                DomainBoundSpelling::Integer(min),
            )),
            Some(ResolvedDomainBound::new(
                max,
                DomainBoundSpelling::Integer(max),
            )),
        ))
    }

    #[test]
    fn full_range_integer_bounds_are_compared_without_float_conversion() {
        let constraint = int_constraint(i64::MIN, i64::MAX);
        check_domain_constraint(&RuntimeValue::Int(i64::MIN), &constraint).unwrap();
        check_domain_constraint(&RuntimeValue::Int(i64::MAX), &constraint).unwrap();
    }

    #[test]
    fn violations_name_the_bound_and_the_entry() {
        let constraint = int_constraint(0, 10);
        let Err(Failure::Error(violation)) =
            check_domain_constraint(&RuntimeValue::Int(11), &constraint)
        else {
            panic!("11 is above the maximum");
        };
        assert_eq!(violation.to_string(), "above maximum (10)");
        let axis = IndexAxis::finite(FiniteIndex::try_from_u64(2).unwrap()).unwrap();
        let indexed = RuntimeValue::Indexed(IndexedValue::for_test(
            axis,
            vec![RuntimeValue::Int(1), RuntimeValue::Int(-1)],
        ));
        let Err(Failure::Error(violation)) = check_domain_constraint(&indexed, &constraint) else {
            panic!("-1 is below the minimum");
        };
        assert_eq!(violation.entries(), [IndexEntryKey::position(1)]);
        assert_eq!(violation.to_string(), "at #1: below minimum (0)");
    }

    #[test]
    fn a_value_of_another_family_is_a_violated_invariant() {
        let constraint = int_constraint(0, 10);
        assert!(matches!(
            check_domain_constraint(&RuntimeValue::Bool(true), &constraint),
            Err(Failure::Invariant(_))
        ));
        let quantity = ResolvedDomainConstraint::quantity(ResolvedDomainBounds::new(
            Some(ResolvedDomainBound::new(
                FiniteQuantity::try_new(1.0).unwrap(),
                DomainBoundSpelling::Number(1.0),
            )),
            None,
        ));
        assert!(matches!(
            check_domain_constraint(&RuntimeValue::Int(1), &quantity),
            Err(Failure::Invariant(_))
        ));
        assert!(matches!(
            check_domain_constraint(&RuntimeValue::quantity(0.5).unwrap(), &quantity),
            Err(Failure::Error(_))
        ));
    }
}
