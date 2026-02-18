//! Conversion utilities between byte offsets/spans and LSP positions/ranges.

use graphcal_syntax::span::Span;
use tower_lsp::lsp_types::{Position, Range};

/// Convert a byte offset in `source` to an LSP `Position` (0-based line and character).
pub fn byte_offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position {
        line,
        character: col,
    }
}

/// Convert an LSP `Position` (0-based line and character) to a byte offset in `source`.
pub fn position_to_byte_offset(source: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if line == position.line && col == position.character {
            return i;
        }
        if ch == '\n' {
            if line == position.line {
                // Past end of target line
                return i;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    // Handle position at end of file
    if line == position.line && col == position.character {
        return source.len();
    }
    source.len()
}

/// Convert a `Span` to an LSP `Range`.
pub fn span_to_range(source: &str, span: Span) -> Range {
    Range {
        start: byte_offset_to_position(source, span.offset),
        end: byte_offset_to_position(source, span.offset + span.len),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

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
        for offset in 0..source.len() {
            let pos = byte_offset_to_position(source, offset);
            let back = position_to_byte_offset(source, pos);
            assert_eq!(back, offset, "round-trip failed for offset {offset}");
        }
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
