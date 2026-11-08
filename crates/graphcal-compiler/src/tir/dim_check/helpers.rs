use crate::dimension::Dimension;
use crate::display::formatting_registry::FormattingRegistry;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;

use crate::semantic::checked_type::{CheckedType, Concreteness, StructTypeRef, Symbolic};

pub(super) fn is_bool_type(ty: &CheckedType<Symbolic>) -> bool {
    match ty {
        CheckedType::Bool => true,
        CheckedType::Indexed { element, .. } => is_bool_type(element),
        _ => false,
    }
}

/// Look up the nominal type of an inferred struct identity: as `dag` records
/// it, else as the DAG of `tir` that records its metadata.
///
/// `dag` records the types it names and constructs. A value it reads from an
/// included instance or a DAG call can have a type `dag` never names; that
/// type's field metadata lives with a DAG that does (#1942).
///
/// This lookup is canonical-owner based only. Falling back from a resolved
/// identity to a bare leaf would make diamond imports with same-named types
/// nondeterministic.
pub(super) fn nominal_for_inferred<'a>(
    ty: &StructTypeRef,
    dag: &'a crate::tir::typed::DagTIR,
    tir: &'a dyn crate::tir::typed::TirRead,
) -> Option<&'a crate::tir::typed::ResolvedNominal> {
    let identity = ty.resolved();
    dag.semantic.type_defs.nominal(identity).or_else(|| {
        tir.dag_with_type_metadata(identity)?
            .semantic
            .type_defs
            .nominal(identity)
    })
}

/// Require `inferred` to be a real quantity; see
/// [`crate::tir::operator_typing::expect_quantity`].
///
/// # Errors
///
/// Returns a dimension mismatch at `span` for any other type.
pub fn expect_quantity<V: Concreteness>(
    inferred: &CheckedType<V>,
    registry: &FormattingRegistry,
    src: SourceId,
    span: crate::syntax::span::Span,
) -> Result<Dimension, SemanticError> {
    crate::tir::operator_typing::expect_quantity(inferred, &registry.dimensions, src, span)
}
