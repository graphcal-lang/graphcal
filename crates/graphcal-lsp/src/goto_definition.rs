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
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        // Try local definitions first.
        if let Some(definition) = analysis.symbol_table.definitions.get(&reference.target) {
            return resolve_local_definition(definition, uri, &analysis.source);
        }
        // Try imported definitions (cross-file).
        if let Some(imported) = analysis.imported_definitions.get(&reference.target) {
            return resolve_imported_definition(imported);
        }
    }

    // Then check if cursor is on a definition name itself.
    if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        return resolve_local_definition(definition, uri, &analysis.source);
    }

    None
}

/// Resolve a definition within the current file.
fn resolve_local_definition(
    definition: &crate::symbol_table::DefinitionInfo,
    uri: &Url,
    source: &str,
) -> Option<GotoDefinitionResponse> {
    // Builtins have no source location.
    if matches!(
        definition.category,
        SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
    ) || definition.name_span.is_empty()
    {
        return None;
    }
    let range = span_to_range(source, definition.name_span);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range,
    }))
}

/// Resolve a definition in an imported file.
fn resolve_imported_definition(
    imported: &crate::server::ImportedDefinition,
) -> Option<GotoDefinitionResponse> {
    if matches!(
        imported.definition.category,
        SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
    ) || imported.definition.name_span.is_empty()
    {
        return None;
    }
    let range = span_to_range(&imported.source, imported.definition.name_span);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: imported.uri.clone(),
        range,
    }))
}
