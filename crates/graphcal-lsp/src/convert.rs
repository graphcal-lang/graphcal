//! Conversion utilities between byte offsets/spans and LSP positions/ranges.

use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::text_position::{Utf16LineIndex, Utf16Position};
use tower_lsp::lsp_types::{Position, Range};

/// Per-handler cache of line-start byte offsets paired with the source text.
///
/// All offset → [`Position`] / [`Span`] → [`Range`] conversions in the LSP
/// go through this type. Construct once at the top of each handler that
/// answers more than one position query, then reuse for every conversion —
/// each call is O(log lines + col) instead of O(n).
pub struct LineIndex<'source> {
    inner: Utf16LineIndex<'source>,
}

impl<'source> LineIndex<'source> {
    /// Build a `LineIndex` for `source`. Scans the source once.
    #[must_use]
    pub fn new(source: &'source str) -> Self {
        Self {
            inner: Utf16LineIndex::new(source),
        }
    }

    /// Convert a byte offset to an LSP `Position` (0-based line and UTF-16
    /// character offset).
    #[must_use]
    pub fn position(&self, offset: usize) -> Position {
        lsp_position(self.inner.position(offset))
    }

    /// Convert a `Span` to an LSP `Range`.
    #[must_use]
    pub fn span_to_range(&self, span: Span) -> Range {
        self.offset_len_to_range(span.offset(), span.len())
    }

    /// Convert a byte offset and length to an LSP `Range`.
    #[must_use]
    pub fn offset_len_to_range(&self, offset: usize, len: usize) -> Range {
        let range = self.inner.range(offset, len);
        Range {
            start: lsp_position(range.start),
            end: lsp_position(range.end),
        }
    }
}

const fn lsp_position(position: Utf16Position) -> Position {
    Position {
        line: position.line,
        character: position.character,
    }
}

/// Convert an LSP `Position` (0-based line and UTF-16 character offset) to a byte offset in `source`.
///
/// The returned offset is always at a valid UTF-8 `char` boundary, clamped to the
/// requested line:
///
/// - If `position.character` falls past the end of the line, the offset of the
///   line-ending sequence (or the source end for the last line) is returned.
/// - If `position.character` lands inside a UTF-16 surrogate pair (e.g., the
///   second code unit of an astral-plane emoji), the offset of the preceding
///   `char` boundary is returned.
/// - If `position.line` exceeds the number of lines in `source`, the returned
///   offset is `source.len()`.
pub fn position_to_byte_offset(source: &str, position: Position) -> usize {
    Utf16LineIndex::new(source).byte_offset(Utf16Position {
        line: position.line,
        character: position.character,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_at_start() {
        let source = "hello\nworld";
        let pos = LineIndex::new(source).position(0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn position_mid_first_line() {
        let source = "hello\nworld";
        let pos = LineIndex::new(source).position(3);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn position_start_second_line() {
        let source = "hello\nworld";
        let pos = LineIndex::new(source).position(6);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn forward_and_reverse_positions_support_lf_cr_and_crlf() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let source = format!("a🙂{line_ending}bc");
            let second_line = source.find('b').unwrap();
            let lines = LineIndex::new(&source);
            assert_eq!(
                lines.position(second_line),
                Position::new(1, 0),
                "forward conversion for {line_ending:?}"
            );
            assert_eq!(
                position_to_byte_offset(&source, Position::new(1, 1)),
                source.find('c').unwrap(),
                "reverse conversion for {line_ending:?}"
            );
            assert_eq!(
                position_to_byte_offset(&source, Position::new(0, u32::MAX)),
                source.find(line_ending).unwrap(),
                "line clamp for {line_ending:?}"
            );
        }
    }

    #[test]
    fn position_mid_second_line() {
        let source = "hello\nworld";
        let pos = LineIndex::new(source).position(8);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 2);
    }

    #[test]
    fn position_past_end_clamps() {
        let source = "hi";
        let pos = LineIndex::new(source).position(100);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 2);
    }

    #[test]
    fn offset_round_trip() {
        let source = "hello\nworld\nfoo";
        let lines = LineIndex::new(source);
        (0..source.len()).for_each(|offset| {
            let pos = lines.position(offset);
            let back = position_to_byte_offset(source, pos);
            assert_eq!(back, offset, "round-trip failed for offset {offset}");
        });
    }

    #[test]
    fn span_to_range_basic() {
        let source = "hello\nworld";
        let range = LineIndex::new(source).span_to_range(Span::new(6, 5));
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
        let lines = LineIndex::new(source);
        for offset in 0..=source.len() {
            if !source.is_char_boundary(offset) {
                continue;
            }
            let pos = lines.position(offset);
            let back = position_to_byte_offset(source, pos);
            assert_eq!(
                back, offset,
                "round-trip failed for offset {offset} via {pos:?}"
            );
        }
    }

    #[test]
    fn line_index_clamps_past_end() {
        let source = "hi";
        let pos = LineIndex::new(source).position(100);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 2);
    }
}
