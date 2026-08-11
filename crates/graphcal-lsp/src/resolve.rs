//! Shared symbol resolution for LSP features (hover, go-to-definition, etc.).

use graphcal_compiler::syntax::span::Span;
use tower_lsp::lsp_types::{Location, Url};

use crate::convert::LineIndex;
use crate::server::{AnalysisResult, ImportedDefinition};
use crate::symbol_identity::ReferenceTarget;
use crate::symbol_table::{DefinitionInfo, SymbolKey};

/// Where a resolved symbol lives.
pub enum SymbolLocation<'a> {
    /// Symbol defined in the current file.
    Local(&'a DefinitionInfo),
    /// Symbol defined in an imported file.
    Imported(&'a ImportedDefinition),
}

/// A resolved symbol at a cursor position, with the key to look it up in the symbol table.
///
pub struct ResolvedSymbol<'a> {
    /// The symbol table key for this symbol.
    pub key: SymbolKey,
    /// Where the definition lives.
    pub location: SymbolLocation<'a>,
    /// Whether the cursor was on a reference (true) or a definition (false).
    pub is_reference: bool,
    /// The span of the token under the cursor.
    pub cursor_span: Span,
}

/// Build an LSP `Location` (URI + range) for a `DefinitionInfo` whose source
/// is either the active document or an imported one. Returns `None` for
/// builtins / synthetic spans (`is_navigable` false).
pub fn definition_location(
    location: &SymbolLocation<'_>,
    active_uri: &Url,
    active_source: &str,
) -> Option<Location> {
    match location {
        SymbolLocation::Local(def) => def.is_navigable().then(|| Location {
            uri: active_uri.clone(),
            range: LineIndex::new(active_source).span_to_range(def.name_span),
        }),
        SymbolLocation::Imported(imported) => {
            imported.definition.is_navigable().then(|| Location {
                uri: imported.uri.clone(),
                range: LineIndex::new(&imported.source)
                    .span_to_range(imported.definition.name_span),
            })
        }
    }
}

/// Resolve the symbol at the given byte offset.
///
/// Checks references first (cursor on a usage), then definitions (cursor on the name
/// in a declaration). Returns `None` if no symbol is found at the offset.
pub fn resolve_symbol_at(analysis: &AnalysisResult, offset: usize) -> Option<ResolvedSymbol<'_>> {
    // First check references.
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        let span = reference.span;
        let key = analysis
            .project_symbols
            .complete()
            .and_then(|project| project.resolve_root_reference(reference))
            .or_else(|| {
                analysis
                    .symbol_table
                    .resolve_local_target(&reference.target)
            })
            .or_else(|| match &reference.target {
                ReferenceTarget::Resolved(target) => Some(target.clone()),
                ReferenceTarget::Unresolved(unresolved) => {
                    analysis.resolve_imported_target(unresolved)
                }
            })?;
        if let Some(def) = analysis.symbol_table.definitions.get(&key) {
            return Some(ResolvedSymbol {
                key,
                location: SymbolLocation::Local(def),
                is_reference: true,
                cursor_span: span,
            });
        }
        if let Some(imported) = analysis.imported_definitions.get(&key) {
            return Some(ResolvedSymbol {
                key,
                location: SymbolLocation::Imported(imported),
                is_reference: true,
                cursor_span: span,
            });
        }
    }
    // Then check definitions.
    if let Some(definition) = analysis.symbol_table.find_definition_at(offset)
        && let Some(key) = analysis.symbol_table.find_definition_key(definition)
    {
        return Some(ResolvedSymbol {
            key,
            location: SymbolLocation::Local(definition),
            is_reference: false,
            cursor_span: definition.name_span,
        });
    }
    None
}

pub fn reference_lookup_keys(key: &SymbolKey) -> Vec<SymbolKey> {
    vec![key.clone()]
}
