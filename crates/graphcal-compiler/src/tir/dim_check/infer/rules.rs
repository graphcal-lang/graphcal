//! Typing rules the HIR inference engine applies beyond the operator rules
//! of [`crate::tir::operator_typing`].

use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::{MismatchOperand, MismatchRule};
use crate::semantic_error::structure::StructError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;

use crate::semantic::checked_type::{CheckedType, Symbolic};

/// Resolve a canonical HIR unit expression's dimension.
pub(in crate::tir::dim_check) fn resolve_unit_dimension_or_diagnose(
    unit: &crate::hir::expr::ResolvedUnitExpr,
    tir: &dyn crate::tir::typed::TirRead,
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    unit.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |dimension, item| {
            let info = tir
                .unit_info(item.name.value.static_definition())
                .ok_or_else(|| {
                    SemanticError::located(
                        src,
                        item.name.span,
                        DimensionError::UnknownUnit {
                            name: item.name.value.spelling().clone(),
                        },
                    )
                })?;
            let exponent = item.power;
            let term_dimension = info.dimension.pow(exponent).map_err(|_| {
                SemanticError::located(src, item.name.span, DimensionError::DimensionOverflow)
            })?;
            let resolved = match item.op {
                crate::syntax::ast::MulDivOp::Mul => dimension.checked_mul(&term_dimension),
                crate::syntax::ast::MulDivOp::Div => dimension.checked_div(&term_dimension),
            };
            resolved.map_err(|_| {
                SemanticError::located(src, item.name.span, DimensionError::DimensionOverflow)
            })
        })
}

/// Typing rule for `match` arms: all arms must have the same type, and at
/// least one arm must exist. `arm_body_span` maps an arm index to the span
/// of its body for diagnostics (the two engines carry different arm types).
pub(in crate::tir::dim_check) fn match_arms_rule(
    arm_types: &[CheckedType<Symbolic>],
    arm_body_span: impl Fn(usize) -> Span,
    expr_span: Span,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    let Some(first) = arm_types.first() else {
        return Err(SemanticError::located(
            src,
            expr_span,
            StructError::EmptyMatch,
        ));
    };
    for (i, arm_type) in arm_types.iter().enumerate().skip(1) {
        if arm_type != first {
            return Err(SemanticError::located(
                src,
                arm_body_span(i),
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Type(first.spelling(&registry.dimensions))),
                    found: Box::new(MismatchOperand::Type(
                        arm_type.spelling(&registry.dimensions),
                    )),
                    help: Box::new(MismatchRule::MatchArmsSameType),
                },
            ));
        }
    }
    Ok(first.clone())
}
