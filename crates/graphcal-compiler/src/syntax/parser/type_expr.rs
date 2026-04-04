use crate::syntax::ast::{
    DimExpr, DimExprItem, DimTerm, Expr, ExprKind, IndexExpr, MulDivOp, TypeExpr, TypeExprKind,
    UnitDef, UnitExpr, UnitExprItem,
};
use crate::syntax::names::UnitName;
use crate::syntax::span::Span;
use crate::syntax::token::Token;

use super::{ParseError, Parser, is_pascal_case, is_uppercase_starting};

impl Parser<'_> {
    // --- Type expressions ---

    /// Parse a type expression: `Dimensionless` or a dimension expression.
    #[expect(
        clippy::too_many_lines,
        reason = "type expression parser handles many keyword cases"
    )]
    pub(super) fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        // Parse the base type first
        let mut base = if let Some((Token::Ident, span)) = self.lexer.peek_with_span() {
            let text = self.lexer.slice_at(span);
            if text == "Dimensionless" {
                let (_, span) = self.advance()?;
                TypeExpr {
                    kind: TypeExprKind::Dimensionless,
                    constraints: vec![],
                    span,
                }
            } else if text == "Bool" {
                let (_, span) = self.advance()?;
                TypeExpr {
                    kind: TypeExprKind::Bool,
                    constraints: vec![],
                    span,
                }
            } else if text == "Int" {
                let (_, span) = self.advance()?;
                TypeExpr {
                    kind: TypeExprKind::Int,
                    constraints: vec![],
                    span,
                }
            } else if text == "Datetime" {
                let ident = self.parse_any_ident()?;
                if self.is_lt_after_ident(ident.span) {
                    // Datetime<TT> — parse as TypeApplication
                    let type_args = self.parse_type_arg_list()?;
                    let end_span = type_args.last().map_or(ident.span, |a| a.span);
                    let span = ident.span.merge(end_span);
                    TypeExpr {
                        kind: TypeExprKind::TypeApplication {
                            name: ident,
                            type_args,
                        },
                        constraints: vec![],
                        span,
                    }
                } else {
                    // Bare Datetime (= Datetime<UTC>)
                    TypeExpr {
                        kind: TypeExprKind::Datetime,
                        constraints: vec![],
                        span: ident.span,
                    }
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
                    constraints: vec![],
                    span,
                }
            } else {
                let dim_expr = self.parse_dim_expr()?;
                let span = dim_expr.span;
                TypeExpr {
                    kind: TypeExprKind::DimExpr(dim_expr),
                    constraints: vec![],
                    span,
                }
            }
        } else {
            let dim_expr = self.parse_dim_expr()?;
            let span = dim_expr.span;
            TypeExpr {
                kind: TypeExprKind::DimExpr(dim_expr),
                constraints: vec![],
                span,
            }
        };

        // Check for optional domain constraints: `(min: expr, max: expr)`
        if self.lexer.peek() == Some(&Token::LParen) {
            let constraints = self.parse_domain_constraints()?;
            if let Some(last) = constraints.last() {
                base.span = base.span.merge(last.span);
            }
            base.constraints = constraints;
        }

        // Check for optional `[Index, ...]` suffix
        // Supports named indexes (`Phase`), generic params (`I`, `N`), and nat literals (`3`)
        if self.lexer.peek() == Some(&Token::LBracket) {
            let (_, _bracket_span) = self.advance()?;
            let mut indexes = Vec::new();
            loop {
                if self.lexer.peek() == Some(&Token::RBracket) {
                    break;
                }
                let idx_expr = self.parse_index_expr()?;
                indexes.push(idx_expr);
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
                constraints: vec![],
                span,
            };
        }

        Ok(base)
    }

    /// Parse domain constraints: `(min: expr, max: expr)`.
    ///
    /// Called when `(` is peeked after a base type expression.
    /// Each constraint is `name: expr` where `name` is an identifier like `min` or `max`.
    fn parse_domain_constraints(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::DomainBound>, ParseError> {
        let (_, _lparen_span) = self.expect(Token::LParen)?;
        let mut constraints = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            let ident = self.parse_any_ident()?;
            let kind_span = ident.span;
            let kind = match ident.name.as_str() {
                "min" => crate::syntax::ast::DomainBoundKind::Min,
                "max" => crate::syntax::ast::DomainBoundKind::Max,
                _ => {
                    return Err(ParseError::InvalidDomainBoundKey {
                        key: ident.name,
                        src: self.named_source(),
                        span: kind_span.into(),
                    });
                }
            };
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            let bound_span = kind_span.merge(value.span);
            constraints.push(crate::syntax::ast::DomainBound {
                kind,
                kind_span,
                value,
                span: bound_span,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        let (_, rparen_span) = self.expect(Token::RParen)?;
        // Update span of last constraint to include rparen for better error reporting
        if constraints.is_empty() {
            return Err(self.unexpected_token(
                "at least one domain constraint (e.g., `min: 0`)",
                ")",
                rparen_span,
            ));
        }
        Ok(constraints)
    }

    /// Parse a dimension expression: `DimTermOrGroup (("*" | "/") DimTermOrGroup)*`
    ///
    /// A term-or-group is either `IDENT ("^" INTEGER)?` or `"(" DimExpr ")" ("^" INTEGER)?`.
    /// Parenthesized groups are flattened: `(A * B / C)^2` becomes `A^2 * B^2 / C^2`,
    /// and `D / (A * B)` becomes `D / A / B`.
    pub(super) fn parse_dim_expr(&mut self) -> Result<DimExpr, ParseError> {
        let first_items = self.parse_dim_term_or_group()?;
        let start_span = first_items[0].term.span;
        let mut terms: Vec<DimExprItem> = first_items;

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let items = self.parse_dim_term_or_group()?;
                    for mut item in items {
                        item.op = Self::combine_ops(MulDivOp::Mul, item.op);
                        terms.push(item);
                    }
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let items = self.parse_dim_term_or_group()?;
                    for mut item in items {
                        item.op = Self::combine_ops(MulDivOp::Div, item.op);
                        terms.push(item);
                    }
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

    /// Parse a single dimension term or a parenthesized group, returning flattened items.
    ///
    /// - `IDENT ("^" INTEGER)?` → single item with op=Mul
    /// - `"(" DimExpr ")" ("^" INTEGER)?` → flattened items with powers multiplied
    fn parse_dim_term_or_group(&mut self) -> Result<Vec<DimExprItem>, ParseError> {
        if self.lexer.peek() == Some(&Token::LParen) {
            self.lexer.next_token();
            let inner = self.parse_dim_expr()?;
            self.expect(Token::RParen)?;

            // Optional outer exponent: `(A * B)^2`
            let outer_power = if self.lexer.peek() == Some(&Token::Caret) {
                self.lexer.next_token();
                let (neg, value, _span) = self.parse_integer_literal()?;
                if neg { -value } else { value }
            } else {
                1
            };

            // Flatten: distribute the outer power to each inner term
            let items = inner
                .terms
                .into_iter()
                .map(|item| {
                    let inner_power = item.term.power.unwrap_or(1);
                    let combined = inner_power * outer_power;
                    DimExprItem {
                        op: item.op,
                        term: DimTerm {
                            name: item.term.name,
                            power: if combined == 1 { None } else { Some(combined) },
                            span: item.term.span,
                        },
                    }
                })
                .collect();
            Ok(items)
        } else {
            let term = self.parse_dim_term()?;
            Ok(vec![DimExprItem {
                op: MulDivOp::Mul,
                term,
            }])
        }
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

    /// Combine two `MulDivOp`s: `Mul*Mul=Mul`, `Mul*Div=Div`, `Div*Mul=Div`, `Div*Div=Mul`.
    const fn combine_ops(outer: MulDivOp, inner: MulDivOp) -> MulDivOp {
        match (outer, inner) {
            (MulDivOp::Mul, MulDivOp::Mul) | (MulDivOp::Div, MulDivOp::Div) => MulDivOp::Mul,
            (MulDivOp::Mul, MulDivOp::Div) | (MulDivOp::Div, MulDivOp::Mul) => MulDivOp::Div,
        }
    }

    // --- Unit expressions ---

    /// Parse a unit expression:
    ///   `unit_term (("*" | "/") unit_term)*`
    /// where `unit_term` is `IDENT ["^" INTEGER]` or `"(" unit_expr ")" ["^" INTEGER]`.
    ///
    /// Parenthesized groups are flattened into the term list (operator
    /// combination and power distribution), so the AST stays flat.
    pub(super) fn parse_unit_expr(&mut self) -> Result<UnitExpr, ParseError> {
        let (first_terms, start_span, mut end_span) =
            self.parse_unit_term_or_group(MulDivOp::Mul)?;

        let mut terms: Vec<UnitExprItem> = first_terms;

        while let Some(&Token::Star | &Token::Slash) = self.lexer.peek() {
            // peek() confirmed a token exists, so next_token() will return Some.
            let Some((op_token, op_span)) = self.lexer.next_token() else {
                break;
            };
            let outer_op = if op_token == Token::Star {
                MulDivOp::Mul
            } else {
                MulDivOp::Div
            };

            // Only continue the unit expression if next token is an identifier
            // or `(` (parenthesized group). Otherwise, put the operator back
            // and let the expression parser handle it as arithmetic
            // (e.g., `459.3 W / (1.0 m^2)`).
            if !matches!(self.lexer.peek(), Some(&Token::Ident | &Token::LParen)) {
                self.lexer.put_back(op_token, op_span);
                break;
            }

            let (new_terms, _, new_end) = self.parse_unit_term_or_group(outer_op)?;
            end_span = new_end;
            terms.extend(new_terms);
        }

        Ok(UnitExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single unit term or a parenthesized unit group.
    ///
    /// Returns `(items, start_span, end_span)`. For a parenthesized group the
    /// items are flattened with the given `outer_op` distributed across them.
    fn parse_unit_term_or_group(
        &mut self,
        outer_op: MulDivOp,
    ) -> Result<(Vec<UnitExprItem>, Span, Span), ParseError> {
        if self.lexer.peek() == Some(&Token::LParen) {
            let (_, lparen_span) = self.expect(Token::LParen)?;
            let inner = self.parse_unit_expr()?;
            let (_, rparen_span) = self.expect(Token::RParen)?;

            let outer_power = if self.lexer.peek() == Some(&Token::Caret) {
                self.lexer.next_token();
                let (neg, value, _span) = self.parse_integer_literal()?;
                if neg { -value } else { value }
            } else {
                1
            };

            let end_span = rparen_span;
            let items = inner
                .terms
                .into_iter()
                .map(|item| {
                    let combined_power = item.power.unwrap_or(1) * outer_power;
                    UnitExprItem {
                        op: Self::combine_ops(outer_op, item.op),
                        name: item.name,
                        power: if combined_power == 1 {
                            None
                        } else {
                            Some(combined_power)
                        },
                    }
                })
                .collect();
            Ok((items, lparen_span, end_span))
        } else {
            let ident = self.parse_any_ident()?;
            let start_span = ident.span;
            let mut end_span = ident.span;
            let name = ident.into_spanned::<UnitName>();
            let power = if self.lexer.peek() == Some(&Token::Caret) {
                self.lexer.next_token();
                let (neg, value, span) = self.parse_integer_literal()?;
                end_span = span;
                Some(if neg { -value } else { value })
            } else {
                None
            };
            Ok((
                vec![UnitExprItem {
                    op: outer_op,
                    name,
                    power,
                }],
                start_span,
                end_span,
            ))
        }
    }

    /// Parse an integer literal, possibly preceded by `-`.
    /// Returns `(is_negative, absolute_value, span)`.
    pub(super) fn parse_integer_literal(
        &mut self,
    ) -> Result<(bool, i32, crate::syntax::span::Span), ParseError> {
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
        let scale_expr = self.parse_unit_scale()?;
        let unit_expr = self.parse_unit_expr()?;
        let span = scale_expr.span.merge(unit_expr.span);
        Ok(UnitDef {
            scale_expr,
            unit_expr,
            span,
        })
    }

    /// Parse the scale part of a unit definition.
    /// Supports: `1000`, `0.001`, `(PI / 180)`, `(expr)`
    fn parse_unit_scale(&mut self) -> Result<Expr, ParseError> {
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
                Ok(Expr {
                    kind: ExprKind::Number(value),
                    span,
                })
            }
            Some(Token::LParen) => {
                let (_, _lp_span) = self.advance()?;
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
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

    /// Parse an index expression in type position: either an identifier or an integer literal.
    ///
    /// - `Phase` → `IndexExpr::Name` (named index or generic param)
    /// - `3` → `IndexExpr::NatLiteral` (desugars to `range(3)`)
    fn parse_index_expr(&mut self) -> Result<IndexExpr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: u64 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected non-negative integer in index position".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok(IndexExpr::NatLiteral(value, span))
            }
            Some(
                Token::Ident
                | Token::Linspace
                | Token::Step
                | Token::Scan
                | Token::Unfold
                | Token::Index,
            ) => {
                let ident =
                    self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
                Ok(IndexExpr::Name(ident))
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("index name or integer literal", &tok.to_string(), span))
            }
        }
    }

    /// Check if `<` follows the current ident token (used for type application detection).
    /// Scans the raw source after the ident span to find `<` (skipping whitespace).
    pub(super) fn is_lt_after_ident(&self, ident_span: crate::syntax::span::Span) -> bool {
        let bytes = self.source.as_bytes();
        let mut pos = ident_span.offset() + ident_span.len();
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        pos < bytes.len() && bytes[pos] == b'<'
    }
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

    use super::*;
    use crate::syntax::ast::{DeclKind, TypeExprKind};

    fn dim_expr_name(te: &crate::syntax::ast::TypeExpr) -> &str {
        match &te.kind {
            TypeExprKind::DimExpr(dim) => {
                assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
                dim.terms[0].term.name.name.as_str()
            }
            other => panic!("expected DimExpr, got {other:?}"),
        }
    }

    #[test]
    fn parse_type_application_in_annotation() {
        let source = "param v: Vec3<Length, ECI> = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.name.as_str(), "Vec3");
                    assert_eq!(type_args.len(), 2);
                    assert_eq!(dim_expr_name(&type_args[0]), "Length");
                    assert_eq!(dim_expr_name(&type_args[1]), "ECI");
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_application_single_arg() {
        let source = "param t: Timestamp<UTC> = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.name.as_str(), "Timestamp");
                    assert_eq!(type_args.len(), 1);
                    assert_eq!(dim_expr_name(&type_args[0]), "UTC");
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_non_generic_type_still_works() {
        let source = "param v: Length = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::DimExpr(_)));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_indexed_type() {
        let source = "param dv: Velocity[Maneuver] = 1.0 m/s;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "dv");
                match &p.type_ann.kind {
                    TypeExprKind::Indexed { base, indexes } => {
                        assert!(matches!(base.kind, TypeExprKind::DimExpr(_)));
                        assert_eq!(indexes.len(), 1);
                        let IndexExpr::Name(ident) = &indexes[0] else {
                            panic!("expected Name")
                        };
                        assert_eq!(ident.name, "Maneuver");
                    }
                    other => panic!("expected Indexed type, got {other:?}"),
                }
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_multi_indexed_type() {
        let source = "param matrix: Dimensionless[Row, Col] = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::Indexed { indexes, .. } => {
                    assert_eq!(indexes.len(), 2);
                    let IndexExpr::Name(ident) = &indexes[0] else {
                        panic!("expected Name")
                    };
                    assert_eq!(ident.name, "Row");
                    let IndexExpr::Name(ident) = &indexes[1] else {
                        panic!("expected Name")
                    };
                    assert_eq!(ident.name, "Col");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_with_min_max() {
        let source = "param m: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::DimExpr(_)));
                assert_eq!(p.type_ann.constraints.len(), 2);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Min
                );
                assert_eq!(
                    p.type_ann.constraints[1].kind,
                    crate::syntax::ast::DomainBoundKind::Max
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_with_min_only() {
        let source = "param f: Force(min: 0.01 N) = 0.5 N;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.type_ann.constraints.len(), 1);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Min
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_dimensionless_with_constraint() {
        let source = "param e: Dimensionless(max: 1.0) = 0.85;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::Dimensionless));
                assert_eq!(p.type_ann.constraints.len(), 1);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Max
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_constrained_indexed_type() {
        let source = "param dv: Velocity(min: 0.0 m/s, max: 10000.0 m/s)[Maneuver] = 1.0 m/s;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::Indexed { base, indexes } => {
                    // Constraints are on the base type, not the outer Indexed
                    assert_eq!(base.constraints.len(), 2);
                    assert_eq!(
                        base.constraints[0].kind,
                        crate::syntax::ast::DomainBoundKind::Min
                    );
                    assert_eq!(
                        base.constraints[1].kind,
                        crate::syntax::ast::DomainBoundKind::Max
                    );
                    assert_eq!(indexes.len(), 1);
                    let IndexExpr::Name(ident) = &indexes[0] else {
                        panic!("expected Name")
                    };
                    assert_eq!(ident.name, "Maneuver");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_int_with_constraints() {
        let source = "param n: Int(min: 1, max: 100) = 10;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::Int));
                assert_eq!(p.type_ann.constraints.len(), 2);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Min
                );
                assert_eq!(
                    p.type_ann.constraints[1].kind,
                    crate::syntax::ast::DomainBoundKind::Max
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_no_constraints_still_works() {
        let source = "param m: Mass = 1.0 kg;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(p.type_ann.constraints.is_empty());
            }
            _ => panic!("expected param"),
        }
    }
}
