//! textDocument/inlayHint handler.

use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use crate::convert::byte_offset_to_position;
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Produce inlay hints for declarations within the given range.
///
/// Shows computed values (from evaluation) when available, falling back to
/// type descriptions. Only top-level `param`, `node`, and `const` declarations
/// produce hints.
pub fn inlay_hints(analysis: &AnalysisResult, range: Range) -> Option<Vec<InlayHint>> {
    let source = &analysis.source;
    let mut hints = Vec::new();

    for (key, def) in &analysis.symbol_table.definitions {
        // Only show hints for top-level declarations with computed values.
        if !matches!(
            def.category,
            SymbolCategory::Param | SymbolCategory::Node | SymbolCategory::Const
        ) {
            continue;
        }

        // Skip builtins and synthetic definitions.
        if def.name_span.len == 0 {
            continue;
        }

        // Check if the definition is within the requested range.
        let name_pos = byte_offset_to_position(source, def.name_span.offset);
        if name_pos.line < range.start.line || name_pos.line > range.end.line {
            continue;
        }

        // Build the label: prefer computed value, fall back to type.
        let label = if let Some(value_str) = analysis.eval_values.get(key) {
            format!(" = {value_str}")
        } else if let Some(type_desc) = &def.type_description {
            format!(": {type_desc}")
        } else {
            continue;
        };

        // Position the hint after the declaration name.
        let hint_position =
            byte_offset_to_position(source, def.name_span.offset + def.name_span.len);

        hints.push(InlayHint {
            position: hint_position,
            label: InlayHintLabel::String(label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: Some(false),
            data: None,
        });
    }

    // Sort by position for consistent ordering.
    hints.sort_by_key(|h| (h.position.line, h.position.character));

    if hints.is_empty() { None } else { Some(hints) }
}
