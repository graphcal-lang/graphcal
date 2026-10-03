//! textDocument/signatureHelp handler.

use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureInformation,
};

use crate::analysis::AnalysisResult;
use crate::cursor_context::find_fn_call_context;
use crate::fn_signatures::ExternCallee;

/// Resolve signature help for a cursor position.
///
/// Returns function parameter information when the cursor is inside a function
/// call's argument list, highlighting the active parameter.
#[expect(
    clippy::cast_possible_truncation,
    reason = "active_param is a small index; truncation is harmless"
)]
/// `source` is the latest editor text (which may be newer than
/// `analysis.source`): the call context must reflect the just-typed `(`
/// or `,`, while the signatures come from the cached analysis.
pub fn signature_help(
    analysis: &AnalysisResult,
    source: &str,
    offset: usize,
) -> Option<SignatureHelp> {
    let ctx = find_fn_call_context(source, offset)?;
    // Builtins are always bare names; extern signatures are keyed by their
    // typed `alias::name` parts, so the two never shadow each other.
    let sig_info = match ctx.callee.as_bare() {
        Some(name) => analysis.fn_signatures.get(name.as_str()),
        None => ExternCallee::from_call_path(&ctx.callee)
            .and_then(|callee| analysis.extern_fn_signatures.get(&callee)),
    }?;

    let active_param = ctx.active_param as u32;

    let parameters: Vec<ParameterInformation> = sig_info
        .parameters
        .iter()
        .map(|p| ParameterInformation {
            label: ParameterLabel::Simple(p.clone()),
            documentation: None,
        })
        .collect();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: sig_info.label.clone(),
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    })
}
