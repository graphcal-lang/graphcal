//! UTF-8 source offsets and UTF-16 editor position conversion.
//!
//! Compiler spans use UTF-8 byte offsets, while browser and LSP editor APIs
//! use zero-based lines with UTF-16 code-unit columns. This module owns that
//! conversion independently of any transport protocol.

use crate::source_line::line_endings;

/// Zero-based line and UTF-16 code-unit column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Position {
    pub line: u32,
    pub character: u32,
}

/// Half-open UTF-16 editor range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Range {
    pub start: Utf16Position,
    pub end: Utf16Position,
}

/// Reusable line index for translating UTF-8 byte offsets into UTF-16
/// positions without rescanning the whole source for every query.
pub struct Utf16LineIndex<'source> {
    source: &'source str,
    line_starts: Vec<usize>,
    line_content_ends: Vec<usize>,
}

impl<'source> Utf16LineIndex<'source> {
    /// Scan `source` once and record every line's UTF-8 content range.
    #[must_use]
    pub fn new(source: &'source str) -> Self {
        let mut line_starts = vec![0];
        let mut line_content_ends = Vec::new();
        line_endings(source).for_each(|(offset, line_ending)| {
            line_content_ends.push(offset);
            line_starts.push(offset + line_ending.len());
        });
        line_content_ends.push(source.len());
        Self {
            source,
            line_starts,
            line_content_ends,
        }
    }

    /// Convert a UTF-8 byte offset and length into a half-open UTF-16 range.
    #[must_use]
    pub fn range(&self, offset: usize, length: usize) -> Utf16Range {
        Utf16Range {
            start: self.position(offset),
            end: self.position(offset.saturating_add(length)),
        }
    }

    /// Convert a UTF-8 byte offset into a zero-based UTF-16 position.
    ///
    /// Out-of-range offsets are clamped to the source end. A defensive
    /// mid-codepoint offset is snapped down to the preceding UTF-8 boundary.
    #[must_use]
    pub fn position(&self, offset: usize) -> Utf16Position {
        let mut offset = offset.min(self.source.len());
        while !self.source.is_char_boundary(offset) {
            offset = offset.saturating_sub(1);
        }

        let line_index = self
            .line_starts
            .partition_point(|start| *start <= offset)
            .max(1)
            - 1;
        let line_start = self.line_starts[line_index];
        let content_offset = offset.min(self.line_content_ends[line_index]);
        let character = self.source[line_start..content_offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();

        Utf16Position {
            line: u32::try_from(line_index).unwrap_or(u32::MAX),
            character: u32::try_from(character).unwrap_or(u32::MAX),
        }
    }

    /// Convert a zero-based UTF-16 editor position into a UTF-8 byte offset.
    ///
    /// Lines past the source clamp to the source end. Columns past a line's
    /// content clamp to the start of its line-ending sequence. A column inside
    /// a surrogate pair snaps down to the preceding UTF-8 character boundary.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "char::len_utf16() returns 1 or 2, never truncates to u32"
    )]
    pub fn byte_offset(&self, position: Utf16Position) -> usize {
        let Ok(line_index) = usize::try_from(position.line) else {
            return self.source.len();
        };
        let Some(&line_start) = self.line_starts.get(line_index) else {
            return self.source.len();
        };
        let line_end = self.line_content_ends[line_index];

        let mut column = 0_u32;
        for (relative_offset, ch) in self.source[line_start..line_end].char_indices() {
            if column >= position.character {
                return line_start + relative_offset;
            }
            let next_column = column + ch.len_utf16() as u32;
            if next_column > position.character {
                return line_start + relative_offset;
            }
            column = next_column;
        }
        line_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_utf8_offsets_to_utf16_positions() {
        let source = "a🙂b\nnext";
        let index = Utf16LineIndex::new(source);
        assert_eq!(
            index.position("a🙂".len()),
            Utf16Position {
                line: 0,
                character: 3,
            }
        );
        assert_eq!(
            index.position("a🙂b\n".len()),
            Utf16Position {
                line: 1,
                character: 0,
            }
        );
    }

    #[test]
    fn line_starts_cover_empty_trailing_and_mixed_ending_lines() {
        assert_eq!(Utf16LineIndex::new("").line_starts, vec![0]);
        assert_eq!(Utf16LineIndex::new("a\nb\n").line_starts, vec![0, 2, 4]);
        assert_eq!(
            Utf16LineIndex::new("a\nb\rc\r\nd").line_starts,
            vec![0, 2, 4, 7]
        );
    }

    #[test]
    fn all_line_endings_produce_the_same_utf16_positions() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let source = format!("a🙂{line_ending}b");
            let index = Utf16LineIndex::new(&source);
            let second_line = source.find('b').unwrap();
            assert_eq!(
                index.position(second_line),
                Utf16Position {
                    line: 1,
                    character: 0,
                },
                "line ending {line_ending:?}"
            );
            assert_eq!(
                index.position(source.find(line_ending).unwrap()),
                Utf16Position {
                    line: 0,
                    character: 3,
                },
                "line ending {line_ending:?}"
            );
        }
    }

    #[test]
    fn utf16_positions_convert_back_across_all_line_endings() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let source = format!("a🙂{line_ending}bc");
            let index = Utf16LineIndex::new(&source);
            assert_eq!(
                index.byte_offset(Utf16Position {
                    line: 1,
                    character: 1,
                }),
                source.find('c').unwrap(),
                "line ending {line_ending:?}"
            );
            assert_eq!(
                index.byte_offset(Utf16Position {
                    line: 0,
                    character: u32::MAX,
                }),
                source.find(line_ending).unwrap(),
                "line ending {line_ending:?}"
            );
        }
    }

    #[test]
    fn position_clamps_to_valid_source_boundaries() {
        let index = Utf16LineIndex::new("a🙂b");
        assert_eq!(
            index.position(2),
            Utf16Position {
                line: 0,
                character: 1,
            }
        );
        assert_eq!(
            index.position(100),
            Utf16Position {
                line: 0,
                character: 4,
            }
        );
    }
}
