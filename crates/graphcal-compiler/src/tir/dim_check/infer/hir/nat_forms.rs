//! Diagnostics for normalized Nat forms used as finite-index cardinalities.
//!
//! Normalization itself happens at the AST-to-HIR boundary
//! (`hir::lower`); inference only validates the resulting forms.

use std::sync::Arc;

use miette::NamedSource;

use crate::graphcal_error::GraphcalError;
use crate::syntax::span::Span;

pub(super) fn finite_index_error(
    err: crate::semantic::index_def::IndexCardinalityError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.describe_finite_index(),
        src: src.clone(),
        span: span.into(),
    }
}
