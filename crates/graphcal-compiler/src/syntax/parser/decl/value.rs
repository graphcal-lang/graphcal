use crate::syntax::ast::{
    AssertBody, AssertDecl, ConstNodeDecl, DeclKind, Declaration, NodeDecl, ParamDecl, SlotKind,
    Visibility,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};
use super::multi::SlotHeader;

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
                let (value, value_range) = if self.lexer.peek() == Some(&Token::Eq) {
                    self.expect(Token::Eq)?;
                    let (value, range) = self.parse_ranged(Self::parse_expr)?;
                    (Some(value), Some(range))
                } else {
                    (None, None)
                };
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::Param(ParamDecl {
                        name,
                        type_ann,
                        value,
                        value_range,
                    }),
                    semi_span,
                )
            }
            SlotKind::Node(visibility) => {
                self.expect(Token::Eq)?;
                let (value, value_range) = self.parse_ranged(Self::parse_expr)?;
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::Node(NodeDecl {
                        visibility,
                        name,
                        type_ann,
                        value,
                        value_range,
                    }),
                    semi_span,
                )
            }
            SlotKind::ConstNode(visibility) => {
                self.expect(Token::Eq)?;
                let (value, value_range) = self.parse_ranged(Self::parse_expr)?;
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::ConstNode(ConstNodeDecl {
                        visibility,
                        name,
                        type_ann,
                        value,
                        value_range,
                    }),
                    semi_span,
                )
            }
        };

        let span = kind_span.merge(semi_span);

        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: decl_kind,
            span,
        })
    }

    /// Run `parse` and return what it parsed with the source range it
    /// consumed: from its first token to its last, so enclosing parentheses
    /// are included (the span of a parenthesized expression excludes them).
    fn parse_ranged<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<(T, Span), ParseError> {
        let start = self.lexer.peek_with_span().map(|(_, span)| span);
        let parsed = parse(self)?;
        let range = self.range_since(start, Span::new(0, 0));
        Ok((parsed, range))
    }

    /// The range from `start` (the first token of a production) to the
    /// last token consumed since; `fallback` when nothing was consumed.
    fn range_since(&self, start: Option<Span>, fallback: Span) -> Span {
        match (start, self.lexer.last_consumed()) {
            (Some(start), Some(end)) if end.offset() >= start.offset() => start.merge(end),
            _ => fallback,
        }
    }

    // --- assert declaration ---

    pub(super) fn parse_assert(
        &mut self,
        visibility: Visibility,
    ) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Assert)?;
        let name: Spanned<DeclName> = self.parse_any_ident()?.classify();
        self.expect(Token::Eq)?;
        let body_start = self.lexer.peek_with_span().map(|(_, span)| span);
        let first_expr = self.parse_expr()?;
        let first_span = first_expr.span;

        let body = if self.lexer.peek() == Some(&Token::TildeEq) {
            // Tolerance syntax: all three operands are full expressions.
            self.lexer.next_token(); // consume ~=
            let expected = self.parse_expr()?;
            self.expect(Token::PlusMinus)?;
            let tolerance = self.parse_expr()?;
            AssertBody::Tolerance {
                actual: Box::new(first_expr),
                expected: Box::new(expected),
                tolerance: Box::new(tolerance),
            }
        } else {
            AssertBody::Expr(first_expr)
        };

        let body_range = self.range_since(body_start, first_span);
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Assert(AssertDecl {
                visibility,
                name,
                body,
                body_range,
            }),
            span,
        })
    }
}
