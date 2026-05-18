use crate::syntax::span::Span;
use crate::syntax::token::Token;
use logos::Logos;
use peek_cache::{Cached, PeekCache};

/// A peekable wrapper around `logos::Lexer` that yields `(Token, Span)` pairs.
///
/// # Internal design
///
/// The inner `logos::Lexer` is **only** advanced by `read_next_token()`, which
/// adapts the tokenizer into the generic private peek cache. `Lexer` cannot read
/// or take the cache slots directly; the cache fields are private to a nested
/// module, and its public methods fill the first slot before returning or
/// consuming it.
///
/// ```text
///   cache.first = Empty    (initial / just consumed)
///       │
///       ▼  cache calls read_next_token()
///   cache.first = Token(_) (token ready)
///   cache.first = Eof      (EOF)
///       │
///       ▼  cache.next_or_fill() takes the token
///   cache.first = Empty    (consumed, ready for next peek)
/// ```
///
/// When the underlying `logos::Lexer` encounters an unrecognized character, the
/// span of the *first* such character is recorded in `first_error_span` and the
/// character is skipped. The parser surfaces this as a `ParseError::UnknownToken`
/// when a top-level `parse_*` entry point finishes, regardless of whether the
/// downstream parse happened to succeed.
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, Token>,
    peek_cache: PeekCache<(Token, Span)>,
    source: &'src str,
    /// Span of the first unrecognized character encountered during lexing, if any.
    first_error_span: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutBackError;

impl<'src> Lexer<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            inner: Token::lexer(source),
            peek_cache: PeekCache::default(),
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
    /// This delegates to the cache, which fills its first slot before returning
    /// a reference to it.
    pub fn peek_with_span(&mut self) -> Option<(&Token, Span)> {
        let inner = &mut self.inner;
        let first_error_span = &mut self.first_error_span;
        self.peek_cache
            .peek_or_fill(|| read_next_token(inner, first_error_span))
            .map(|(tok, span)| (tok, *span))
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
    /// The cache fills its first slot before taking from it, so this method
    /// cannot accidentally consume an uninitialized cache slot.
    pub fn next_token(&mut self) -> Option<(Token, Span)> {
        let inner = &mut self.inner;
        let first_error_span = &mut self.first_error_span;
        self.peek_cache
            .next_or_fill(|| read_next_token(inner, first_error_span))
    }

    /// Put back a consumed token so the next `peek`/`next_token` returns it.
    ///
    /// If a token is already peeked, the currently peeked token is moved to
    /// the second peek slot. Only one level of put-back is supported.
    ///
    /// # Errors
    ///
    /// Returns [`PutBackError`] if both peek slots are occupied.
    pub fn put_back(&mut self, token: Token, span: Span) -> Result<(), PutBackError> {
        self.peek_cache.put_back((token, span))
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

fn read_next_token(
    inner: &mut logos::Lexer<'_, Token>,
    first_error_span: &mut Option<Span>,
) -> Cached<(Token, Span)> {
    loop {
        let Some(result) = inner.next() else {
            break Cached::Eof;
        };
        let slice_span = inner.span();
        let span = Span::new(slice_span.start, slice_span.end - slice_span.start);
        match result {
            Ok(token) => break Cached::Some((token, span)),
            Err(()) => {
                if first_error_span.is_none() {
                    *first_error_span = Some(span);
                }
            }
        }
    }
}

mod peek_cache {
    use super::PutBackError;

    pub(super) struct PeekCache<T> {
        first: Cached<T>,
        /// Second peek slot for 2-token lookahead. When set, `next_or_fill()`
        /// drains this slot before requesting a new item.
        second: Option<T>,
    }

    impl<T> PeekCache<T> {
        pub(super) fn peek_or_fill<F>(&mut self, read_next: F) -> Option<&T>
        where
            F: FnOnce() -> Cached<T>,
        {
            self.fill_if_needed(read_next);
            self.first.as_ref()
        }

        pub(super) fn next_or_fill<F>(&mut self, read_next: F) -> Option<T>
        where
            F: FnOnce() -> Cached<T>,
        {
            self.fill_if_needed(read_next);
            std::mem::take(&mut self.first).into_item()
        }

        /// Put back a consumed token so the next `peek`/`next_token` returns it.
        ///
        /// If a token is already peeked, the currently peeked token is moved to
        /// the second peek slot. Only one level of put-back is supported.
        ///
        /// # Errors
        ///
        /// Returns [`PutBackError`] if both peek slots are occupied.
        pub(super) fn put_back(&mut self, item: T) -> Result<(), PutBackError> {
            if matches!(self.first, Cached::Some(..)) && self.second.is_some() {
                return Err(PutBackError);
            }

            match std::mem::replace(&mut self.first, Cached::Some(item)) {
                Cached::Some(existing) => {
                    self.second = Some(existing);
                }
                Cached::None | Cached::Eof => {}
            }
            Ok(())
        }

        fn fill_if_needed<F>(&mut self, read_next: F)
        where
            F: FnOnce() -> Cached<T>,
        {
            if !self.first.is_none() {
                return;
            }

            self.first = self.second.take().map_or_else(read_next, Cached::Some);
        }
    }

    #[derive(Default)]
    pub(super) enum Cached<T> {
        #[default]
        None,
        Some(T),
        Eof,
    }

    impl<T> Default for PeekCache<T> {
        fn default() -> Self {
            Self {
                first: Cached::None,
                second: None,
            }
        }
    }

    impl<T> Cached<T> {
        const fn is_none(&self) -> bool {
            matches!(self, Self::None)
        }

        const fn as_ref(&self) -> Option<&T> {
            match self {
                Self::None | Self::Eof => None,
                Self::Some(item) => Some(item),
            }
        }

        fn into_item(self) -> Option<T> {
            match self {
                Self::None | Self::Eof => None,
                Self::Some(item) => Some(item),
            }
        }
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
    use super::peek_cache::Cached;
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
    fn put_back_returns_error_when_both_slots_are_occupied() {
        let input = "param x";
        let mut lexer = Lexer::new(input);

        let (token, span) = lexer.next_token().unwrap();
        assert_eq!(lexer.peek(), Some(&Token::Ident));
        assert_eq!(lexer.put_back(token, span), Ok(()));

        assert_eq!(lexer.put_back(Token::Const, span), Err(PutBackError));

        let (token, _) = lexer.next_token().unwrap();
        assert_eq!(token, Token::Param);
        let (token, _) = lexer.next_token().unwrap();
        assert_eq!(token, Token::Ident);
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

    #[test]
    fn peek_cache_uses_supplied_items_without_lexer_knowledge() {
        let mut cache = PeekCache::<u8>::default();
        let mut next = 0;

        assert_eq!(
            cache.peek_or_fill(|| {
                next += 1;
                Cached::Some(next)
            }),
            Some(&1)
        );
        assert_eq!(
            cache.peek_or_fill(|| {
                next += 1;
                Cached::Some(next)
            }),
            Some(&1)
        );
        assert_eq!(
            cache.next_or_fill(|| {
                next += 1;
                Cached::Some(next)
            }),
            Some(1)
        );
        assert_eq!(
            cache.next_or_fill(|| {
                next += 1;
                Cached::Some(next)
            }),
            Some(2)
        );
        assert_eq!(next, 2);
    }
}
