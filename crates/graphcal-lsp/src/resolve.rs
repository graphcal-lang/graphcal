//! Shared symbol resolution for LSP features (hover, go-to-definition, etc.).

use graphcal_compiler::syntax::span::Span;
use tower_lsp::lsp_types::{Location, Url};

use crate::convert::LineIndex;
use crate::server::{AnalysisResult, ImportedDefinition};
use crate::symbol_table::{DefinitionInfo, SymbolKey, SymbolPath};

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
        for key in reference_lookup_keys(&reference.target) {
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
    let mut keys = vec![key.clone()];
    match key {
        SymbolKey::Qualified { module, name } => {
            if let Some((leaf, parent_module)) = module.split_last() {
                let parent = if parent_module.is_empty() {
                    SymbolPath::local(leaf.clone())
                } else {
                    SymbolPath::Qualified {
                        module: parent_module.to_vec(),
                        name: leaf.clone(),
                    }
                };
                keys.push(SymbolKey::Variant {
                    parent,
                    variant: name.clone(),
                });
            }
            keys.push(SymbolKey::Constructor(SymbolPath::Qualified {
                module: module.clone(),
                name: name.clone(),
            }));
        }
        SymbolKey::Variant { parent, variant } => {
            let (module, name) = qualified_key_parts_from_parent(parent, variant);
            keys.push(SymbolKey::Qualified { module, name });
        }
        SymbolKey::Constructor(path) => match path {
            SymbolPath::Local(name) => keys.push(SymbolKey::TopLevel(name.clone())),
            SymbolPath::Qualified { module, name } => keys.push(SymbolKey::Qualified {
                module: module.clone(),
                name: name.clone(),
            }),
        },
        SymbolKey::TopLevel(name) => {
            keys.push(SymbolKey::Constructor(SymbolPath::local(name.clone())));
        }
        _ => {}
    }
    keys
}

fn qualified_key_parts_from_parent(parent: &SymbolPath, leaf: &str) -> (Vec<String>, String) {
    match parent {
        SymbolPath::Local(parent_name) => (vec![parent_name.clone()], leaf.to_string()),
        SymbolPath::Qualified { module, name } => {
            let mut qualified = Vec::with_capacity(module.len() + 1);
            qualified.extend(module.iter().cloned());
            qualified.push(name.clone());
            (qualified, leaf.to_string())
        }
    }
}
