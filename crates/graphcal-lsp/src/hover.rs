//! textDocument/hover handler.

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;
use crate::symbol_table::{DefinitionInfo, SymbolCategory};

/// Resolve hover information for a position in an analyzed document.
pub fn hover(analysis: &AnalysisResult, offset: usize) -> Option<Hover> {
    // First check if cursor is on a reference.
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        // Try local definitions first.
        if let Some(definition) = analysis.symbol_table.definitions.get(&reference.target) {
            let content = format_hover(definition);
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: content,
                }),
                range: Some(span_to_range(&analysis.source, reference.span)),
            });
        }
        // Try imported definitions (cross-file).
        if let Some(imported) = analysis.imported_definitions.get(&reference.target) {
            let content = format_hover(&imported.definition);
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: content,
                }),
                range: Some(span_to_range(&analysis.source, reference.span)),
            });
        }
    }

    // Then check if cursor is on a definition name itself.
    if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        let content = format_hover(definition);
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: Some(span_to_range(&analysis.source, definition.name_span)),
        });
    }

    None
}

/// Format hover content for a definition.
fn format_hover(def: &DefinitionInfo) -> String {
    match def.category {
        SymbolCategory::Param => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            format!("```graphcal\nparam {}: {type_str}\n```", def.name)
        }
        SymbolCategory::Node => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            format!("```graphcal\nnode {}: {type_str}\n```", def.name)
        }
        SymbolCategory::Const => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            format!("```graphcal\nconst {}: {type_str}\n```", def.name)
        }
        SymbolCategory::Function => {
            let fallback = format!("fn {}", def.name);
            let sig = def.type_description.as_deref().unwrap_or(&fallback);
            format!("```graphcal\n{sig}\n```")
        }
        SymbolCategory::Dimension => {
            let fallback = format!("dimension {}", def.name);
            let desc = def.type_description.as_deref().unwrap_or(&fallback);
            format!("```graphcal\n{desc}\n```")
        }
        SymbolCategory::Unit => {
            let desc = def.type_description.as_deref().unwrap_or("");
            format!("```graphcal\nunit {}: {desc}\n```", def.name)
        }
        SymbolCategory::Index => {
            let desc = def.type_description.as_deref().unwrap_or("...");
            format!(
                "```graphcal\nindex {} = {desc}\n```\n(named index labels are first-class value variants)",
                def.name
            )
        }
        SymbolCategory::StructType => {
            let desc = def.type_description.as_deref().unwrap_or("...");
            format!("```graphcal\ntype {} = {desc}\n```", def.name)
        }
        SymbolCategory::IndexVariant => {
            let detail = def.detail.as_deref().unwrap_or("");
            format!("`{}` ({detail})", def.name)
        }
        SymbolCategory::Field => {
            format!("`{}`", def.name)
        }
        SymbolCategory::LocalVar => {
            let detail = def.detail.as_deref().unwrap_or("");
            if detail.is_empty() {
                format!("`{}`", def.name)
            } else {
                format!("`{}` ({detail})", def.name)
            }
        }
        SymbolCategory::BuiltinFn => {
            let fallback = format!("fn {}", def.name);
            let sig = def.type_description.as_deref().unwrap_or(&fallback);
            let detail = def.detail.as_deref().unwrap_or("");
            format!("```graphcal\n{sig}\n```\n{detail}")
        }
        SymbolCategory::BuiltinConst => {
            let type_str = def.type_description.as_deref().unwrap_or("Dimensionless");
            format!(
                "```graphcal\nconst {}: {type_str}\n```\n(builtin)",
                def.name
            )
        }
        SymbolCategory::Assert => {
            format!("```graphcal\nassert {}: Bool\n```", def.name)
        }
    }
}
