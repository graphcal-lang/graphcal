use crate::syntax::comments::{BlankLine, Comment, CommentBody, CommentDelimiter, SourceMetadata};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::{LexicalItem, LexicalToken, Token, TriviaToken};
use logos::Logos;
use peek_cache::{PeekCache, SourceItem};
use std::num::NonZeroUsize;

const LEXER_MAX_LOOKAHEAD: NonZeroUsize = NonZeroUsize::new(3).unwrap();

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
///   cache = []             (initial / just consumed)
///       │
///       ▼  cache calls read_next_token()
///   cache = [Token(_), ...] (token ready)
///   cache.eof_seen = true   (EOF)
///       │
///       ▼  cache.next() takes the token
///   cache = [...]           (remaining lookahead)
/// ```
///
/// When the underlying `logos::Lexer` encounters an unrecognized character, the
/// span of the *first* such character is recorded in `first_error_span` and the
/// character is skipped. The parser surfaces this as a `ParseError::UnknownToken`
/// when a top-level `parse_*` entry point finishes, regardless of whether the
/// downstream parse happened to succeed.
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, LexicalToken>,
    peek_cache: PeekCache<(Token, Span)>,
    source: &'src str,
    source_metadata: SourceMetadata,
    /// Span of the first unrecognized character encountered during lexing, if any.
    first_error_span: Option<Span>,
}

impl<'src> Lexer<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            inner: LexicalToken::lexer(source),
            peek_cache: PeekCache::new(LEXER_MAX_LOOKAHEAD),
            source,
            source_metadata: SourceMetadata::default(),
            first_error_span: None,
        }
    }

    /// Peek at the next token without consuming it.
    pub fn peek(&mut self) -> Option<&Token> {
        self.peek_with_span().map(|(tok, _)| tok)
    }

    /// Peek at the token after the next token without consuming either one.
    pub fn peek_second(&mut self) -> Option<&Token> {
        self.peek_token_at(1)
    }

    /// Peek at the third token from the current position without consuming any token.
    pub fn peek_third(&mut self) -> Option<&Token> {
        self.peek_token_at(2)
    }

    /// Peek at the next token and its span without consuming it.
    ///
    /// This delegates to the cache, which fills its first slot before returning
    /// a reference to it.
    pub fn peek_with_span(&mut self) -> Option<(&Token, Span)> {
        let inner = &mut self.inner;
        let source = self.source;
        let source_metadata = &mut self.source_metadata;
        let first_error_span = &mut self.first_error_span;
        self.peek_cache
            .peek(|| read_next_token(inner, source, source_metadata, first_error_span))
            .map(|(tok, span)| (tok, *span))
    }

    fn peek_token_at(&mut self, offset: usize) -> Option<&Token> {
        self.peek_with_span_at(offset).map(|(tok, _)| tok)
    }

    fn peek_with_span_at(&mut self, offset: usize) -> Option<(&Token, Span)> {
        let inner = &mut self.inner;
        let source = self.source;
        let source_metadata = &mut self.source_metadata;
        let first_error_span = &mut self.first_error_span;
        self.peek_cache
            .peek_at(offset, || {
                read_next_token(inner, source, source_metadata, first_error_span)
            })
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
        let source = self.source;
        let source_metadata = &mut self.source_metadata;
        let first_error_span = &mut self.first_error_span;
        self.peek_cache
            .next(|| read_next_token(inner, source, source_metadata, first_error_span))
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

    #[must_use]
    pub fn into_source_metadata(self) -> SourceMetadata {
        self.source_metadata
    }
}

fn read_next_token(
    inner: &mut logos::Lexer<'_, LexicalToken>,
    source: &str,
    source_metadata: &mut SourceMetadata,
    first_error_span: &mut Option<Span>,
) -> SourceItem<(Token, Span)> {
    loop {
        let Some(result) = inner.next() else {
            break SourceItem::Eof;
        };
        let slice_span = inner.span();
        let span = Span::new(slice_span.start, slice_span.end - slice_span.start);
        match result.map(LexicalToken::classify) {
            Ok(LexicalItem::Trivia(TriviaToken::Whitespace)) => {
                record_blank_lines(source, span, source_metadata);
            }
            Ok(LexicalItem::Trivia(TriviaToken::Comment)) => {
                record_comment(source, span, source_metadata);
            }
            Ok(LexicalItem::Syntax(token)) => break SourceItem::Item((token, span)),
            Err(()) => {
                if first_error_span.is_none() {
                    *first_error_span = Some(span);
                }
            }
        }
    }
}

fn record_comment(source: &str, span: Span, source_metadata: &mut SourceMetadata) {
    let lexeme = &source[span.offset()..span.offset() + span.len()];
    let delimiter = comment_delimiter(lexeme);
    let body = CommentBody::new(&lexeme[delimiter.len()..]);
    source_metadata.push_comment(Spanned::new(Comment::new(delimiter, body), span));
}

fn comment_delimiter(lexeme: &str) -> CommentDelimiter {
    let bytes = lexeme.as_bytes();
    match (bytes.get(2).copied(), bytes.get(3).copied()) {
        (Some(b'/'), Some(b'/')) => CommentDelimiter::Line,
        (Some(b'/'), _) => CommentDelimiter::Doc,
        _ => CommentDelimiter::Line,
    }
}

fn record_blank_lines(source: &str, span: Span, source_metadata: &mut SourceMetadata) {
    let whitespace = &source[span.offset()..span.offset() + span.len()];
    let mut previous_line_ending_start: Option<usize> = None;
    let mut pos = 0;
    while pos < whitespace.len() {
        match line_ending_at(whitespace.as_bytes(), pos) {
            Some(line_ending) => {
                if let Some(start) = previous_line_ending_start {
                    let end = pos + line_ending.len();
                    source_metadata.push_blank_line(BlankLine::new(Span::new(
                        span.offset() + start,
                        end - start,
                    )));
                }
                previous_line_ending_start = Some(pos);
                pos += line_ending.len();
            }
            None => pos += 1,
        }
    }
}

fn line_ending_at(bytes: &[u8], pos: usize) -> Option<LineEnding> {
    match bytes.get(pos).copied() {
        Some(b'\n') => Some(LineEnding::Lf),
        Some(b'\r') if bytes.get(pos + 1) == Some(&b'\n') => Some(LineEnding::CrLf),
        Some(b'\r') => Some(LineEnding::Cr),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    const fn len(self) -> usize {
        match self {
            Self::Lf | Self::Cr => 1,
            Self::CrLf => 2,
        }
    }
}

mod peek_cache {
    use std::collections::VecDeque;
    use std::num::NonZeroUsize;

    pub(super) struct PeekCache<T> {
        items: VecDeque<T>,
        eof_seen: bool,
        max_lookahead: NonZeroUsize,
    }

    impl<T> PeekCache<T> {
        pub(super) fn new(max_lookahead: NonZeroUsize) -> Self {
            Self {
                items: VecDeque::with_capacity(max_lookahead.get()),
                eof_seen: false,
                max_lookahead,
            }
        }

        pub(super) fn peek<F>(&mut self, load_next: F) -> Option<&T>
        where
            F: FnMut() -> SourceItem<T>,
        {
            self.peek_at(0, load_next)
        }

        pub(super) fn peek_at<F>(&mut self, offset: usize, load_next: F) -> Option<&T>
        where
            F: FnMut() -> SourceItem<T>,
        {
            debug_assert!(offset < self.max_lookahead.get());
            if offset >= self.max_lookahead.get() {
                return None;
            }

            self.fill_until(offset, load_next);
            self.items.get(offset)
        }

        pub(super) fn next<F>(&mut self, load_next: F) -> Option<T>
        where
            F: FnMut() -> SourceItem<T>,
        {
            self.fill_until(0, load_next);
            self.items.pop_front()
        }

        fn fill_until<F>(&mut self, offset: usize, mut load_next: F)
        where
            F: FnMut() -> SourceItem<T>,
        {
            // Private callers preserve this invariant: `peek_at` validates
            // arbitrary offsets, and `next` only asks for slot 0, which is
            // guaranteed by the nonzero lookahead bound.
            debug_assert!(offset < self.max_lookahead.get());
            while self.items.len() <= offset && !self.eof_seen {
                match load_next() {
                    SourceItem::Item(item) => self.items.push_back(item),
                    SourceItem::Eof => self.eof_seen = true,
                }
            }
        }
    }

    pub(super) enum SourceItem<T> {
        Item(T),
        Eof,
    }
}

#[cfg(test)]
mod tests {
    use super::peek_cache::SourceItem;
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
    fn lookahead_does_not_consume() {
        let input = "param x = 1.0;";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.peek(), Some(&Token::Param));
        assert_eq!(lexer.peek_second(), Some(&Token::Ident));
        assert_eq!(lexer.peek_third(), Some(&Token::Eq));

        let (token, _) = lexer.next_token().unwrap();
        assert_eq!(token, Token::Param);
        let (token, _) = lexer.next_token().unwrap();
        assert_eq!(token, Token::Ident);
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
        let mut cache = PeekCache::<u8>::new(NonZeroUsize::new(2).unwrap());
        let mut next = 0;

        assert_eq!(
            cache.peek(|| {
                next += 1;
                SourceItem::Item(next)
            }),
            Some(&1)
        );
        assert_eq!(
            cache.peek(|| {
                next += 1;
                SourceItem::Item(next)
            }),
            Some(&1)
        );
        assert_eq!(
            cache.next(|| {
                next += 1;
                SourceItem::Item(next)
            }),
            Some(1)
        );
        assert_eq!(
            cache.next(|| {
                next += 1;
                SourceItem::Item(next)
            }),
            Some(2)
        );
        assert_eq!(next, 2);
    }

    #[test]
    fn peek_cache_respects_caller_supplied_lookahead() {
        let mut cache = PeekCache::<u8>::new(NonZeroUsize::new(4).unwrap());
        let mut next = 0;

        assert_eq!(
            cache.peek_at(3, || {
                next += 1;
                SourceItem::Item(next)
            }),
            Some(&4)
        );
        assert_eq!(next, 4);
    }
}
