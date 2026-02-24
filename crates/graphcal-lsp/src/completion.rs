//! textDocument/completion handler.

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::cursor_context::{CompletionContext, determine_completion_context};
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Top-level declaration keywords.
const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "param",
    "node",
    "const",
    "fn",
    "type",
    "dimension",
    "unit",
    "index",
    "import",
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
    let mut items = Vec::new();

    for def in analysis.symbol_table.definitions.values() {
        if !matches!(def.category, SymbolCategory::Param | SymbolCategory::Node) {
            continue;
        }
        // Skip builtins.
        if def.name_span.is_empty() {
            continue;
        }
        items.push(CompletionItem {
            label: def.name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: def.type_description.clone(),
            ..Default::default()
        });
    }

    // Include imported param/node definitions.
    for (name, imported) in &analysis.imported_definitions {
        if matches!(
            imported.definition.category,
            SymbolCategory::Param | SymbolCategory::Node
        ) {
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: imported.definition.type_description.clone(),
                ..Default::default()
            });
        }
    }

    items
}

/// Complete type names (after `:`).
fn complete_types(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Built-in type keywords.
    for kw in TYPE_KEYWORDS {
        items.push(CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    for def in analysis.symbol_table.definitions.values() {
        // Skip builtins.
        if def.name_span.is_empty() {
            continue;
        }
        match def.category {
            SymbolCategory::Dimension => {
                items.push(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: def.type_description.clone(),
                    ..Default::default()
                });
            }
            SymbolCategory::StructType => {
                items.push(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(CompletionItemKind::STRUCT),
                    detail: def.type_description.clone(),
                    ..Default::default()
                });
            }
            SymbolCategory::Index => {
                items.push(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(CompletionItemKind::ENUM),
                    detail: def.type_description.clone(),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    // Include imported type-like definitions.
    for (name, imported) in &analysis.imported_definitions {
        match imported.definition.category {
            SymbolCategory::Dimension | SymbolCategory::StructType | SymbolCategory::Index => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(match imported.definition.category {
                        SymbolCategory::Dimension => CompletionItemKind::CLASS,
                        SymbolCategory::StructType => CompletionItemKind::STRUCT,
                        SymbolCategory::Index => CompletionItemKind::ENUM,
                        _ => continue,
                    }),
                    detail: imported.definition.type_description.clone(),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

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
    let mut items = Vec::new();

    // Boolean keywords.
    for kw in &["true", "false"] {
        items.push(CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    for def in analysis.symbol_table.definitions.values() {
        match def.category {
            SymbolCategory::Const | SymbolCategory::BuiltinConst => {
                items.push(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(CompletionItemKind::CONSTANT),
                    detail: def.type_description.clone(),
                    ..Default::default()
                });
            }
            SymbolCategory::Function | SymbolCategory::BuiltinFn => {
                items.push(CompletionItem {
                    label: def.name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: def.type_description.clone(),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    // Include imported constants and functions.
    for (name, imported) in &analysis.imported_definitions {
        match imported.definition.category {
            SymbolCategory::Const | SymbolCategory::Function => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(match imported.definition.category {
                        SymbolCategory::Const => CompletionItemKind::CONSTANT,
                        SymbolCategory::Function => CompletionItemKind::FUNCTION,
                        _ => continue,
                    }),
                    detail: imported.definition.type_description.clone(),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    items
}
