//! textDocument/definition handler.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Url};

use crate::convert::span_to_range;
use crate::resolve::{SymbolLocation, resolve_symbol_at};
use crate::server::AnalysisResult;

/// Resolve go-to-definition for a position in an analyzed document.
pub fn goto_definition(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
) -> Option<GotoDefinitionResponse> {
    let resolved = resolve_symbol_at(analysis, offset)?;
    match &resolved.location {
        SymbolLocation::Local(def) => resolve_local_definition(def, uri, &analysis.source),
        SymbolLocation::Imported(imported) => resolve_imported_definition(imported),
    }
}

/// Resolve a definition within the current file.
fn resolve_local_definition(
    definition: &crate::symbol_table::DefinitionInfo,
    uri: &Url,
    source: &str,
) -> Option<GotoDefinitionResponse> {
    if !definition.is_navigable() {
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
    if !imported.definition.is_navigable() {
        return None;
    }
    let range = span_to_range(&imported.source, imported.definition.name_span);
    Some(GotoDefinitionResponse::Scalar(Location {
        uri: imported.uri.clone(),
        range,
    }))
}
