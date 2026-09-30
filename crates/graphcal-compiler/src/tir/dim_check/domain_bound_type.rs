//! The type a domain bound expression must have for its constrained target,
//! and the check that an inferred bound type has it.

use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::{CheckedType, Concrete, Concreteness};

use super::helpers::format_checked_type;

/// What a domain bound expression must infer to for a given target type.
pub(super) enum ExpectedBound {
    /// Bound must be `Quantity(d)`. `Int` is also accepted when `d` is dimensionless.
    Quantity(Dimension),
    /// Bound must be exactly `Int`, preserving the full `i64` range.
    Int,
    /// Bound must be a datetime in exactly this declared time scale.
    Datetime(crate::semantic::time_scale::TimeScale),
}

pub(super) fn expected_bound_from_resolved(
    resolved: &crate::tir::typed::ResolvedValueType,
) -> Option<ExpectedBound> {
    use crate::tir::typed::{ResolvedDim, ResolvedValueType};

    match resolved {
        ResolvedValueType::Quantity(ResolvedDim::Concrete(dimension)) => {
            Some(ExpectedBound::Quantity(dimension.clone()))
        }
        ResolvedValueType::Int => Some(ExpectedBound::Int),
        ResolvedValueType::Datetime(scale) => Some(ExpectedBound::Datetime(*scale)),
        _ => None,
    }
}

pub(super) fn expected_bound_from_inferred<V: Concreteness>(
    inferred: &CheckedType<V>,
) -> Option<ExpectedBound> {
    match inferred {
        CheckedType::Indexed { element, .. } => expected_bound_from_inferred(element),
        CheckedType::Quantity(dimension) => Some(ExpectedBound::Quantity(dimension.clone())),
        CheckedType::Int => Some(ExpectedBound::Int),
        CheckedType::Datetime(scale) => Some(ExpectedBound::Datetime(*scale)),
        CheckedType::Complex(_)
        | CheckedType::Bool
        | CheckedType::Key(_)
        | CheckedType::Struct(..) => None,
    }
}

/// Variant of [`check_one_bound`] that takes a pre-formatted display name
/// for the constrained target (e.g. `"SatelliteSpec.mass"`) so a single
/// helper can serve both top-level decls and struct fields.
pub(super) fn check_one_bound_with_display_name<V: Concreteness>(
    display_name: &str,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &CheckedType<V>,
    expected: &ExpectedBound,
    registry: &FormattingRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match expected {
        ExpectedBound::Quantity(target_dim) => {
            let ok = match inferred {
                CheckedType::Int => target_dim.is_dimensionless(),
                other => other.quantity_dimension() == Some(target_dim),
            };
            if ok {
                return Ok(());
            }
            let bound_dim_str = inferred.quantity_dimension().map_or_else(
                || format_checked_type(inferred, registry),
                |d| registry.dimensions.format_dimension(d),
            );
            Err(GraphcalError::DomainDimensionMismatch {
                name: display_name.to_string(),
                type_dim: registry.dimensions.format_dimension(target_dim),
                bound_name: bound.kind.to_string(),
                bound_dim: bound_dim_str,
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Int => {
            if matches!(inferred, CheckedType::Int) {
                return Ok(());
            }
            Err(GraphcalError::IntDomainBoundTypeMismatch {
                name: display_name.to_string(),
                bound_name: bound.kind.to_string(),
                bound_type: format_checked_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Datetime(target_scale) => {
            if matches!(inferred, CheckedType::Datetime(bound_scale) if bound_scale == target_scale)
            {
                return Ok(());
            }
            Err(GraphcalError::DatetimeDomainBoundTypeMismatch {
                name: display_name.to_string(),
                target_type: format_checked_type(
                    &CheckedType::<Concrete>::Datetime(*target_scale),
                    registry,
                ),
                bound_name: bound.kind.to_string(),
                bound_type: format_checked_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
    }
}
