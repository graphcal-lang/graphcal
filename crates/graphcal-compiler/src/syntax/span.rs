/// Byte-offset span in source code. Compatible with `miette::SourceSpan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    offset: usize,
    len: usize,
}

impl Span {
    #[must_use]
    pub const fn new(offset: usize, len: usize) -> Self {
        Self { offset, len }
    }

    /// Returns the byte offset.
    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    /// Returns the byte length.
    #[must_use]
    pub const fn len(self) -> usize {
        self.len
    }

    /// Returns `true` if the span has zero length.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Merge two spans into one that covers both.
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        let start = self.offset.min(other.offset);
        let end = (self.offset + self.len).max(other.offset + other.len);
        Self::new(start, end - start)
    }
}

impl From<Span> for miette::SourceSpan {
    fn from(s: Span) -> Self {
        (s.offset, s.len).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_merge() {
        let a = Span::new(0, 5);
        let b = Span::new(10, 3);
        let merged = a.merge(b);
        assert_eq!(merged.offset(), 0);
        assert_eq!(merged.len(), 13);
    }

    #[test]
    fn span_merge_overlapping() {
        let a = Span::new(2, 5);
        let b = Span::new(4, 6);
        let merged = a.merge(b);
        assert_eq!(merged.offset(), 2);
        assert_eq!(merged.len(), 8);
    }

    #[test]
    fn span_to_miette() {
        let s = Span::new(10, 5);
        let ms: miette::SourceSpan = s.into();
        assert_eq!(ms.offset(), 10);
        assert_eq!(ms.len(), 5);
    }
}
