//! textDocument/definition handler.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Url};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Resolve go-to-definition for a position in an analyzed document.
pub fn goto_definition(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
) -> Option<GotoDefinitionResponse> {
    // First check if cursor is on a reference.
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset)
        && let Some(definition) = analysis.symbol_table.definitions.get(&reference.target)
    {
        // Builtins have no source location.
        if matches!(
            definition.category,
            SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
        ) {
            return None;
        }
        // Skip synthetic definitions (zero-length span).
        if definition.name_span.len == 0 {
            return None;
        }
        let range = span_to_range(&analysis.source, definition.name_span);
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range,
        }));
    }

    // Then check if cursor is on a definition name itself.
    if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        if matches!(
            definition.category,
            SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
        ) {
            return None;
        }
        if definition.name_span.len == 0 {
            return None;
        }
        let range = span_to_range(&analysis.source, definition.name_span);
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range,
        }));
    }

    None
}
