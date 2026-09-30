//! Public projection of a presented runtime value, displayed as presented.

use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::checked_type::CheckedType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::CheckedTir;

use super::display::attach_presentation;
use super::public_projection::EvaluatedValue;
use super::types::Value;
use crate::presentation_evidence::LeafPresentationDiagnostic;
use crate::runtime_presentation::ResolvedValue;

/// The public projection of `presented`'s value, without its presentation.
pub(super) fn project_si(
    presented: &ResolvedValue,
    declared_type: &CheckedType,
    tir: &CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<Value, GraphcalError> {
    EvaluatedValue::new(&presented.value(), declared_type).project(tir, src)
}

/// Display `projected`, the public projection of `presented`'s value (see
/// [`project_si`]), as `presented` says, reporting each quantity leaf whose
/// display failed; such a leaf keeps its SI value.
///
/// # Errors
///
/// A projection that does not have the shape of its runtime value violates
/// an invariant, reported as an internal error.
pub(super) fn display(
    projected: &mut Value,
    presented: &ResolvedValue,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<LeafPresentationDiagnostic>, GraphcalError> {
    attach_presentation(projected, presented).map_err(|invariant| {
        GraphcalError::internal_error(invariant.to_string(), src, DiagnosticAnchor::WholeFile)
    })
}

/// The public projection of `presented`'s value, displayed as presented.
pub(super) fn project_presented(
    presented: &ResolvedValue,
    declared_type: &CheckedType,
    tir: &CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<(Value, Vec<LeafPresentationDiagnostic>), GraphcalError> {
    let mut value = project_si(presented, declared_type, tir, src)?;
    let diagnostics = display(&mut value, presented, src)?;
    Ok((value, diagnostics))
}
