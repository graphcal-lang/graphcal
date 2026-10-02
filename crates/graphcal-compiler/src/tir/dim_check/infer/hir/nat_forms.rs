//! Diagnostics for normalized Nat forms used as finite-index cardinalities.
//!
//! Normalization itself happens at the AST-to-HIR boundary
//! (`hir::lower`); inference only validates the resulting forms.

use crate::semantic_error::SemanticError;
use crate::semantic_error::index::IndexError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;

pub(super) fn finite_index_error(
    err: crate::semantic::index_def::IndexCardinalityError,
    src: SourceId,
    span: Span,
) -> SemanticError {
    SemanticError::located(
        src,
        span,
        IndexError::InvalidFiniteIndexCardinality { error: err },
    )
}
