use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::hir::NominalTypeDef;
use crate::registry::error::GraphcalError;
use crate::registry::types::FormattingRegistry;

use crate::registry::checked_type::{CheckedType, StructTypeRef};

pub(super) fn is_bool_type(ty: &CheckedType) -> bool {
    match ty {
        CheckedType::Bool => true,
        CheckedType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Look up the definition for an inferred struct identity.
///
/// This lookup is canonical-owner based only. Falling back from a resolved
/// identity to a bare leaf would make diamond imports with same-named types
/// nondeterministic.
pub(super) fn struct_type_def_for_inferred<'a>(
    ty: &StructTypeRef,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    _registry: &'a FormattingRegistry,
) -> Option<&'a NominalTypeDef> {
    dag.map(|dag| &dag.semantic.type_defs)
        .and_then(|defs| defs.struct_types.get(ty.resolved()))
        .map(AsRef::as_ref)
}

/// Format a checked type for display in diagnostics.
#[must_use]
pub fn format_checked_type(ty: &CheckedType, registry: &FormattingRegistry) -> String {
    ty.format(&registry.dimensions)
}

/// Format unequal inferred types without emitting a self-contradictory
/// leaf-only diagnostic such as `expected Foo, found Foo`.
pub(super) fn format_distinct_types(
    expected: &CheckedType,
    found: &CheckedType,
    registry: &FormattingRegistry,
) -> (String, String) {
    let expected_display = expected.format(&registry.dimensions);
    let found_display = found.format(&registry.dimensions);
    if expected_display != found_display {
        return (expected_display, found_display);
    }
    (
        expected.format_owner_qualified(&registry.dimensions),
        found.format_owner_qualified(&registry.dimensions),
    )
}

pub fn expect_quantity(
    inferred: &CheckedType,
    registry: &FormattingRegistry,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<Dimension, GraphcalError> {
    let found_kind = match inferred {
        CheckedType::Quantity(d) => return Ok(d.clone()),
        CheckedType::Complex(_) => "a Complex value",
        CheckedType::Bool => "a Bool value",
        CheckedType::Int => "an Int value",
        CheckedType::Datetime(_) => "a Datetime value",
        CheckedType::Key(_) => "an index-key value",
        CheckedType::Struct(..) => "a struct",
        CheckedType::Indexed { .. } => "an indexed value",
    };
    Err(GraphcalError::DimensionMismatch {
        expected: "quantity type".to_string(),
        found: format_checked_type(inferred, registry),
        help: format!("expected a quantity value, not {found_kind}"),
        src: src.clone(),
        span: span.into(),
    })
}
