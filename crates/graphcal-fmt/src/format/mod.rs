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

pub const INDENT: isize = 4;

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
        &self.source[span.offset()..span.offset() + span.len()]
    }

    /// Drain all comments whose span starts before `before_offset`,
    /// returning them as a Doc with hardlines.
    ///
    /// Returns `None` when there are no comments to emit, so callers can
    /// avoid rendering a known-empty doc just to check for emptiness.
    pub fn drain_comments_before(&mut self, before_offset: usize) -> Option<RcDoc<'static>> {
        let mut docs: Vec<RcDoc<'static>> = Vec::new();
        while self.next_comment < self.metadata.comments.len() {
            let comment = &self.metadata.comments[self.next_comment];
            if comment.span.offset() >= before_offset {
                break;
            }
            docs.push(RcDoc::text(comment.text.clone()));
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
        if self.next_comment >= self.metadata.comments.len() {
            return None;
        }
        let comment = &self.metadata.comments[self.next_comment];
        // A trailing comment must be on the same line — its offset must be
        // between the end of the node and the next newline. The boundary is
        // inclusive: a comment starting exactly at `line_end_offset` (no
        // whitespace between the node and `//`) is still trailing.
        if comment.span.offset() >= line_end_offset {
            let between =
                &self.source[line_end_offset..comment.span.offset().min(self.source.len())];
            if !between.contains('\n') {
                self.next_comment += 1;
                return Some(RcDoc::text(format!(" {}", comment.text)));
            }
        }
        None
    }

    /// Check if there's a blank line in the source between two byte offsets.
    pub fn has_blank_line_between(&self, start: usize, end: usize) -> bool {
        self.metadata
            .blank_line_offsets
            .iter()
            .any(|&offset| offset >= start && offset < end)
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn format_file(file: &File, source: &str, metadata: &SourceMetadata) -> RcDoc<'static> {
    let mut fmt = Formatter::new(source, metadata);
    let mut docs: Vec<RcDoc<'static>> = Vec::new();

    let mut prev_end: usize = 0;
    for (i, decl) in file.declarations.iter().enumerate() {
        // Emit leading comments before this declaration
        let leading = fmt.drain_comments_before(decl.span.offset());
        let has_leading_comments = leading.is_some();

        if i > 0 {
            docs.push(RcDoc::hardline());
            // Extra blank line before comments or when original had a blank line
            if has_leading_comments || fmt.has_blank_line_between(prev_end, decl.span.offset()) {
                docs.push(RcDoc::hardline());
            }
        }
        if let Some(leading) = leading {
            docs.push(leading);
        }

        let decl_doc = format_decl(&mut fmt, decl);
        let decl_end = decl.span.offset() + decl.span.len();
        let trailing = fmt
            .drain_trailing_comment(decl_end)
            .unwrap_or_else(RcDoc::nil);
        docs.push(decl_doc.append(trailing));
        prev_end = decl_end;
    }

    // Drain any remaining comments at end of file
    if let Some(remaining) = fmt.drain_comments_before(usize::MAX) {
        docs.push(RcDoc::hardline());
        docs.push(remaining);
    }

    // Final newline
    docs.push(RcDoc::hardline());

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

/// Render an `RcDoc` to a string (for measuring column widths).
pub fn render_doc_to_string(doc: &RcDoc<'static>) -> String {
    let mut buf = Vec::new();
    // Use a large width so we get single-line rendering for cell values.
    let _ = doc.render(1000, &mut buf);
    String::from_utf8(buf).unwrap_or_default()
}
