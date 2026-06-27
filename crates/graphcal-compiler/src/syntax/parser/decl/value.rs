use crate::syntax::ast::{
    AssertBody, AssertDecl, ConstNodeDecl, DeclKind, Declaration, NodeDecl, ParamDecl, Visibility,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};
use super::multi::{SlotHeader, SlotKind};

impl Parser<'_> {
    /// Complete a single `param` / `node` / `const node` declaration starting
    /// from an already-parsed slot header. Parses the optional (`param`) or
    /// mandatory (`node` / `const node`) initializer and the terminating `;`.
    pub(super) fn finish_single_value_decl(
        &mut self,
        header: SlotHeader,
    ) -> Result<Declaration, ParseError> {
        let SlotHeader {
            kind,
            kind_span,
            name,
            type_ann,
            ..
        } = header;

        let (decl_kind, semi_span) = match kind {
            SlotKind::Param => {
                let value = if self.lexer.peek() == Some(&Token::Eq) {
                    self.expect(Token::Eq)?;
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::Param(ParamDecl {
                        name,
                        type_ann,
                        value,
                    }),
                    semi_span,
                )
            }
            SlotKind::Node => {
                self.expect(Token::Eq)?;
                let value = self.parse_expr()?;
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::Node(NodeDecl {
                        visibility: Visibility::Private,
                        name,
                        type_ann,
                        value,
                    }),
                    semi_span,
                )
            }
            SlotKind::ConstNode => {
                self.expect(Token::Eq)?;
                let value = self.parse_expr()?;
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::ConstNode(ConstNodeDecl {
                        visibility: Visibility::Private,
                        name,
                        type_ann,
                        value,
                    }),
                    semi_span,
                )
            }
        };

        let span = kind_span.merge(semi_span);

        Ok(Declaration {
            attributes: vec![],
            kind: decl_kind,
            span,
        })
    }

    // --- assert declaration ---

    pub(super) fn parse_assert(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Assert)?;
        let name = self.parse_any_ident()?.into_spanned::<DeclName>();
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
            kind: DeclKind::Assert(AssertDecl {
                visibility: Visibility::Private,
                name,
                body,
            }),
            span,
        })
    }
}
