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
///
/// The returned offset is always at a valid UTF-8 `char` boundary, clamped to the
/// requested line:
///
/// - If `position.character` falls past the end of the line, the offset of the
///   line-terminating `\n` (or the source end for the last line) is returned.
/// - If `position.character` lands inside a UTF-16 surrogate pair (e.g., the
///   second code unit of an astral-plane emoji), the offset of the preceding
///   `char` boundary is returned.
/// - If `position.line` exceeds the number of lines in `source`, the returned
///   offset is `source.len()`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "char::len_utf16() returns 1 or 2, never truncates to u32"
)]
pub fn position_to_byte_offset(source: &str, position: Position) -> usize {
    let line_start = line_start_offset(source, position.line);
    if line_start >= source.len() {
        return source.len();
    }

    let mut col = 0u32;
    for (offset, ch) in source[line_start..].char_indices() {
        if ch == '\n' {
            return line_start + offset;
        }
        if col >= position.character {
            return line_start + offset;
        }
        let next_col = col + ch.len_utf16() as u32;
        if next_col > position.character {
            // `position.character` lands inside a surrogate pair; snap to the
            // current (valid) char boundary.
            return line_start + offset;
        }
        col = next_col;
    }
    source.len()
}

/// Return the byte offset where `line` (0-based) starts in `source`.
///
/// If `line` exceeds the number of lines, returns `source.len()`.
fn line_start_offset(source: &str, line: u32) -> usize {
    if line == 0 {
        return 0;
    }
    let mut seen = 0u32;
    for (i, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            seen += 1;
            if seen == line {
                return i + 1;
            }
        }
    }
    source.len()
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

    #[test]
    fn position_past_end_of_line_clamps_to_newline() {
        let source = "hi\nworld";
        // Column 10 on line 0 — past the end of "hi".
        let offset = position_to_byte_offset(
            source,
            Position {
                line: 0,
                character: 10,
            },
        );
        // Must not spill onto line 1; should clamp at the `\n`.
        assert_eq!(offset, 2);
    }

    #[test]
    fn position_past_last_line_clamps_to_source_end() {
        let source = "hi";
        let offset = position_to_byte_offset(
            source,
            Position {
                line: 5,
                character: 0,
            },
        );
        assert_eq!(offset, source.len());
    }

    #[test]
    fn position_with_emoji_before_char_boundary() {
        // "a🙂b" — the emoji is 4 UTF-8 bytes and 2 UTF-16 code units.
        let source = "a🙂b";
        assert_eq!(
            position_to_byte_offset(
                source,
                Position {
                    line: 0,
                    character: 0
                }
            ),
            0
        );
        // After the 'a': UTF-16 col 1.
        assert_eq!(
            position_to_byte_offset(
                source,
                Position {
                    line: 0,
                    character: 1
                }
            ),
            1
        );
        // After the emoji: UTF-16 col 3.
        assert_eq!(
            position_to_byte_offset(
                source,
                Position {
                    line: 0,
                    character: 3
                }
            ),
            5
        );
        // After the 'b': UTF-16 col 4.
        assert_eq!(
            position_to_byte_offset(
                source,
                Position {
                    line: 0,
                    character: 4
                }
            ),
            6
        );
    }

    #[test]
    fn position_inside_surrogate_pair_snaps_to_previous_boundary() {
        // UTF-16 column 2 is the second code unit of 🙂 — snap back to offset 1.
        let source = "a🙂b";
        assert_eq!(
            position_to_byte_offset(
                source,
                Position {
                    line: 0,
                    character: 2
                }
            ),
            1
        );
    }

    #[test]
    fn round_trip_with_non_bmp_characters() {
        let source = "a🙂\nb🎉c";
        for offset in 0..=source.len() {
            if !source.is_char_boundary(offset) {
                continue;
            }
            let pos = byte_offset_to_position(source, offset);
            let back = position_to_byte_offset(source, pos);
            assert_eq!(
                back, offset,
                "round-trip failed for offset {offset} via {pos:?}"
            );
        }
    }
}
