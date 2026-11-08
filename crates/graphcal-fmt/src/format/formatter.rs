//! The formatter's comment cursor over one source file.

use graphcal_compiler::source_line::line_endings;
use graphcal_compiler::syntax::comments::SourceMetadata;
use graphcal_compiler::syntax::span::Span;
use pretty::RcDoc;

use super::doc::text_with_hardlines;

/// Comments drained in one run, with the end offset of the last one.
struct CommentRun {
    doc: RcDoc<'static>,
    end: usize,
}

/// State for tracking comments during formatting.
pub struct Formatter<'src> {
    source: &'src str,
    metadata: &'src SourceMetadata,
    next_comment: usize,
    /// Set by [`Self::request_verbatim`] while a region is being formatted;
    /// consumed by the enclosing [`Self::format_or_verbatim`].
    verbatim_requested: bool,
    /// End offset of the innermost construct being formatted (see
    /// [`Self::within`]): a trailing comment drained inside it must start
    /// before this offset.
    construct_end: usize,
}

impl<'src> Formatter<'src> {
    pub const fn new(source: &'src str, metadata: &'src SourceMetadata) -> Self {
        Self {
            source,
            metadata,
            next_comment: 0,
            verbatim_requested: false,
            construct_end: usize::MAX,
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
    /// A blank line between two drained comments is kept: it separates a
    /// detached `///` block from the doc block that follows it, so removing
    /// it would merge the two.
    ///
    /// Returns `None` when there are no comments to emit, so callers can
    /// avoid rendering a known-empty doc just to check for emptiness.
    pub fn drain_comments_before(&mut self, before_offset: usize) -> Option<RcDoc<'static>> {
        self.drain_comment_run_before(before_offset)
            .map(|run| run.doc)
    }

    /// Drain the comments leading a declaration (or a multi-declaration
    /// slot) that starts at `declaration_offset`.
    ///
    /// Like [`Self::drain_comments_before`], and additionally keeps a blank
    /// line between the last comment and the declaration: that blank line
    /// decides whether a `///` block documents the declaration, so removing
    /// it would attach a detached block to the declaration.
    pub fn drain_comments_before_declaration(
        &mut self,
        declaration_offset: usize,
    ) -> Option<RcDoc<'static>> {
        self.drain_comment_run_before(declaration_offset)
            .map(|run| {
                if self.has_blank_line_between(run.end, declaration_offset) {
                    run.doc.append(RcDoc::hardline())
                } else {
                    run.doc
                }
            })
    }

    fn drain_comment_run_before(&mut self, before_offset: usize) -> Option<CommentRun> {
        let mut run: Option<CommentRun> = None;
        while let Some(comment) = self.metadata.comments().get(self.next_comment) {
            if comment.span.offset() >= before_offset {
                break;
            }
            let text = RcDoc::text(comment.value.lexeme()).append(RcDoc::hardline());
            let end = comment.span.offset() + comment.span.len();
            run = Some(match run {
                None => CommentRun { doc: text, end },
                Some(previous) => {
                    let separator =
                        if self.has_blank_line_between(previous.end, comment.span.offset()) {
                            RcDoc::hardline()
                        } else {
                            RcDoc::nil()
                        };
                    CommentRun {
                        doc: previous.doc.append(separator).append(text),
                        end,
                    }
                }
            });
            self.next_comment += 1;
        }
        run
    }

    /// Drain a trailing comment on the same line as `line_end_offset`.
    /// Returns the comment text (with leading space) or `None` when there
    /// isn't one.
    ///
    /// Only a comment inside the innermost construct being formatted (see
    /// [`Self::within`]) qualifies: in `f(x); // note` the comment follows the
    /// call, not its argument `x`, so the argument must not take it.
    pub fn drain_trailing_comment(&mut self, line_end_offset: usize) -> Option<RcDoc<'static>> {
        let comment = self.metadata.comments().get(self.next_comment)?;
        // A trailing comment must be on the same line — its offset must be
        // between the end of the node and the next line ending (LF, CRLF, or
        // lone CR). The boundary is inclusive: a comment starting exactly at
        // `line_end_offset` (no whitespace between the node and `//`) is still
        // trailing.
        if comment.span.offset() >= line_end_offset && comment.span.offset() < self.construct_end {
            let between =
                &self.source[line_end_offset..comment.span.offset().min(self.source.len())];
            if line_endings(between).next().is_none() {
                self.next_comment += 1;
                return Some(RcDoc::text(format!(" {}", comment.value.lexeme())));
            }
        }
        None
    }

    /// Check if there's a blank line in the source between two byte offsets.
    pub fn has_blank_line_between(&self, start: usize, end: usize) -> bool {
        self.metadata.has_blank_line_starting_in(start..end)
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

    /// Run `render` on a speculative formatter, used for pre-rendering
    /// aligned table cells.
    ///
    /// Cell width pre-rendering must not consume row/slice comments from the
    /// real formatter cursor. Comments before the cell value are skipped in the
    /// fork so row-level drains can decide where they belong; comments inside
    /// the cell remain undrained in the real formatter and trigger the usual
    /// declaration-level verbatim fallback. The fork's comment cursor is
    /// discarded, but a verbatim request made while rendering is carried back
    /// to `self`: the fork only exists inside this call, so no request can be
    /// lost with it.
    pub fn with_fork_skipping_comments_before<T>(
        &mut self,
        offset: usize,
        render: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let mut next_comment = self.next_comment;
        while self
            .metadata
            .comments()
            .get(next_comment)
            .is_some_and(|comment| comment.span.offset() < offset)
        {
            next_comment += 1;
        }
        let mut fork = Self {
            source: self.source,
            metadata: self.metadata,
            next_comment,
            verbatim_requested: false,
            construct_end: self.construct_end,
        };
        let rendered = render(&mut fork);
        self.verbatim_requested |= fork.verbatim_requested;
        rendered
    }

    /// Report that the region being formatted has a shape the formatter
    /// cannot reproduce faithfully (for example a table with duplicate
    /// labels), so the enclosing [`Self::format_or_verbatim`] must emit its
    /// original source instead of the formatted doc.
    pub const fn request_verbatim(&mut self) {
        self.verbatim_requested = true;
    }

    /// Run `render` with `span` as the innermost construct being formatted,
    /// so trailing-comment drains inside it cannot take a comment that
    /// follows the construct.
    pub fn within<T>(&mut self, span: Span, render: impl FnOnce(&mut Self) -> T) -> T {
        let end = (span.offset() + span.len()).min(self.construct_end);
        let outer = std::mem::replace(&mut self.construct_end, end);
        let rendered = render(self);
        self.construct_end = outer;
        rendered
    }

    /// Format the source region `span` with `format`, falling back to the
    /// region's original text when `format` leaves a comment inside `span`
    /// undrained or calls [`Self::request_verbatim`].
    ///
    /// Leftover comments would silently migrate to the next declaration (or
    /// vanish, for multi-decl sugar), so the formatted doc is discarded and the
    /// source is emitted verbatim instead. The formatter state is the comment
    /// cursor and the verbatim request, so the rollback is a cursor reset.
    pub fn format_or_verbatim(
        &mut self,
        span: Span,
        format: impl FnOnce(&mut Self) -> RcDoc<'static>,
    ) -> RcDoc<'static> {
        let end = span.offset() + span.len();
        let comment_snapshot = self.next_comment;
        let outer_request = std::mem::replace(&mut self.verbatim_requested, false);
        let formatted = self.within(span, format);
        let verbatim_requested = std::mem::replace(&mut self.verbatim_requested, outer_request);
        if verbatim_requested || self.has_comment_before(end) {
            self.next_comment = comment_snapshot;
            self.skip_comments_before(end);
            text_with_hardlines(self.slice(span))
        } else {
            formatted
        }
    }
}
