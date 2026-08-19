//! Attach `///` doc comments to the declarations they document.
//!
//! The lexer records every comment in [`SourceMetadata`] without yielding it
//! to the parser. This pass runs after a file parse and attaches each doc
//! block — a contiguous run of own-line `///` comments immediately preceding
//! a declaration, with no blank line or line comment in between — to that
//! declaration's [`Declaration::doc`] field.

use crate::syntax::ast::{DeclKind, Declaration, File};
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
        declaration.doc = doc_block_before(declaration.span.offset(), source, comments);
        if let DeclKind::Dag(dag) = &mut declaration.kind {
            attach_to_declarations(&mut dag.body, source, comments);
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
mod tests {
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> crate::syntax::ast::File {
        Parser::new(source).parse_file().unwrap()
    }

    fn doc_of(file: &crate::syntax::ast::File, index: usize) -> Option<&str> {
        file.declarations[index]
            .doc
            .as_ref()
            .map(crate::syntax::comments::DocComment::text)
    }

    #[test]
    fn doc_comment_attaches_to_following_declaration() {
        let file = parse("/// The mass budget.\nparam x: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), Some("The mass budget."));
    }

    #[test]
    fn multi_line_doc_block_joins_lines() {
        let file = parse("/// First line.\n/// Second line.\nnode y: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), Some("First line.\nSecond line."));
    }

    #[test]
    fn blank_line_detaches_doc_comment() {
        let file = parse("/// Dangling.\n\nparam x: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), None);
    }

    #[test]
    fn line_comment_is_not_documentation() {
        let file = parse("// plain comment\nparam x: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), None);
    }

    #[test]
    fn four_slashes_is_not_documentation() {
        let file = parse("//// separator\nparam x: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), None);
    }

    #[test]
    fn line_comment_breaks_a_doc_run() {
        let file = parse("/// Lost.\n// interrupt\nparam x: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), None);
    }

    #[test]
    fn trailing_comment_of_previous_declaration_does_not_attach() {
        let file =
            parse("param x: Dimensionless = 1.0; /// trailing\nparam y: Dimensionless = 2.0;");
        assert_eq!(doc_of(&file, 0), None);
        assert_eq!(doc_of(&file, 1), None);
    }

    #[test]
    fn doc_attaches_through_attributes() {
        let file = parse(
            "/// Hidden helper plot.\n#[hidden]\nplot p = { mark: line, encode: { x: 1.0, y: 1.0 } };",
        );
        assert_eq!(doc_of(&file, 0), Some("Hidden helper plot."));
    }

    #[test]
    fn each_declaration_gets_its_own_doc() {
        let file = parse(
            "/// First.\nparam x: Dimensionless = 1.0;\n/// Second.\nparam y: Dimensionless = 2.0;",
        );
        assert_eq!(doc_of(&file, 0), Some("First."));
        assert_eq!(doc_of(&file, 1), Some("Second."));
    }

    #[test]
    fn docs_attach_inside_dag_bodies() {
        let file = parse("dag d {\n    /// Inner doc.\n    param x: Dimensionless = 1.0;\n}\n");
        let crate::syntax::ast::DeclKind::Dag(dag) = &file.declarations[0].kind else {
            panic!("expected dag declaration");
        };
        assert_eq!(
            dag.body[0]
                .doc
                .as_ref()
                .map(crate::syntax::comments::DocComment::text),
            Some("Inner doc.")
        );
    }

    #[test]
    fn doc_span_covers_the_whole_block() {
        let source = "/// a\n/// b\nparam x: Dimensionless = 1.0;";
        let file = parse(source);
        let doc = file.declarations[0].doc.as_ref().unwrap();
        assert_eq!(doc.span().offset(), 0);
        assert_eq!(doc.span().len(), "/// a\n/// b".len());
    }

    #[test]
    fn indented_doc_comment_still_attaches() {
        let file = parse("    /// Indented.\n    param x: Dimensionless = 1.0;");
        assert_eq!(doc_of(&file, 0), Some("Indented."));
    }

    #[test]
    fn crlf_line_endings_attach() {
        let file = parse("/// Windows.\r\nparam x: Dimensionless = 1.0;\r\n");
        assert_eq!(doc_of(&file, 0), Some("Windows."));
    }
}
