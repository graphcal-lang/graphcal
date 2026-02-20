use crate::ast::{BinOp, Expr, ExprKind, IndexArg, UnaryOp};
use crate::names::{DeclName, FieldName, FnName, IndexName, Spanned, StructTypeName, VariantName};
use crate::token::Token;

use super::{ParseError, Parser, is_lower_snake_case, is_pascal_case, is_upper_snake_case};

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
        self.parse_convert()
    }

    /// Parse conversion: `expr -> unit_expr` (lowest precedence).
    fn parse_convert(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_conditional()?;

        if self.lexer.peek() == Some(&Token::Arrow) {
            self.lexer.next_token();
            let target = self.parse_unit_expr()?;
            let span = expr.span.merge(target.span);
            Ok(Expr {
                kind: ExprKind::Convert {
                    expr: Box::new(expr),
                    target,
                },
                span,
            })
        } else if self.lexer.peek() == Some(&Token::As) {
            self.lexer.next_token();
            let target_type = self.parse_type_expr()?;
            let span = expr.span.merge(target_type.span);
            Ok(Expr {
                kind: ExprKind::AsCast {
                    expr: Box::new(expr),
                    target_type,
                },
                span,
            })
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
            Ok(Expr {
                kind: ExprKind::If {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
                span,
            })
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
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_comparison()?;
        while self.lexer.peek() == Some(&Token::AmpAmp) {
            self.lexer.next_token();
            let rhs = self.parse_comparison()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = match self.lexer.peek() {
            Some(Token::EqEq) => Some(BinOp::Eq),
            Some(Token::BangEq) => Some(BinOp::Ne),
            Some(Token::Lt) => Some(BinOp::Lt),
            Some(Token::Gt) => Some(BinOp::Gt),
            Some(Token::LtEq) => Some(BinOp::Le),
            Some(Token::GtEq) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.lexer.next_token();
            let rhs = self.parse_add()?;
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            })
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
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
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
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    pub(super) fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Minus) => {
                let (_, op_span) = self.advance()?;
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                })
            }
            Some(Token::Bang) => {
                let (_, op_span) = self.advance()?;
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                })
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
            Ok(Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Pow,
                    lhs: Box::new(base),
                    rhs: Box::new(exp),
                },
                span,
            })
        } else {
            Ok(base)
        }
    }

    /// Parse postfix operators (field access `.field`, index access `[i]`).
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_atom()?;
        loop {
            match self.lexer.peek() {
                Some(Token::Dot) => {
                    self.lexer.next_token(); // consume '.'
                    let field_ident = self.parse_any_ident()?;
                    let span = expr.span.merge(field_ident.span);
                    expr = Expr {
                        kind: ExprKind::FieldAccess {
                            expr: Box::new(expr),
                            field: field_ident.into_spanned::<FieldName>(),
                        },
                        span,
                    };
                }
                Some(Token::LBracket) => {
                    self.lexer.next_token(); // consume '['
                    let mut args = Vec::new();
                    loop {
                        if self.lexer.peek() == Some(&Token::RBracket) {
                            break;
                        }
                        args.push(self.parse_index_arg()?);
                        if self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                        } else {
                            break;
                        }
                    }
                    let (_, end_span) = self.expect(Token::RBracket)?;
                    let span = expr.span.merge(end_span);
                    expr = Expr {
                        kind: ExprKind::IndexAccess {
                            expr: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one match arm per atom kind; splitting would obscure"
    )]
    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let is_integer = !text.contains('.') && !text.contains('e') && !text.contains('E');

                if is_integer {
                    // Integer literal: no decimal point or scientific notation
                    if self.lexer.peek() == Some(&Token::Ident) {
                        // Integer followed by unit is an error: must use float
                        return Err(ParseError::InvalidNumber {
                            reason: format!(
                                "integer literal cannot have units; write `{text}.0` instead"
                            ),
                            src: self.named_source(),
                            span: span.into(),
                        });
                    }
                    let value: i64 = text.parse().map_err(|e: std::num::ParseIntError| {
                        ParseError::InvalidNumber {
                            reason: e.to_string(),
                            src: self.named_source(),
                            span: span.into(),
                        }
                    })?;
                    Ok(Expr {
                        kind: ExprKind::Integer(value),
                        span,
                    })
                } else {
                    // Float literal: has decimal point or scientific notation
                    let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                        ParseError::InvalidNumber {
                            reason: e.to_string(),
                            src: self.named_source(),
                            span: span.into(),
                        }
                    })?;

                    // Check if followed by an identifier (unit literal): `400.0 km`
                    if self.lexer.peek() == Some(&Token::Ident) {
                        let unit_expr = self.parse_unit_expr()?;
                        let full_span = span.merge(unit_expr.span);
                        Ok(Expr {
                            kind: ExprKind::UnitLiteral {
                                value,
                                unit: unit_expr,
                            },
                            span: full_span,
                        })
                    } else {
                        Ok(Expr {
                            kind: ExprKind::Number(value),
                            span,
                        })
                    }
                }
            }
            Some(Token::True) => {
                let (_, span) = self.advance()?;
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    span,
                })
            }
            Some(Token::False) => {
                let (_, span) = self.advance()?;
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    span,
                })
            }
            Some(Token::At) => {
                let (_, at_span) = self.advance()?;
                let ident =
                    self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
                // Check for module-qualified graph ref: @module::name
                if self.lexer.peek() == Some(&Token::ColonColon) {
                    self.lexer.next_token(); // consume '::'
                    let member =
                        self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
                    let span = at_span.merge(member.span);
                    Ok(Expr {
                        kind: ExprKind::QualifiedGraphRef {
                            module: ident,
                            name: member.into_spanned::<DeclName>(),
                        },
                        span,
                    })
                } else {
                    let span = at_span.merge(ident.span);
                    Ok(Expr {
                        kind: ExprKind::GraphRef(ident.into_spanned::<DeclName>()),
                        span,
                    })
                }
            }
            Some(Token::Ident) => {
                let (_, span) = self.advance()?;
                let name = self.lexer.slice_at(span).to_string();

                if is_upper_snake_case(&name) {
                    // Const reference: PI, E, G0, UPPER_NAME
                    Ok(Expr {
                        kind: ExprKind::ConstRef(Spanned::new(DeclName::new(name), span)),
                        span,
                    })
                } else if is_pascal_case(&name)
                    && self.lexer.peek() == Some(&Token::Lt)
                    && self.is_type_args_followed_by_brace()
                {
                    // Generic struct construction: Vec3<Length, ECI> { x: 1 km, ... }
                    let type_args = self.parse_type_arg_list()?;
                    self.parse_struct_construction_with_type_args(
                        Spanned::new(StructTypeName::new(name), span),
                        type_args,
                    )
                } else if is_pascal_case(&name) && self.lexer.peek() == Some(&Token::LBrace) {
                    // Struct/variant construction with fields: TypeName { field1: expr, field2 }
                    self.parse_struct_construction(Spanned::new(StructTypeName::new(name), span))
                } else if is_pascal_case(&name) && self.lexer.peek() == Some(&Token::ColonColon) {
                    // Qualified variant literal: Maneuver::Departure
                    self.lexer.next_token(); // consume '::'
                    let variant = self
                        .parse_ident_with_casing("PascalCase", is_pascal_case)?
                        .into_spanned::<VariantName>();
                    let full_span = span.merge(variant.span);
                    Ok(Expr {
                        kind: ExprKind::VariantLiteral {
                            index: Spanned::new(IndexName::new(name), span),
                            variant,
                        },
                        span: full_span,
                    })
                } else if is_pascal_case(&name) {
                    // Bare variant construction (no fields): `Nominal`
                    Ok(Expr {
                        kind: ExprKind::StructConstruction {
                            type_name: Spanned::new(StructTypeName::new(name), span),
                            type_args: Vec::new(),
                            fields: Vec::new(),
                        },
                        span,
                    })
                } else if is_lower_snake_case(&name)
                    && self.lexer.peek() == Some(&Token::ColonColon)
                {
                    // Module-qualified reference: module::CONST or module::fn(args)
                    self.lexer.next_token(); // consume '::'
                    let member_ident = self.parse_any_ident()?;
                    let member = member_ident.name.clone();
                    if is_upper_snake_case(&member) {
                        // module::CONST_NAME
                        let full_span = span.merge(member_ident.span);
                        Ok(Expr {
                            kind: ExprKind::QualifiedConstRef {
                                module: crate::ast::Ident { name, span },
                                name: Spanned::new(DeclName::new(member), member_ident.span),
                            },
                            span: full_span,
                        })
                    } else if is_lower_snake_case(&member)
                        && self.lexer.peek() == Some(&Token::LParen)
                    {
                        // module::fn_name(args...)
                        self.lexer.next_token(); // consume '('
                        let mut args = Vec::new();
                        if self.lexer.peek() != Some(&Token::RParen) {
                            args.push(self.parse_expr()?);
                            while self.lexer.peek() == Some(&Token::Comma) {
                                self.lexer.next_token();
                                if self.lexer.peek() == Some(&Token::RParen) {
                                    break; // trailing comma
                                }
                                args.push(self.parse_expr()?);
                            }
                        }
                        let (_, rparen_span) = self.expect(Token::RParen)?;
                        let call_span = span.merge(rparen_span);
                        Ok(Expr {
                            kind: ExprKind::QualifiedFnCall {
                                module: crate::ast::Ident { name, span },
                                name: Spanned::new(FnName::new(member), member_ident.span),
                                args,
                            },
                            span: call_span,
                        })
                    } else {
                        Err(self.unexpected_token(
                            "a CONST_NAME or function_name after `::`",
                            &member,
                            member_ident.span,
                        ))
                    }
                } else if name == "scan" && self.lexer.peek() == Some(&Token::LParen) {
                    // Scan expression: scan(source, init, |acc, val| body)
                    self.parse_scan(&crate::ast::Ident { name, span })
                } else if name == "unfold" && self.lexer.peek() == Some(&Token::LParen) {
                    // Unfold expression: unfold(init, |prev_i, i| body)
                    self.parse_unfold(&crate::ast::Ident { name, span })
                } else if self.lexer.peek() == Some(&Token::LParen) {
                    // Function call: name(args...)
                    self.lexer.next_token(); // consume '('
                    let mut args = Vec::new();
                    if self.lexer.peek() != Some(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        while self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                            if self.lexer.peek() == Some(&Token::RParen) {
                                break; // trailing comma
                            }
                            args.push(self.parse_expr()?);
                        }
                    }
                    let (_, rparen_span) = self.expect(Token::RParen)?;
                    let call_span = span.merge(rparen_span);
                    Ok(Expr {
                        kind: ExprKind::FnCall {
                            name: Spanned::new(FnName::new(name), span),
                            args,
                        },
                        span: call_span,
                    })
                } else {
                    // Bare lowercase identifier -> LocalRef (let binding reference)
                    // Semantic validation happens in resolve/dim_check.
                    Ok(Expr {
                        kind: ExprKind::LocalRef(crate::ast::Ident { name, span }),
                        span,
                    })
                }
            }
            Some(Token::For) => {
                // For comprehension: for m: Maneuver { expr }
                self.parse_for_comp()
            }
            Some(Token::LBrace) => {
                // Disambiguate: map literal vs block expression
                // Consume '{' and peek at what follows
                let (_, start_span) = self.advance()?;
                if let Some((Token::Ident, ident_span)) = self.lexer.peek_with_span() {
                    let text = self.lexer.slice_at(ident_span);
                    if is_pascal_case(text) {
                        // Could be map literal (PascalCase :: ...) or struct in block
                        // Save the ident text to check further
                        let saved_text = text.to_string();
                        // Consume the ident to peek at what's next
                        let (_, saved_span) = self.advance()?;
                        if self.lexer.peek() == Some(&Token::ColonColon) {
                            // Could be map literal or VariantLiteral in block.
                            // Consume `::` and variant, then check next token.
                            self.lexer.next_token(); // consume '::'
                            let variant_ident =
                                self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
                            if self.lexer.peek() == Some(&Token::Colon) {
                                // Map literal: { Index::Variant: expr, ... }
                                self.parse_map_literal_after_first_entry(
                                    start_span,
                                    Spanned::new(IndexName::new(saved_text), saved_span),
                                    variant_ident.into_spanned::<VariantName>(),
                                )
                            } else {
                                // Block containing a VariantLiteral expression
                                let variant_span = variant_ident.span;
                                let variant = variant_ident.into_spanned::<VariantName>();
                                let full_span = saved_span.merge(variant_span);
                                let first_expr = Expr {
                                    kind: ExprKind::VariantLiteral {
                                        index: Spanned::new(IndexName::new(saved_text), saved_span),
                                        variant,
                                    },
                                    span: full_span,
                                };
                                let expr = self.continue_parsing_expr(first_expr)?;
                                let (_, end_span) = self.expect(Token::RBrace)?;
                                let span = start_span.merge(end_span);
                                Ok(Expr {
                                    kind: ExprKind::Block {
                                        stmts: vec![],
                                        expr: Box::new(expr),
                                    },
                                    span,
                                })
                            }
                        } else {
                            // Not a map literal — reparse as block with already-consumed tokens
                            // The consumed ident is the start of an expression in the block
                            self.parse_block_after_open_brace_and_ident(
                                start_span,
                                &saved_text,
                                saved_span,
                            )
                        }
                    } else {
                        // lowercase ident or other — block expression
                        self.parse_block_after_open_brace(start_span)
                    }
                } else if self.lexer.peek() == Some(&Token::LParen) {
                    // Could be tuple-key map literal: { (Index::Variant, ...): expr, ... }
                    self.parse_tuple_key_map_literal(start_span)
                } else {
                    // Not an ident after { — could be `{ let ...` or `{ expr }`
                    self.parse_block_after_open_brace(start_span)
                }
            }
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

    // --- Index access ---

    /// Parse an index argument: either `Index::Variant` or a loop variable `m`.
    pub(super) fn parse_index_arg(&mut self) -> Result<IndexArg, ParseError> {
        let (_, span) = self
            .lexer
            .next_token()
            .ok_or_else(|| self.unexpected_eof("index argument"))?;
        let name = self.lexer.slice_at(span).to_string();

        if is_pascal_case(&name) && self.lexer.peek() == Some(&Token::ColonColon) {
            // Qualified variant: Index::Variant
            self.lexer.next_token(); // consume '::'
            let variant = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<VariantName>();
            Ok(IndexArg::Variant {
                index: Spanned::new(IndexName::new(name), span),
                variant,
            })
        } else if is_lower_snake_case(&name) {
            // Loop variable
            Ok(IndexArg::Var(crate::ast::Ident { name, span }))
        } else {
            Err(self.unexpected_token("loop variable or `Index::Variant`", &name, span))
        }
    }

    /// Look ahead to check if `<...>` is followed by `{`.
    /// Used to disambiguate `Vec3<Length, ECI> { ... }` (struct construction with type args)
    /// from `Foo < bar` (comparison).
    ///
    /// Scans the raw source string from the current position to find matching angle brackets.
    pub(super) fn is_type_args_followed_by_brace(&mut self) -> bool {
        // Get the byte offset where `<` starts
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
                        // Skip whitespace after `>`
                        let mut p = pos + 1;
                        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                            p += 1;
                        }
                        return p < bytes.len() && bytes[p] == b'{';
                    }
                }
                _ => {}
            }
            pos += 1;
        }
        false
    }
}
