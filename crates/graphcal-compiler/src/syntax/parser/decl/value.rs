use crate::syntax::ast::{
    AssertBody, AssertDecl, ConstNodeDecl, DeclKind, Declaration, NodeDecl, ParamDecl,
};
use crate::syntax::names::DeclName;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case};

impl Parser<'_> {
    // --- param/node/const node with required type annotation ---

    pub(super) fn parse_param(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Param)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        let value = if self.lexer.peek() == Some(&Token::Eq) {
            self.expect(Token::Eq)?;
            Some(self.parse_expr()?)
        } else {
            None
        };
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            is_pub: false,
            kind: DeclKind::Param(ParamDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    pub(super) fn parse_node(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Node)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            is_pub: false,
            kind: DeclKind::Node(NodeDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    pub(super) fn parse_const_node(
        &mut self,
        const_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        // `const` keyword already consumed by the caller.
        // Next token must be `node`.
        self.expect(Token::Node)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = const_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            is_pub: false,
            kind: DeclKind::ConstNode(ConstNodeDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    // --- assert declaration ---

    pub(super) fn parse_assert(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Assert)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Eq)?;
        let first_expr = self.parse_expr()?;

        let body = if self.lexer.peek() == Some(&Token::TildeEq) {
            // Tolerance syntax: actual ~= expected +/- tolerance [%]
            self.lexer.next_token(); // consume ~=
            let expected = self.parse_expr()?;
            self.expect(Token::PlusMinus)?;
            // Parse tolerance as a unary expr (not full expr) so `%` isn't consumed as modulo
            let tolerance = self.parse_unary()?;
            let is_relative = if self.lexer.peek() == Some(&Token::Percent) {
                self.lexer.next_token(); // consume %
                true
            } else {
                false
            };
            AssertBody::Tolerance {
                actual: Box::new(first_expr),
                expected: Box::new(expected),
                tolerance: Box::new(tolerance),
                is_relative,
            }
        } else {
            AssertBody::Expr(first_expr)
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            is_pub: false,
            kind: DeclKind::Assert(AssertDecl { name, body }),
            span,
        })
    }
}
