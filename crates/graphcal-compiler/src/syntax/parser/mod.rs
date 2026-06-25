use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::syntax::ast::{Expr, Ident, IdentPath};
use crate::syntax::comments::SourceMetadata;
use crate::syntax::lexer::Lexer;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;
use crate::syntax::token::Token;

mod compound;
mod decl;
mod expr;
mod table;
mod type_expr;

/// Build a [`NamedSource`] from `(name, source)` for parser diagnostics.
///
/// The source is shared through `Arc` so diagnostics built from the same file
/// do not re-copy it.
#[must_use]
fn named_source<N: Into<String>, S: Into<Arc<String>>>(
    name: N,
    source: S,
) -> NamedSource<Arc<String>> {
    NamedSource::new(name.into(), source.into())
}

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

    #[error("stray character in source")]
    #[diagnostic(
        code(graphcal::P006),
        help("remove or replace this character; it is not part of the graphcal grammar")
    )]
    UnknownToken {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("stray character")]
        span: SourceSpan,
    },

    #[error(
        "multi-decl slot tuple has {tuple_count} entr{}, but the multi-decl declares {slot_count} slot{}",
        if *tuple_count == 1 { "y" } else { "ies" },
        if *slot_count == 1 { "" } else { "s" }
    )]
    #[diagnostic(
        code(graphcal::P007),
        help(
            "the slot tuple in `table[..., (…)]` must contain exactly one entry per declared slot"
        )
    )]
    MultiDeclTupleArity {
        slot_count: usize,
        tuple_count: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("slot tuple here")]
        span: SourceSpan,
    },

    #[error(
        "multi-decl header row has {header_count} cell{}, but the multi-decl declares {slot_count} slot{}",
        if *header_count == 1 { "" } else { "s" },
        if *slot_count == 1 { "" } else { "s" }
    )]
    #[diagnostic(
        code(graphcal::P008),
        help("the header row (`: _, _, …;`) must have exactly one cell per slot")
    )]
    MultiDeclHeaderArity {
        slot_count: usize,
        header_count: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("header row here")]
        span: SourceSpan,
    },

    #[error(
        "multi-decl row `{row_label}` has {got} value(s), but the multi-decl declares {slot_count} slot{}",
        if *slot_count == 1 { "" } else { "s" }
    )]
    #[diagnostic(
        code(graphcal::P009),
        help("each row must have exactly one value per slot")
    )]
    MultiDeclRowArity {
        slot_count: usize,
        got: usize,
        row_label: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this row has {got} value(s)")]
        span: SourceSpan,
    },

    #[error("multi-decl requires at least two slots")]
    #[diagnostic(
        code(graphcal::P010),
        help(
            "for a single declaration, use the regular `param`/`node`/`const node` form without a trailing comma"
        )
    )]
    MultiDeclSingleSlot {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("single slot here")]
        span: SourceSpan,
    },

    #[error("multi-decl requires at least one shared axis")]
    #[diagnostic(
        code(graphcal::P011),
        help("declare the row axis in `table[SharedAxis, (…)]`")
    )]
    MultiDeclNoSharedAxis {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("missing shared axis")]
        span: SourceSpan,
    },

    #[error("{reason}")]
    #[diagnostic(
        code(graphcal::P012),
        help(
            "this multi-decl shape is scheduled for a later version; see issue #481 for the incremental plan"
        )
    )]
    MultiDeclUnsupportedShape {
        reason: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("inline DAG call requires `.<out>` projection")]
    #[diagnostic(
        code(graphcal::P014),
        help(
            "add `.<output_name>` after the call; an instantiated DAG without a projection is not a node"
        )
    )]
    InlineDagCallMissingProjection {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expected `.<out>` projection here")]
        span: SourceSpan,
    },

    #[error("expression nesting is too deep")]
    #[diagnostic(
        code(graphcal::P015),
        help("the parser limits nesting to {MAX_NESTING_DEPTH} levels; simplify the expression")
    )]
    TooDeeplyNested {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("nesting exceeds the limit here")]
        span: SourceSpan,
    },

    #[error("unit reference path is too deep")]
    #[diagnostic(
        code(graphcal::P017),
        help(
            "unit references are at most `alias.unit` — a bare name for local, selectively imported, or prelude units, or one module-alias qualifier for module-imported units"
        )
    )]
    UnitReferenceTooDeep {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("at most one `alias.` qualifier is allowed here")]
        span: SourceSpan,
    },

    #[error("`^0` exponent has no effect")]
    #[diagnostic(
        code(graphcal::P016),
        help(
            "a zero power erases its term; remove the term (or the exponent) instead of raising to zero"
        )
    )]
    ZeroExponent {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("exponent must be a non-zero integer")]
        span: SourceSpan,
    },

    #[error("duplicate `{field}` in {context}")]
    #[diagnostic(
        code(graphcal::P018),
        help("each field may appear at most once; remove or rename the duplicate")
    )]
    DuplicatePlotField {
        field: String,
        context: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate field here")]
        span: SourceSpan,
    },

    #[error("plot declaration has no encoding channels")]
    #[diagnostic(
        code(graphcal::P019),
        help(
            "add an `encode:` block with at least one channel, e.g. `encode: {{ x: ..., y: ... }}`"
        )
    )]
    MissingPlotEncoding {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this plot has an empty or missing `encode:` block")]
        span: SourceSpan,
    },

    #[error("{kind} declaration has no plots")]
    #[diagnostic(
        code(graphcal::P020),
        help("add a non-empty `plots:` list, e.g. `plots: [my_plot]`")
    )]
    EmptyCompositionPlots {
        kind: &'static str,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this {kind} has an empty or missing `plots:` list")]
        span: SourceSpan,
    },
}

/// Maximum nesting depth for recursive grammar productions (expressions,
/// unary chains, type expressions).
///
/// The recursive-descent parser consumes one or more stack frames per
/// nesting level; without a bound, pathological input like 100k nested
/// parentheses overflows the stack and aborts the process (including the
/// LSP server). The limit is far above any realistic engineering program —
/// note that left-nested operator *chains* (`1.0 + 1.0 + …`) are parsed
/// iteratively and are not limited by this bound.
pub const MAX_NESTING_DEPTH: usize = 256;

impl ParseError {
    /// Return the `NamedSource` embedded in this error.
    ///
    /// Every variant carries the file's name and full source text via miette's
    /// `#[source_code]` field. Exposing it as a typed accessor lets diagnostic
    /// emitters pair the error's offsets with the exact source they index into
    /// — instead of inferring (name, source) from external context, which can
    /// silently desynchronize when an imported file is the origin.
    #[must_use]
    pub const fn named_source(&self) -> &NamedSource<Arc<String>> {
        match self {
            Self::UnexpectedToken { src, .. }
            | Self::UnexpectedEof { src, .. }
            | Self::InvalidNumber { src, .. }
            | Self::TableRowLengthMismatch { src, .. }
            | Self::InvalidDomainBoundKey { src, .. }
            | Self::UnknownToken { src, .. }
            | Self::MultiDeclTupleArity { src, .. }
            | Self::MultiDeclHeaderArity { src, .. }
            | Self::MultiDeclRowArity { src, .. }
            | Self::MultiDeclSingleSlot { src, .. }
            | Self::MultiDeclNoSharedAxis { src, .. }
            | Self::MultiDeclUnsupportedShape { src, .. }
            | Self::InlineDagCallMissingProjection { src, .. }
            | Self::TooDeeplyNested { src, .. }
            | Self::ZeroExponent { src, .. }
            | Self::UnitReferenceTooDeep { src, .. }
            | Self::DuplicatePlotField { src, .. }
            | Self::MissingPlotEncoding { src, .. }
            | Self::EmptyCompositionPlots { src, .. } => src,
        }
    }
}

pub struct Parser<'src> {
    pub(super) lexer: Lexer<'src>,
    pub(super) source: Arc<String>,
    pub(super) source_name: String,
    /// Current nesting depth of recursive grammar productions; bounded by
    /// [`MAX_NESTING_DEPTH`] via [`Self::with_depth`].
    depth: usize,
}

impl<'src> Parser<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: "input".to_string(),
            depth: 0,
        }
    }

    #[must_use]
    pub fn with_name(source: &'src str, name: &str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: name.to_string(),
            depth: 0,
        }
    }

    /// Run `f` one nesting level deeper, erroring out once the depth budget
    /// is exhausted instead of overflowing the stack.
    ///
    /// Within the budget, the stack is grown on demand (`stacker`): the
    /// recursive-descent frames for [`MAX_NESTING_DEPTH`] levels exceed the
    /// default stack of secondary threads (tests, LSP workers) in debug
    /// builds, so the bound alone would not prevent an abort.
    pub(super) fn with_depth<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            let span = self.lexer.peek_with_span().map(|(_, span)| span);
            return Err(ParseError::TooDeeplyNested {
                src: self.named_source(),
                span: span
                    .unwrap_or_else(|| Span::new(self.lexer.source_len(), 0))
                    .into(),
            });
        }
        self.depth += 1;
        let result = crate::stack::with_stack_growth(|| f(self));
        self.depth -= 1;
        result
    }

    #[must_use]
    pub fn into_source_metadata(self) -> SourceMetadata {
        self.lexer.into_source_metadata()
    }

    pub(super) fn named_source(&self) -> NamedSource<Arc<String>> {
        named_source(&self.source_name, Arc::clone(&self.source))
    }

    pub(super) fn unexpected_token(&self, expected: &str, found: &str, span: Span) -> ParseError {
        ParseError::UnexpectedToken {
            expected: expected.to_string(),
            found: found.to_string(),
            src: self.named_source(),
            span: span.into(),
        }
    }

    /// Build a duplicate-field error for plot/figure/layer block parsing.
    pub(super) fn duplicate_plot_field(
        &self,
        field: &str,
        context: &str,
        span: Span,
    ) -> ParseError {
        ParseError::DuplicatePlotField {
            field: field.to_string(),
            context: context.to_string(),
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

    /// Consume any remaining tokens and, if the lexer encountered an unrecognized
    /// character at any point, replace `result` with a `ParseError::UnknownToken`
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
            return Err(ParseError::UnknownToken {
                src: self.named_source(),
                span: span.into(),
            });
        }
        result
    }

    /// Consume the next token, returning an error if the lexer is exhausted.
    ///
    /// Use this after `peek()` has confirmed `Some`.
    pub(super) fn advance(&mut self) -> Result<(Token, Span), ParseError> {
        self.lexer
            .next_token()
            .ok_or_else(|| self.unexpected_eof("token"))
    }

    /// Parse a finite `f64` literal from already-normalized token text.
    pub(super) fn parse_finite_f64_literal(
        &self,
        text: &str,
        span: Span,
    ) -> Result<f64, ParseError> {
        let value: f64 =
            text.parse()
                .map_err(|e: std::num::ParseFloatError| ParseError::InvalidNumber {
                    reason: e.to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ParseError::InvalidNumber {
                reason: "floating-point literal must be finite".to_string(),
                src: self.named_source(),
                span: span.into(),
            })
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
            return Err(self.unexpected_token("end of input", &tok.to_string(), span));
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
            return Err(self.unexpected_token("end of input", &tok.to_string(), span));
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
            return Err(self.unexpected_token("end of input", &tok.to_string(), span));
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

    fn parse_file_inner(&mut self) -> Result<crate::syntax::ast::File, ParseError> {
        let mut declarations = Vec::new();
        while self.lexer.peek().is_some() {
            declarations.push(self.parse_declaration()?);
        }
        Ok(crate::syntax::ast::File { declarations })
    }

    // --- Helper methods ---

    pub(super) fn expect(&mut self, expected: Token) -> Result<(Token, Span), ParseError> {
        let expected_str = format!("`{expected}`");
        match self.lexer.next_token() {
            Some((tok, span)) if tok == expected => Ok((tok, span)),
            Some((tok, span)) => Err(self.unexpected_token(&expected_str, &tok.to_string(), span)),
            None => Err(self.unexpected_eof(&expected_str)),
        }
    }

    /// Parse a comma-separated list of items until `end_token` is peeked.
    ///
    /// Supports trailing commas. Does **not** consume the `end_token`.
    pub(super) fn parse_comma_separated<T>(
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

    /// Parse any identifier regardless of casing.
    pub(super) fn parse_any_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => Ok(Ident {
                name: NameAtom::new_unchecked_for_parser(self.lexer.slice_at(span).to_string()),
                span,
            }),
            Some((tok, span)) => Err(self.unexpected_token("identifier", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("identifier")),
        }
    }

    /// Parse a non-empty dot-separated identifier path.
    pub(super) fn parse_ident_path(&mut self) -> Result<IdentPath, ParseError> {
        let first = self.parse_any_ident()?;
        let mut rest = Vec::new();
        while self.lexer.peek() == Some(&Token::Dot)
            && self.lexer.peek_second() == Some(&Token::Ident)
        {
            self.lexer.next_token(); // consume `.`
            rest.push(self.parse_any_ident()?);
        }
        Ok(IdentPath::new(crate::syntax::non_empty::NonEmpty::new(
            first, rest,
        )))
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::parser::{ParseError, Parser};

    #[test]
    fn stray_character_in_source_surfaces_as_unknown_token() {
        let input = "param x = 1.0; §";
        let mut parser = Parser::new(input);
        let err = parser.parse_file().expect_err("expected parse error");
        match err {
            ParseError::UnknownToken { span, .. } => {
                let byte_start: usize = span.offset();
                let byte_end = byte_start + span.len();
                assert_eq!(&input[byte_start..byte_end], "§");
            }
            other => panic!("expected UnknownToken, got {other:?}"),
        }
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
            matches!(err, ParseError::UnknownToken { .. }),
            "expected UnknownToken, got {err:?}"
        );
    }
}
