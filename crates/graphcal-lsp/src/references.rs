//! textDocument/references handler.

use tower_lsp::lsp_types::{Location, Url};

use crate::convert::span_to_range;
use crate::resolve::{SymbolLocation, resolve_symbol_at};
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

    let mut locations: Vec<Location> = analysis
        .symbol_table
        .find_all_references(target_key)
        .into_iter()
        .map(|r| Location {
            uri: uri.clone(),
            range: span_to_range(&analysis.source, r.span),
        })
        .collect();

    // Include the definition location if requested.
    if include_declaration {
        let declaration_location = analysis
            .symbol_table
            .definitions
            .get(target_key)
            .filter(|def| def.is_navigable())
            .map(|def| Location {
                uri: uri.clone(),
                range: span_to_range(&analysis.source, def.name_span),
            })
            .or_else(|| {
                analysis
                    .imported_definitions
                    .get(target_key)
                    .filter(|imported| !imported.definition.name_span.is_empty())
                    .map(|imported| Location {
                        uri: imported.uri.clone(),
                        range: span_to_range(&imported.source, imported.definition.name_span),
                    })
            });

        if let Some(loc) = declaration_location {
            locations.push(loc);
        }
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}
