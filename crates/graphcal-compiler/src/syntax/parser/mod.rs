use crate::outcome::Outcome;
use crate::syntax::ast::{Expr, Ident, IdentPath};
use crate::syntax::comments::SourceMetadata;
use crate::syntax::span::Span;
use crate::syntax::token::{ContextualKeyword, SourceIdentifier, Token};

mod compound;
mod decl;
mod error;
mod expected;
mod expr;
mod nesting_limit;
mod table;
mod token_stream;
mod type_expr;

pub use error::{
    CompositionKind, InvalidNumberReason, ParseError, ParseErrorKind, PlotFieldContext,
    UnsupportedMultiDeclShape,
};
pub use expected::{Expected, Found};
use nesting_limit::MAX_NESTING_DEPTH;
use token_stream::ParserTokenStream;

/// Recursive-descent parser over one source text.
///
/// The parser knows nothing about where its source came from: errors carry
/// only spans into it, and the shell that owns the source attaches its
/// identity with [`ParseError::located`].
#[derive(Clone)]
pub struct Parser<'src> {
    lexer: ParserTokenStream<'src>,
    source: &'src str,
    /// Current nesting depth of recursive grammar productions; bounded by
    /// [`MAX_NESTING_DEPTH`] via [`Self::with_nesting_budget`].
    nesting_depth: usize,
}

impl<'src> Parser<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            lexer: ParserTokenStream::new(source),
            source,
            nesting_depth: 0,
        }
    }

    /// Run `f` one nesting level deeper, erroring out once the depth budget
    /// is exhausted instead of overflowing the stack.
    ///
    /// Within the budget, the stack is grown on demand (`stacker`): the
    /// recursive-descent frames for [`MAX_NESTING_DEPTH`] levels exceed the
    /// default stack of secondary threads (tests, LSP workers) in debug
    /// builds, so the bound alone would not prevent an abort.
    fn with_nesting_budget<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.nesting_depth >= MAX_NESTING_DEPTH {
            let span = self.lexer.peek_with_span().map(|(_, span)| span);
            return Err(ParseError::new(
                ParseErrorKind::TooDeeplyNested,
                span.unwrap_or_else(|| self.end_of_source()),
            ));
        }
        self.nesting_depth += 1;
        let result = crate::stack::with_stack_growth(|| f(self));
        self.nesting_depth -= 1;
        result
    }

    #[must_use]
    pub fn into_source_metadata(self) -> SourceMetadata {
        self.lexer.into_source_metadata()
    }

    /// Empty span at the end of the source.
    const fn end_of_source(&self) -> Span {
        Span::new(self.lexer.source_len(), 0)
    }

    /// Reject `found` at `span` where `expected` was required.
    const fn unexpected_token(expected: Expected, found: Found, span: Span) -> ParseError {
        ParseError::new(ParseErrorKind::UnexpectedToken { expected, found }, span)
    }

    /// Reject `token` at `span` where `expected` was required.
    const fn unexpected(expected: Expected, token: Token, span: Span) -> ParseError {
        Self::unexpected_token(expected, Found::Token(token), span)
    }

    /// Build a duplicate-field error for plot/figure/layer block parsing.
    const fn duplicate_plot_field(
        field: SourceIdentifier,
        context: PlotFieldContext,
        span: Span,
    ) -> ParseError {
        ParseError::new(ParseErrorKind::DuplicatePlotField { field, context }, span)
    }

    const fn unexpected_eof(&self, expected: Expected) -> ParseError {
        ParseError::new(
            ParseErrorKind::UnexpectedEof { expected },
            self.end_of_source(),
        )
    }

    /// Reject the next token (or the end of the source) where `expected` was
    /// required.
    fn unexpected_next(&mut self, expected: Expected) -> ParseError {
        match self.lexer.next_token() {
            Some((token, span)) => Self::unexpected(expected, token, span),
            None => self.unexpected_eof(expected),
        }
    }

    const fn invalid_number(reason: InvalidNumberReason, span: Span) -> ParseError {
        ParseError::new(ParseErrorKind::InvalidNumber { reason }, span)
    }

    const fn nat_subtraction_unsupported(span: Span) -> ParseError {
        ParseError::new(ParseErrorKind::NatSubtractionUnsupported, span)
    }

    /// Consume any remaining tokens and, if the lexer encountered an unrecognized
    /// character at any point, replace `result` with a `ParseErrorKind::UnknownToken`
    /// pointing at the first such span.
    ///
    /// A stray character is a root-cause lex-level failure; it should eclipse any
    /// downstream parse error that was caused by the character having been
    /// silently skipped.
    fn finalize<T>(&mut self, result: Result<T, ParseError>) -> Result<T, ParseError> {
        while self.lexer.peek().is_some() {
            self.lexer.next_token();
        }
        if let Some(span) = self.lexer.first_error_span() {
            return Err(ParseError::new(ParseErrorKind::UnknownToken, span));
        }
        result
    }

    /// Consume the next token, returning an error if the lexer is exhausted.
    ///
    /// Use this after `peek()` has confirmed `Some`.
    fn advance(&mut self) -> Result<(Token, Span), ParseError> {
        self.lexer
            .next_token()
            .ok_or_else(|| self.unexpected_eof(Expected::AnyToken))
    }

    /// Parse a finite `f64` literal from already-normalized token text.
    fn parse_finite_f64_literal(text: &str, span: Span) -> Result<f64, ParseError> {
        let value: f64 = text
            .parse()
            .map_err(|error| Self::invalid_number(InvalidNumberReason::Float(error), span))?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(Self::invalid_number(
                InvalidNumberReason::NonFiniteFloat,
                span,
            ))
        }
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
        let result = self.parse_single_expr_inner();
        self.finalize(result)
    }

    fn parse_single_expr_inner(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_expr()?;
        if let Some((tok, span)) = self.lexer.peek_with_span() {
            let tok = *tok;
            return Err(Self::unexpected(Expected::EndOfInput, tok, span));
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
    pub fn parse_standalone_unit_expr(
        &mut self,
    ) -> Result<crate::syntax::ast::UnitExpr, ParseError> {
        let result = self.parse_standalone_unit_expr_inner();
        self.finalize(result)
    }

    fn parse_standalone_unit_expr_inner(
        &mut self,
    ) -> Result<crate::syntax::ast::UnitExpr, ParseError> {
        let expr = self.parse_unit_expr()?;
        if let Some((tok, span)) = self.lexer.peek_with_span() {
            let tok = *tok;
            return Err(Self::unexpected(Expected::EndOfInput, tok, span));
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
        let result = self.parse_standalone_dim_expr_inner();
        self.finalize(result)
    }

    fn parse_standalone_dim_expr_inner(
        &mut self,
    ) -> Result<crate::syntax::ast::DimExpr, ParseError> {
        let expr = self.parse_dim_expr()?;
        if let Some((tok, span)) = self.lexer.peek_with_span() {
            let tok = *tok;
            return Err(Self::unexpected(Expected::EndOfInput, tok, span));
        }
        Ok(expr)
    }

    /// Parse the full source file into a [`File`](crate::syntax::ast::File) AST node.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source contains invalid syntax.
    pub fn parse_file(&mut self) -> Result<crate::syntax::ast::File, ParseError> {
        let result = self.parse_file_inner();
        self.finalize(result)
    }

    /// Parse a full source file with cooperative cancellation at a bounded
    /// syntax-token interval, including within a single large declaration.
    ///
    /// # Errors
    ///
    /// Returns [`Outcome::Failed`] with the [`ParseError`] for invalid source
    /// and [`Outcome::Cancelled`] after cancellation is requested, so a
    /// cancelled analysis cannot be rendered as invalid source.
    pub fn parse_file_with_cancellation(
        &mut self,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<crate::syntax::ast::File, Outcome<ParseError>> {
        self.lexer.enable_cancellation(cancellation);
        let result = self.parse_file_with_active_cancellation();
        self.lexer.disable_cancellation();
        result
    }

    fn parse_file_with_active_cancellation(
        &mut self,
    ) -> Result<crate::syntax::ast::File, Outcome<ParseError>> {
        self.lexer.checkpoint()?;
        let result = self.parse_file_inner();
        while self.lexer.peek().is_some() {
            if self.lexer.next_token().is_none() {
                break;
            }
        }
        self.lexer.checkpoint()?;
        if let Some(span) = self.lexer.first_error_span() {
            return Err(Outcome::Failed(ParseError::new(
                ParseErrorKind::UnknownToken,
                span,
            )));
        }
        result.map_err(Outcome::Failed)
    }

    fn parse_file_inner(&mut self) -> Result<crate::syntax::ast::File, ParseError> {
        let mut declarations = Vec::new();
        while self.lexer.peek().is_some() {
            declarations.push(self.parse_declaration()?);
        }
        let mut file = crate::syntax::ast::File { declarations };
        // The lexer has consumed (and recorded) every comment by now, so doc
        // blocks can be attached to the declarations they precede.
        crate::syntax::doc_attach::attach_doc_comments(
            &mut file,
            self.source,
            self.lexer.source_metadata(),
        );
        Ok(file)
    }

    // --- Helper methods ---

    fn expect(&mut self, expected: Token) -> Result<(Token, Span), ParseError> {
        match self.lexer.next_token() {
            Some((tok, span)) if tok == expected => Ok((tok, span)),
            Some((tok, span)) => Err(Self::unexpected(Expected::Token(expected), tok, span)),
            None => Err(self.unexpected_eof(Expected::Token(expected))),
        }
    }

    /// Parse a comma-separated list of items until `end_token` is peeked.
    ///
    /// Supports trailing commas. Does **not** consume the `end_token`.
    fn parse_comma_separated<T>(
        &mut self,
        end_token: Token,
        mut parse_item: impl FnMut(&mut Self) -> Result<T, ParseError>,
    ) -> Result<Vec<T>, ParseError> {
        let mut items = Vec::new();
        loop {
            if self.lexer.peek() == Some(&end_token) {
                break;
            }
            items.push(parse_item(self)?);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        Ok(items)
    }

    /// Parse a comma-separated list that requires at least one item.
    ///
    /// Supports trailing commas. Does **not** consume the `end_token`.
    fn parse_non_empty_comma_separated<T>(
        &mut self,
        end_token: Token,
        mut parse_item: impl FnMut(&mut Self) -> Result<T, ParseError>,
    ) -> Result<crate::syntax::non_empty::NonEmpty<T>, ParseError> {
        let first = parse_item(self)?;
        let mut rest = Vec::new();
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token();
            if self.lexer.peek() == Some(&end_token) {
                break;
            }
            rest.push(parse_item(self)?);
        }
        Ok(crate::syntax::non_empty::NonEmpty::new(first, rest))
    }

    /// The contextual keyword spelled by the next token, if any.
    fn peek_contextual_keyword(&mut self) -> Option<ContextualKeyword> {
        match self.lexer.peek() {
            Some(&Token::ContextualKeyword(keyword)) => Some(keyword),
            _ => None,
        }
    }

    /// Parse any identifier regardless of casing.
    fn parse_any_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((token, span)) if token.is_identifier() => Ok(Ident {
                name: SourceIdentifier::new_unchecked_for_parser(
                    self.lexer.slice_at(span).to_string(),
                ),
                span,
            }),
            Some((tok, span)) => Err(Self::unexpected(Expected::Identifier, tok, span)),
            None => Err(self.unexpected_eof(Expected::Identifier)),
        }
    }

    /// Parse a local name or a member selected after a dotted DAG path and
    /// `::`. A dot sequence is consumed only when a later `::` proves that it
    /// is a namespace owner; otherwise the first identifier remains local and
    /// expression parsing handles the dot as field projection.
    fn parse_ident_path(&mut self) -> Result<IdentPath, ParseError> {
        let first = self.parse_any_ident()?;
        let mut probe = self.lexer.clone();
        while probe.peek() == Some(&Token::Dot)
            && probe
                .peek_second()
                .is_some_and(|token| token.is_identifier())
        {
            probe.next_token();
            probe.next_token();
        }
        if probe.peek() != Some(&Token::DoubleColon) {
            return Ok(IdentPath::local(first));
        }

        let mut owner_rest = Vec::new();
        while self.lexer.peek() == Some(&Token::Dot) {
            self.lexer.next_token();
            owner_rest.push(self.parse_any_ident()?);
        }
        self.expect(Token::DoubleColon)?;
        let member = self.parse_any_ident()?;
        Ok(IdentPath::qualified(
            crate::syntax::non_empty::NonEmpty::new(first, owner_rest),
            member,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::outcome::Outcome;
    use crate::syntax::parser::{
        ParseErrorKind, Parser, token_stream::CANCELLATION_CHECKPOINT_INTERVAL,
    };

    #[test]
    fn stray_character_in_source_surfaces_as_unknown_token() {
        let input = "param x = 1.0; §";
        let mut parser = Parser::new(input);
        let err = parser.parse_file().expect_err("expected parse error");
        assert!(
            matches!(err.kind, ParseErrorKind::UnknownToken),
            "expected UnknownToken, got {err:?}"
        );
        let byte_start: usize = err.span.offset();
        let byte_end = byte_start + err.span.len();
        assert_eq!(&input[byte_start..byte_end], "§");
    }

    #[test]
    fn stray_character_preempts_other_parse_errors() {
        // Even when the parse would otherwise fail with UnexpectedToken on the
        // trailing `+`, the stray `§` earlier in the source is the root cause
        // and should be reported.
        let input = "param x = §1.0 +";
        let mut parser = Parser::new(input);
        let err = parser.parse_file().expect_err("expected parse error");
        assert!(
            matches!(err.kind, ParseErrorKind::UnknownToken),
            "expected UnknownToken, got {err:?}"
        );
    }

    #[test]
    fn cancellation_checkpoints_interrupt_large_single_declarations() {
        let term_count = CANCELLATION_CHECKPOINT_INTERVAL * 2;
        let expression = (0..term_count)
            .map(|_| "1.0")
            .collect::<Vec<_>>()
            .join(" + ");
        let table_rows = (0..term_count)
            .map(|_| "1.0;")
            .collect::<Vec<_>>()
            .join(" ");
        let nested_dags = (0..term_count)
            .map(|index| format!("dag d{index} {{"))
            .collect::<Vec<_>>()
            .join(" ");
        let closing_braces = "}".repeat(term_count);
        let cases = [
            format!("node x: Dimensionless = {expression};"),
            format!(
                "param x: Dimensionless[Fin({term_count})] = \
                 table[Fin({term_count})] {{ {table_rows} }};"
            ),
            format!("{nested_dags} {closing_braces}"),
        ];

        for source in cases {
            let mut parser = Parser::new(&source);
            // The operation-boundary checkpoint succeeds, then the next
            // periodic checkpoint deterministically cancels after tokens from
            // the sole top-level declaration have already been consumed.
            parser.lexer.cancel_after_successful_checkpoints(1);
            let error = parser
                .parse_file_with_active_cancellation()
                .expect_err("large declaration should reach an internal checkpoint");
            assert!(matches!(error, Outcome::Cancelled));
        }
    }
}
