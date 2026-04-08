//! textDocument/completion handler.

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::cursor_context::{CompletionContext, determine_completion_context};
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Top-level declaration keywords.
const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "param", "node", "const", "fn", "type", "dim", "unit", "cat", "range", "import",
];

/// Built-in type keywords available in type annotation position.
const TYPE_KEYWORDS: &[&str] = &["Dimensionless", "Bool", "Int", "Datetime"];

/// Produce completion items for the given cursor position.
pub fn completion(analysis: &AnalysisResult, offset: usize) -> Option<Vec<CompletionItem>> {
    let context = determine_completion_context(&analysis.source, offset);

    let items = match context {
        CompletionContext::GraphRef => complete_graph_refs(analysis),
        CompletionContext::TypeAnnotation => complete_types(analysis),
        CompletionContext::TopLevel => complete_top_level(),
        CompletionContext::Expression => complete_expression(analysis),
    };

    if items.is_empty() { None } else { Some(items) }
}

/// Complete param and node names (after `@`).
fn complete_graph_refs(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = analysis
        .symbol_table
        .definitions
        .values()
        .filter(|def| matches!(def.category, SymbolCategory::Param | SymbolCategory::Node))
        .filter(|def| !def.name_span.is_empty())
        .map(|def| CompletionItem {
            label: def.name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: def.type_description.clone(),
            ..Default::default()
        })
        .collect();

    // Include imported param/node definitions.
    items.extend(
        analysis
            .imported_definitions
            .iter()
            .filter(|(_, imported)| {
                matches!(
                    imported.definition.category,
                    SymbolCategory::Param | SymbolCategory::Node
                )
            })
            .map(|(_, imported)| CompletionItem {
                label: imported.definition.name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: imported.definition.type_description.clone(),
                ..Default::default()
            }),
    );

    items
}

/// Complete type names (after `:`).
fn complete_types(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    // Built-in type keywords.
    let mut items: Vec<CompletionItem> = TYPE_KEYWORDS
        .iter()
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect();

    items.extend(
        analysis
            .symbol_table
            .definitions
            .values()
            .filter(|def| !def.name_span.is_empty())
            .filter_map(|def| {
                let kind = match def.category {
                    SymbolCategory::Dimension => Some(CompletionItemKind::CLASS),
                    SymbolCategory::StructType => Some(CompletionItemKind::STRUCT),
                    SymbolCategory::Index => Some(CompletionItemKind::ENUM),
                    _ => None,
                }?;
                Some(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(kind),
                    detail: def.type_description.clone(),
                    ..Default::default()
                })
            }),
    );

    // Include imported type-like definitions.
    items.extend(
        analysis
            .imported_definitions
            .iter()
            .filter_map(|(_, imported)| {
                let kind = match imported.definition.category {
                    SymbolCategory::Dimension => Some(CompletionItemKind::CLASS),
                    SymbolCategory::StructType => Some(CompletionItemKind::STRUCT),
                    SymbolCategory::Index => Some(CompletionItemKind::ENUM),
                    _ => None,
                }?;
                Some(CompletionItem {
                    label: imported.definition.name.clone(),
                    kind: Some(kind),
                    detail: imported.definition.type_description.clone(),
                    ..Default::default()
                })
            }),
    );

    items
}

/// Complete top-level keywords.
fn complete_top_level() -> Vec<CompletionItem> {
    TOP_LEVEL_KEYWORDS
        .iter()
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

/// Complete expression-level items: constants, functions, boolean keywords.
fn complete_expression(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    // Boolean keywords.
    let mut items: Vec<CompletionItem> = ["true", "false"]
        .iter()
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect();

    items.extend(
        analysis
            .symbol_table
            .definitions
            .values()
            .filter_map(|def| {
                let kind = match def.category {
                    SymbolCategory::Const | SymbolCategory::BuiltinConst => {
                        Some(CompletionItemKind::CONSTANT)
                    }
                    SymbolCategory::BuiltinFn => Some(CompletionItemKind::FUNCTION),
                    _ => None,
                }?;
                Some(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(kind),
                    detail: def.type_description.clone(),
                    ..Default::default()
                })
            }),
    );

    // Include imported constants and functions.
    items.extend(
        analysis
            .imported_definitions
            .iter()
            .filter_map(|(_, imported)| {
                let kind = match imported.definition.category {
                    SymbolCategory::Const => Some(CompletionItemKind::CONSTANT),
                    _ => None,
                }?;
                Some(CompletionItem {
                    label: imported.definition.name.clone(),
                    kind: Some(kind),
                    detail: imported.definition.type_description.clone(),
                    ..Default::default()
                })
            }),
    );

    items
}
