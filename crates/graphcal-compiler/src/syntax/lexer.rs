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
///   peeked = Token(_)      (token ready)
///   peeked = Eof           (EOF)
///       │
///       ▼  next_token() takes the peeked value
///   peeked = None          (consumed, ready for next peek)
/// ```
///
/// When the underlying `logos::Lexer` encounters an unrecognized character, the
/// span of the *first* such character is recorded in `first_error_span` and the
/// character is skipped. The parser surfaces this as a `ParseError::UnknownToken`
/// when a top-level `parse_*` entry point finishes, regardless of whether the
/// downstream parse happened to succeed.
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, Token>,
    peeked: PeekedToken,
    /// Second peek slot for 2-token lookahead. When set, `next_token()` drains
    /// this slot before consuming `peeked`.
    peeked2: Option<(Token, Span)>,
    source: &'src str,
    /// Span of the first unrecognized character encountered during lexing, if any.
    first_error_span: Option<Span>,
}

#[derive(Default)]
enum PeekedToken {
    #[default]
    None,
    Some(Token, Span),
    Eof,
}

impl PeekedToken {
    const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    const fn as_ref(&self) -> Option<(&Token, Span)> {
        match self {
            Self::None | Self::Eof => None,
            Self::Some(token, span) => Some((token, *span)),
        }
    }

    const fn into_token(self) -> Option<(Token, Span)> {
        match self {
            Self::None | Self::Eof => None,
            Self::Some(token, span) => Some((token, span)),
        }
    }
}

impl<'src> Lexer<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            inner: Token::lexer(source),
            peeked: PeekedToken::None,
            peeked2: None,
            source,
            first_error_span: None,
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
                self.peeked = PeekedToken::Some(second.0, second.1);
            } else {
                // Advance the inner lexer, skipping unrecognized characters.
                // The first bad span is remembered in `first_error_span` so the
                // parser can surface it as a `ParseError::UnknownToken` rather
                // than letting a stray character trigger a misleading downstream
                // diagnostic.
                self.peeked = loop {
                    let Some(result) = self.inner.next() else {
                        break PeekedToken::Eof;
                    };
                    let slice_span = self.inner.span();
                    let span = Span::new(slice_span.start, slice_span.end - slice_span.start);
                    match result {
                        Ok(token) => break PeekedToken::Some(token, span),
                        Err(()) => {
                            if self.first_error_span.is_none() {
                                self.first_error_span = Some(span);
                            }
                        }
                    }
                };
            }
        }
        self.peeked.as_ref()
    }

    /// Return the span of the first unrecognized character encountered during lexing.
    ///
    /// Returns `None` if the lexer has not yet seen any invalid input. The value
    /// is set lazily as tokens are consumed; callers that need an up-to-date
    /// answer at a specific point should ensure lexing has progressed past the
    /// region of interest (e.g., by draining the remaining tokens).
    #[must_use]
    pub const fn first_error_span(&self) -> Option<Span> {
        self.first_error_span
    }

    /// Consume and return the next token and its span.
    ///
    /// Internally peeks first (if needed) then takes the peeked value,
    /// so the inner lexer is only ever advanced via `peek_with_span()`.
    pub fn next_token(&mut self) -> Option<(Token, Span)> {
        self.peek_with_span(); // ensure peeked is populated
        std::mem::take(&mut self.peeked).into_token()
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
        match std::mem::take(&mut self.peeked) {
            PeekedToken::Some(existing_token, existing_span) => {
                assert!(
                    self.peeked2.is_none(),
                    "cannot put_back: both peek slots are occupied"
                );
                self.peeked2 = Some((existing_token, existing_span));
            }
            PeekedToken::None | PeekedToken::Eof => {}
        }
        self.peeked = PeekedToken::Some(token, span);
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
        let input = "const node g0 = 9.80665;";
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
    fn unknown_character_is_recorded() {
        // `§` is not part of the grammar. The lexer should skip it while
        // recording the span for the parser to surface as `UnknownToken`.
        let input = "param §x = 1.0;";
        let mut lexer = Lexer::new(input);
        // Drain all tokens so the lexer encounters the stray character.
        while lexer.next_token().is_some() {}
        let err_span = lexer.first_error_span().expect("expected an error span");
        assert_eq!(
            &input[err_span.offset()..err_span.offset() + err_span.len()],
            "§"
        );
    }

    #[test]
    fn only_first_unknown_character_is_recorded() {
        // Multiple stray characters: the lexer reports only the first so that
        // the diagnostic points at the original culprit.
        let input = "§§";
        let mut lexer = Lexer::new(input);
        assert!(lexer.next_token().is_none());
        let err_span = lexer.first_error_span().expect("expected an error span");
        assert_eq!(err_span.offset(), 0);
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
