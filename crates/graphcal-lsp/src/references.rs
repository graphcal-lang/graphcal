//! textDocument/references handler.

use tower_lsp::lsp_types::{Location, Url};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Find all references to the symbol at the given byte offset.
pub fn references(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    // Determine the target name: either from a reference or a definition at cursor.
    let target_name = match (
        analysis.symbol_table.find_reference_at(offset),
        analysis.symbol_table.find_definition_at(offset),
    ) {
        (Some(reference), _) => reference.target.clone(),
        (None, Some(definition))
            if matches!(
                definition.category,
                SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
            ) =>
        {
            return None;
        }
        (None, Some(definition)) => definition.name.clone(),
        (None, None) => return None,
    };

    let mut locations: Vec<Location> = analysis
        .symbol_table
        .find_all_references(&target_name)
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
            .get(&target_name)
            .filter(|def| {
                !matches!(
                    def.category,
                    SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst
                ) && !def.name_span.is_empty()
            })
            .map(|def| Location {
                uri: uri.clone(),
                range: span_to_range(&analysis.source, def.name_span),
            })
            .or_else(|| {
                analysis
                    .imported_definitions
                    .get(&target_name)
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
