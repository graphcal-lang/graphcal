use crate::exact_rational::{ExactRational, ExactRationalError};
use crate::source_line::line_ending_at;
use crate::syntax::ast::{
    BinOp, Expr, ExprKind, FieldInit, Ident, IndexArg, KeyFormKind, ModulePath, PowerExponent,
    UnaryOp,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::{ContextualKeyword, Token};
use crate::syntax::type_name::FieldName;

use super::{ParseError, Parser};

fn skip_ws_and_line_comments(bytes: &[u8], mut pos: usize) -> usize {
    loop {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos + 1 < bytes.len() && bytes[pos] == b'/' && bytes[pos + 1] == b'/' {
            while pos < bytes.len() && line_ending_at(bytes, pos).is_none() {
                pos += 1;
            }
            continue;
        }
        return pos;
    }
}

fn scan_ascii_ident(bytes: &[u8], pos: usize) -> Option<usize> {
    if pos >= bytes.len() || (!bytes[pos].is_ascii_alphabetic() && bytes[pos] != b'_') {
        return None;
    }
    let mut end = pos + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Some(end)
}

/// Parse a decimal/scientific literal as an exact rational when its reduced
/// value fits the HIR exact-exponent representation.
fn exact_decimal_literal(text: &str) -> Option<ExactRational> {
    let normalized = text.replace('_', "");
    let (mantissa, exponent) = match normalized.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (normalized.as_str(), 0_i32),
    };
    let (whole, fractional) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fractional}");
    let numerator = digits.parse::<i128>().ok()?;
    let fractional_digits = i32::try_from(fractional.len()).ok()?;
    let decimal_scale = fractional_digits.checked_sub(exponent)?;

    let (numerator, denominator) = if decimal_scale >= 0 {
        let scale = u32::try_from(decimal_scale).ok()?;
        (numerator, 10_i128.checked_pow(scale)?)
    } else {
        let scale = decimal_scale
            .checked_neg()
            .and_then(|n| u32::try_from(n).ok())?;
        (numerator.checked_mul(10_i128.checked_pow(scale)?)?, 1)
    };
    ExactRational::try_new_i128(numerator, denominator).ok()
}

fn signed_integer_expr(expr: &Expr) -> Option<i64> {
    match &expr.kind {
        ExprKind::Integer(value) => Some(*value),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match operand.kind {
            ExprKind::Integer(value) => value.checked_neg(),
            _ => None,
        },
        _ => None,
    }
}

fn signed_float_literal_span(expr: &Expr) -> Option<(bool, Span)> {
    match &expr.kind {
        ExprKind::Number(_) => Some((false, expr.span)),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } if matches!(operand.kind, ExprKind::Number(_)) => Some((true, operand.span)),
        _ => None,
    }
}

/// Map comparison tokens to their corresponding `BinOp`.
const fn token_to_comparison_op(token: Token) -> Option<BinOp> {
    match token {
        Token::EqEq => Some(BinOp::Eq),
        Token::BangEq => Some(BinOp::Ne),
        Token::Lt => Some(BinOp::Lt),
        Token::Gt => Some(BinOp::Gt),
        Token::LtEq => Some(BinOp::Le),
        Token::GtEq => Some(BinOp::Ge),
        _ => None,
    }
}

impl Parser<'_> {
    // --- Expression parsing ---
    // Precedence (lowest to highest):
    //   0. -> (conversion, lowest)
    //   1. if/else (conditional)
    //   2. || (or)
    //   3. && (and)
    //   4. ==, !=, <, >, <=, >= (comparison, non-chaining)
    //   5. +, - (additive)
    //   6. *, / (multiplicative)
    //   7. unary -, ! (prefix)
    //   8. ^ (power, right-associative)
    //   9. atoms (including NUMBER UNIT_EXPR)

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.with_nesting_budget(Self::parse_convert)
    }

    /// Parse a comma-separated argument list: `(expr1, expr2, ...)`
    /// Expects the opening `(` to have already been consumed.
    /// Supports trailing commas.
    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.parse_comma_separated(Token::RParen, Self::parse_expr)
    }

    /// Parse a single named field initializer for a constructor call:
    /// `field: expr`. Used inside the paren-form `Ctor(field: expr, ...)`.
    fn parse_named_field_init(&mut self) -> Result<FieldInit, ParseError> {
        let ident = self.parse_any_ident()?;
        let name = ident.into_spanned::<FieldName>();
        self.expect(Token::Colon)?;
        let value = self.parse_expr()?;
        Ok(FieldInit { name, value })
    }

    /// Parse conversion: `expr -> unit_expr` (lowest precedence).
    fn parse_convert(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_conditional()?;

        if self.lexer.peek() == Some(&Token::Arrow) {
            self.lexer.next_token();
            // If the next token is a string literal, this is a timezone display conversion
            if self.lexer.peek() == Some(&Token::StringLiteral) {
                let (_, tz_span) = self.advance()?;
                let raw = self.lexer.slice_at(tz_span);
                let timezone = raw[1..raw.len() - 1].to_string();
                let span = expr.span.merge(tz_span);
                return Ok(Expr::new(
                    ExprKind::DisplayTimezone {
                        expr: Box::new(expr),
                        timezone,
                    },
                    span,
                ));
            }
            let target = self.parse_unit_expr()?;
            let span = expr.span.merge(target.span);
            Ok(Expr::new(
                ExprKind::Convert {
                    expr: Box::new(expr),
                    target,
                },
                span,
            ))
        } else {
            Ok(expr)
        }
    }

    fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
        if self.lexer.peek() == Some(&Token::If) {
            let (_, if_span) = self.advance()?;
            let condition = self.parse_expr()?;
            self.expect(Token::LBrace)?;
            let then_branch = self.parse_expr()?;
            self.expect(Token::RBrace)?;
            self.expect(Token::Else)?;
            self.expect(Token::LBrace)?;
            let else_branch = self.parse_expr()?;
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            let span = if_span.merge(rbrace_span);
            Ok(Expr::new(
                ExprKind::If {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
                span,
            ))
        } else {
            self.parse_or()
        }
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.lexer.peek() == Some(&Token::PipePipe) {
            self.lexer.next_token();
            let rhs = self.parse_and()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_comparison()?;
        while self.lexer.peek() == Some(&Token::AmpAmp) {
            self.lexer.next_token();
            let rhs = self.parse_comparison()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = self.lexer.peek().copied().and_then(token_to_comparison_op);
        if let Some(op) = op {
            self.lexer.next_token();
            let rhs = self.parse_add()?;
            let span = lhs.span.merge(rhs.span);
            Ok(Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            ))
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.lexer.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.lexer.next_token();
            let rhs = self.parse_mul()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.lexer.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                Some(Token::Percent) => BinOp::Mod,
                _ => break,
            };
            self.lexer.next_token();
            let rhs = self.parse_unary()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Ok(lhs)
    }

    pub(super) fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.with_nesting_budget(Self::parse_unary_inner)
    }

    fn parse_unary_inner(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Minus) => {
                let (_, op_span) = self.advance()?;
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            Some(Token::Bang) => {
                let (_, op_span) = self.advance()?;
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Expr, ParseError> {
        let base = self.parse_postfix()?;
        if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            // Right-associative: recurse into parse_unary, not parse_power.
            let exponent = self.parse_unary()?;
            let exponent_kind = self.classify_power_exponent(&exponent)?;
            let span = base.span.merge(exponent.span);
            Ok(Expr::new(
                ExprKind::BinOp {
                    op: BinOp::Pow(exponent_kind),
                    lhs: Box::new(base),
                    rhs: Box::new(exponent),
                },
                span,
            ))
        } else {
            Ok(base)
        }
    }

    /// Preserve exact exponent syntax before HIR loses token spelling.
    fn classify_power_exponent(&self, exponent: &Expr) -> Result<PowerExponent, ParseError> {
        if let Some(value) = signed_integer_expr(exponent) {
            return Ok(PowerExponent::Exact(ExactRational::from_integer(value)));
        }

        if let ExprKind::BinOp {
            op: BinOp::Div,
            lhs,
            rhs,
        } = &exponent.kind
            && let (Some(numerator), Some(denominator)) =
                (signed_integer_expr(lhs), signed_integer_expr(rhs))
        {
            return ExactRational::try_new(numerator, denominator)
                .map(PowerExponent::Exact)
                .map_err(|error| ParseError::InvalidNumber {
                    reason: match error {
                        ExactRationalError::ZeroDenominator => {
                            "power exponent denominator must be non-zero".to_string()
                        }
                        ExactRationalError::Overflow => {
                            "exact power exponent overflows `i64`".to_string()
                        }
                    },
                    src: self.named_source(),
                    span: exponent.span.into(),
                });
        }

        if let Some((negative, literal_span)) = signed_float_literal_span(exponent) {
            let exact =
                exact_decimal_literal(self.lexer.slice_at(literal_span)).and_then(|value| {
                    if negative {
                        value.checked_neg().ok()
                    } else {
                        Some(value)
                    }
                });
            return Ok(PowerExponent::FloatSyntax { exact });
        }

        Ok(PowerExponent::Runtime)
    }

    /// Apply postfix operators (field access `.field`, index access `[i]`) to an already-parsed expression.
    fn apply_postfix(&mut self, mut expr: Expr) -> Result<Expr, ParseError> {
        loop {
            match self.lexer.peek() {
                Some(Token::Dot) => {
                    self.lexer.next_token(); // consume '.'
                    let field_ident = self.parse_any_ident()?;
                    let span = expr.span.merge(field_ident.span);
                    expr = Expr::new(
                        ExprKind::FieldAccess {
                            expr: Box::new(expr),
                            field: field_ident.into_spanned::<FieldName>(),
                        },
                        span,
                    );
                }
                Some(Token::LBracket) => {
                    self.lexer.next_token(); // consume '['
                    let args = self
                        .parse_non_empty_comma_separated(Token::RBracket, Self::parse_index_arg)?;
                    let (_, end_span) = self.expect(Token::RBracket)?;
                    let span = expr.span.merge(end_span);
                    expr = Expr::new(
                        ExprKind::IndexAccess {
                            expr: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// Parse postfix operators (field access `.field`, index access `[i]`).
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_atom()?;
        self.apply_postfix(expr)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek().copied() {
            Some(Token::Number) => self.parse_number_expr(),
            Some(Token::True) => {
                let (_, span) = self.advance()?;
                Ok(Expr::new(ExprKind::Bool(true), span))
            }
            Some(Token::False) => {
                let (_, span) = self.advance()?;
                Ok(Expr::new(ExprKind::Bool(false), span))
            }
            Some(Token::StringLiteral) => {
                let (_, span) = self.advance()?;
                let raw = self.lexer.slice_at(span);
                // Strip surrounding quotes
                let text = raw[1..raw.len() - 1].to_string();
                Ok(Expr::new(ExprKind::StringLiteral(text), span))
            }
            Some(Token::At) => self.parse_at_expr(),
            Some(Token::ContextualKeyword(ContextualKeyword::Scan))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_scan(span)
            }
            Some(Token::ContextualKeyword(ContextualKeyword::Unfold))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_unfold(span)
            }
            Some(Token::ContextualKeyword(ContextualKeyword::Key))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_key_form(KeyFormKind::Static, span)
            }
            Some(Token::ContextualKeyword(ContextualKeyword::FinKey))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_key_form(KeyFormKind::Fin, span)
            }
            Some(Token::ContextualKeyword(ContextualKeyword::FloorKey))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_key_form(KeyFormKind::Floor, span)
            }
            Some(Token::ContextualKeyword(ContextualKeyword::CeilKey))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_key_form(KeyFormKind::Ceil, span)
            }
            Some(Token::ContextualKeyword(ContextualKeyword::NearestKey))
                if self.lexer.peek_second() == Some(&Token::LParen) =>
            {
                let (_, span) = self.advance()?;
                self.parse_key_form(KeyFormKind::Nearest, span)
            }
            Some(token) if token.is_identifier() => self.parse_identifier_expr(),
            Some(Token::For) => {
                // For comprehension: for m: Maneuver { expr }
                self.parse_for_comp()
            }
            Some(Token::LBrace) => self.parse_brace_expr(),
            Some(Token::Table) => self.parse_table_expr(),
            Some(Token::Match) => self.parse_match_expr(),
            Some(Token::LParen) => {
                self.lexer.next_token();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(_) => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("expression", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("expression")),
        }
    }

    /// Parse a syntax-directed `@` reference.
    ///
    /// - `@value.field` reads a local graph Term and projects runtime fields.
    /// - `@module.child::value.field` crosses one explicit member boundary.
    /// - `@module.child(args)::output.field` creates a fresh DAG instance.
    fn parse_at_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, at_span) = self.advance()?;
        let first = self.parse_any_ident()?;

        if self.lexer.peek() == Some(&Token::LParen) {
            return self.finish_inline_dag_call(at_span, vec![first]);
        }

        let mut probe = self.lexer.clone();
        while probe.peek() == Some(&Token::Dot)
            && probe
                .peek_second()
                .is_some_and(|token| token.is_identifier())
        {
            probe.next_token();
            probe.next_token();
        }
        let terminator = probe.peek().copied();
        if !matches!(terminator, Some(Token::LParen | Token::DoubleColon)) {
            let span = at_span.merge(first.span);
            let name = ScopedName::from(DeclName::from_atom(first.name));
            return Ok(Expr::new(
                ExprKind::GraphRef(Spanned::new(name, span)),
                span,
            ));
        }

        let mut namespace = vec![first];
        while self.lexer.peek() == Some(&Token::Dot) {
            self.lexer.next_token();
            namespace.push(self.parse_any_ident()?);
        }

        if terminator == Some(Token::LParen) {
            return self.finish_inline_dag_call(at_span, namespace);
        }

        self.expect(Token::DoubleColon)?;
        let member = self.parse_any_ident()?;
        let member_span = member.span;
        let scoped = ScopedName::qualified_path(
            namespace.into_iter().map(|segment| {
                crate::syntax::module_name::ModuleAliasName::from_atom(segment.name)
            }),
            DeclName::from_atom(member.name),
        );
        Ok(Expr::new(
            ExprKind::GraphRef(Spanned::new(scoped, at_span.merge(member_span))),
            at_span.merge(member_span),
        ))
    }

    /// Finish an inline DAG call after its dotted namespace path. The output
    /// after `::` is mandatory because a DAG instance is not itself a value.
    fn finish_inline_dag_call(
        &mut self,
        at_span: crate::syntax::span::Span,
        segments: Vec<Ident>,
    ) -> Result<Expr, ParseError> {
        #[expect(
            clippy::expect_used,
            reason = "callers seed inline DAG paths with at least one segment"
        )]
        let segments = crate::syntax::non_empty::NonEmpty::try_from_vec(segments)
            .expect("inline dag call must have at least one path segment");
        let path_start = at_span.merge(segments.first().span);
        let path_end = segments.last().span;
        let path = ModulePath {
            segments,
            span: path_start.merge(path_end),
        };
        let args = self.parse_import_param_bindings()?;
        match self.lexer.peek_with_span() {
            Some((Token::DoubleColon, _)) => {
                self.lexer.next_token();
            }
            Some((_, span)) => {
                return Err(ParseError::InlineDagCallMissingProjection {
                    src: self.named_source(),
                    span: span.into(),
                });
            }
            None => {
                return Err(self.unexpected_eof("`::<output>` projection"));
            }
        }
        let output = self.parse_any_ident()?;
        let span = at_span.merge(output.span);
        Ok(Expr::new(
            ExprKind::InlineDagRef {
                path,
                args,
                output: output.into_spanned::<DeclName>(),
            },
            span,
        ))
    }

    /// Return the next token's span when it can begin a unit expression on
    /// the same line as `literal_span`.
    ///
    /// Unit suffixes can begin with a name (`2.0 m`), a group
    /// (`2.0 (m/s)`), or the exact reciprocal shorthand (`2.0 1/s`). The
    /// token-based classification keeps ordinary division (`2.0 / ...`) in
    /// the expression grammar.
    fn same_line_unit_expr_start(&mut self, literal_span: Span) -> Option<Span> {
        let (token, start_span) = self.lexer.peek_with_span()?;
        let token = *token;
        let starts_unit_expr = token.is_identifier()
            || token == Token::LParen
            || (token == Token::Number
                && self.lexer.slice_at(start_span) == "1"
                && self.lexer.peek_second() == Some(&Token::Slash));

        (starts_unit_expr && !self.has_line_break_between(literal_span, start_span))
            .then_some(start_span)
    }

    /// Parse a number literal: integer, float, or float with unit.
    fn parse_number_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, span) = self.advance()?;
        let text = self.lexer.slice_at(span).replace('_', "");
        let is_integer = !text.contains('.') && !text.contains('e') && !text.contains('E');

        if is_integer {
            // Integer literal: no decimal point or scientific notation.
            if self.same_line_unit_expr_start(span).is_some() {
                // Integer followed by unit is an error: must use float
                return Err(ParseError::InvalidNumber {
                    reason: format!("integer literal cannot have units; write `{text}.0` instead"),
                    src: self.named_source(),
                    span: span.into(),
                });
            }
            let value: i64 =
                text.parse()
                    .map_err(|e: std::num::ParseIntError| ParseError::InvalidNumber {
                        reason: e.to_string(),
                        src: self.named_source(),
                        span: span.into(),
                    })?;
            Ok(Expr::new(ExprKind::Integer(value), span))
        } else {
            // Floating-point literal: has a decimal point or scientific notation
            let value = self.parse_finite_f64_literal(&text, span)?;

            // A newline starts the next syntactic item; without this guard,
            // a missing comma between match arms could be misread as a unit
            // suffix on the preceding literal.
            if self.same_line_unit_expr_start(span).is_some() {
                let unit_expr = self.parse_unit_expr()?;
                let full_span = span.merge(unit_expr.span);
                Ok(Expr::new(
                    ExprKind::QuantityLiteral {
                        value,
                        unit: unit_expr,
                    },
                    full_span,
                ))
            } else {
                Ok(Expr::new(ExprKind::Number(value), span))
            }
        }
    }

    fn has_line_break_between(&self, left: Span, right: Span) -> bool {
        let start = left.offset() + left.len();
        let end = right.offset();
        self.source
            .get(start..end)
            .is_some_and(|gap| gap.contains('\n') || gap.contains('\r'))
    }

    /// Parse an identifier-based expression.
    ///
    /// Dispatches on following tokens without namespace probing:
    /// - `Index#Label` / `module::Index#Label` → owner-qualified label
    /// - `callee<T>(args)` / `callee(args)` → one Term call syntax
    /// - local/member name → unresolved Term reference
    ///
    /// The old brace-form construction `Name { field: val }` is no
    /// longer accepted — constructor calls use parens.
    fn parse_identifier_expr(&mut self) -> Result<Expr, ParseError> {
        let path = self.parse_ident_path()?;

        if self.lexer.peek() == Some(&Token::Hash) {
            self.lexer.next_token();
            let label = self.parse_any_ident()?.into_spanned::<IndexVariantName>();
            let span = path.span().merge(label.span);
            return Ok(Expr::new(
                ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::IndexLabel {
                    index: path,
                    label,
                    span,
                }),
                span,
            ));
        }

        if self.lexer.peek() == Some(&Token::LParen)
            || (self.lexer.peek() == Some(&Token::Lt) && self.is_generic_args_followed_by_paren())
        {
            // `path(args)` or `path<T>(args)`. The path is syntactic: bare
            // and qualified callees have the same AST representation. We only
            // use argument shape to distinguish constructor-call syntax (named
            // args) from function-call syntax (positional args).
            let generic_args = if self.lexer.peek() == Some(&Token::Lt) {
                let (generic_args, _) = self.parse_generic_arg_list()?;
                generic_args.into_vec()
            } else {
                vec![]
            };
            if self.is_named_arg_call() {
                self.lexer.next_token(); // consume `(`
                let fields =
                    self.parse_comma_separated(Token::RParen, Self::parse_named_field_init)?;
                let (_, rparen_span) = self.expect(Token::RParen)?;
                let call_span = path.span().merge(rparen_span);
                return Ok(Expr::new(
                    ExprKind::ConstructorCall {
                        callee: path,
                        generic_args,
                        fields,
                    },
                    call_span,
                ));
            }
            self.lexer.next_token(); // consume '('
            let args = self.parse_arg_list()?;
            let (_, rparen_span) = self.expect(Token::RParen)?;
            let call_span = path.span().merge(rparen_span);
            Ok(Expr::new(
                ExprKind::FnCall {
                    callee: path,
                    generic_args,
                    args,
                },
                call_span,
            ))
        } else {
            Ok(Expr::new(
                ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path.clone())),
                path.span(),
            ))
        }
    }

    /// Parse a brace-delimited expression (map literal).
    fn parse_brace_expr(&mut self) -> Result<Expr, ParseError> {
        // Consume '{' and peek at what follows
        let (_, start_span) = self.advance()?;
        if self.lexer.peek().is_some_and(|token| token.is_identifier()) {
            let (index, variant, _) = self.parse_index_variant_path()?;
            if self.lexer.peek() == Some(&Token::Colon) {
                self.parse_map_literal_after_first_entry(start_span, index, variant)
            } else {
                let (found, found_span) = self.lexer.peek_with_span().map_or_else(
                    || ("EOF".to_string(), start_span),
                    |(tok, span)| (tok.to_string(), span),
                );
                Err(self.unexpected_token(
                    "`:` after label in map literal",
                    &found,
                    found_span,
                ))
            }
        } else if self.lexer.peek() == Some(&Token::LParen) {
            // Could be tuple-key map literal: { (Index#Label, ...): expr, ... }
            self.parse_tuple_key_map_literal(start_span)
        } else {
            let (found, found_span) = self.lexer.peek_with_span().map_or_else(
                || ("EOF".to_string(), start_span),
                |(tok, span)| (tok.to_string(), span),
            );
            Err(self.unexpected_token(
                "map literal (`{ Index#Variant: expr, ... }`)",
                &found,
                found_span,
            ))
        }
    }

    // --- Index access ---

    pub(super) fn parse_index_variant_path(
        &mut self,
    ) -> Result<(Spanned<NamePath>, Spanned<IndexVariantName>, Span), ParseError> {
        let index = self.parse_ident_path()?;
        let index_span = index.span();
        self.expect(Token::Hash)?;
        let variant = self.parse_any_ident()?.into_spanned::<IndexVariantName>();
        let full_span = index_span.merge(variant.span);
        Ok((index.into_spanned_name_path(), variant, full_span))
    }

    /// Parse an index argument. `#` identifies owner-qualified labels before
    /// semantic lookup; a bare identifier remains a lexical key variable.
    fn parse_index_arg(&mut self) -> Result<IndexArg, ParseError> {
        let expr = self.parse_expr()?;

        match &expr.kind {
            ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::IndexLabel {
                index,
                label,
                ..
            }) => Ok(IndexArg::Variant {
                index: index.clone().into_spanned_name_path(),
                variant: label.clone(),
            }),
            ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
                match path.clone().into_bare() {
                    Ok(ident) => Ok(IndexArg::Var(ident)),
                    Err(path) => Ok(IndexArg::Expr(Box::new(Expr::new(
                        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)),
                        expr.span,
                    )))),
                }
            }
            _ => Ok(IndexArg::Expr(Box::new(expr))),
        }
    }

    /// Scan balanced `<…>` angle brackets from the current position and check
    /// whether the byte immediately after `>` (skipping whitespace) equals `expected`.
    ///
    /// Returns `false` if the next token is not `<`, the brackets are
    /// unbalanced, or a byte that can never occur inside a generic-argument
    /// list shows up first — in that case the `<` is a comparison operator,
    /// so an ordinary boolean expression like `a < b && c > (d)` is not
    /// misparsed as turbofish. Line comments are skipped so their contents
    /// affect neither the bracket balance nor the operator bail-out.
    fn are_generic_args_followed_by(&mut self, expected: u8) -> bool {
        let Some((&Token::Lt, lt_span)) = self.lexer.peek_with_span() else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let mut pos = lt_span.offset() + lt_span.len(); // byte after `<`
        let mut depth: usize = 1;
        while pos < bytes.len() {
            match bytes[pos] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        let p = skip_ws_and_line_comments(bytes, pos + 1);
                        return p < bytes.len() && bytes[p] == expected;
                    }
                }
                b'/' if bytes.get(pos + 1) == Some(&b'/') => {
                    while pos < bytes.len() && line_ending_at(bytes, pos).is_none() {
                        pos += 1;
                    }
                    continue;
                }
                // Sort-aware generic arguments are type or Nat expressions;
                // none of these bytes can occur in them.
                b'&' | b'|' | b';' | b'=' | b'{' | b'}' | b'"' | b'@' | b'!' => return false,
                _ => {}
            }
            pos += 1;
        }
        false
    }

    /// Look ahead to check if `(` starts tuple-key sugar: `(ident, ident, ...) =>`.
    ///
    /// Scans the raw source string from the current position without consuming tokens.
    /// Returns `true` only if the `(...)` contains only identifiers, commas,
    /// whitespace, and line comments, followed by `)` then `=>`.
    pub(super) fn is_tuple_key_sugar(&mut self) -> bool {
        let Some((&Token::LParen, lp_span)) = self.lexer.peek_with_span() else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let mut pos = lp_span.offset() + lp_span.len(); // byte after `(`

        // Scan for matching `)`: expect only identifiers, commas, whitespace,
        // and line comments inside.
        loop {
            pos = skip_ws_and_line_comments(bytes, pos);
            if pos >= bytes.len() {
                return false;
            }
            if bytes[pos] == b')' {
                // Found `)`, now check for `=>`
                pos += 1;
                pos = skip_ws_and_line_comments(bytes, pos);
                return pos + 1 < bytes.len() && bytes[pos] == b'=' && bytes[pos + 1] == b'>';
            }
            let Some(ident_end) = scan_ascii_ident(bytes, pos) else {
                return false;
            };
            pos = skip_ws_and_line_comments(bytes, ident_end);
            if pos >= bytes.len() {
                return false;
            }
            // After identifier, expect `,` or `)` (`)` handled by loop top)
            if bytes[pos] == b',' {
                pos += 1; // consume `,`
            } else if bytes[pos] != b')' {
                return false;
            } else {
                // It's `)`, will be handled at the start of the next iteration
            }
        }
    }

    /// Look ahead to check if `<...>` is followed by `(`.
    /// Used to disambiguate `eye<3>()` (turbofish fn call)
    /// from `f < x` (comparison).
    fn is_generic_args_followed_by_paren(&mut self) -> bool {
        self.are_generic_args_followed_by(b'(')
    }

    /// After peeking `(`, look ahead to decide whether the parenthesized
    /// argument list is a *constructor call* (named args, `field: expr,
    /// ...`) or a *function call* (positional args). Returns `true` if
    /// the first argument is of the form `IDENT :` — the constructor
    /// form. Pure byte scan; the lexer is not advanced.
    ///
    /// Empty `()` is treated as a function call (returns `false`) — a
    /// unit constructor is written `Ctor`, not `Ctor()`. A `Ctor()` call
    /// site with empty parens parses as a zero-arg function call and
    /// later fails at name-resolution time if no such function exists.
    fn is_named_arg_call(&mut self) -> bool {
        let Some((&Token::LParen, lp_span)) = self.lexer.peek_with_span() else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let mut pos = lp_span.offset() + lp_span.len();

        pos = skip_ws_and_line_comments(bytes, pos);
        if pos >= bytes.len() {
            return false;
        }

        let Some(ident_end) = scan_ascii_ident(bytes, pos) else {
            return false;
        };
        pos = skip_ws_and_line_comments(bytes, ident_end);
        if pos >= bytes.len() {
            return false;
        }
        // `:` (single colon) signals a named argument. Graphcal's surface
        // module path uses `.`, so a second `:` here would be a stray token;
        // either way, we only commit to the named-arg path on a lone `:`.
        bytes[pos] == b':' && bytes.get(pos + 1).is_none_or(|c| *c != b':')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{BinOp, DeclKind, ExprKind, UnaryOp};

    fn parse_node_expr(input: &str) -> crate::syntax::ast::Expr {
        let full = format!("node x: Dimensionless = {input};");
        let file = Parser::new(&full).parse_file().unwrap();
        match file.declarations.into_iter().next().unwrap().kind {
            DeclKind::Node(n) => n.value,
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn deeply_nested_parens_error_instead_of_stack_overflow() {
        // Regression: the recursive-descent parser had no depth bound, so
        // pathological nesting overflowed the stack and aborted the process
        // (including the LSP server hosting the parser).
        let depth = 100_000;
        let src = format!(
            "node x: Dimensionless = {}1.0{};",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let err = Parser::new(&src).parse_file().unwrap_err();
        assert!(matches!(err, ParseError::TooDeeplyNested { .. }), "{err:?}");
    }

    #[test]
    fn deeply_nested_unary_chain_errors_instead_of_stack_overflow() {
        let src = format!("node x: Dimensionless = {}1.0;", "-".repeat(100_000));
        let err = Parser::new(&src).parse_file().unwrap_err();
        assert!(matches!(err, ParseError::TooDeeplyNested { .. }), "{err:?}");
    }

    #[test]
    fn reasonable_nesting_stays_below_the_depth_limit() {
        // Realistic engineering formulas nest a few dozen levels at most;
        // 60 nested parens must keep parsing fine.
        let depth = 60;
        let src = format!(
            "node x: Dimensionless = {}1.0{};",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        Parser::new(&src).parse_file().unwrap();
    }

    #[test]
    fn long_operator_chains_are_not_depth_limited() {
        // Left-nested chains are parsed iteratively; a 10k-term sum must not
        // trip the nesting bound.
        let src = format!(
            "node x: Dimensionless = {};",
            vec!["1.0"; 10_000].join(" + ")
        );
        Parser::new(&src).parse_file().unwrap();
    }

    #[test]
    fn brace_expr_error_points_at_offending_token() {
        let source = "node x: Dimensionless = { nope };";
        let err = Parser::new(source).parse_file().unwrap_err();
        match err {
            ParseError::UnexpectedToken { found, span, .. } => {
                assert_eq!(found, "}");
                assert_eq!(span.offset(), source.find('}').unwrap());
            }
            other => panic!("expected UnexpectedToken, got {other:?}"),
        }
    }

    #[test]
    fn parse_same_line_unicode_string_literal() {
        let expr = parse_node_expr(r#""Δv 🚀""#);
        assert!(matches!(
            expr.kind,
            ExprKind::StringLiteral(ref text) if text == "Δv 🚀"
        ));
    }

    #[test]
    fn reject_string_literals_with_physical_line_breaks() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let source = format!("node label: Dimensionless = \"first{line_ending}second\";");
            let error = Parser::new(&source).parse_file().unwrap_err();
            assert!(
                matches!(error, ParseError::UnknownToken { .. }),
                "line ending {line_ending:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn parse_quantity_literal() {
        let file = Parser::new("param alt: Length = 400.0 km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::QuantityLiteral { value, unit } => {
                    assert!((value - 400.0).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 1);
                    assert_eq!(unit.terms[0].name.value.to_string(), "km");
                }
                _ => panic!("expected QuantityLiteral"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_compound_quantity_literal() {
        let file = Parser::new("const node g0: Acceleration = 9.80665 m/s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::ConstNode(c) => match &c.value.kind {
                ExprKind::QuantityLiteral { value, unit } => {
                    assert!((value - 9.80665).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 2);
                    assert_eq!(unit.terms[0].name.value.to_string(), "m");
                    assert_eq!(unit.terms[1].op, crate::syntax::ast::MulDivOp::Div);
                    assert_eq!(unit.terms[1].name.value.to_string(), "s");
                    assert_eq!(
                        unit.terms[1].power,
                        Some(crate::dimension::Rational::from(2))
                    );
                }
                _ => panic!("expected QuantityLiteral"),
            },
            _ => panic!("expected const"),
        }
    }

    #[test]
    fn parse_reciprocal_quantity_literal() {
        let expr = Parser::new("2.0 1/s").parse_single_expr().unwrap();
        let ExprKind::QuantityLiteral { value, unit } = &expr.kind else {
            panic!("expected reciprocal QuantityLiteral");
        };
        assert!((*value - 2.0).abs() < f64::EPSILON);
        assert_eq!(unit.terms.len(), 1);
        assert_eq!(unit.terms[0].op, crate::syntax::ast::MulDivOp::Div);
        assert_eq!(unit.terms[0].name.value.to_string(), "s");
    }

    #[test]
    fn parse_grouped_quantity_literal() {
        let expr = Parser::new("3.0 (m/s)").parse_single_expr().unwrap();
        let ExprKind::QuantityLiteral { value, unit } = &expr.kind else {
            panic!("expected grouped QuantityLiteral");
        };
        assert!((*value - 3.0).abs() < f64::EPSILON);
        assert_eq!(unit.terms.len(), 2);
        assert_eq!(unit.terms[0].op, crate::syntax::ast::MulDivOp::Mul);
        assert_eq!(unit.terms[0].name.value.to_string(), "m");
        assert_eq!(unit.terms[1].op, crate::syntax::ast::MulDivOp::Div);
        assert_eq!(unit.terms[1].name.value.to_string(), "s");
    }

    #[test]
    fn division_by_parenthesized_quantity_literal_remains_arithmetic() {
        let expr = Parser::new("1.0 / (2.0 m)").parse_single_expr().unwrap();
        let ExprKind::BinOp { op, lhs, rhs } = &expr.kind else {
            panic!("expected arithmetic division");
        };
        assert_eq!(*op, BinOp::Div);
        assert!(matches!(lhs.kind, ExprKind::Number(value) if (value - 1.0).abs() < f64::EPSILON));
        assert!(matches!(rhs.kind, ExprKind::QuantityLiteral { .. }));
    }

    #[test]
    fn reciprocal_quantity_literal_requires_exact_integer_one() {
        for source in ["2.0 1.0/s", "2.0 1_0/s"] {
            let error = Parser::new(source).parse_single_expr().unwrap_err();
            assert!(
                matches!(error, ParseError::UnexpectedToken { .. }),
                "`{source}` must not be classified as a reciprocal unit suffix: {error:?}"
            );
        }
    }

    #[test]
    fn parse_conversion() {
        let file = Parser::new("node speed_kmh: Velocity = @speed -> km/h;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { expr, target } => {
                    assert!(
                        matches!(&expr.kind, ExprKind::GraphRef(id) if id.value.member() == "speed")
                    );
                    assert_eq!(target.terms.len(), 2);
                    assert_eq!(target.terms[0].name.value.to_string(), "km");
                    assert_eq!(target.terms[1].op, crate::syntax::ast::MulDivOp::Div);
                    assert_eq!(target.terms[1].name.value.to_string(), "h");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_conversion_with_qualified_unit() {
        let file = Parser::new("node b: Length = @a -> u::mile;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { target, .. } => {
                    assert_eq!(target.terms.len(), 1);
                    let unit_ref = &target.terms[0].name.value;
                    assert_eq!(
                        unit_ref.qualifier().map(ToString::to_string).as_deref(),
                        Some("u")
                    );
                    assert_eq!(unit_ref.name().as_str(), "mile");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_qualified_quantity_literal() {
        let file = Parser::new("node d: Length = 2.0 u::mile;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::QuantityLiteral { unit, .. } => {
                    assert_eq!(unit.terms[0].name.value.to_string(), "u::mile");
                }
                _ => panic!("expected QuantityLiteral"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_unit_member_accepts_nested_dag_owner() {
        let file = Parser::new("node b: Length = @a -> app.units::mile;")
            .parse_file()
            .unwrap();
        let DeclKind::Node(node) = &file.declarations[0].kind else {
            panic!("expected node");
        };
        let ExprKind::Convert { target, .. } = &node.value.kind else {
            panic!("expected conversion");
        };
        assert_eq!(target.terms[0].name.value.to_string(), "app.units::mile");
    }

    #[test]
    fn parse_convert_binds_loosely() {
        let file = Parser::new("node x: Length = @a + @b -> km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { expr, target } => {
                    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
                    assert_eq!(target.terms[0].name.value.to_string(), "km");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_arithmetic_precedence() {
        let expr = parse_node_expr("1.0 + 2.0 * 3.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        if let ExprKind::BinOp { rhs, .. } = &expr.kind {
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Mul, .. }));
        }
    }

    #[test]
    fn parse_left_associative_add() {
        let expr = parse_node_expr("1.0 - 2.0 - 3.0");
        if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Sub);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Sub, .. }));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_power_right_assoc() {
        let expr = parse_node_expr("2.0 ^ 3.0 ^ 2.0");
        if let ExprKind::BinOp { op, rhs, .. } = &expr.kind {
            assert!(matches!(op, BinOp::Pow(PowerExponent::Runtime)));
            assert!(matches!(
                rhs.kind,
                ExprKind::BinOp {
                    op: BinOp::Pow(PowerExponent::FloatSyntax { .. }),
                    ..
                }
            ));
        } else {
            panic!("expected Pow");
        }
    }

    #[test]
    fn parse_power_preserves_exact_rational_and_decimal_syntax() {
        let rational = parse_node_expr("@x ^ (-6/4)");
        assert!(matches!(
            rational.kind,
            ExprKind::BinOp {
                op: BinOp::Pow(PowerExponent::Exact(value)),
                ..
            } if value == ExactRational::try_new(-3, 2).unwrap()
        ));

        let decimal = parse_node_expr("@x ^ 2.5e-1");
        assert!(matches!(
            decimal.kind,
            ExprKind::BinOp {
                op: BinOp::Pow(PowerExponent::FloatSyntax {
                    exact: Some(value)
                }),
                ..
            } if value == ExactRational::try_new(1, 4).unwrap()
        ));
    }

    #[test]
    fn parse_neg_power_precedence() {
        let expr = parse_node_expr("-@x ^ 2.0");
        if let ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } = &expr.kind
        {
            assert!(matches!(
                operand.kind,
                ExprKind::BinOp {
                    op: BinOp::Pow(_),
                    ..
                }
            ));
        } else {
            panic!("expected Neg(Pow(...))");
        }
    }

    #[test]
    fn parse_graph_ref() {
        let expr = parse_node_expr("@x + 1.0");
        if let ExprKind::BinOp { lhs, .. } = &expr.kind {
            assert!(matches!(&lhs.kind, ExprKind::GraphRef(id) if id.value.member() == "x"));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_name_ref() {
        let expr = parse_node_expr("PI * 2.0");
        if let ExprKind::BinOp { lhs, .. } = &expr.kind {
            assert!(matches!(
                &lhs.kind,
                ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path))
                    if path.as_bare().is_some_and(|id| id.name.as_str() == "PI")
            ));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_function_call_one_arg() {
        let expr = parse_node_expr("sqrt(@x)");
        if let ExprKind::FnCall { callee, args, .. } = &expr.kind {
            assert_eq!(callee.as_bare().unwrap().name, "sqrt");
            assert_eq!(args.len(), 1);
            assert!(matches!(&args[0].kind, ExprKind::GraphRef(id) if id.value.member() == "x"));
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_qualified_function_call_preserves_callee_path() {
        let expr = parse_node_expr("module::sqrt(@x)");
        if let ExprKind::FnCall { callee, args, .. } = &expr.kind {
            assert_eq!(callee.owner_segments().unwrap()[0].name, "module");
            assert_eq!(callee.leaf().name, "sqrt");
            assert_eq!(args.len(), 1);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn qualified_contextual_keyword_call_is_ordinary_call() {
        let expr = parse_node_expr("plugin::scan(1.0)");
        match &expr.kind {
            ExprKind::FnCall { callee, args, .. } => {
                assert_eq!(callee.display_path(), "plugin::scan");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected ordinary FnCall, got {other:?}"),
        }
    }

    #[test]
    fn contextual_keyword_paths_and_generic_calls_are_ordinary_expressions() {
        let path_call = parse_node_expr("scan.unfold.linspace::step()");
        assert!(
            matches!(&path_call.kind, ExprKind::FnCall { callee, .. } if callee.display_path() == "scan.unfold.linspace::step")
        );

        let generic_call = parse_node_expr("scan<Length>()");
        assert!(matches!(generic_call.kind, ExprKind::FnCall { .. }));

        for name in ["range", "linspace", "step", "points", "Fin"] {
            let ordinary_call = parse_node_expr(&format!("{name}()"));
            assert!(
                matches!(ordinary_call.kind, ExprKind::FnCall { .. }),
                "`{name}(` must be an ordinary call outside an index declaration"
            );
        }
    }

    #[test]
    fn raw_lookahead_treats_lf_cr_and_crlf_comments_consistently() {
        for line_ending in ["\n", "\r", "\r\n"] {
            let constructor = parse_node_expr(&format!(
                "Point(// constructor lookahead{line_ending}value: 1.0)"
            ));
            assert!(
                matches!(constructor.kind, ExprKind::ConstructorCall { .. }),
                "constructor lookahead failed for {line_ending:?}"
            );

            let generic_call = parse_node_expr(&format!(
                "make<Length // generic lookahead{line_ending}>(@x)"
            ));
            assert!(
                matches!(generic_call.kind, ExprKind::FnCall { .. }),
                "generic-call lookahead failed for {line_ending:?}"
            );

            let tuple_source = format!(
                "node x: Dimensionless[Phase, Mode] = for p: Phase, m: Mode {{ (p, // tuple lookahead{line_ending}m) => 1.0 }};"
            );
            Parser::new(&tuple_source)
                .parse_file()
                .unwrap_or_else(|error| {
                    panic!("tuple-sugar lookahead failed for {line_ending:?}: {error}")
                });
        }
    }

    #[test]
    fn parse_function_call_two_args() {
        let expr = parse_node_expr("atan2(@a, @b)");
        if let ExprKind::FnCall { callee, args, .. } = &expr.kind {
            assert_eq!(callee.as_bare().unwrap().name, "atan2");
            assert_eq!(args.len(), 2);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_function_call_zero_args() {
        let expr = parse_node_expr("foo()");
        if let ExprKind::FnCall { callee, args, .. } = &expr.kind {
            assert_eq!(callee.as_bare().unwrap().name, "foo");
            assert_eq!(args.len(), 0);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_turbofish_nat_arg() {
        let expr = parse_node_expr("eye<3>()");
        if let ExprKind::FnCall {
            callee,
            generic_args,
            args,
        } = &expr.kind
        {
            assert_eq!(callee.as_bare().unwrap().name, "eye");
            assert_eq!(generic_args.len(), 1);
            assert!(matches!(
                &generic_args[0],
                crate::syntax::ast::GenericArg::Nat(crate::syntax::ast::NatExpr::Literal(3, _))
            ));
            assert_eq!(args.len(), 0);
        } else {
            panic!("expected FnCall, got {:?}", expr.kind);
        }
    }

    #[test]
    fn parse_turbofish_nat_arg_with_separators() {
        let expr = parse_node_expr("eye<1_000>()");
        if let ExprKind::FnCall { generic_args, .. } = &expr.kind {
            assert!(matches!(
                &generic_args[0],
                crate::syntax::ast::GenericArg::Nat(crate::syntax::ast::NatExpr::Literal(1000, _))
            ));
        } else {
            panic!("expected FnCall, got {:?}", expr.kind);
        }
    }

    #[test]
    fn parse_turbofish_type_arg() {
        let expr = parse_node_expr("make<Length>(@x)");
        if let ExprKind::FnCall {
            callee,
            generic_args,
            args,
        } = &expr.kind
        {
            assert_eq!(callee.as_bare().unwrap().name, "make");
            assert_eq!(generic_args.len(), 1);
            assert!(matches!(
                &generic_args[0],
                crate::syntax::ast::GenericArg::Ambiguous(
                    crate::syntax::ast::AmbiguousGenericArg::Name(_)
                )
            ));
            assert_eq!(args.len(), 1);
        } else {
            panic!("expected FnCall, got {:?}", expr.kind);
        }
    }

    #[test]
    fn parse_turbofish_multiple_args() {
        let expr = parse_node_expr("foo<3, Length>(@x)");
        if let ExprKind::FnCall {
            callee,
            generic_args,
            args,
        } = &expr.kind
        {
            assert_eq!(callee.as_bare().unwrap().name, "foo");
            assert_eq!(generic_args.len(), 2);
            assert!(matches!(
                &generic_args[0],
                crate::syntax::ast::GenericArg::Nat(crate::syntax::ast::NatExpr::Literal(3, _))
            ));
            assert!(matches!(
                &generic_args[1],
                crate::syntax::ast::GenericArg::Ambiguous(
                    crate::syntax::ast::AmbiguousGenericArg::Name(_)
                )
            ));
            assert_eq!(args.len(), 1);
        } else {
            panic!("expected FnCall, got {:?}", expr.kind);
        }
    }

    #[test]
    fn parse_comparison_not_turbofish() {
        // `f < x` should NOT be parsed as turbofish since `x` is not followed by `>`+`(`
        let expr = parse_node_expr("@a < @b");
        assert!(
            matches!(&expr.kind, ExprKind::BinOp { op: BinOp::Lt, .. }),
            "expected comparison, got {:?}",
            expr.kind
        );
    }

    #[test]
    fn comparison_with_and_then_gt_paren_is_not_turbofish() {
        // Regression: the raw-byte turbofish lookahead used to treat
        // `limit < threshold && other > (1.0)` as `limit<…>(…)`, rejecting
        // a perfectly ordinary boolean expression with a confusing error.
        let expr = parse_node_expr("@limit < @threshold && @other > (1.0)");
        assert!(
            matches!(&expr.kind, ExprKind::BinOp { op: BinOp::And, .. }),
            "expected `&&` at the top, got {:?}",
            expr.kind
        );
    }

    #[test]
    fn comparison_with_comment_containing_gt_paren_is_not_turbofish() {
        // Regression: the lookahead also scanned comment bytes, so a
        // comment containing `> (` could fabricate a turbofish.
        let expr = parse_node_expr("@a < @b // note: > (\n || @c");
        assert!(
            matches!(&expr.kind, ExprKind::BinOp { op: BinOp::Or, .. }),
            "expected `||` at the top, got {:?}",
            expr.kind
        );
    }

    #[test]
    fn parse_if_else() {
        let expr = parse_node_expr("if @x > 0.0 { @x } else { 0.0 }");
        if let ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } = &expr.kind
        {
            assert!(matches!(
                condition.kind,
                ExprKind::BinOp { op: BinOp::Gt, .. }
            ));
            assert!(matches!(
                &then_branch.kind,
                ExprKind::GraphRef(id) if id.value.member() == "x"
            ));
            assert!(matches!(else_branch.kind, ExprKind::Number(_)));
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn parse_nested_parens() {
        let expr = parse_node_expr("(1.0 + 2.0) * 3.0");
        if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Mul);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        } else {
            panic!("expected Mul");
        }
    }

    #[test]
    fn parse_boolean_and() {
        let expr = parse_node_expr("@a > 0.0 && @b > 0.0");
        if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::And);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
        } else {
            panic!("expected And");
        }
    }

    #[test]
    fn parse_boolean_or() {
        let expr = parse_node_expr("@a > 0.0 || @b > 0.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Or, .. }));
    }

    #[test]
    fn parse_unary_neg() {
        let expr = parse_node_expr("-1.0");
        assert!(matches!(
            expr.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    #[test]
    fn parse_unary_not() {
        let expr = parse_node_expr("!true");
        assert!(matches!(
            expr.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn parse_complex_expression() {
        let expr = parse_node_expr("@v_exhaust * ln(@mass_ratio)");
        if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::Mul);
            assert!(
                matches!(&lhs.kind, ExprKind::GraphRef(id) if id.value.member() == "v_exhaust")
            );
            assert!(
                matches!(&rhs.kind, ExprKind::FnCall { callee, .. } if callee.as_bare().is_some_and(|name| name.name == "ln"))
            );
        } else {
            panic!("expected Mul");
        }
    }

    #[test]
    fn parse_comparison_eq() {
        let expr = parse_node_expr("@x == 1.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Eq, .. }));
    }

    #[test]
    fn parse_comparison_ne() {
        let expr = parse_node_expr("@x != 1.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Ne, .. }));
    }

    #[test]
    fn parse_field_access() {
        let source = "node x: Dimensionless = @transfer.dv1;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::FieldAccess { expr, field } => {
                    assert!(
                        matches!(&expr.kind, ExprKind::GraphRef(ident) if ident.value.member() == "transfer")
                    );
                    assert_eq!(field.value.as_str(), "dv1");
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_chained_field_access() {
        let source = "node x: Dimensionless = @mission.transfer.dv1;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::FieldAccess { expr, field } => {
                    assert_eq!(field.value.as_str(), "dv1");
                    match &expr.kind {
                        ExprKind::FieldAccess {
                            expr: inner,
                            field: mid_field,
                        } => {
                            assert_eq!(mid_field.value.as_str(), "transfer");
                            assert!(
                                matches!(&inner.kind, ExprKind::GraphRef(ident) if ident.value.member() == "mission")
                            );
                        }
                        other => panic!("expected inner FieldAccess, got {other:?}"),
                    }
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_at_with_field_access_parses_as_field_chain() {
        // `@instance.field` is valid: `instance` is a single in-scope ident
        // (typically an include's instance alias), and `.field` is postfix
        // field access producing the instance's projected output value.
        let file = Parser::new("node x: Dimensionless = @stage.delta_v;")
            .parse_file()
            .unwrap();
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        // The expression should be `FieldAccess(GraphRef(stage), delta_v)`,
        // not a qualified-graph-ref construct.
        match &node.value.kind {
            ExprKind::FieldAccess { expr: inner, field } => {
                assert!(
                    matches!(&inner.kind, ExprKind::GraphRef(id) if id.value.member() == "stage")
                );
                assert_eq!(field.value.as_str(), "delta_v");
            }
            other => panic!("expected FieldAccess on GraphRef, got {other:?}"),
        }
    }

    #[test]
    fn parse_inline_dag_ref_basic() {
        let file = Parser::new("node y: Length = @clamp(x: @p)::result;")
            .parse_file()
            .unwrap();
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        match &node.value.kind {
            ExprKind::InlineDagRef { path, args, output } => {
                assert_eq!(path.segments.len(), 1);
                assert_eq!(path.segments[0].name, "clamp");
                assert_eq!(args.len(), 1);
                assert_eq!(args[0].name.name, "x");
                assert!(
                    matches!(&args[0].value.kind, ExprKind::GraphRef(id) if id.value.member() == "p")
                );
                assert_eq!(output.value.as_str(), "result");
            }
            other => panic!("expected InlineDagRef, got {other:?}"),
        }
    }

    #[test]
    fn parse_inline_dag_ref_multi_arg() {
        let file = Parser::new("node y: Velocity = @scale(factor: 2.0, v: @speed)::out;")
            .parse_file()
            .unwrap();
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        match &node.value.kind {
            ExprKind::InlineDagRef { path, args, output } => {
                assert_eq!(path.segments.len(), 1);
                assert_eq!(path.segments[0].name, "scale");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0].name.name, "factor");
                assert_eq!(args[1].name.name, "v");
                assert_eq!(output.value.as_str(), "out");
            }
            other => panic!("expected InlineDagRef, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_inline_dag_binding_is_rejected() {
        let err = Parser::new("node y: Dimensionless = @combine(a: 1.0, a: 2.0)::result;")
            .parse_file()
            .unwrap_err();
        assert!(
            matches!(err, ParseError::DuplicateDagBinding { name, .. } if name.as_str() == "a")
        );
    }

    #[test]
    fn parse_inline_dag_ref_qualified_accepted() {
        // `@<module>.<dag>(args)::<out>` projects a graph value from a DAG
        // brought into scope via `import path as module` (or `import path;`).
        // The value may be an explicit node export or a param input port.
        let file = Parser::new("node y: Length = @geom.clamp(x: @p)::result;")
            .parse_file()
            .expect("qualified inline DAG call should parse");
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        match &node.value.kind {
            ExprKind::InlineDagRef { path, args, output } => {
                assert_eq!(path.segments.len(), 2);
                assert_eq!(path.segments[0].name, "geom");
                assert_eq!(path.segments[1].name, "clamp");
                assert_eq!(path.display_path(), "geom.clamp");
                assert_eq!(args.len(), 1);
                assert_eq!(args[0].name.name, "x");
                assert_eq!(output.value.as_str(), "result");
            }
            other => panic!("expected InlineDagRef, got {other:?}"),
        }
    }

    #[test]
    fn parse_at_field_access_still_works() {
        // `@orbit.altitude` is a struct-field projection on a graph reference,
        // not an inline DAG call (no `(...)` follows). It must still parse as
        // `FieldAccess(GraphRef("orbit"), "altitude")` — the path-greedy
        // `@`-parser falls back to this shape when no `(` is found.
        let file = Parser::new("node y: Length = @orbit.altitude;")
            .parse_file()
            .expect("graph-ref field access should parse");
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        match &node.value.kind {
            ExprKind::FieldAccess { expr, field } => {
                assert_eq!(field.value.as_str(), "altitude");
                match &expr.kind {
                    ExprKind::GraphRef(name) => assert_eq!(name.value.member(), "orbit"),
                    other => panic!("expected inner GraphRef, got {other:?}"),
                }
            }
            other => panic!("expected FieldAccess, got {other:?}"),
        }
    }

    #[test]
    fn parse_at_field_access_chain_still_works() {
        // `@a.b.c` (no parens) → `FieldAccess(FieldAccess(GraphRef(a), b), c)`.
        // The path-greedy parser must rebuild this chain when no `(` follows.
        let file = Parser::new("node y: Length = @a.b.c;")
            .parse_file()
            .expect("graph-ref field access chain should parse");
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        // Outer: FieldAccess(.., c)
        let ExprKind::FieldAccess {
            expr: outer_inner,
            field: c,
        } = &node.value.kind
        else {
            panic!("expected outer FieldAccess");
        };
        assert_eq!(c.value.as_str(), "c");
        let ExprKind::FieldAccess {
            expr: inner,
            field: b,
        } = &outer_inner.kind
        else {
            panic!("expected inner FieldAccess");
        };
        assert_eq!(b.value.as_str(), "b");
        let ExprKind::GraphRef(a) = &inner.kind else {
            panic!("expected innermost GraphRef");
        };
        assert_eq!(a.value.member(), "a");
    }

    #[test]
    fn parse_inline_dag_ref_no_projection_rejected() {
        // `@dag(args)` (no `::<out>` projection) is rejected because an
        // unprojected DAG instance is not a graph value.
        let result = Parser::new("node y: Length = @dag(x: @p);").parse_file();
        assert!(
            result.is_err(),
            "@dag(args) without .<out> projection must be rejected"
        );
    }

    #[test]
    fn parse_inline_dag_ref_qualified_no_projection_rejected() {
        // `@module.dag(args)` (no `::<out>` projection) is rejected for the
        // same reason — same rule, applied to a qualified path.
        let result = Parser::new("node y: Length = @geom.clamp(x: @p);").parse_file();
        assert!(
            result.is_err(),
            "@module.dag(args) without .<out> projection must be rejected"
        );
    }

    #[test]
    fn parse_inline_dag_call_basic_fixture() {
        let source =
            include_str!("../../../../../tests/fixtures/valid/inline_dag_call_basic/main.gcl");
        let file = Parser::new(source)
            .parse_file()
            .expect("fixture should parse");
        // Locate the `doubled_result` node and assert its body is an InlineDagRef.
        let node = file
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Node(n) if n.name.value.as_str() == "doubled_result" => Some(n),
                _ => None,
            })
            .expect("doubled_result node");
        assert!(matches!(&node.value.kind, ExprKind::InlineDagRef { .. }));
    }

    #[test]
    fn parse_member_identifier_path_ref() {
        let file = Parser::new("node x: Dimensionless = constants.physics::G0;")
            .parse_file()
            .unwrap();
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        match &node.value.kind {
            ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
                assert_eq!(
                    path.owner_segments()
                        .unwrap()
                        .iter()
                        .map(|segment| segment.name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["constants", "physics"]
                );
                assert_eq!(path.leaf().name, "G0");
            }
            other => panic!("expected unresolved path, got {other:?}"),
        }
    }

    #[test]
    fn empty_call_generic_arguments_are_rejected() {
        for source in ["sqrt<>(4.0)", "Wrapper<>(value: 4.0)"] {
            let error = Parser::new(source).parse_single_expr().unwrap_err();
            assert!(
                matches!(error, ParseError::UnexpectedToken { .. }),
                "unexpected error for `{source}`: {error:?}"
            );
        }
    }

    #[test]
    fn empty_index_access_is_rejected() {
        let error = Parser::new("@values[]").parse_single_expr().unwrap_err();
        assert!(matches!(error, ParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn single_expr_quantity_literal() {
        let expr = Parser::new("450.0 s").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::QuantityLiteral { .. }));
    }

    #[test]
    fn single_expr_integer_with_unit_errors() {
        let result = Parser::new("450 s").parse_single_expr();
        assert!(
            result.is_err(),
            "integer literal with unit should be an error"
        );
    }

    #[test]
    fn single_expr_number() {
        let expr = Parser::new("3.0").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::Number(n) if (n - 3.0).abs() < f64::EPSILON));
    }

    #[test]
    fn single_expr_compound_quantity_literal() {
        let expr = Parser::new("9.80665 m/s^2").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::QuantityLiteral { .. }));
    }

    #[test]
    fn single_expr_arithmetic_with_const() {
        let expr = Parser::new("2.0 * PI").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
    }

    #[test]
    fn single_expr_trailing_tokens_error() {
        let result = Parser::new("450.0 s; extra").parse_single_expr();
        assert!(result.is_err());
    }
}
