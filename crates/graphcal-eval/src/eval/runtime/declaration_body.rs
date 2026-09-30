//! Lookup of a declaration's checked body by its identity.

use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::tir::typed::DeclarationBody;

/// The source of `declaration` in the scope of its owner.
pub(super) fn declaration_body<'tir>(
    tir: &'tir graphcal_compiler::tir::typed::CheckedTir,
    declaration: &ResolvedDeclName,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclarationBody<'tir>, GraphcalError> {
    tir.declaration_body(declaration).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("declaration `{declaration}` is absent from its owner's checked body"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}
