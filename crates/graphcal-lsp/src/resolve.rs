//! Shared symbol resolution for LSP features (hover, go-to-definition, etc.).

use graphcal_syntax::span::Span;

use crate::server::{AnalysisResult, ImportedDefinition};
use crate::symbol_table::DefinitionInfo;

/// Where a resolved symbol lives.
pub enum SymbolLocation<'a> {
    /// Symbol defined in the current file.
    Local(&'a DefinitionInfo),
    /// Symbol defined in an imported file.
    Imported(&'a ImportedDefinition),
}

/// A resolved symbol at a cursor position, with the key to look it up in the symbol table.
#[expect(
    dead_code,
    reason = "fields are part of the public API for future LSP feature callers"
)]
pub struct ResolvedSymbol<'a> {
    /// The symbol table key for this symbol.
    pub key: String,
    /// Where the definition lives.
    pub location: SymbolLocation<'a>,
    /// Whether the cursor was on a reference (true) or a definition (false).
    pub is_reference: bool,
    /// The span of the token under the cursor.
    pub cursor_span: Span,
}

/// Resolve the symbol at the given byte offset.
///
/// Checks references first (cursor on a usage), then definitions (cursor on the name
/// in a declaration). Returns `None` if no symbol is found at the offset.
pub fn resolve_symbol_at(analysis: &AnalysisResult, offset: usize) -> Option<ResolvedSymbol<'_>> {
    // First check references.
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        let key = reference.target.clone();
        let span = reference.span;
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
    if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        let span = definition.name_span;
        // Find the actual key in the definitions map (may differ from `definition.name`
        // for scoped symbols like `fn_name::param`).
        let key = analysis
            .symbol_table
            .definitions
            .iter()
            .find(|(_, d)| std::ptr::eq(*d, definition))
            .map_or_else(|| definition.name.clone(), |(k, _)| k.clone());
        return Some(ResolvedSymbol {
            key,
            location: SymbolLocation::Local(definition),
            is_reference: false,
            cursor_span: span,
        });
    }
    None
}
