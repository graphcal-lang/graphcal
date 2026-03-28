use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::syntax::ast::{Expr, Ident};
use crate::syntax::lexer::Lexer;
use crate::syntax::span::Span;
use crate::syntax::token::Token;

mod compound;
mod decl;
mod expr;
mod fn_decl;
mod table;
mod type_expr;

/// Rich parse error with miette diagnostics.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum ParseError {
    #[error("unexpected token `{found}`")]
    #[diagnostic(code(graphcal::P001), help("expected {expected}"))]
    UnexpectedToken {
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("unexpected end of file")]
    #[diagnostic(code(graphcal::P002), help("expected {expected}"))]
    UnexpectedEof {
        expected: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("invalid number literal")]
    #[diagnostic(code(graphcal::P003))]
    InvalidNumber {
        reason: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("{reason}")]
        span: SourceSpan,
    },

    #[error("table row has {got} value(s), but the header has {expected} column(s)")]
    #[diagnostic(code(graphcal::P004))]
    TableRowLengthMismatch {
        expected: usize,
        got: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this row has {got} value(s)")]
        span: SourceSpan,
    },

    #[error("unknown domain constraint key `{key}`")]
    #[diagnostic(
        code(graphcal::P005),
        help("valid domain constraint keys are `min` and `max`")
    )]
    InvalidDomainBoundKey {
        key: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown key")]
        span: SourceSpan,
    },
}

pub struct Parser<'src> {
    pub(super) lexer: Lexer<'src>,
    pub(super) source: Arc<String>,
    pub(super) source_name: String,
}

impl<'src> Parser<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: "input".to_string(),
        }
    }

    #[must_use]
    pub fn with_name(source: &'src str, name: &str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: name.to_string(),
        }
    }

    pub(super) fn named_source(&self) -> NamedSource<Arc<String>> {
        NamedSource::new(&self.source_name, Arc::clone(&self.source))
    }

    pub(super) fn unexpected_token(&self, expected: &str, found: &str, span: Span) -> ParseError {
        ParseError::UnexpectedToken {
            expected: expected.to_string(),
            found: found.to_string(),
            src: self.named_source(),
            span: span.into(),
        }
    }

    pub(super) fn unexpected_eof(&self, expected: &str) -> ParseError {
        ParseError::UnexpectedEof {
            expected: expected.to_string(),
            src: self.named_source(),
            span: Span::new(self.lexer.source_len(), 0).into(),
        }
    }

    /// Consume the next token, returning an error if the lexer is exhausted.
    ///
    /// Use this after `peek()` has confirmed `Some`.
    pub(super) fn advance(&mut self) -> Result<(Token, Span), ParseError> {
        self.lexer
            .next_token()
            .ok_or_else(|| self.unexpected_eof("token"))
    }

    /// Parse a single expression from the source string.
    ///
    /// Expects the entire input to be consumed; returns an error if there
    /// are trailing tokens after the expression.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source is not a valid expression
    /// or if there are unexpected trailing tokens.
    pub fn parse_single_expr(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_expr()?;
        if let Some((tok, span)) = self.lexer.peek_with_span() {
            let tok = tok.clone();
            return Err(self.unexpected_token("end of input", &format!("{tok:?}"), span));
        }
        Ok(expr)
    }

    /// Parse a standalone unit expression (e.g., `m/s^2`, `kg * m / s^2`).
    ///
    /// Expects the entire input to be consumed; returns an error if there
    /// are trailing tokens after the unit expression.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source is not a valid unit expression.
    pub fn parse_standalone_unit_expr(&mut self) -> Result<crate::syntax::ast::UnitExpr, ParseError> {
        let expr = self.parse_unit_expr()?;
        if let Some((tok, span)) = self.lexer.peek_with_span() {
            let tok = tok.clone();
            return Err(self.unexpected_token("end of input", &format!("{tok:?}"), span));
        }
        Ok(expr)
    }

    /// Parse a standalone dimension expression (e.g., `Length / Time`).
    ///
    /// Expects the entire input to be consumed; returns an error if there
    /// are trailing tokens after the dimension expression.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source is not a valid dimension expression.
    pub fn parse_standalone_dim_expr(&mut self) -> Result<crate::syntax::ast::DimExpr, ParseError> {
        let expr = self.parse_dim_expr()?;
        if let Some((tok, span)) = self.lexer.peek_with_span() {
            let tok = tok.clone();
            return Err(self.unexpected_token("end of input", &format!("{tok:?}"), span));
        }
        Ok(expr)
    }

    /// Parse the full source file into a [`File`](crate::syntax::ast::File) AST node.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source contains invalid syntax.
    pub fn parse_file(&mut self) -> Result<crate::syntax::ast::File, ParseError> {
        let mut declarations = Vec::new();
        while self.lexer.peek().is_some() {
            declarations.push(self.parse_declaration()?);
        }
        Ok(crate::syntax::ast::File { declarations })
    }

    // --- Helper methods ---

    #[expect(
        clippy::needless_pass_by_value,
        reason = "Token is small and the API is cleaner with by-value"
    )]
    pub(super) fn expect(&mut self, expected: Token) -> Result<(Token, Span), ParseError> {
        let expected_str = format!("`{expected}`");
        match self.lexer.next_token() {
            Some((tok, span)) if tok == expected => Ok((tok, span)),
            Some((tok, span)) => Err(self.unexpected_token(&expected_str, &tok.to_string(), span)),
            None => Err(self.unexpected_eof(&expected_str)),
        }
    }

    /// Parse an identifier and check that it matches the expected casing.
    pub(super) fn parse_ident_with_casing(
        &mut self,
        casing_desc: &str,
        check: fn(&str) -> bool,
    ) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => {
                let name = self.lexer.slice_at(span).to_string();
                if check(&name) {
                    Ok(Ident { name, span })
                } else {
                    Err(self.unexpected_token(
                        &format!("{casing_desc} identifier"),
                        &format!("identifier `{name}`"),
                        span,
                    ))
                }
            }
            Some((tok, span)) => Err(self.unexpected_token(
                &format!("{casing_desc} identifier"),
                &tok.to_string(),
                span,
            )),
            None => Err(self.unexpected_eof(&format!("{casing_desc} identifier"))),
        }
    }

    /// Parse any identifier regardless of casing.
    pub(super) fn parse_any_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => Ok(Ident {
                name: self.lexer.slice_at(span).to_string(),
                span,
            }),
            Some((tok, span)) => Err(self.unexpected_token("identifier", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("identifier")),
        }
    }
}

pub(super) fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub(super) fn is_lower_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `PascalCase`: starts with uppercase, contains at least one lowercase letter
/// (to distinguish from `UPPER_SNAKE_CASE` like `GRAVITY`).
pub(super) fn is_pascal_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars().any(|c| c.is_ascii_lowercase())
}

/// Uppercase-starting identifier: `PascalCase` names or single-letter generic params like `I`.
/// Used where both concrete index names (`Maneuver`) and generic params (`I`) are valid.
pub(super) fn is_uppercase_starting(s: &str) -> bool {
    !s.is_empty() && s.starts_with(|c: char| c.is_ascii_uppercase())
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

    use super::is_pascal_case;

    #[test]
    fn is_pascal_case_examples() {
        assert!(is_pascal_case("TransferResult"));
        assert!(is_pascal_case("Orbit"));
        assert!(is_pascal_case("Ab"));
        assert!(!is_pascal_case("ORBIT"));
        assert!(!is_pascal_case("UPPER_SNAKE"));
        assert!(!is_pascal_case("orbit"));
        assert!(!is_pascal_case("lower_snake"));
        assert!(!is_pascal_case(""));
    }
}
