use crate::source_line::line_endings;
use crate::syntax::comments::{BlankLine, Comment, CommentBody, CommentDelimiter, SourceMetadata};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::{LexicalItem, LexicalToken, Token, TriviaToken};
use logos::Logos;
use std::num::NonZeroUsize;

/// Complete output of lexing one physical source string.
///
/// Unknown characters are skipped by the lexer and retained as the first error
/// span, matching the parser's lexical-error contract.
#[derive(Debug)]
pub struct TokenizedSource {
    tokens: Vec<(Token, Span)>,
    source_metadata: SourceMetadata,
    first_error_span: Option<Span>,
}

impl TokenizedSource {
    /// Parser-facing syntax tokens and their byte spans in source order.
    #[must_use]
    pub fn tokens(&self) -> &[(Token, Span)] {
        &self.tokens
    }

    /// Formatter trivia collected while lexing.
    #[must_use]
    pub const fn source_metadata(&self) -> &SourceMetadata {
        &self.source_metadata
    }

    /// First unrecognized source region, if any.
    #[must_use]
    pub const fn first_error_span(&self) -> Option<Span> {
        self.first_error_span
    }

    /// Formatter trivia collected while lexing, by value.
    #[must_use]
    pub(crate) fn into_source_metadata(self) -> SourceMetadata {
        self.source_metadata
    }
}

/// Tokenize one source string to completion.
#[must_use]
pub fn tokenize(source: &str) -> TokenizedSource {
    let never_interrupted: Result<_, std::convert::Infallible> =
        tokenize_with_checkpoints(source, NonZeroUsize::MAX, || Ok(()));
    match never_interrupted {
        Ok(tokenized) => tokenized,
    }
}

/// Tokenize one source string, calling `checkpoint` after every `interval`
/// syntax tokens and stopping with its error as soon as it fails.
pub(crate) fn tokenize_with_checkpoints<E>(
    source: &str,
    interval: NonZeroUsize,
    mut checkpoint: impl FnMut() -> Result<(), E>,
) -> Result<TokenizedSource, E> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
        if tokens.len() % interval == 0 {
            checkpoint()?;
        }
    }
    let first_error_span = lexer.first_error_span();
    let source_metadata = lexer.into_source_metadata();
    Ok(TokenizedSource {
        tokens,
        source_metadata,
        first_error_span,
    })
}

/// A streaming wrapper around `logos::Lexer` that yields `(Token, Span)` pairs.
///
/// Trivia (whitespace and comments) is recorded into [`SourceMetadata`] as it
/// is skipped. The lexer deliberately offers no lookahead and is not `Clone`:
/// the parser tokenizes the whole source once with [`tokenize`] and looks
/// ahead over the resulting token slice, so speculative parsing never copies
/// recorded comments.
///
/// When the underlying `logos::Lexer` encounters an unrecognized character, the
/// span of the *first* such character is recorded in `first_error_span` and the
/// character is skipped. The parser surfaces this as a `ParseErrorKind::UnknownToken`
/// when a top-level `parse_*` entry point finishes, regardless of whether the
/// downstream parse happened to succeed.
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, LexicalToken>,
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
            source,
            source_metadata: SourceMetadata::default(),
            first_error_span: None,
        }
    }

    /// Return the span of the first unrecognized character encountered so far.
    ///
    /// Returns `None` if the lexer has not yet seen any invalid input. The value
    /// is set lazily as tokens are consumed.
    #[must_use]
    pub(crate) const fn first_error_span(&self) -> Option<Span> {
        self.first_error_span
    }

    /// Consume and return the next syntax token and its span, recording any
    /// trivia skipped on the way.
    pub fn next_token(&mut self) -> Option<(Token, Span)> {
        read_next_token(
            &mut self.inner,
            self.source,
            &mut self.source_metadata,
            &mut self.first_error_span,
        )
    }

    /// Get the source text corresponding to a span.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn slice_at(&self, span: Span) -> &'src str {
        &self.source[span.offset()..span.offset() + span.len()]
    }

    #[must_use]
    pub(crate) fn into_source_metadata(self) -> SourceMetadata {
        self.source_metadata
    }
}

fn read_next_token(
    inner: &mut logos::Lexer<'_, LexicalToken>,
    source: &str,
    source_metadata: &mut SourceMetadata,
    first_error_span: &mut Option<Span>,
) -> Option<(Token, Span)> {
    loop {
        let result = inner.next()?;
        let slice_span = inner.span();
        let span = Span::new(slice_span.start, slice_span.end - slice_span.start);
        match result.map(LexicalToken::classify) {
            Ok(LexicalItem::Trivia(TriviaToken::Whitespace)) => {
                record_blank_lines(source, span, source_metadata);
            }
            Ok(LexicalItem::Trivia(TriviaToken::Comment)) => {
                record_comment(source, span, source_metadata);
            }
            Ok(LexicalItem::Syntax(token)) => break Some((token, span)),
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
    line_endings(whitespace).for_each(|(pos, line_ending)| {
        if let Some(start) = previous_line_ending_start {
            let end = pos + line_ending.len();
            source_metadata.push_blank_line(BlankLine::new(Span::new(
                span.offset() + start,
                end - start,
            )));
        }
        previous_line_ending_start = Some(pos);
    });
}

#[cfg(test)]
mod tests {
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
    fn lexer_accepts_exponent_digit_separators() {
        let input = "1e1_0";
        let mut lexer = Lexer::new(input);
        let (tok, span) = lexer.next_token().unwrap();
        assert_eq!(tok, Token::Number);
        assert_eq!(lexer.slice_at(span), "1e1_0");
        assert!(lexer.next_token().is_none());
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
    fn broken_multiline_string_preserves_following_source_metadata() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let input = format!("\"first{line_ending}second\"{line_ending}{line_ending}// after");
            let mut lexer = Lexer::new(&input);
            while lexer.next_token().is_some() {}

            assert!(
                lexer.first_error_span().is_some(),
                "line ending {line_ending:?} should produce a lexical error"
            );
            let blank_line_offset = input
                .find(&format!("{line_ending}{line_ending}"))
                .expect("test input should contain a blank line");
            let comment_offset = input
                .find("// after")
                .expect("test input should contain a comment");
            let metadata = lexer.into_source_metadata();
            assert_eq!(metadata.blank_lines().len(), 1, "{line_ending:?}");
            assert_eq!(
                metadata.blank_lines()[0].span(),
                Span::new(blank_line_offset, line_ending.len() * 2),
                "{line_ending:?}"
            );
            assert_eq!(metadata.comments().len(), 1, "{line_ending:?}");
            assert_eq!(metadata.comments()[0].span.offset(), comment_offset);
            assert_eq!(metadata.comments()[0].value.lexeme(), "// after");
        }
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
    fn failing_checkpoint_stops_tokenization() {
        let source = "x ".repeat(10_000);
        let interval = NonZeroUsize::new(100).unwrap();
        let mut checkpoints = 0;
        let result = tokenize_with_checkpoints(&source, interval, || {
            checkpoints += 1;
            if checkpoints == 3 { Err(()) } else { Ok(()) }
        });
        assert!(result.is_err());
        assert_eq!(
            checkpoints, 3,
            "tokenization continued after the failed checkpoint"
        );

        let tokenized = tokenize_with_checkpoints(&source, interval, || Ok::<_, ()>(()))
            .expect("checkpoints succeed");
        assert_eq!(tokenized.tokens().len(), 10_000);
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
