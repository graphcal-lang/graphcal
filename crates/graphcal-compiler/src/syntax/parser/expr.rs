use crate::syntax::ast::{BinOp, Expr, ExprKind, FieldInit, Ident, IndexArg, ModulePath, UnaryOp};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::Token;
use crate::syntax::type_name::FieldName;

use super::{ParseError, Parser};

/// Map comparison tokens to their corresponding `BinOp`.
pub(super) const fn token_to_comparison_op(token: Token) -> Option<BinOp> {
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
        self.with_depth(Self::parse_convert)
    }

    /// Parse a comma-separated argument list: `(expr1, expr2, ...)`
    /// Expects the opening `(` to have already been consumed.
    /// Supports trailing commas.
    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.parse_comma_separated(Token::RParen, Self::parse_expr)
    }

    /// Parse a single named field initializer for a constructor call:
    /// `field: expr`. Used inside the paren-form `Ctor(field: expr, ...)`.
    pub(super) fn parse_named_field_init(&mut self) -> Result<FieldInit, ParseError> {
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
        self.with_depth(Self::parse_unary_inner)
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
            // Right-associative: recurse into parse_unary, not parse_power
            let exp = self.parse_unary()?;
            let span = base.span.merge(exp.span);
            Ok(Expr::new(
                ExprKind::BinOp {
                    op: BinOp::Pow,
                    lhs: Box::new(base),
                    rhs: Box::new(exp),
                },
                span,
            ))
        } else {
            Ok(base)
        }
    }

    /// Apply postfix operators (field access `.field`, index access `[i]`) to an already-parsed expression.
    pub(super) fn apply_postfix(&mut self, mut expr: Expr) -> Result<Expr, ParseError> {
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
                    let args =
                        self.parse_comma_separated(Token::RBracket, Self::parse_index_arg)?;
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
        match self.lexer.peek().cloned() {
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
            Some(Token::Scan) if self.lexer.peek_second() == Some(&Token::LParen) => {
                let (_, span) = self.advance()?;
                self.parse_scan(span)
            }
            Some(Token::Unfold) if self.lexer.peek_second() == Some(&Token::LParen) => {
                let (_, span) = self.advance()?;
                self.parse_unfold(span)
            }
            Some(Token::Ident | Token::Scan | Token::Unfold | Token::Linspace | Token::Step) => {
                self.parse_identifier_expr()
            }
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

    /// Parse a `@`-prefixed expression: a graph reference or an inline-DAG
    /// invocation, possibly with a module-qualified path.
    ///
    /// Forms accepted:
    ///   `@<seg>`                                    — `GraphRef`
    ///   `@<seg>(args).<out>`                        — same-file inline DAG
    ///   `@<seg>.<seg>...<seg>(args).<out>`          — qualified inline DAG
    ///                                                 (cross-file via `import`)
    ///   `@<seg>.<seg>...<seg>` (no `(`)             — `FieldAccess` chain on
    ///                                                 a `GraphRef` (e.g.
    ///                                                 struct field projection)
    ///
    /// The grammar is unambiguous because the `(` after a path commits to the
    /// inline-DAG reading; otherwise the path is a `GraphRef` followed by zero
    /// or more `.<field>` projections — the same shape `apply_postfix` would
    /// produce if we returned the bare `GraphRef` and let it consume the dotted
    /// suffix. Synthesizing the `FieldAccess` chain inline avoids the lexer's
    /// 2-slot put-back limit when the path turns out not to be an inline DAG.
    ///
    /// The semantic invariant `@` enforces — that the post-`@` expression must
    /// denote a *node* — is checked downstream (parser rejects an inline-DAG
    /// form without `.<out>` projection; resolver/dim-checker reject unknown
    /// references). The parser itself only enforces the syntactic shape.
    fn parse_at_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, at_span) = self.advance()?; // consume `@`
        let first_seg = self.parse_any_ident()?;

        // Bare same-file inline DAG: `@<seg>(args).<out>`.
        if self.lexer.peek() == Some(&Token::LParen) {
            return self.finish_inline_dag_call(at_span, vec![first_seg]);
        }

        // Bare `GraphRef`: nothing more to consume here. Let `apply_postfix`
        // handle any `.<field>` continuation uniformly.
        if self.lexer.peek() != Some(&Token::Dot) {
            let span = at_span.merge(first_seg.span);
            return Ok(Expr::new(
                ExprKind::GraphRef(first_seg.into_spanned::<ScopedName>()),
                span,
            ));
        }

        // We see `@<seg>.`. Greedily consume `.<seg>` segments. If we hit `(`
        // afterward, commit to a qualified inline-DAG call. If we exhaust the
        // path without finding `(`, rebuild a `GraphRef`-with-`FieldAccess`
        // chain — the same shape `apply_postfix` would have produced.
        let mut segments = vec![first_seg];
        while self.lexer.peek() == Some(&Token::Dot)
            && self.lexer.peek_second() == Some(&Token::Ident)
        {
            self.advance()?; // consume `.`
            let seg = self.parse_any_ident()?;
            segments.push(seg);

            if self.lexer.peek() == Some(&Token::LParen) {
                return self.finish_inline_dag_call(at_span, segments);
            }
        }

        // No `(` — the path is a `GraphRef` followed by zero or more field
        // projections. Synthesize the equivalent `FieldAccess` chain.
        let mut iter = segments.into_iter();
        #[expect(
            clippy::expect_used,
            reason = "loop seeded with first_seg, so segments is non-empty"
        )]
        let head = iter.next().expect("path always has at least one segment");
        let head_span = head.span;
        let mut expr = Expr::new(
            ExprKind::GraphRef(head.into_spanned::<ScopedName>()),
            at_span.merge(head_span),
        );
        for seg in iter {
            let seg_span = seg.span;
            let span = expr.span.merge(seg_span);
            expr = Expr::new(
                ExprKind::FieldAccess {
                    expr: Box::new(expr),
                    field: seg.into_spanned::<FieldName>(),
                },
                span,
            );
        }
        Ok(expr)
    }

    /// Finish parsing an inline DAG call after the `@<path>` prefix has been
    /// consumed and the next token is a confirmed `(`. Reads the param
    /// bindings, the mandatory `.<out>` projection, and assembles the
    /// `InlineDagRef` AST node.
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
        // The `.<out>` projection is mandatory: an instantiated DAG without
        // projection is not a node, and `@` requires a node. Surface a
        // dedicated diagnostic instead of the generic "expected `.`".
        match self.lexer.peek_with_span() {
            Some((Token::Dot, _)) => {
                self.lexer.next_token();
            }
            Some((_, span)) => {
                return Err(ParseError::InlineDagCallMissingProjection {
                    src: self.named_source(),
                    span: span.into(),
                });
            }
            None => {
                return Err(self.unexpected_eof("`.<out>` projection"));
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

    /// Parse a number literal: integer, float, or float with unit.
    fn parse_number_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, span) = self.advance()?;
        let text = self.lexer.slice_at(span).replace('_', "");
        let is_integer = !text.contains('.') && !text.contains('e') && !text.contains('E');

        if is_integer {
            // Integer literal: no decimal point or scientific notation
            if self.lexer.peek() == Some(&Token::Ident) {
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
            // Float literal: has decimal point or scientific notation
            let value = self.parse_finite_f64_literal(&text, span)?;

            // Check if followed by an identifier (unit literal): `400.0 km`
            if self.lexer.peek() == Some(&Token::Ident) {
                let unit_expr = self.parse_unit_expr()?;
                let full_span = span.merge(unit_expr.span);
                Ok(Expr::new(
                    ExprKind::UnitLiteral {
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

    /// Parse an identifier-based expression.
    ///
    /// Dispatches on following tokens (syntax-based disambiguation):
    /// - `ident.member` / `ident.member.leaf` → unresolved identifier path
    ///   (resolved later to variant or const)
    /// - `ident<T>(args)` or `ident(args)` — disambiguated structurally
    ///   by the first argument's shape: `IDENT :` → constructor call,
    ///   otherwise → function call
    /// - bare `ident` → unresolved identifier path (resolved later to const,
    ///   local, or unit constructor)
    ///
    /// The old brace-form construction `Name { field: val }` is no
    /// longer accepted — constructor calls use parens.
    fn parse_identifier_expr(&mut self) -> Result<Expr, ParseError> {
        let path = self.parse_ident_path()?;

        if self.lexer.peek() == Some(&Token::LParen)
            || (self.lexer.peek() == Some(&Token::Lt) && self.is_type_args_followed_by_paren())
        {
            // `path(args)` or `path<T>(args)`. The path is syntactic: bare
            // and qualified callees have the same AST representation. We only
            // use argument shape to distinguish constructor-call syntax (named
            // args) from function-call syntax (positional args).
            let generic_args = if self.lexer.peek() == Some(&Token::Lt) {
                self.parse_generic_arg_list()?
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
                    type_args: generic_args,
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
        if let Some((Token::Ident, ident_span)) = self.lexer.peek_with_span() {
            // Could be map literal: { Index.Variant: expr, ... }
            let saved_text = self.lexer.slice_at(ident_span).to_string();
            if self.lexer.peek_second() == Some(&Token::Dot) {
                let (index, variant, _) = self.parse_index_variant_path()?;
                if self.lexer.peek() == Some(&Token::Colon) {
                    // Map literal: { Index.Variant: expr, ... }
                    self.parse_map_literal_after_first_entry(start_span, index, variant)
                } else {
                    let found = self
                        .lexer
                        .peek()
                        .map_or_else(|| "EOF".to_string(), std::string::ToString::to_string);
                    Err(self.unexpected_token(
                        "`:` after variant in map literal",
                        &found,
                        start_span,
                    ))
                }
            } else {
                // Ident not followed by `.` — not a map literal
                Err(self.unexpected_token(
                    "map literal (`{ Index.Variant: expr, ... }`)",
                    &saved_text,
                    start_span,
                ))
            }
        } else if self.lexer.peek() == Some(&Token::LParen) {
            // Could be tuple-key map literal: { (Index.Variant, ...): expr, ... }
            self.parse_tuple_key_map_literal(start_span)
        } else {
            let found = self
                .lexer
                .peek()
                .map_or_else(|| "EOF".to_string(), std::string::ToString::to_string);
            Err(self.unexpected_token(
                "map literal (`{ Index.Variant: expr, ... }`)",
                &found,
                start_span,
            ))
        }
    }

    // --- Index access ---

    pub(super) fn index_name_path_from_segments(index_segments: &[Ident]) -> Spanned<NamePath> {
        let index_ident = &index_segments[index_segments.len() - 1];
        let span = index_segments
            .first()
            .map_or(index_ident.span, |first| first.span.merge(index_ident.span));
        let qualifier = index_segments[..index_segments.len().saturating_sub(1)]
            .iter()
            .map(|ident| ident.name.clone());
        Spanned::new(
            NamePath::qualified_path(qualifier, index_ident.name.clone()),
            span,
        )
    }

    pub(super) fn parse_index_variant_path(
        &mut self,
    ) -> Result<(Spanned<NamePath>, Spanned<IndexVariantName>, Span), ParseError> {
        let first = self.parse_any_ident()?;
        let start_span = first.span;
        self.expect(Token::Dot)?;
        let second = self.parse_any_ident()?;
        let mut segments = vec![first, second];
        while self.lexer.peek() == Some(&Token::Dot) {
            self.lexer.next_token();
            segments.push(self.parse_any_ident()?);
        }
        let variant_ident = segments.remove(segments.len() - 1);
        let full_span = start_span.merge(variant_ident.span);
        let index = Self::index_name_path_from_segments(&segments);
        let variant = variant_ident.into_spanned::<IndexVariantName>();
        Ok((index, variant, full_span))
    }

    /// Parse an index argument: `Index.Variant`, `module.Index.Variant`, a loop variable `m`, or an expression `i + 1`.
    ///
    /// Strategy:
    /// 1. Peek for a dotted identifier path (qualified variant). The parser
    ///    treats the last segment as the variant and all preceding segments as
    ///    a structurally scoped index name.
    /// 2. Parse a full expression:
    ///    - If it's a single-segment unresolved path → convert to `IndexArg::Var`.
    ///    - Otherwise → `IndexArg::Expr`.
    pub(super) fn parse_index_arg(&mut self) -> Result<IndexArg, ParseError> {
        if self.lexer.peek() == Some(&Token::Ident)
            && self.lexer.peek_second() == Some(&Token::Dot)
            && self.lexer.peek_third() == Some(&Token::Ident)
        {
            let (index, variant, _) = self.parse_index_variant_path()?;
            return Ok(IndexArg::Variant { index, variant });
        }

        // Parse a full expression
        let expr = self.parse_expr()?;

        // If it's a bare name reference, use IndexArg::Var for backward compatibility.
        if let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) = &expr.kind {
            match path.clone().into_bare() {
                Ok(ident) => Ok(IndexArg::Var(ident)),
                Err(path) => Ok(IndexArg::Expr(Box::new(Expr::new(
                    ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)),
                    expr.span,
                )))),
            }
        } else {
            Ok(IndexArg::Expr(Box::new(expr)))
        }
    }

    /// Scan balanced `<…>` angle brackets from the current position and check
    /// whether the byte immediately after `>` (skipping whitespace) equals `expected`.
    ///
    /// Returns `false` if the next token is not `<`, the brackets are
    /// unbalanced, or a byte that can never occur inside a generic-argument
    /// list shows up first — in that case the `<` is a comparison operator,
    /// so an ordinary boolean expression like `a < b && c > (d)` is not
    /// misparsed as turbofish. Comments are skipped so their contents affect
    /// neither the bracket balance nor the operator bail-out.
    fn is_type_args_followed_by(&mut self, expected: u8) -> bool {
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
                        let mut p = pos + 1;
                        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                            p += 1;
                        }
                        return p < bytes.len() && bytes[p] == expected;
                    }
                }
                b'/' if bytes.get(pos + 1) == Some(&b'/') => {
                    while pos < bytes.len() && bytes[pos] != b'\n' {
                        pos += 1;
                    }
                    continue;
                }
                b'/' if bytes.get(pos + 1) == Some(&b'*') => {
                    pos += 2;
                    while pos + 1 < bytes.len() && !(bytes[pos] == b'*' && bytes[pos + 1] == b'/') {
                        pos += 1;
                    }
                    pos += 2;
                    continue;
                }
                // Generic arguments are type expressions or Nat literals;
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
    /// Returns `true` only if the `(...)` contains only identifiers and commas,
    /// followed by `)` then `=>`.
    pub(super) fn is_tuple_key_sugar(&mut self) -> bool {
        let Some((&Token::LParen, lp_span)) = self.lexer.peek_with_span() else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let mut pos = lp_span.offset() + lp_span.len(); // byte after `(`

        // Scan for matching `)`: expect only identifiers, commas, and whitespace inside.
        loop {
            // Skip whitespace
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos >= bytes.len() {
                return false;
            }
            if bytes[pos] == b')' {
                // Found `)`, now check for `=>`
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                return pos + 1 < bytes.len() && bytes[pos] == b'=' && bytes[pos + 1] == b'>';
            }
            // Expect an identifier (ASCII alphanumeric or underscore, starting with letter/underscore)
            if !bytes[pos].is_ascii_alphabetic() && bytes[pos] != b'_' {
                return false;
            }
            while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
                pos += 1;
            }
            // Skip whitespace after identifier
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
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
    pub(super) fn is_type_args_followed_by_paren(&mut self) -> bool {
        self.is_type_args_followed_by(b'(')
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
    pub(super) fn is_named_arg_call(&mut self) -> bool {
        let Some((&Token::LParen, lp_span)) = self.lexer.peek_with_span() else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let mut pos = lp_span.offset() + lp_span.len();

        // Skip whitespace and line comments.
        loop {
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos + 1 < bytes.len() && bytes[pos] == b'/' && bytes[pos + 1] == b'/' {
                while pos < bytes.len() && bytes[pos] != b'\n' {
                    pos += 1;
                }
                continue;
            }
            break;
        }
        if pos >= bytes.len() {
            return false;
        }

        // First char of the first arg must be an identifier-start.
        if !bytes[pos].is_ascii_alphabetic() && bytes[pos] != b'_' {
            return false;
        }
        while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
            pos += 1;
        }
        // Skip whitespace.
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            return false;
        }
        // `:` (single colon) signals a named argument. Graphcal's surface
        // module path uses `.`, so a second `:` here would be a stray token;
        // either way, we only commit to the named-arg path on a lone `:`.
        bytes[pos] == b':' && bytes.get(pos + 1).is_none_or(|c| *c != b':')
    }

    /// Parse a generic argument list: `<GenericArg, GenericArg, ...>`
    ///
    /// Each argument is either a nat expression (integer literal) or a type expression.
    pub(super) fn parse_generic_arg_list(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::GenericArg>, ParseError> {
        self.expect(Token::Lt)?;
        let args = self.parse_comma_separated(Token::Gt, Self::parse_generic_arg)?;
        self.expect(Token::Gt)?;
        Ok(args)
    }

    /// Parse a single generic argument.
    ///
    /// If the next token is a number literal (and parses as a valid integer), parse as
    /// `GenericArg::Nat`. Otherwise, parse as `GenericArg::Type`.
    fn parse_generic_arg(&mut self) -> Result<crate::syntax::ast::GenericArg, ParseError> {
        use crate::syntax::ast::{GenericArg, NatExpr};
        // Check if it's a number literal (Nat argument)
        if let Some((&Token::Number, _)) = self.lexer.peek_with_span() {
            let (_, lit_span) = self.advance()?;
            let text = self.lexer.slice_at(lit_span);
            let value: u64 = text.parse().map_err(|_| {
                self.unexpected_token("a valid non-negative integer", text, lit_span)
            })?;
            Ok(GenericArg::Nat(NatExpr::Literal(value, lit_span)))
        } else {
            // Parse as type expression
            let te = self.parse_type_expr()?;
            Ok(GenericArg::Type(te))
        }
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
    fn parse_unit_literal() {
        let file = Parser::new("param alt: Length = 400.0 km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.as_ref().unwrap().kind {
                ExprKind::UnitLiteral { value, unit } => {
                    assert!((value - 400.0).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 1);
                    assert_eq!(unit.terms[0].name.value.to_string(), "km");
                }
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_compound_unit_literal() {
        let file = Parser::new("const node g0: Acceleration = 9.80665 m/s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::ConstNode(c) => match &c.value.kind {
                ExprKind::UnitLiteral { value, unit } => {
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
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected const"),
        }
    }

    #[test]
    fn parse_conversion() {
        let file = Parser::new("node speed_kmh: Velocity = @speed -> km/hour;")
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
                    assert_eq!(target.terms[1].name.value.to_string(), "hour");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_conversion_with_qualified_unit() {
        let file = Parser::new("node b: Length = @a -> u.mile;")
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
    fn parse_qualified_unit_literal() {
        let file = Parser::new("node d: Length = 2.0 u.mile;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::UnitLiteral { unit, .. } => {
                    assert_eq!(unit.terms[0].name.value.to_string(), "u.mile");
                }
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_unit_path_deeper_than_alias_is_rejected() {
        // Module aliases are single segments, so `a.b.mile` can never resolve
        // as a unit reference (P017).
        let err = Parser::new("node b: Length = @a -> app.units.mile;")
            .parse_file()
            .unwrap_err();
        assert!(
            matches!(err, super::ParseError::UnitReferenceTooDeep { .. }),
            "expected UnitReferenceTooDeep, got {err:?}"
        );
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
            assert_eq!(*op, BinOp::Pow);
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Pow, .. }));
        } else {
            panic!("expected Pow");
        }
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
                ExprKind::BinOp { op: BinOp::Pow, .. }
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
        let expr = parse_node_expr("module.sqrt(@x)");
        if let ExprKind::FnCall { callee, args, .. } = &expr.kind {
            assert_eq!(callee.segments.len(), 2);
            assert_eq!(callee.segments[0].name, "module");
            assert_eq!(callee.segments[1].name, "sqrt");
            assert_eq!(args.len(), 1);
        } else {
            panic!("expected FnCall");
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
            type_args,
            args,
        } = &expr.kind
        {
            assert_eq!(callee.as_bare().unwrap().name, "eye");
            assert_eq!(type_args.len(), 1);
            assert!(matches!(
                &type_args[0],
                crate::syntax::ast::GenericArg::Nat(crate::syntax::ast::NatExpr::Literal(3, _))
            ));
            assert_eq!(args.len(), 0);
        } else {
            panic!("expected FnCall, got {:?}", expr.kind);
        }
    }

    #[test]
    fn parse_turbofish_type_arg() {
        let expr = parse_node_expr("make<Length>(@x)");
        if let ExprKind::FnCall {
            callee,
            type_args,
            args,
        } = &expr.kind
        {
            assert_eq!(callee.as_bare().unwrap().name, "make");
            assert_eq!(type_args.len(), 1);
            assert!(matches!(
                &type_args[0],
                crate::syntax::ast::GenericArg::Type(_)
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
            type_args,
            args,
        } = &expr.kind
        {
            assert_eq!(callee.as_bare().unwrap().name, "foo");
            assert_eq!(type_args.len(), 2);
            assert!(matches!(
                &type_args[0],
                crate::syntax::ast::GenericArg::Nat(crate::syntax::ast::NatExpr::Literal(3, _))
            ));
            assert!(matches!(
                &type_args[1],
                crate::syntax::ast::GenericArg::Type(_)
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
        // field access producing the instance's projected output node.
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
        let file = Parser::new("node y: Length = @clamp(x: @p).result;")
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
        let file = Parser::new("node y: Velocity = @scale(factor: 2.0, v: @speed).out;")
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
    fn parse_inline_dag_ref_qualified_accepted() {
        // `@<module>.<dag>(args).<out>` projects a node from a DAG brought
        // into scope via `import path as module` (or `import path;`). What
        // `@` enforces is that the post-`@` expression denotes a node — and
        // `module.dag(args).out` does, so the form is well-formed.
        let file = Parser::new("node y: Length = @geom.clamp(x: @p).result;")
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
        // `@dag(args)` (no `.<out>` projection) is rejected: a DAG instance
        // without projection is not a node, and `@` requires a node.
        let result = Parser::new("node y: Length = @dag(x: @p);").parse_file();
        assert!(
            result.is_err(),
            "@dag(args) without .<out> projection must be rejected"
        );
    }

    #[test]
    fn parse_inline_dag_ref_qualified_no_projection_rejected() {
        // `@module.dag(args)` (no `.<out>` projection) is rejected for the
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
    fn parse_dotted_identifier_path_ref() {
        let file = Parser::new("node x: Dimensionless = constants.physics.G0;")
            .parse_file()
            .unwrap();
        let decl = &file.declarations[0].kind;
        let DeclKind::Node(node) = decl else {
            panic!("expected Node");
        };
        match &node.value.kind {
            ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
                let names = path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(names, vec!["constants", "physics", "G0"]);
            }
            other => panic!("expected unresolved path, got {other:?}"),
        }
    }

    #[test]
    fn single_expr_unit_literal() {
        let expr = Parser::new("450.0 s").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
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
    fn single_expr_compound_unit() {
        let expr = Parser::new("9.80665 m/s^2").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
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
