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
fn comments_end_before_lf_cr_and_crlf() {
    for line_ending in ["\n", "\r", "\r\n"] {
        let source = format!("// first{line_ending}// second{line_ending}param x = 1;");
        let meta = metadata(&source);
        assert_eq!(meta.comments.len(), 2, "line ending {line_ending:?}");
        assert_eq!(meta.comments[0].value.lexeme(), "// first");
        assert_eq!(meta.comments[0].span, Span::new(0, 8));
        assert_eq!(meta.comments[1].value.lexeme(), "// second");
    }
}

#[test]
fn multiple_comments() {
    let source = "// first\n// second\nparam x = 1;";
    let meta = metadata(source);
    assert_eq!(meta.comments.len(), 2);
    assert_eq!(meta.comments[0].value.lexeme(), "// first");
    assert_eq!(meta.comments[1].value.lexeme(), "// second");
}
