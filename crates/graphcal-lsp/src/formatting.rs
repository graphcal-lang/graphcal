//! `textDocument/formatting` handler.

use tower_lsp::lsp_types::{Position, Range, TextEdit};

use crate::convert::byte_offset_to_position;

/// Format a graphcal source string, returning a single whole-document [`TextEdit`]
/// if the formatted output differs from the original.
///
/// Returns `None` if:
/// - The source fails to format (user already sees parse-error diagnostics for
///   the underlying parse failure — the LSP surfaces format failure silently
///   because there is no `TextEdit` we could offer).
/// - The formatted output is identical to the original (no changes needed).
pub fn format_document(source: &str) -> Option<Vec<TextEdit>> {
    let formatted = graphcal_fmt::format_source(source).ok()?;

    if formatted == source {
        return None;
    }

    let end = byte_offset_to_position(source, source.len());
    let range = Range::new(Position::new(0, 0), end);

    Some(vec![TextEdit {
        range,
        new_text: formatted,
    }])
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;

    #[test]
    fn returns_none_on_parse_error() {
        let result = format_document("this is not valid gcl {{{{");
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_when_already_formatted() {
        let source = "node x: Dimensionless = 1;\n";
        let result = format_document(source);
        assert!(result.is_none());
    }

    #[test]
    fn returns_edit_when_changed() {
        let source = "node   x:   Dimensionless  =  1;\n";
        let result = format_document(source);
        assert!(result.is_some());
        let edits = result.unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].new_text, "node x: Dimensionless = 1;\n");
    }
}
