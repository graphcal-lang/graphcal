//! Shared source-line-ending recognition.
//!
//! Graphcal accepts LF, CRLF, and lone CR as physical line endings. Keeping
//! recognition here prevents the lexer, raw parser lookahead, and editor
//! position conversion from assigning different meanings to the same bytes.

/// A physical line ending in Graphcal source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub(crate) const fn len(self) -> usize {
        match self {
            Self::Lf | Self::Cr => 1,
            Self::CrLf => 2,
        }
    }
}

/// Recognize a line ending beginning at `offset`.
///
/// CRLF is returned as one line ending rather than separate CR and LF endings.
pub(crate) fn line_ending_at(bytes: &[u8], offset: usize) -> Option<LineEnding> {
    match bytes.get(offset).copied() {
        Some(b'\n') => Some(LineEnding::Lf),
        Some(b'\r') if bytes.get(offset + 1) == Some(&b'\n') => Some(LineEnding::CrLf),
        Some(b'\r') => Some(LineEnding::Cr),
        _ => None,
    }
}

/// Iterate over physical line endings as `(byte_offset, kind)` pairs.
#[must_use = "the iterator is lazy"]
pub fn line_endings(source: &str) -> impl Iterator<Item = (usize, LineEnding)> + '_ {
    let bytes = source.as_bytes();
    let mut offset = 0;
    std::iter::from_fn(move || {
        while offset < bytes.len() {
            let current = offset;
            match line_ending_at(bytes, current) {
                Some(line_ending) => {
                    offset += line_ending.len();
                    return Some((current, line_ending));
                }
                None => offset += 1,
            }
        }
        None
    })
}

/// Split `source` at every physical line ending, excluding the endings.
///
/// Like `str::split('\n')` (`n` line endings yield `n + 1` pieces, the last
/// possibly empty), but CRLF and lone CR are line endings too, so no piece
/// keeps a stray `\r`.
#[must_use = "the iterator is lazy"]
pub fn split_lines(source: &str) -> impl Iterator<Item = &str> + '_ {
    let mut start = 0;
    line_endings(source)
        .map(Some)
        .chain(std::iter::once(None))
        .map(move |line_ending| {
            let line_start = start;
            match line_ending {
                Some((offset, line_ending)) => {
                    start = offset + line_ending.len();
                    &source[line_start..offset]
                }
                None => &source[line_start..],
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_lf_crlf_and_lone_cr_without_splitting_crlf() {
        let source = "a\nb\r\nc\rd";
        assert_eq!(
            line_endings(source).collect::<Vec<_>>(),
            vec![
                (1, LineEnding::Lf),
                (3, LineEnding::CrLf),
                (6, LineEnding::Cr),
            ]
        );
    }

    #[test]
    fn split_lines_drops_every_line_ending() {
        assert_eq!(
            split_lines("a\nb\r\nc\rd").collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        assert_eq!(split_lines("a\r\n").collect::<Vec<_>>(), ["a", ""]);
        assert_eq!(split_lines("").collect::<Vec<_>>(), [""]);
    }
}
