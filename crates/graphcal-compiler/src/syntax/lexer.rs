use crate::syntax::span::Span;
use crate::syntax::token::Token;
use logos::Logos;

/// A peekable wrapper around `logos::Lexer` that yields `(Token, Span)` pairs.
///
/// # Internal design
///
/// The inner `logos::Lexer` is **only** advanced inside `peek_with_span()`.
/// Every other public method goes through `peek_with_span()` to get the next
/// token, which keeps lexer-position bookkeeping in a single place and makes
/// the state machine easy to reason about:
///
/// ```text
///   peeked = None          (initial / just consumed)
///       │
///       ▼  peek_with_span() calls inner.next()
///   peeked = Some(Some(_)) (token ready)
///   peeked = Some(None)    (EOF)
///       │
///       ▼  next_token() takes the peeked value
///   peeked = None          (consumed, ready for next peek)
/// ```
#[expect(
    clippy::option_option,
    reason = "None = not peeked, Some(None) = EOF, Some(Some(_)) = peeked token"
)]
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, Token>,
    peeked: Option<Option<(Token, Span)>>,
    /// Second peek slot for 2-token lookahead. When set, `next_token()` drains
    /// this slot before consuming `peeked`.
    peeked2: Option<(Token, Span)>,
    source: &'src str,
}

impl<'src> Lexer<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            inner: Token::lexer(source),
            peeked: None,
            peeked2: None,
            source,
        }
    }

    /// Peek at the next token without consuming it.
    pub fn peek(&mut self) -> Option<&Token> {
        self.peek_with_span().map(|(tok, _)| tok)
    }

    /// Peek at the next token and its span without consuming it.
    ///
    /// This is the **only** method that advances the inner logos lexer.
    /// All other public methods delegate here to ensure a single point of
    /// state mutation.
    pub fn peek_with_span(&mut self) -> Option<(&Token, Span)> {
        if self.peeked.is_none() {
            if let Some(second) = self.peeked2.take() {
                self.peeked = Some(Some(second));
            } else {
                // Advance the inner lexer, skipping unrecognized tokens.
                // Error reporting for those will be handled by the parser.
                self.peeked = Some(loop {
                    let result = self.inner.next()?;
                    let slice_span = self.inner.span();
                    let span = Span::new(slice_span.start, slice_span.end - slice_span.start);
                    if let Ok(token) = result {
                        break Some((token, span));
                    }
                });
            }
        }
        self.peeked
            .as_ref()
            .and_then(|inner| inner.as_ref().map(|(tok, span)| (tok, *span)))
    }

    /// Consume and return the next token and its span.
    ///
    /// Internally peeks first (if needed) then takes the peeked value,
    /// so the inner lexer is only ever advanced via `peek_with_span()`.
    pub fn next_token(&mut self) -> Option<(Token, Span)> {
        self.peek_with_span(); // ensure peeked is Some
        self.peeked.take().flatten()
    }

    /// Put back a consumed token so the next `peek`/`next_token` returns it.
    ///
    /// If a token is already peeked, the currently peeked token is moved to
    /// the second peek slot. Only one level of put-back is supported.
    ///
    /// # Panics
    ///
    /// Panics if both peek slots are occupied.
    pub fn put_back(&mut self, token: Token, span: Span) {
        if let Some(Some(existing)) = self.peeked.take() {
            assert!(
                self.peeked2.is_none(),
                "cannot put_back: both peek slots are occupied"
            );
            self.peeked2 = Some(existing);
        }
        self.peeked = Some(Some((token, span)));
    }

    /// Get the source text corresponding to a span.
    #[must_use]
    pub fn slice_at(&self, span: Span) -> &'src str {
        &self.source[span.offset()..span.offset() + span.len()]
    }

    /// Return the total length (in bytes) of the source string.
    #[must_use]
    pub const fn source_len(&self) -> usize {
        self.source.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
    use super::*;

    #[test]
    fn lexer_yields_tokens_with_spans() {
        let input = "param x = 1.0;";
        let mut lexer = Lexer::new(input);

        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Param);
        assert_eq!(lexer.slice_at(span), "param");

        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Ident);
        assert_eq!(lexer.slice_at(span), "x");

        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Eq);
        assert_eq!(lexer.slice_at(span), "=");

        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Number);
        assert_eq!(lexer.slice_at(span), "1.0");

        let (tok, _) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Semicolon);

        assert!(lexer.next_token().is_none());
    }

    #[test]
    fn peek_does_not_consume() {
        let input = "param x";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.peek(), Some(&Token::Param));
        assert_eq!(lexer.peek(), Some(&Token::Param));

        lexer.next_token();
        assert_eq!(lexer.peek(), Some(&Token::Ident));
    }

    #[test]
    fn peek_with_span_returns_correct_span() {
        let input = "const G0 = 9.80665;";
        let mut lexer = Lexer::new(input);

        let (tok, span) = lexer.peek_with_span().unwrap();
        assert_eq!(*tok, Token::Const);
        assert_eq!(lexer.slice_at(span), "const");
    }

    #[test]
    fn exhaust_lexer() {
        let input = "42";
        let mut lexer = Lexer::new(input);
        let (tok, _) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Number);
        assert!(lexer.next_token().is_none());
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn next_token_without_prior_peek() {
        // next_token() internally peeks first, so calling it directly
        // (without an explicit peek()) must work identically.
        let input = "param x";
        let mut lexer = Lexer::new(input);

        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Param);
        assert_eq!(lexer.slice_at(span), "param");

        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Ident);
        assert_eq!(lexer.slice_at(span), "x");

        assert!(lexer.next_token().is_none());
    }

    #[test]
    fn spans_are_byte_accurate() {
        //          0123456789...
        let input = "node delta_v = @v_exhaust * ln(@mass_ratio);";
        let mut lexer = Lexer::new(input);

        let (_, span) = lexer.next_token().unwrap(); // node
        assert_eq!(span.offset(), 0);
        assert_eq!(span.len(), 4);

        let (_, span) = lexer.next_token().unwrap(); // delta_v
        assert_eq!(span.offset(), 5);
        assert_eq!(span.len(), 7);
    }
}
