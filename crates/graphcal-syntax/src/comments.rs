use crate::span::Span;

/// The kind of a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `// ...`
    Line,
    /// `/// ...`
    Doc,
}

/// A comment extracted from source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub kind: CommentKind,
    pub text: String,
    pub span: Span,
}

/// Metadata extracted from source text for the formatter.
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// All comments in source order.
    pub comments: Vec<Comment>,
    /// Byte offsets where blank lines (2+ consecutive newlines) occur.
    pub blank_line_offsets: Vec<usize>,
}

/// Extract comments and blank-line positions from source text.
///
/// This is a pre-scan pass that runs before lexing/parsing.
/// The lexer skips comments, so we need to extract them separately
/// to preserve them during formatting.
#[must_use]
pub fn extract_source_metadata(source: &str) -> SourceMetadata {
    let mut scanner = Scanner::new(source);
    scanner.scan();
    SourceMetadata {
        comments: scanner.comments,
        blank_line_offsets: scanner.blank_line_offsets,
    }
}

struct Scanner<'a> {
    bytes: &'a [u8],
    source: &'a str,
    pos: usize,
    comments: Vec<Comment>,
    blank_line_offsets: Vec<usize>,
}

impl<'a> Scanner<'a> {
    const fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            source,
            pos: 0,
            comments: Vec::new(),
            blank_line_offsets: Vec::new(),
        }
    }

    fn scan(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'\n' => self.scan_newline(),
                b'"' => self.skip_string_literal(),
                b'/' if self.next_byte() == Some(b'/') => self.scan_comment(),
                _ => self.pos += 1,
            }
        }
    }

    fn next_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos + 1).copied()
    }

    /// Record the offset if a blank line follows (two newlines with only
    /// whitespace between them), then advance past the newline.
    fn scan_newline(&mut self) {
        let mut j = self.pos + 1;
        while j < self.bytes.len() && matches!(self.bytes[j], b' ' | b'\t') {
            j += 1;
        }
        if j < self.bytes.len() && self.bytes[j] == b'\n' {
            self.blank_line_offsets.push(self.pos);
        }
        self.pos += 1;
    }

    /// Skip past a string literal to avoid false-positive `//` matches.
    fn skip_string_literal(&mut self) {
        self.pos += 1; // skip opening quote
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
            self.pos += 1;
        }
        if self.pos < self.bytes.len() {
            self.pos += 1; // skip closing quote
        }
    }

    /// Extract a `//` or `///` comment and advance to the end of the line.
    fn scan_comment(&mut self) {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let kind = if text.starts_with("///") && !text.starts_with("////") {
            CommentKind::Doc
        } else {
            CommentKind::Line
        };
        self.comments.push(Comment {
            kind,
            text: text.to_string(),
            span: Span::new(start, self.pos - start),
        });
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
    fn extract_line_comment() {
        let source = "// hello world\nparam x = 1;";
        let meta = extract_source_metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].kind, CommentKind::Line);
        assert_eq!(meta.comments[0].text, "// hello world");
        assert_eq!(meta.comments[0].span.offset, 0);
    }

    #[test]
    fn extract_doc_comment() {
        let source = "/// doc comment\nparam x = 1;";
        let meta = extract_source_metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].kind, CommentKind::Doc);
        assert_eq!(meta.comments[0].text, "/// doc comment");
    }

    #[test]
    fn extract_inline_comment() {
        let source = "param x = 1; // inline";
        let meta = extract_source_metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].text, "// inline");
    }

    #[test]
    fn no_false_positive_in_string() {
        let source = r#"use "//not-a-comment.gcl" { x };"#;
        let meta = extract_source_metadata(source);
        assert_eq!(meta.comments.len(), 0);
    }

    #[test]
    fn extract_blank_lines() {
        let source = "param x = 1;\n\nparam y = 2;";
        let meta = extract_source_metadata(source);
        assert_eq!(meta.blank_line_offsets.len(), 1);
    }

    #[test]
    fn multiple_comments() {
        let source = "// first\n// second\nparam x = 1;";
        let meta = extract_source_metadata(source);
        assert_eq!(meta.comments.len(), 2);
        assert_eq!(meta.comments[0].text, "// first");
        assert_eq!(meta.comments[1].text, "// second");
    }
}
