//! textDocument/documentSymbol handler.

use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Build document symbols from an analysis result.
#[expect(
    deprecated,
    reason = "DocumentSymbol::deprecated field is deprecated but required by the type"
)]
pub fn build_document_symbols(analysis: &AnalysisResult) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    for def in analysis.symbol_table.definitions.values() {
        // Skip builtins, locals, and variants (variants are shown as children).
        // Also skip definitions with zero-length spans (synthetic/builtins).
        if def.decl_span.is_empty() {
            continue;
        }

        let kind = match def.category {
            SymbolCategory::Param | SymbolCategory::Node => SymbolKind::VARIABLE,
            SymbolCategory::Const | SymbolCategory::Unit => SymbolKind::CONSTANT,
            SymbolCategory::Function | SymbolCategory::Dag => SymbolKind::FUNCTION,
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
        let children = collect_children(analysis, &def.name);

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

/// Collect child symbols (variants) for a given parent name.
#[expect(
    deprecated,
    reason = "DocumentSymbol::deprecated field is deprecated but required by the type"
)]
fn collect_children(analysis: &AnalysisResult, parent_name: &str) -> Vec<DocumentSymbol> {
    let mut children = Vec::new();

    for (key, def) in &analysis.symbol_table.definitions {
        if def.category == SymbolCategory::IndexVariant && key.is_variant_of(parent_name) {
            let range = span_to_range(&analysis.source, def.decl_span);
            let selection_range = span_to_range(&analysis.source, def.name_span);
            children.push(DocumentSymbol {
                name: def.name.clone(),
                detail: def.detail.clone(),
                kind: SymbolKind::ENUM_MEMBER,
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: None,
            });
        }
    }

    children.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    children
}
