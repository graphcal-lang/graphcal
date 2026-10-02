use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::{
    MismatchOperand, MismatchRule, NonQuantityValue, OperandExpectation,
};
use crate::source_id::SourceId;

use crate::semantic::checked_type::{CheckedType, Concreteness, StructTypeRef, Symbolic};

pub(super) fn is_bool_type(ty: &CheckedType<Symbolic>) -> bool {
    match ty {
        CheckedType::Bool => true,
        CheckedType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Look up the nominal type of an inferred struct identity as `dag` records
/// it.
///
/// This lookup is canonical-owner based only. Falling back from a resolved
/// identity to a bare leaf would make diamond imports with same-named types
/// nondeterministic.
pub(super) fn nominal_for_inferred<'a>(
    ty: &StructTypeRef,
    dag: &'a crate::tir::typed::DagTIR,
) -> Option<&'a crate::tir::typed::ResolvedNominal> {
    dag.semantic.type_defs.nominal(ty.resolved())
}

/// Format a checked type for display in diagnostics.
#[must_use]
pub fn format_checked_type<V: Concreteness>(
    ty: &CheckedType<V>,
    registry: &FormattingRegistry,
) -> String {
    ty.format(&registry.dimensions)
}

pub fn expect_quantity<V: Concreteness>(
    inferred: &CheckedType<V>,
    registry: &FormattingRegistry,
    src: SourceId,
    span: crate::syntax::span::Span,
) -> Result<Dimension, SemanticError> {
    let found_kind = match inferred {
        CheckedType::Quantity(d) => return Ok(d.clone()),
        CheckedType::Complex(_) => NonQuantityValue::Complex,
        CheckedType::Bool => NonQuantityValue::Bool,
        CheckedType::Int => NonQuantityValue::Int,
        CheckedType::Datetime(_) => NonQuantityValue::Datetime,
        CheckedType::Key(_) => NonQuantityValue::Key,
        CheckedType::Struct(..) => NonQuantityValue::Struct,
        CheckedType::Indexed { .. } => NonQuantityValue::Indexed,
    };
    Err(SemanticError::located(
        src,
        span,
        DimensionError::DimensionMismatch {
            expected: Box::new(MismatchOperand::Expected(OperandExpectation::QuantityType)),
            found: Box::new(MismatchOperand::Type(
                inferred.spelling(&registry.dimensions),
            )),
            help: Box::new(MismatchRule::ExpectedQuantity(found_kind)),
        },
    ))
}
