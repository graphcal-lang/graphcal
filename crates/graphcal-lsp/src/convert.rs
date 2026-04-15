//! Conversion utilities between byte offsets/spans and LSP positions/ranges.

use graphcal_compiler::syntax::span::Span;
use tower_lsp::lsp_types::{Position, Range};

/// Convert a byte offset in `source` to an LSP `Position` (0-based line and UTF-16 character offset).
///
/// LSP positions use UTF-16 code units for the character offset, so characters
/// outside the Basic Multilingual Plane (e.g., emoji, some CJK) count as 2.
#[expect(
    clippy::cast_possible_truncation,
    reason = "char::len_utf16() returns 1 or 2, never truncates to u32"
)]
pub fn byte_offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let (line, col) = source.char_indices().take_while(|(i, _)| *i < offset).fold(
        (0u32, 0u32),
        |(line, col), (_, ch)| match ch {
            '\n' => (line + 1, 0),
            _ => (line, col + ch.len_utf16() as u32),
        },
    );
    Position {
        line,
        character: col,
    }
}

/// Convert an LSP `Position` (0-based line and UTF-16 character offset) to a byte offset in `source`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "char::len_utf16() returns 1 or 2, never truncates to u32"
)]
pub fn position_to_byte_offset(source: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut col = 0u32;
    source
        .char_indices()
        .find_map(|(i, ch)| {
            if line == position.line && col == position.character {
                return Some(i);
            }
            match ch {
                '\n' if line == position.line => Some(i),
                '\n' => {
                    line += 1;
                    col = 0;
                    None
                }
                _ => {
                    col += ch.len_utf16() as u32;
                    None
                }
            }
        })
        .unwrap_or(source.len())
}

/// Convert a `Span` to an LSP `Range`.
pub fn span_to_range(source: &str, span: Span) -> Range {
    offset_len_to_range(source, span.offset(), span.len())
}

/// Convert a byte offset and length to an LSP `Range`.
pub fn offset_len_to_range(source: &str, offset: usize, len: usize) -> Range {
    Range {
        start: byte_offset_to_position(source, offset),
        end: byte_offset_to_position(source, offset + len),
    }
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
    fn position_at_start() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn position_mid_first_line() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 3);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn position_start_second_line() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 6);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn position_mid_second_line() {
        let source = "hello\nworld";
        let pos = byte_offset_to_position(source, 8);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 2);
    }

    #[test]
    fn position_past_end_clamps() {
        let source = "hi";
        let pos = byte_offset_to_position(source, 100);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 2);
    }

    #[test]
    fn offset_round_trip() {
        let source = "hello\nworld\nfoo";
        (0..source.len()).for_each(|offset| {
            let pos = byte_offset_to_position(source, offset);
            let back = position_to_byte_offset(source, pos);
            assert_eq!(back, offset, "round-trip failed for offset {offset}");
        });
    }

    #[test]
    fn span_to_range_basic() {
        let source = "hello\nworld";
        let range = span_to_range(source, Span::new(6, 5));
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 1);
        assert_eq!(range.end.character, 5);
    }
}
