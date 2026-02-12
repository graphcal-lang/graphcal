use crate::span::Span;
use crate::token::Token;
use logos::Logos;

/// A peekable wrapper around `logos::Lexer` that yields `(Token, Span)` pairs.
pub struct Lexer<'src> {
    inner: logos::Lexer<'src, Token>,
    peeked: Option<Option<(Token, Span)>>,
    source: &'src str,
}

impl<'src> Lexer<'src> {
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
    pub fn peek_with_span(&mut self) -> Option<(&Token, Span)> {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance());
        }
        self.peeked
            .as_ref()
            .unwrap()
            .as_ref()
            .map(|(tok, span)| (tok, *span))
    }

    /// Consume and return the next token and its span.
    pub fn next_token(&mut self) -> Option<(Token, Span)> {
        if let Some(peeked) = self.peeked.take() {
            peeked
        } else {
            self.advance()
        }
    }

    /// Get the source text corresponding to a span.
    pub fn slice_at(&self, span: Span) -> &'src str {
        &self.source[span.offset..span.offset + span.len]
    }

    /// Get the full source text.
    pub fn source(&self) -> &'src str {
        self.source
    }

    /// Get the byte offset of the current position in the source.
    /// Useful for generating error spans when the lexer has no more tokens.
    pub fn current_offset(&mut self) -> usize {
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
            match result {
                Ok(token) => return Some((token, span)),
                Err(()) => {
                    // Skip unrecognized tokens for now; error reporting will
                    // be handled by the parser in later steps.
                    continue;
                }
            }
        }
    }
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
        assert_eq!(tok, Token::LowerIdent);
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
        assert_eq!(lexer.peek(), Some(&Token::LowerIdent));
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
