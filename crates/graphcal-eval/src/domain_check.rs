//! Pure runtime-value checks against resolved domain constraints.
//!
//! Used both at compile time (validating const values in `exec_plan`) and at
//! evaluation time (validating param/node values in `eval::runtime`).

use crate::eval_expr::RuntimeValue;
use crate::tir::ResolvedDomainConstraint;

/// Check a runtime value against a resolved domain constraint.
///
/// Returns `Some(violation_message)` if the value violates the constraint,
/// or `None` if it satisfies the constraint. Indexed values are checked
/// element-wise; the first violation found is reported with its variant prefix.
pub fn check_domain_constraint(
    rv: &RuntimeValue,
    constraint: &ResolvedDomainConstraint,
) -> Option<String> {
    match rv {
        RuntimeValue::Scalar(si_value) => check_scalar_constraint(*si_value, constraint),
        #[expect(
            clippy::cast_precision_loss,
            reason = "domain bound comparison on small integers"
        )]
        RuntimeValue::Int(i) => check_scalar_constraint(*i as f64, constraint),
        RuntimeValue::Indexed { entries, .. } => {
            for (variant, entry_rv) in entries {
                if let Some(violation) = check_domain_constraint(entry_rv, constraint) {
                    return Some(format!("at {variant}: {violation}"));
                }
            }
            None
        }
        // Bool, Label, Struct, Datetime, RangeLabel: no constraint checking
        _ => None,
    }
}

/// Check a scalar SI value against min/max bounds.
fn check_scalar_constraint(si_value: f64, constraint: &ResolvedDomainConstraint) -> Option<String> {
    if let Some(min) = constraint.min
        && si_value < min
    {
        let min_display = constraint.min_display.as_deref().unwrap_or("?");
        return Some(format!("below minimum ({min_display})"));
    }
    if let Some(max) = constraint.max
        && si_value > max
    {
        let max_display = constraint.max_display.as_deref().unwrap_or("?");
        return Some(format!("above maximum ({max_display})"));
    }
    None
}
