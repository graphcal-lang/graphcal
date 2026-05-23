use crate::syntax::span::{Span, Spanned};

/// The delimiter that starts a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentDelimiter {
    /// `// ...`
    Line,
    /// `/// ...`
    Doc,
}

impl CommentDelimiter {
    #[must_use]
    pub(crate) const fn lexeme(self) -> &'static str {
        match self {
            Self::Line => "//",
            Self::Doc => "///",
        }
    }

    #[must_use]
    pub(crate) const fn len(self) -> usize {
        self.lexeme().len()
    }
}

/// The text after a comment delimiter, excluding the line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommentBody(String);

impl CommentBody {
    #[must_use]
    pub(crate) fn new(body: impl Into<String>) -> Self {
        Self(body.into())
    }

    #[must_use]
    fn as_str(&self) -> &str {
        &self.0
    }
}

/// A comment extracted from source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    delimiter: CommentDelimiter,
    body: CommentBody,
}

impl Comment {
    #[must_use]
    pub(crate) const fn new(delimiter: CommentDelimiter, body: CommentBody) -> Self {
        Self { delimiter, body }
    }

    /// Reconstruct the source lexeme without the trailing line ending.
    #[must_use]
    pub fn lexeme(&self) -> String {
        format!("{}{}", self.delimiter.lexeme(), self.body.as_str())
    }
}

/// A comment paired with its source span.
pub type SpannedComment = Spanned<Comment>;

/// A blank line represented by the span from the previous line ending through
/// the line ending that completes the blank line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlankLine {
    span: Span,
}

impl BlankLine {
    #[must_use]
    pub(crate) const fn new(span: Span) -> Self {
        Self { span }
    }

    #[must_use]
    pub const fn span(self) -> Span {
        self.span
    }
}

/// Metadata extracted from source text for the formatter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceMetadata {
    /// All comments in source order.
    comments: Vec<SpannedComment>,
    /// Blank-line separators in source order.
    blank_lines: Vec<BlankLine>,
}

impl SourceMetadata {
    #[must_use]
    pub fn comments(&self) -> &[SpannedComment] {
        &self.comments
    }

    #[must_use]
    pub fn blank_lines(&self) -> &[BlankLine] {
        &self.blank_lines
    }

    pub(crate) fn push_comment(&mut self, comment: SpannedComment) {
        self.comments.push(comment);
    }

    pub(crate) fn push_blank_line(&mut self, blank_line: BlankLine) {
        self.blank_lines.push(blank_line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(source: &str) -> SourceMetadata {
        let mut lexer = crate::syntax::lexer::Lexer::new(source);
        while lexer.next_token().is_some() {}
        assert_eq!(lexer.first_error_span(), None);
        lexer.into_source_metadata()
    }

    #[test]
    fn extract_line_comment() {
        let source = "// hello world\nparam x = 1;";
        let meta = metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].value.lexeme(), "// hello world");
        assert_eq!(meta.comments[0].span.offset(), 0);
    }

    #[test]
    fn extract_doc_comment() {
        let source = "/// doc comment\nparam x = 1;";
        let meta = metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].value.lexeme(), "/// doc comment");
    }

    #[test]
    fn four_slashes_is_a_line_comment() {
        let source = "//// not doc\nparam x = 1;";
        let meta = metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].value.lexeme(), "//// not doc");
    }

    #[test]
    fn extract_inline_comment() {
        let source = "param x = 1; // inline";
        let meta = metadata(source);
        assert_eq!(meta.comments.len(), 1);
        assert_eq!(meta.comments[0].value.lexeme(), "// inline");
    }

    #[test]
    fn no_false_positive_in_string() {
        let source = r#"import "//not-a-comment.gcl" { x };"#;
        let meta = metadata(source);
        assert_eq!(meta.comments.len(), 0);
    }

    #[test]
    fn records_lexer_error_for_unrecognized_token() {
        let source = r#"import "//not-a-comment.gcl"#;
        let mut lexer = crate::syntax::lexer::Lexer::new(source);
        while lexer.next_token().is_some() {}
        assert_eq!(
            lexer.first_error_span(),
            Some(Span::new(7, source.len() - 7))
        );
    }

    #[test]
    fn extract_blank_lines() {
        let source = "param x = 1;\n\nparam y = 2;";
        let meta = metadata(source);
        assert_eq!(meta.blank_lines.len(), 1);
        assert_eq!(meta.blank_lines[0].span(), Span::new(12, 2));
    }

    #[test]
    fn extract_blank_lines_with_crlf() {
        let source = "param x = 1;\r\n\t\r\nparam y = 2;";
        let meta = metadata(source);
        assert_eq!(meta.blank_lines.len(), 1);
        assert_eq!(meta.blank_lines[0].span(), Span::new(12, 5));
    }

    #[test]
    fn crlf_comment_excludes_line_ending_from_body() {
        let source = "// first\r\nparam x = 1;";
        let meta = metadata(source);
        assert_eq!(meta.comments[0].value.lexeme(), "// first");
        assert_eq!(meta.comments[0].span, Span::new(0, 8));
    }

    #[test]
    fn multiple_comments() {
        let source = "// first\n// second\nparam x = 1;";
        let meta = metadata(source);
        assert_eq!(meta.comments.len(), 2);
        assert_eq!(meta.comments[0].value.lexeme(), "// first");
        assert_eq!(meta.comments[1].value.lexeme(), "// second");
    }
}
