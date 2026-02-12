use crate::span::Span;
use crate::token::Token;
use logos::Logos;

/// A peekable wrapper around `logos::Lexer` that yields `(Token, Span)` pairs.
#[expect(clippy::option_option)] // Intentional: None = not peeked, Some(None) = EOF, Some(Some(_)) = peeked token
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, Token>,
    peeked: Option<Option<(Token, Span)>>,
    source: &'src str,
}

impl<'src> Lexer<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            inner: Token::lexer(source),
            peeked: None,
            source,
        }
    }

    /// Peek at the next token without consuming it.
    pub fn peek(&mut self) -> Option<&Token> {
        self.peek_with_span().map(|(tok, _)| tok)
    }

    /// Peek at the next token and its span without consuming it.
    ///
    /// # Panics
    ///
    /// This method will not panic in practice. The internal `unwrap()` is safe
    /// because `self.peeked` is always `Some` after the preceding `is_none` check.
    pub fn peek_with_span(&mut self) -> Option<(&Token, Span)> {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance());
        }
        self.peeked
            .as_ref()
            .expect("peeked is always Some after the is_none check above")
            .as_ref()
            .map(|(tok, span)| (tok, *span))
    }

    /// Consume and return the next token and its span.
    pub fn next_token(&mut self) -> Option<(Token, Span)> {
        self.peeked.take().unwrap_or_else(|| self.advance())
    }

    /// Get the source text corresponding to a span.
    #[must_use]
    pub fn slice_at(&self, span: Span) -> &'src str {
        &self.source[span.offset..span.offset + span.len]
    }

    /// Get the full source text.
    #[must_use]
    pub const fn source(&self) -> &'src str {
        self.source
    }

    /// Get the byte offset of the current position in the source.
    /// Useful for generating error spans when the lexer has no more tokens.
    pub const fn current_offset(&mut self) -> usize {
        if let Some(ref peeked) = self.peeked
            && let Some((_, span)) = peeked
        {
            return span.offset;
        }
        // If no peeked token, the offset is at the end of source
        // (logos doesn't expose position after exhaustion, so we approximate)
        self.source.len()
    }

    fn advance(&mut self) -> Option<(Token, Span)> {
        loop {
            let result = self.inner.next()?;
            let slice_span = self.inner.span();
            let span = Span::new(slice_span.start, slice_span.end - slice_span.start);
            if let Ok(token) = result {
                return Some((token, span));
            }
            // Skip unrecognized tokens for now; error reporting will
            // be handled by the parser in later steps.
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
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
    fn source_returns_full_input() {
        let input = "param x = 1.0;";
        let lexer = Lexer::new(input);
        assert_eq!(lexer.source(), input);
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
    fn spans_are_byte_accurate() {
        //          0123456789...
        let input = "node delta_v = @v_exhaust * ln(@mass_ratio);";
        let mut lexer = Lexer::new(input);

        let (_, span) = lexer.next_token().unwrap(); // node
        assert_eq!(span.offset, 0);
        assert_eq!(span.len, 4);

        let (_, span) = lexer.next_token().unwrap(); // delta_v
        assert_eq!(span.offset, 5);
        assert_eq!(span.len, 7);
    }
}
