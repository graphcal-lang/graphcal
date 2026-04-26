//! textDocument/definition handler.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Url};

use crate::resolve::{definition_location, resolve_symbol_at};
use crate::server::AnalysisResult;

/// Resolve go-to-definition for a position in an analyzed document.
pub fn goto_definition(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
) -> Option<GotoDefinitionResponse> {
    let resolved = resolve_symbol_at(analysis, offset)?;
    let location = definition_location(&resolved.location, uri, &analysis.source)?;
    Some(GotoDefinitionResponse::Scalar(location))
}
