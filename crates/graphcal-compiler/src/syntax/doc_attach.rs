//! Attach `///` doc comments to the declarations they document.
//!
//! The lexer records every comment in [`SourceMetadata`] without yielding it
//! to the parser. This pass runs after a file parse and attaches each doc
//! block — a contiguous run of own-line `///` comments immediately preceding
//! a declaration, with no blank line or line comment in between — to that
//! declaration's [`Declaration::doc`] field (or the individual multi-decl
//! slot's doc field).

use crate::syntax::ast::{DeclKind, Declaration, File, RawDeclSugar};
use crate::syntax::comments::{DocComment, SourceMetadata, SpannedComment};
use crate::syntax::span::Span;

/// Attach every doc block to its following declaration, recursing into
/// `dag` bodies.
pub fn attach_doc_comments(file: &mut File, source: &str, metadata: &SourceMetadata) {
    attach_to_declarations(&mut file.declarations, source, metadata.comments());
}

fn attach_to_declarations(
    declarations: &mut [Declaration],
    source: &str,
    comments: &[SpannedComment],
) {
    for declaration in declarations {
        match &mut declaration.kind {
            DeclKind::Sugar(RawDeclSugar::Multi(multi)) => {
                multi.attach_slot_docs(|span| doc_block_before(span.offset(), source, comments));
            }
            kind => {
                declaration.doc = doc_block_before(declaration.span.offset(), source, comments);
                if let DeclKind::Dag(dag) = kind {
                    attach_to_declarations(&mut dag.body, source, comments);
                }
            }
        }
    }
}

/// The doc block ending immediately before `decl_offset`, when one exists.
///
/// A block is the maximal backwards run of comments where every member is a
/// `///` comment on its own line and every gap (member to member, and last
/// member to the declaration) is whitespace containing exactly one line
/// break. `decl_offset` is the declaration start including attributes, so
/// `/// doc` above `#[hidden]` attaches through the attribute.
fn doc_block_before(
    decl_offset: usize,
    source: &str,
    comments: &[SpannedComment],
) -> Option<DocComment> {
    // Comments are recorded in source order, so every candidate ends before
    // the declaration starts.
    let before = comments.partition_point(|comment| span_end(comment.span) <= decl_offset);
    let mut anchor = decl_offset;
    let mut run_start = before;
    for index in (0..before).rev() {
        let comment = &comments[index];
        if !comment.value.is_doc()
            || !starts_own_line(source, comment.span.offset())
            || !is_adjacent_gap(source, span_end(comment.span), anchor)
        {
            break;
        }
        anchor = comment.span.offset();
        run_start = index;
    }
    let run = comments
        .get(run_start..before)
        .filter(|run| !run.is_empty())?;
    let text = run
        .iter()
        .map(|comment| {
            let body = comment.value.body_text();
            body.strip_prefix(' ').unwrap_or(body)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let first = run.first()?;
    let last = run.last()?;
    let span = Span::new(
        first.span.offset(),
        span_end(last.span) - first.span.offset(),
    );
    Some(DocComment::new(text, span))
}

const fn span_end(span: Span) -> usize {
    span.offset() + span.len()
}

/// Whether only whitespace precedes `offset` on its line.
fn starts_own_line(source: &str, offset: usize) -> bool {
    source[..offset]
        .chars()
        .rev()
        .take_while(|&c| c != '\n' && c != '\r')
        .all(char::is_whitespace)
}

/// Whether the gap between a comment end and the next anchor is whitespace
/// containing exactly one line break (i.e. no blank line and no other token).
fn is_adjacent_gap(source: &str, gap_start: usize, gap_end: usize) -> bool {
    let gap = &source[gap_start..gap_end];
    gap.chars().all(char::is_whitespace) && line_break_count(gap) == 1
}

/// Count line breaks, treating `\r\n` as one break and lone `\r` as a break.
fn line_break_count(gap: &str) -> usize {
    let mut count = 0;
    let mut chars = gap.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => count += 1,
            '\r' => {
                count += 1;
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            _ => {}
        }
    }
    count
}

#[cfg(test)]
mod tests;
