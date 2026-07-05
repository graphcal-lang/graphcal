//! Token-level cursor context detection for Signature Help and Completion.
//!
//! Since the Graphcal parser has no error recovery, the AST may not exist when
//! the user is mid-keystroke. This module tokenizes raw source text and scans
//! backward from the cursor to determine context.

use graphcal_compiler::syntax::lexer::Lexer;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::token::Token;

/// Context for a cursor inside a function call's argument list.
pub struct FnCallContext {
    /// The function name being called.
    pub fn_name: String,
    /// 0-based index of the parameter the cursor is currently on.
    pub active_param: usize,
}

/// The broad completion context at the cursor position.
pub enum CompletionContext {
    /// After `@` — complete param and node names.
    GraphRef,
    /// After `:` in a declaration context — complete type names.
    TypeAnnotation,
    /// After `->` — complete unit names (conversion target, #648 U5).
    ConversionTarget,
    /// Top-level position (start of file, after `;`, after `}`) — complete keywords.
    TopLevel,
    /// Inside an expression — complete constants, functions, etc.
    Expression,
}

/// Tokenize the source and collect all `(Token, Span)` pairs.
fn tokenize(source: &str) -> Vec<(Token, Span)> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    while let Some((tok, span)) = lexer.next_token() {
        tokens.push((tok, span));
    }
    tokens
}

/// Whether a token starting *at* `offset` counts as "at or before" the cursor
/// ([`Boundary::Inclusive`]) or should be skipped over ([`Boundary::Exclusive`]).
///
/// Used to parameterize [`find_token_before`] — callers that want the token
/// the cursor is inside (`Inclusive`) differ from callers that want the token
/// immediately *preceding* the cursor (`Exclusive`) only by this single bit.
#[derive(Debug, Clone, Copy)]
enum Boundary {
    /// Include a token whose span starts exactly at `offset`.
    Inclusive,
    /// Skip a token whose span starts exactly at `offset` (pick the one before).
    Exclusive,
}

/// Find the index of the last token whose span starts at or before (respectively:
/// strictly before) the given byte offset.
fn find_token_before(tokens: &[(Token, Span)], offset: usize, boundary: Boundary) -> Option<usize> {
    // Tokens are sorted by offset. partition_point finds the split between
    // "match" and "past" according to the boundary predicate; the preceding
    // token is the last index that still matches.
    let idx = tokens.partition_point(|(_, span)| match boundary {
        Boundary::Inclusive => span.offset() <= offset,
        Boundary::Exclusive => span.offset() < offset,
    });
    idx.checked_sub(1)
}

/// Detect whether the cursor is inside a function call's argument list.
///
/// Scans backward from the cursor, counting unmatched parentheses. When an
/// unmatched `(` is found preceded by an identifier, that's the function name.
/// Commas at depth 0 are counted to determine the active parameter index.
pub fn find_fn_call_context(source: &str, offset: usize) -> Option<FnCallContext> {
    let tokens = tokenize(source);
    let start_idx = find_token_before(&tokens, offset, Boundary::Inclusive)?;

    // If the cursor is exactly on or past a token, we might be "after" it.
    // Start scanning from start_idx backward.
    let mut depth: usize = 0;
    let mut comma_count: usize = 0;

    // Determine where to start: if the cursor is inside a token that starts
    // before offset, start from that token. Otherwise skip tokens that start
    // at exactly offset (cursor is right before them).
    let scan_start = if tokens[start_idx].1.offset() + tokens[start_idx].1.len() > offset {
        // Cursor is inside this token — don't count it as a delimiter.
        if start_idx == 0 {
            return None;
        }
        start_idx - 1
    } else {
        start_idx
    };

    for i in (0..=scan_start).rev() {
        let (ref tok, _span) = tokens[i];
        match tok {
            Token::RParen | Token::RBracket | Token::RBrace => {
                depth += 1;
            }
            Token::LParen => {
                if depth == 0 {
                    // Found unmatched `(`. Check if preceded by identifier,
                    // walking back over `alias.` qualifier segments so
                    // extern calls (`fluids.density(`) keep their qualified
                    // name.
                    if i > 0
                        && let (Token::Ident, name_span) = &tokens[i - 1]
                    {
                        let mut segments = vec![
                            source[name_span.offset()..name_span.offset() + name_span.len()]
                                .to_string(),
                        ];
                        let mut j = i - 1;
                        while j >= 2
                            && tokens[j - 1].0 == Token::Dot
                            && let (Token::Ident, seg_span) = &tokens[j - 2]
                        {
                            segments.push(
                                source[seg_span.offset()..seg_span.offset() + seg_span.len()]
                                    .to_string(),
                            );
                            j -= 2;
                        }
                        segments.reverse();
                        return Some(FnCallContext {
                            fn_name: segments.join("."),
                            active_param: comma_count,
                        });
                    }
                    // `(` not preceded by identifier — not a function call.
                    return None;
                }
                depth -= 1;
            }
            Token::LBracket | Token::LBrace => {
                if depth == 0 {
                    // We've left the expression context (e.g., inside a struct or index).
                    return None;
                }
                depth -= 1;
            }
            Token::Comma if depth == 0 => {
                comma_count += 1;
            }
            Token::Semicolon if depth == 0 => {
                // Passed a statement boundary — not in a function call.
                return None;
            }
            _ => {}
        }
    }

    None
}

/// Determine the completion context at the given cursor offset.
pub fn determine_completion_context(source: &str, offset: usize) -> CompletionContext {
    let tokens = tokenize(source);

    if tokens.is_empty() {
        return CompletionContext::TopLevel;
    }

    // Find the last token that ends at or before the cursor.
    // We want the token *before* where the user is typing.
    let preceding_idx = find_token_before(&tokens, offset, Boundary::Exclusive);

    let Some(idx) = preceding_idx else {
        // No token before cursor — start of file.
        return CompletionContext::TopLevel;
    };

    let (ref tok, _span) = tokens[idx];

    // If cursor is inside or immediately after an identifier, look at what's before it.
    if *tok == Token::Ident && idx > 0 {
        let (ref prev_tok, _) = tokens[idx - 1];
        match prev_tok {
            Token::At => return CompletionContext::GraphRef,
            Token::Colon => return CompletionContext::TypeAnnotation,
            Token::Arrow => return CompletionContext::ConversionTarget,
            _ => {}
        }
    }

    match tok {
        Token::At => CompletionContext::GraphRef,
        Token::Colon => CompletionContext::TypeAnnotation,
        Token::Arrow => CompletionContext::ConversionTarget,
        Token::Semicolon | Token::RBrace => CompletionContext::TopLevel,
        _ => CompletionContext::Expression,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- FnCallContext tests ----

    #[test]
    fn fn_call_cursor_after_open_paren() {
        // sqrt(|)
        let source = "sqrt()";
        let offset = 5; // after `(`
        let ctx = find_fn_call_context(source, offset).unwrap();
        assert_eq!(ctx.fn_name, "sqrt");
        assert_eq!(ctx.active_param, 0);
    }

    #[test]
    fn fn_call_cursor_on_second_param() {
        // atan2(@y, |)
        let source = "atan2(@y, )";
        let offset = 10; // after `, `
        let ctx = find_fn_call_context(source, offset).unwrap();
        assert_eq!(ctx.fn_name, "atan2");
        assert_eq!(ctx.active_param, 1);
    }

    #[test]
    fn fn_call_nested() {
        // sqrt(min(@a, @b))  — cursor after outer `(`
        let source = "sqrt(min(@a, @b))";
        let offset = 5; // after outer `(`
        let ctx = find_fn_call_context(source, offset).unwrap();
        assert_eq!(ctx.fn_name, "sqrt");
        assert_eq!(ctx.active_param, 0);
    }

    #[test]
    fn fn_call_nested_inner() {
        // sqrt(min(@a, |))  — cursor inside inner min()
        let source = "sqrt(min(@a, ))";
        let offset = 13; // after `, ` inside min()
        let ctx = find_fn_call_context(source, offset).unwrap();
        assert_eq!(ctx.fn_name, "min");
        assert_eq!(ctx.active_param, 1);
    }

    #[test]
    fn fn_call_with_partial_arg() {
        // lerp(@a, @b, |)
        let source = "lerp(@a, @b, )";
        let offset = 13; // after third `, `
        let ctx = find_fn_call_context(source, offset).unwrap();
        assert_eq!(ctx.fn_name, "lerp");
        assert_eq!(ctx.active_param, 2);
    }

    #[test]
    fn no_fn_call_outside_parens() {
        let source = "param x: Dimensionless = 1.0;";
        let offset = 28;
        assert!(find_fn_call_context(source, offset).is_none());
    }

    #[test]
    fn no_fn_call_in_plain_parens() {
        // (1 + 2) — not a function call
        let source = "(1 + 2)";
        let offset = 1;
        assert!(find_fn_call_context(source, offset).is_none());
    }

    // ---- CompletionContext tests ----

    #[test]
    fn context_at_start_of_file() {
        assert!(matches!(
            determine_completion_context("", 0),
            CompletionContext::TopLevel
        ));
    }

    #[test]
    fn context_after_semicolon() {
        let source = "param x: Dimensionless = 1.0;\n";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::TopLevel
        ));
    }

    #[test]
    fn context_after_at() {
        // node y = @|
        let source = "node y = @";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::GraphRef
        ));
    }

    #[test]
    fn context_partial_graph_ref() {
        // node y = @par|
        let source = "node y = @par";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::GraphRef
        ));
    }

    #[test]
    fn context_after_colon() {
        // param x: |
        let source = "param x: ";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::TypeAnnotation
        ));
    }

    #[test]
    fn context_partial_type() {
        // param x: Len|
        let source = "param x: Len";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::TypeAnnotation
        ));
    }

    #[test]
    fn context_in_expression() {
        // node y = |
        let source = "node y = ";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::Expression
        ));
    }

    #[test]
    fn context_after_closing_brace() {
        let source = "type Foo { x: Dimensionless }\n";
        let offset = source.len();
        assert!(matches!(
            determine_completion_context(source, offset),
            CompletionContext::TopLevel
        ));
    }
}
