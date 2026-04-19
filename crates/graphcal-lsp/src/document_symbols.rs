//! textDocument/documentSymbol handler.

use std::collections::HashMap;

use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;
use crate::symbol_table::{SymbolCategory, SymbolKey};

/// Build document symbols from an analysis result.
#[expect(
    deprecated,
    reason = "DocumentSymbol::deprecated field is deprecated but required by the type"
)]
pub fn build_document_symbols(analysis: &AnalysisResult) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    // Pre-group variants by their parent name so `collect_children` is O(variants)
    // per call rather than re-scanning all definitions (O(N*M) in total).
    let variants_by_parent = build_variants_index(analysis);

    for def in analysis.symbol_table.definitions.values() {
        // Skip builtins, locals, and variants (variants are shown as children).
        // Also skip definitions with zero-length spans (synthetic/builtins).
        if def.decl_span.is_empty() {
            continue;
        }

        let kind = match def.category {
            SymbolCategory::Param | SymbolCategory::Node => SymbolKind::VARIABLE,
            SymbolCategory::Const | SymbolCategory::Unit => SymbolKind::CONSTANT,
            SymbolCategory::Dag => SymbolKind::FUNCTION,
            SymbolCategory::StructType => SymbolKind::STRUCT,
            SymbolCategory::Dimension => SymbolKind::TYPE_PARAMETER,
            SymbolCategory::Index => SymbolKind::ENUM,
            SymbolCategory::Assert
            | SymbolCategory::Plot
            | SymbolCategory::Figure
            | SymbolCategory::Layer => SymbolKind::EVENT,
            SymbolCategory::IndexVariant
            | SymbolCategory::Field
            | SymbolCategory::LocalVar
            | SymbolCategory::BuiltinFn
            | SymbolCategory::BuiltinConst => continue,
        };

        let range = span_to_range(&analysis.source, def.decl_span);
        let selection_range = span_to_range(&analysis.source, def.name_span);

        // Collect children (variants for indexes and tagged unions).
        let children = collect_children(analysis, &variants_by_parent, &def.name);

        symbols.push(DocumentSymbol {
            name: def.name.clone(),
            detail: def.type_description.clone(),
            kind,
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: if children.is_empty() {
                None
            } else {
                Some(children)
            },
        });
    }

    // Sort by range start for consistent ordering.
    symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    symbols
}

/// Build an index from `parent name` to its `IndexVariant` definitions.
///
/// Replaces a linear scan of `analysis.symbol_table.definitions` per parent
/// call (O(N*M)) with a single pre-pass that groups by parent.
fn build_variants_index(
    analysis: &AnalysisResult,
) -> HashMap<&str, Vec<&crate::symbol_table::DefinitionInfo>> {
    let mut out: HashMap<&str, Vec<_>> = HashMap::new();
    for (key, def) in &analysis.symbol_table.definitions {
        if def.category != SymbolCategory::IndexVariant {
            continue;
        }
        if let SymbolKey::Variant { parent, .. } = key {
            out.entry(parent.as_str()).or_default().push(def);
        }
    }
    out
}

/// Collect child symbols (variants) for a given parent name.
#[expect(
    deprecated,
    reason = "DocumentSymbol::deprecated field is deprecated but required by the type"
)]
fn collect_children(
    analysis: &AnalysisResult,
    variants_by_parent: &HashMap<&str, Vec<&crate::symbol_table::DefinitionInfo>>,
    parent_name: &str,
) -> Vec<DocumentSymbol> {
    let Some(defs) = variants_by_parent.get(parent_name) else {
        return Vec::new();
    };

    let mut children: Vec<DocumentSymbol> = defs
        .iter()
        .map(|def| {
            let range = span_to_range(&analysis.source, def.decl_span);
            let selection_range = span_to_range(&analysis.source, def.name_span);
            DocumentSymbol {
                name: def.name.clone(),
                detail: def.detail.clone(),
                kind: SymbolKind::ENUM_MEMBER,
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: None,
            }
        })
        .collect();

    children.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    children
}
