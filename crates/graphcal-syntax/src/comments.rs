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
    let mut comments = Vec::new();
    let mut blank_line_offsets = Vec::new();

    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut prev_was_newline = true; // treat start of file as after newline

    while i < len {
        let b = bytes[i];

        // Track blank lines: two consecutive newlines with only whitespace between
        if b == b'\n' {
            let after = i + 1;
            // Skip whitespace (spaces/tabs) after the newline
            let mut j = after;
            while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < len && bytes[j] == b'\n' {
                blank_line_offsets.push(i);
            }
            prev_was_newline = true;
            i += 1;
            continue;
        }

        // Skip inside string literals to avoid false-positive `//` matches
        if b == b'"' {
            i += 1;
            while i < len && bytes[i] != b'"' {
                i += 1;
            }
            if i < len {
                i += 1; // skip closing quote
            }
            prev_was_newline = false;
            continue;
        }

        // Detect `//` comment start
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            let start = i;
            // Find end of line
            let mut end = i;
            while end < len && bytes[end] != b'\n' {
                end += 1;
            }
            let text = &source[start..end];
            let kind = if text.starts_with("///") && !text.starts_with("////") {
                CommentKind::Doc
            } else {
                CommentKind::Line
            };
            comments.push(Comment {
                kind,
                text: text.to_string(),
                span: Span::new(start, end - start),
            });
            i = end;
            prev_was_newline = false;
            continue;
        }

        if b != b' ' && b != b'\t' && b != b'\r' {
            prev_was_newline = false;
        }
        i += 1;
    }

    // Suppress the unused variable warning — we track this for potential future use
    let _ = prev_was_newline;

    SourceMetadata {
        comments,
        blank_line_offsets,
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
        assert_eq!(meta.comments[0].span.offset(), 0);
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
