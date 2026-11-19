//! The type a domain bound expression must have for its constrained target,
//! and the check that an inferred bound type has it.

use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::semantic::checked_type::{CheckedType, Concrete, Concreteness};
use crate::semantic_error::SemanticError;
use crate::semantic_error::domain::DomainError;
use crate::semantic_error::domain::{DomainSubject, DomainTypeSpelling};
use crate::source_id::SourceId;

/// What a domain bound expression must infer to for a given target type.
pub(super) enum ExpectedBound {
    /// Bound must be `Quantity(d)`, also when `d` is dimensionless: an `Int`
    /// bound is never compared with a quantity value.
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
    display_name: &DomainSubject,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &CheckedType<V>,
    expected: &ExpectedBound,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<(), SemanticError> {
    match expected {
        ExpectedBound::Quantity(target_dim) => {
            if inferred.quantity_dimension() == Some(target_dim) {
                return Ok(());
            }
            if matches!(inferred, CheckedType::Int) && target_dim.is_dimensionless() {
                // Located at the bound's value, so an editor can replace a
                // written literal with its `Dimensionless` spelling.
                return Err(SemanticError::located(
                    src,
                    bound.value.span,
                    DomainError::IntBoundOnDimensionlessDomain {
                        name: display_name.clone(),
                        bound_name: bound.kind,
                        literal: written_int_literal(&bound.value),
                    },
                ));
            }
            let bound_dim = inferred.quantity_dimension().map_or_else(
                || DomainTypeSpelling::Checked(inferred.spelling(&registry.dimensions)),
                |d| DomainTypeSpelling::Dimension(registry.dimensions.dimension_spelling(d)),
            );
            Err(SemanticError::located(
                src,
                bound.span,
                DomainError::DomainDimensionMismatch {
                    name: display_name.clone(),
                    type_dim: DomainTypeSpelling::Dimension(
                        registry.dimensions.dimension_spelling(target_dim),
                    ),
                    bound_name: bound.kind,
                    bound_dim,
                },
            ))
        }
        ExpectedBound::Int => {
            if matches!(inferred, CheckedType::Int) {
                return Ok(());
            }
            Err(SemanticError::located(
                src,
                bound.span,
                DomainError::IntDomainBoundTypeMismatch {
                    name: display_name.clone(),
                    bound_name: bound.kind,
                    bound_type: inferred.spelling(&registry.dimensions),
                },
            ))
        }
        ExpectedBound::Datetime(target_scale) => {
            if matches!(inferred, CheckedType::Datetime(bound_scale) if bound_scale == target_scale)
            {
                return Ok(());
            }
            Err(SemanticError::located(
                src,
                bound.span,
                DomainError::DatetimeDomainBoundTypeMismatch {
                    name: display_name.clone(),
                    target_type: CheckedType::<Concrete>::Datetime(*target_scale)
                        .spelling(&registry.dimensions),
                    bound_name: bound.kind,
                    bound_type: inferred.spelling(&registry.dimensions),
                },
            ))
        }
    }
}

/// The value of a bound written as an integer literal, possibly negated
/// (`0`, `-3`), so a diagnostic can spell the same number as a
/// `Dimensionless` literal. Other `Int` expressions return `None`.
fn written_int_literal(expr: &crate::hir::expr::Expr) -> Option<i64> {
    use crate::desugar::desugared_ast::UnaryOp;
    use crate::hir::expr::ExprKind;
    match expr.kind() {
        ExprKind::Integer(value) => Some(*value),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match operand.kind() {
            ExprKind::Integer(value) => value.checked_neg(),
            _ => None,
        },
        _ => None,
    }
}
