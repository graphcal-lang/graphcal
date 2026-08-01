//! UTF-8 source offsets and UTF-16 editor position conversion.
//!
//! Compiler spans use UTF-8 byte offsets, while browser and LSP editor APIs
//! use zero-based lines with UTF-16 code-unit columns. This module owns that
//! conversion independently of any transport protocol.

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
}

impl<'source> Utf16LineIndex<'source> {
    /// Scan `source` once and record every line's UTF-8 start offset.
    #[must_use]
    pub fn new(source: &'source str) -> Self {
        let mut line_starts = Vec::with_capacity(
            source
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                .saturating_add(1),
        );
        line_starts.push(0);
        line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(offset, byte)| (byte == b'\n').then_some(offset + 1)),
        );
        Self {
            source,
            line_starts,
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
        let character = self.source[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();

        Utf16Position {
            line: u32::try_from(line_index).unwrap_or(u32::MAX),
            character: u32::try_from(character).unwrap_or(u32::MAX),
        }
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
    fn line_starts_cover_empty_and_trailing_lines() {
        assert_eq!(Utf16LineIndex::new("").line_starts, vec![0]);
        assert_eq!(Utf16LineIndex::new("a\nb\n").line_starts, vec![0, 2, 4]);
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
