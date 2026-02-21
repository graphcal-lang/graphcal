use crate::ast::{
    DimExpr, DimExprItem, DimTerm, MulDivOp, TypeExpr, TypeExprKind, UnitDef, UnitExpr,
    UnitExprItem,
};
use crate::names::UnitName;
use crate::token::Token;

use super::{ParseError, Parser, is_pascal_case, is_uppercase_starting};

impl Parser<'_> {
    // --- Type expressions ---

    /// Parse a type expression: `Dimensionless` or a dimension expression.
    pub(super) fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        // Parse the base type first
        let mut base = if let Some((Token::Ident, span)) = self.lexer.peek_with_span() {
            let text = self.lexer.slice_at(span);
            if text == "Dimensionless" {
                let (_, span) = self.advance()?;
                TypeExpr {
                    kind: TypeExprKind::Dimensionless,
                    span,
                }
            } else if text == "Bool" {
                let (_, span) = self.advance()?;
                TypeExpr {
                    kind: TypeExprKind::Bool,
                    span,
                }
            } else if text == "Int" {
                let (_, span) = self.advance()?;
                TypeExpr {
                    kind: TypeExprKind::Int,
                    span,
                }
            } else if is_pascal_case(text) && self.is_lt_after_ident(span) {
                // Type application: Vec3<Length, ECI>
                let ident = self.parse_any_ident()?;
                let type_args = self.parse_type_arg_list()?;
                let end_span = type_args.last().map_or(ident.span, |a| a.span);
                let span = ident.span.merge(end_span);
                TypeExpr {
                    kind: TypeExprKind::TypeApplication {
                        name: ident,
                        type_args,
                    },
                    span,
                }
            } else {
                let dim_expr = self.parse_dim_expr()?;
                let span = dim_expr.span;
                TypeExpr {
                    kind: TypeExprKind::DimExpr(dim_expr),
                    span,
                }
            }
        } else {
            let dim_expr = self.parse_dim_expr()?;
            let span = dim_expr.span;
            TypeExpr {
                kind: TypeExprKind::DimExpr(dim_expr),
                span,
            }
        };

        // Check for optional `[Index, ...]` suffix
        if self.lexer.peek() == Some(&Token::LBracket) {
            let (_, _bracket_span) = self.advance()?;
            let mut indexes = Vec::new();
            loop {
                if self.lexer.peek() == Some(&Token::RBracket) {
                    break;
                }
                let idx_name =
                    self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
                indexes.push(idx_name);
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            let (_, end_span) = self.expect(Token::RBracket)?;
            let span = base.span.merge(end_span);
            base = TypeExpr {
                kind: TypeExprKind::Indexed {
                    base: Box::new(base),
                    indexes,
                },
                span,
            };
        }

        Ok(base)
    }

    /// Parse a dimension expression: `DimTerm (("*" | "/") DimTerm)*`
    pub(super) fn parse_dim_expr(&mut self) -> Result<DimExpr, ParseError> {
        let first_term = self.parse_dim_term()?;
        let start_span = first_term.span;
        let mut terms = vec![DimExprItem {
            op: MulDivOp::Mul,
            term: first_term,
        }];

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let term = self.parse_dim_term()?;
                    terms.push(DimExprItem {
                        op: MulDivOp::Mul,
                        term,
                    });
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let term = self.parse_dim_term()?;
                    terms.push(DimExprItem {
                        op: MulDivOp::Div,
                        term,
                    });
                }
                _ => break,
            }
        }

        let end_span = terms
            .last()
            .ok_or_else(|| self.unexpected_eof("dimension term"))?
            .term
            .span;
        Ok(DimExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single dimension term: `IDENT ("^" INTEGER)?`
    fn parse_dim_term(&mut self) -> Result<DimTerm, ParseError> {
        let name = self.parse_any_ident()?;
        let mut end_span = name.span;

        let power = if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (neg, value, span) = self.parse_integer_literal()?;
            end_span = span;
            Some(if neg { -value } else { value })
        } else {
            None
        };

        Ok(DimTerm {
            span: name.span.merge(end_span),
            name,
            power,
        })
    }

    // --- Unit expressions ---

    /// Parse a unit expression: `IDENT (("*" | "/") IDENT ("^" INTEGER)?)*`
    pub(super) fn parse_unit_expr(&mut self) -> Result<UnitExpr, ParseError> {
        let first_ident = self.parse_any_ident()?;
        let start_span = first_ident.span;
        let mut end_span = first_ident.span;
        let first_name = first_ident.into_spanned::<UnitName>();

        let first_power = if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (neg, value, span) = self.parse_integer_literal()?;
            end_span = span;
            Some(if neg { -value } else { value })
        } else {
            None
        };

        let mut terms = vec![UnitExprItem {
            op: MulDivOp::Mul,
            name: first_name,
            power: first_power,
        }];

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let ident = self.parse_any_ident()?;
                    end_span = ident.span;
                    let name = ident.into_spanned::<UnitName>();
                    let power = if self.lexer.peek() == Some(&Token::Caret) {
                        self.lexer.next_token();
                        let (neg, value, span) = self.parse_integer_literal()?;
                        end_span = span;
                        Some(if neg { -value } else { value })
                    } else {
                        None
                    };
                    terms.push(UnitExprItem {
                        op: MulDivOp::Mul,
                        name,
                        power,
                    });
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let ident = self.parse_any_ident()?;
                    end_span = ident.span;
                    let name = ident.into_spanned::<UnitName>();
                    let power = if self.lexer.peek() == Some(&Token::Caret) {
                        self.lexer.next_token();
                        let (neg, value, span) = self.parse_integer_literal()?;
                        end_span = span;
                        Some(if neg { -value } else { value })
                    } else {
                        None
                    };
                    terms.push(UnitExprItem {
                        op: MulDivOp::Div,
                        name,
                        power,
                    });
                }
                _ => break,
            }
        }

        Ok(UnitExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse an integer literal, possibly preceded by `-`.
    /// Returns `(is_negative, absolute_value, span)`.
    pub(super) fn parse_integer_literal(
        &mut self,
    ) -> Result<(bool, i32, crate::span::Span), ParseError> {
        let neg = if self.lexer.peek() == Some(&Token::Minus) {
            self.lexer.next_token();
            true
        } else {
            false
        };

        match self.lexer.next_token() {
            Some((Token::Number, span)) => {
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: i32 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected integer".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok((neg, value, span))
            }
            Some((tok, span)) => Err(self.unexpected_token("integer", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("integer")),
        }
    }

    /// Parse the RHS of a unit definition: `NUMBER UNIT_EXPR`
    /// E.g., `1000 m`, `1 kg * m / s^2`, `(PI / 180) rad`
    pub(super) fn parse_unit_def(&mut self) -> Result<UnitDef, ParseError> {
        // Parse the scale expression: either a plain number or `(expr)`
        let (scale, scale_span) = self.parse_unit_scale()?;
        let unit_expr = self.parse_unit_expr()?;
        let span = scale_span.merge(unit_expr.span);
        Ok(UnitDef {
            scale,
            unit_expr,
            span,
        })
    }

    /// Parse the scale part of a unit definition.
    /// Supports: `1000`, `0.001`, `(PI / 180)`, `(expr)`
    fn parse_unit_scale(&mut self) -> Result<(f64, crate::span::Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                    ParseError::InvalidNumber {
                        reason: e.to_string(),
                        src: self.named_source(),
                        span: span.into(),
                    }
                })?;
                Ok((value, span))
            }
            Some(Token::LParen) => {
                let (_, lp_span) = self.advance()?;
                let expr = self.parse_expr()?;
                let (_, rp_span) = self.expect(Token::RParen)?;
                let span = lp_span.merge(rp_span);
                let scale = self.eval_const_expr(&expr)?;
                Ok((scale, span))
            }
            Some(_) => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("number or `(`", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("number or `(`")),
        }
    }

    /// Parse a type argument list: `<TypeExpr, TypeExpr, ...>`
    pub(super) fn parse_type_arg_list(&mut self) -> Result<Vec<TypeExpr>, ParseError> {
        self.expect(Token::Lt)?;
        let mut args = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::Gt) {
                break;
            }
            args.push(self.parse_type_expr()?);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::Gt)?;
        Ok(args)
    }

    /// Check if `<` follows the current ident token (used for type application detection).
    /// Scans the raw source after the ident span to find `<` (skipping whitespace).
    pub(super) fn is_lt_after_ident(&self, ident_span: crate::span::Span) -> bool {
        let bytes = self.source.as_bytes();
        let mut pos = ident_span.offset() + ident_span.len();
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        pos < bytes.len() && bytes[pos] == b'<'
    }
}
