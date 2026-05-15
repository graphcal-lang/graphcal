//! textDocument/references handler.

use tower_lsp::lsp_types::{Location, Url};

use crate::convert::LineIndex;
use crate::resolve::{SymbolLocation, definition_location, resolve_symbol_at};
use crate::server::AnalysisResult;

/// Find all references to the symbol at the given byte offset.
pub fn references(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let resolved = resolve_symbol_at(analysis, offset)?;

    // If cursor is on a builtin *definition* (not a reference to one), skip.
    if !resolved.is_reference
        && let SymbolLocation::Local(def) = &resolved.location
        && def.is_builtin()
    {
        return None;
    }

    let target_key = &resolved.key;

    let lines = LineIndex::new(&analysis.source);
    let mut locations: Vec<Location> = analysis
        .symbol_table
        .find_all_references(target_key)
        .into_iter()
        .map(|r| Location {
            uri: uri.clone(),
            range: lines.span_to_range(r.span),
        })
        .collect();

    if include_declaration
        && let Some(loc) = definition_location(&resolved.location, uri, &analysis.source)
    {
        locations.push(loc);
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}
