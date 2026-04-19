//! textDocument/inlayHint handler.

use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use crate::server::AnalysisResult;

/// Produce inlay hints for declarations within the given range.
///
/// Shows computed values (from evaluation) when available, falling back to
/// type descriptions. Only top-level `param`, `node`, and `const` declarations
/// produce hints.
///
/// Iterates the precomputed `inlay_hint_entries` list (already filtered to
/// the right categories and paired with cached LSP `Position`s at
/// `SymbolTable::finalize` time), so the request path does no
/// O(source-length) scans and no work for builtins/imports.
pub fn inlay_hints(analysis: &AnalysisResult, range: Range) -> Option<Vec<InlayHint>> {
    let mut hints = Vec::new();

    for entry in analysis.symbol_table.inlay_hint_entries() {
        // Check if the definition is within the requested range.
        if entry.name_start.line < range.start.line || entry.name_start.line > range.end.line {
            continue;
        }

        let Some(def) = analysis.symbol_table.definitions.get(&entry.key) else {
            continue;
        };

        // Build the label: prefer computed value, fall back to type.
        let label = if let Some(value_str) = entry
            .key
            .top_level_name()
            .and_then(|name| analysis.eval_values.get(name))
        {
            format!(" = {value_str}")
        } else if let Some(type_desc) = &def.type_description {
            format!(": {type_desc}")
        } else {
            continue;
        };

        hints.push(InlayHint {
            position: entry.name_end,
            label: InlayHintLabel::String(label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: Some(true),
            padding_right: Some(false),
            data: None,
        });
    }

    // `inlay_hint_entries` is already sorted by `name_start`, which matches
    // the order of `name_end` for declarations that don't span lines, so the
    // resulting hints are already in position order.

    if hints.is_empty() { None } else { Some(hints) }
}
