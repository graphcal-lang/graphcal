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
    const fn lexeme(self) -> &'static str {
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

    /// Whether this is a `///` doc comment (`////` is a line comment).
    #[must_use]
    pub(crate) const fn is_doc(&self) -> bool {
        matches!(self.delimiter, CommentDelimiter::Doc)
    }

    /// The text after the comment delimiter, excluding the line ending.
    #[must_use]
    pub(crate) fn body_text(&self) -> &str {
        self.body.as_str()
    }
}

/// A comment paired with its source span.
pub type SpannedComment = Spanned<Comment>;

/// One documentation block: the contiguous `///` lines immediately preceding
/// a declaration, attached to it by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocComment {
    text: String,
    span: Span,
}

impl DocComment {
    #[must_use]
    pub(crate) const fn new(text: String, span: Span) -> Self {
        Self { text, span }
    }

    /// The documentation text: one line per `///` line, with the delimiter
    /// and at most one leading space removed, joined with `\n`.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Source span covering every line of the block.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

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
mod tests;
