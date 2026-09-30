//! The formatter's comment cursor over one source file.

use graphcal_compiler::syntax::comments::SourceMetadata;
use graphcal_compiler::syntax::span::Span;
use pretty::RcDoc;

use super::doc::text_with_hardlines;

/// State for tracking comments during formatting.
pub struct Formatter<'src> {
    source: &'src str,
    metadata: &'src SourceMetadata,
    next_comment: usize,
}

impl<'src> Formatter<'src> {
    pub const fn new(source: &'src str, metadata: &'src SourceMetadata) -> Self {
        Self {
            source,
            metadata,
            next_comment: 0,
        }
    }

    /// Get the original source text for a span.
    pub fn slice(&self, span: Span) -> &'src str {
        self.source_range(span.offset()..span.offset() + span.len())
    }

    /// Get the original source text for a byte range.
    pub fn source_range(&self, range: std::ops::Range<usize>) -> &'src str {
        &self.source[range]
    }

    /// Drain all comments whose span starts before `before_offset`,
    /// returning them as a Doc with hardlines.
    ///
    /// Returns `None` when there are no comments to emit, so callers can
    /// avoid rendering a known-empty doc just to check for emptiness.
    pub fn drain_comments_before(&mut self, before_offset: usize) -> Option<RcDoc<'static>> {
        let mut docs: Vec<RcDoc<'static>> = Vec::new();
        while self.next_comment < self.metadata.comments().len() {
            let comment = &self.metadata.comments()[self.next_comment];
            if comment.span.offset() >= before_offset {
                break;
            }
            docs.push(RcDoc::text(comment.value.lexeme()));
            docs.push(RcDoc::hardline());
            self.next_comment += 1;
        }
        if docs.is_empty() {
            None
        } else {
            Some(RcDoc::concat(docs))
        }
    }

    /// Drain a trailing comment on the same line as `line_end_offset`.
    /// Returns the comment text (with leading space) or `None` when there
    /// isn't one.
    pub fn drain_trailing_comment(&mut self, line_end_offset: usize) -> Option<RcDoc<'static>> {
        if self.next_comment >= self.metadata.comments().len() {
            return None;
        }
        let comment = &self.metadata.comments()[self.next_comment];
        // A trailing comment must be on the same line — its offset must be
        // between the end of the node and the next newline. The boundary is
        // inclusive: a comment starting exactly at `line_end_offset` (no
        // whitespace between the node and `//`) is still trailing.
        if comment.span.offset() >= line_end_offset {
            let between =
                &self.source[line_end_offset..comment.span.offset().min(self.source.len())];
            if !between.contains('\n') {
                self.next_comment += 1;
                return Some(RcDoc::text(format!(" {}", comment.value.lexeme())));
            }
        }
        None
    }

    /// Check if there's a blank line in the source between two byte offsets.
    pub fn has_blank_line_between(&self, start: usize, end: usize) -> bool {
        self.metadata.blank_lines().iter().any(|blank_line| {
            blank_line.span().offset() >= start && blank_line.span().offset() < end
        })
    }

    /// Returns `true` if the next undrained comment starts before `offset`.
    pub fn has_comment_before(&self, offset: usize) -> bool {
        self.metadata
            .comments()
            .get(self.next_comment)
            .is_some_and(|c| c.span.offset() < offset)
    }

    /// Advance past all comments that start before `offset` without
    /// emitting them (used when the surrounding source is emitted verbatim,
    /// so the comments are already part of the output).
    fn skip_comments_before(&mut self, offset: usize) {
        while self.has_comment_before(offset) {
            self.next_comment += 1;
        }
    }

    /// Make a speculative formatter for pre-rendering aligned table cells.
    ///
    /// Cell width pre-rendering must not consume row/slice comments from the
    /// real formatter cursor. Comments before the cell value are skipped in the
    /// fork so row-level drains can decide where they belong; comments inside
    /// the cell remain undrained in the real formatter and trigger the usual
    /// declaration-level verbatim fallback.
    pub fn fork_skipping_comments_before(&self, offset: usize) -> Self {
        let mut next_comment = self.next_comment;
        while self
            .metadata
            .comments()
            .get(next_comment)
            .is_some_and(|comment| comment.span.offset() < offset)
        {
            next_comment += 1;
        }
        Self {
            source: self.source,
            metadata: self.metadata,
            next_comment,
        }
    }

    /// Format the source region `span` with `format`, falling back to the
    /// region's original text when `format` leaves a comment inside `span`
    /// undrained.
    ///
    /// Leftover comments would silently migrate to the next declaration (or
    /// vanish, for multi-decl sugar), so the formatted doc is discarded and the
    /// source is emitted verbatim instead. The only formatter state is the
    /// comment cursor, so the rollback is a cursor reset.
    pub fn format_or_verbatim(
        &mut self,
        span: Span,
        format: impl FnOnce(&mut Self) -> RcDoc<'static>,
    ) -> RcDoc<'static> {
        let end = span.offset() + span.len();
        let comment_snapshot = self.next_comment;
        let formatted = format(self);
        if self.has_comment_before(end) {
            self.next_comment = comment_snapshot;
            self.skip_comments_before(end);
            text_with_hardlines(self.slice(span))
        } else {
            formatted
        }
    }
}
