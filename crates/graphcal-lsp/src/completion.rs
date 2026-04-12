//! textDocument/completion handler.

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::cursor_context::{CompletionContext, determine_completion_context};
use crate::server::AnalysisResult;
use crate::symbol_table::{DefinitionInfo, SymbolCategory};

/// Top-level declaration keywords.
const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "param", "node", "const", "fn", "type", "dim", "unit", "cat", "range", "import",
];

/// Built-in type keywords available in type annotation position.
const TYPE_KEYWORDS: &[&str] = &["Dimensionless", "Bool", "Int", "Datetime"];

/// Iterate over all visible definitions: local (from symbol table) and imported.
fn all_definitions(analysis: &AnalysisResult) -> impl Iterator<Item = &DefinitionInfo> {
    let local = analysis.symbol_table.definitions.values();
    let imported = analysis
        .imported_definitions
        .values()
        .map(|imp| &imp.definition);
    local.chain(imported)
}

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

/// Complete param, node, and const node names (after `@`).
fn complete_graph_refs(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    all_definitions(analysis)
        .filter(|def| {
            matches!(
                def.category,
                SymbolCategory::Param | SymbolCategory::Node | SymbolCategory::Const
            )
        })
        .filter(|def| !def.name_span.is_empty())
        .map(|def| CompletionItem {
            label: def.name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: def.type_description.clone(),
            ..Default::default()
        })
        .collect()
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
        all_definitions(analysis)
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

    // Constants and functions from both local and imported definitions.
    // Imported definitions never have BuiltinConst/BuiltinFn categories,
    // so the filter is safe to apply uniformly.
    items.extend(all_definitions(analysis).filter_map(|def| {
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
    }));

    items
}
