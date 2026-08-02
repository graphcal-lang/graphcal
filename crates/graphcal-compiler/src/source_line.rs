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
    pub const fn len(self) -> usize {
        match self {
            Self::Lf | Self::Cr => 1,
            Self::CrLf => 2,
        }
    }
}

/// Recognize a line ending beginning at `offset`.
///
/// CRLF is returned as one line ending rather than separate CR and LF endings.
pub fn line_ending_at(bytes: &[u8], offset: usize) -> Option<LineEnding> {
    match bytes.get(offset).copied() {
        Some(b'\n') => Some(LineEnding::Lf),
        Some(b'\r') if bytes.get(offset + 1) == Some(&b'\n') => Some(LineEnding::CrLf),
        Some(b'\r') => Some(LineEnding::Cr),
        _ => None,
    }
}

/// Iterate over physical line endings as `(byte_offset, kind)` pairs.
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
}
