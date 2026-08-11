//! textDocument/references handler.

use tower_lsp::lsp_types::{Location, Url};

use crate::convert::LineIndex;
use crate::resolve::{
    SymbolLocation, definition_location, reference_lookup_keys, resolve_symbol_at,
};
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

    let target_keys = reference_lookup_keys(&resolved.key);

    let mut locations: Vec<Location> = analysis.project_symbols.complete().map_or_else(
        || {
            let lines = LineIndex::new(&analysis.source);
            target_keys
                .iter()
                .flat_map(|target_key| analysis.symbol_table.find_all_references(target_key))
                .map(|reference| Location {
                    uri: uri.clone(),
                    range: lines.span_to_range(reference.span),
                })
                .collect()
        },
        |project| {
            target_keys
                .iter()
                .flat_map(|target| project.references(target))
                .filter_map(|occurrence| {
                    let document = project.document(&occurrence.uri)?;
                    Some(Location {
                        uri: occurrence.uri,
                        range: LineIndex::new(&document.source).span_to_range(occurrence.span),
                    })
                })
                .collect()
        },
    );

    if include_declaration {
        let project_location = analysis.project_symbols.complete().and_then(|project| {
            let definition = project.definition(&resolved.key)?;
            let document = project.document(&definition.occurrence.uri)?;
            Some(Location {
                uri: definition.occurrence.uri,
                range: LineIndex::new(&document.source).span_to_range(definition.occurrence.span),
            })
        });
        if let Some(location) = project_location
            .or_else(|| definition_location(&resolved.location, uri, &analysis.source))
        {
            locations.push(location);
        }
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}
