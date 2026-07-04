mod decl;
mod expr;
mod type_expr;

use graphcal_compiler::syntax::ast::File;
use graphcal_compiler::syntax::comments::SourceMetadata;
use graphcal_compiler::syntax::span::Span;
use pretty::RcDoc;

// Re-export the public entry point
pub use decl::format_decl;
pub use expr::format_expr;
pub use type_expr::{format_dim_expr_inline, format_type_expr_inline, format_unit_expr_inline};

/// Indentation step used everywhere in formatter docs. Typed as `isize`
/// because `pretty::RcDoc::nest` takes `isize`; the few `repeat` sites
/// cast through `usize` explicitly.
///
/// `pub` here is module-scoped: the parent module is private, so this is
/// effectively crate-internal.
pub const INDENT: isize = 4;

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn pad_left_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{}{}", " ".repeat(padding), text)
}

fn pad_right_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{}{}", text, " ".repeat(padding))
}

fn text_with_hardlines(text: &str) -> RcDoc<'static> {
    RcDoc::intersperse(
        text.split('\n').map(|line| {
            if line.is_empty() {
                RcDoc::nil()
            } else {
                RcDoc::text(line.to_string())
            }
        }),
        RcDoc::hardline(),
    )
}

/// State for tracking comments during formatting.
///
/// `pub` is module-scoped (parent module is private).
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
        &self.source[span.offset()..span.offset() + span.len()]
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
    fn has_comment_before(&self, offset: usize) -> bool {
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
}

/// Format a declaration list (file body or `dag` body) into doc parts.
///
/// Handles leading comments, blank-line preservation, trailing comments, and
/// the comment-preservation fallback: when a declaration contains comments
/// in positions the formatter does not drain (multi-decl bodies, `if`
/// branches, `scan`/`unfold` lambdas, …), reformatting would silently drop
/// or relocate them. In that case the declaration's original source is
/// emitted verbatim — a formatter must never lose user content.
fn format_decl_sequence(
    fmt: &mut Formatter<'_>,
    declarations: &[graphcal_compiler::syntax::ast::Declaration],
) -> Vec<RcDoc<'static>> {
    let mut docs: Vec<RcDoc<'static>> = Vec::new();
    let mut prev_end: usize = 0;
    for (i, decl) in declarations.iter().enumerate() {
        let emit_start = decl.span.offset();
        let emit_end = emit_start + decl.span.len();

        // Emit leading comments before this declaration
        let leading = fmt.drain_comments_before(emit_start);
        let has_leading_comments = leading.is_some();

        if i > 0 {
            docs.push(RcDoc::hardline());
            // Extra blank line before comments or when original had a blank line
            if has_leading_comments || fmt.has_blank_line_between(prev_end, emit_start) {
                docs.push(RcDoc::hardline());
            }
        }
        if let Some(leading) = leading {
            docs.push(leading);
        }

        // Format, then verify every comment inside the span was drained by
        // some drain point. Leftovers would silently migrate to the next
        // declaration (or vanish, for multi-decl sugar) — roll back and emit
        // the original source verbatim instead. The only formatter state is
        // the comment cursor, so the rollback is a cursor reset.
        let comment_snapshot = fmt.next_comment;
        let formatted = format_decl(fmt, decl);
        let decl_doc = if fmt.has_comment_before(emit_end) {
            fmt.next_comment = comment_snapshot;
            fmt.skip_comments_before(emit_end);
            text_with_hardlines(fmt.slice(decl.span))
        } else {
            formatted
        };
        let trailing = fmt
            .drain_trailing_comment(emit_end)
            .unwrap_or_else(RcDoc::nil);
        docs.push(decl_doc.append(trailing));
        prev_end = emit_end;
    }
    docs
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn format_file(file: &File, source: &str, metadata: &SourceMetadata) -> RcDoc<'static> {
    let mut fmt = Formatter::new(source, metadata);
    let mut docs = format_decl_sequence(&mut fmt, &file.declarations);

    // Drain any remaining comments at end of file. `drain_comments_before`
    // already appends a hardline after every emitted comment, so that hardline
    // is the final newline when trailing comments exist.
    let had_remaining = fmt
        .drain_comments_before(usize::MAX)
        .is_some_and(|remaining| {
            if !docs.is_empty() {
                docs.push(RcDoc::hardline());
            }
            docs.push(remaining);
            true
        });

    if !had_remaining {
        docs.push(RcDoc::hardline());
    }

    RcDoc::concat(docs)
}

/// Prepend leading comments before a doc. Returns the doc unchanged if
/// there are no comments. Like Gleam's `commented()` helper.
pub fn prepend_comments(leading: Option<RcDoc<'static>>, doc: RcDoc<'static>) -> RcDoc<'static> {
    match leading {
        Some(leading) => leading.append(doc),
        None => doc,
    }
}

/// Pretty-printer pattern: try `single` on one line, otherwise lay `multi`
/// out. Both branches are required because `pretty::RcDoc::group` cannot
/// switch between fundamentally different shapes (e.g., adding hardlines).
pub fn flat_alt_group(single: RcDoc<'static>, multi: RcDoc<'static>) -> RcDoc<'static> {
    multi.flat_alt(single).group()
}

/// Wrap a possibly-multiline child document in soft parentheses.
///
/// Delimited expression contexts must not append a child directly after the
/// opening delimiter (for example, `sum(` + `for ... { ... }`): if the child
/// later chooses a multiline layout, its internal hardlines inherit the wrong
/// indentation anchor. This combinator centralizes the safe layout invariant:
/// the body is preceded and followed by `line_()`, so it stays inline when the
/// whole group fits but moves to its own indented line whenever any nested
/// hardline or width break makes the group multiline.
pub fn soft_parenthesized(body: RcDoc<'static>) -> RcDoc<'static> {
    RcDoc::text("(")
        .append(RcDoc::line_().append(body).nest(INDENT))
        .append(RcDoc::line_())
        .append(RcDoc::text(")"))
        .group()
}

/// Format a comma-separated argument list in soft parentheses.
///
/// Empty argument lists are always emitted as the atomic token pair `()`.
/// Without this special case, a long callee can force `soft_parenthesized(nil)`
/// into the multiline layout and produce a visually empty parenthesis block:
/// `foo(\n\n)`. List formatters should use this helper instead of building
/// parenthesized comma lists by hand so empty delimiter pairs stay stable in
/// every surrounding layout.
pub fn soft_parenthesized_list(
    items: Vec<RcDoc<'static>>,
    trailing_comma_when_multiline: bool,
) -> RcDoc<'static> {
    if items.is_empty() {
        return RcDoc::text("()");
    }

    let body = RcDoc::intersperse(items, RcDoc::text(",").append(RcDoc::line()));
    let body = if trailing_comma_when_multiline {
        body.append(RcDoc::text(",").flat_alt(RcDoc::nil()))
    } else {
        body
    };

    soft_parenthesized(body)
}

/// Render an `RcDoc` to a string (for measuring column widths).
pub fn render_doc_to_string(doc: &RcDoc<'static>) -> String {
    let mut buf = Vec::new();
    // Use a large width so we get single-line rendering for cell values.
    let _ = doc.render(1000, &mut buf);
    // The doc is built from valid UTF-8 source slices and string literals,
    // so render output is always valid UTF-8 — panic loudly if that
    // invariant is ever violated rather than silently producing "".
    #[expect(
        clippy::expect_used,
        reason = "doc bytes are always valid UTF-8 by construction"
    )]
    String::from_utf8(buf).expect("rendered doc must be valid UTF-8")
}

#[cfg(test)]
mod tests {
    use pretty::RcDoc;

    use super::{display_width, pad_left_to_width, pad_right_to_width, text_with_hardlines};

    #[test]
    fn alignment_helpers_use_display_width_not_bytes() {
        assert_eq!(display_width("界"), 2);
        assert_eq!("界".len(), 3);
        assert_eq!(pad_left_to_width("界", 4), "  界");
        assert_eq!(pad_right_to_width("界", 4), "界  ");
    }

    #[test]
    fn multiline_text_uses_hardlines_for_nesting() {
        let doc = RcDoc::text("{")
            .append(
                RcDoc::hardline()
                    .append(text_with_hardlines("a\nb"))
                    .nest(4),
            )
            .append(RcDoc::hardline())
            .append(RcDoc::text("}"));
        let mut out = Vec::new();
        doc.render(80, &mut out).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered, "{\n    a\n    b\n}");
    }
}
