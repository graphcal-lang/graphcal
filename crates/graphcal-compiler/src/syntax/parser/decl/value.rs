use crate::node_definition::NodeDefinition;
use crate::syntax::ast::{
    AssertBody, AssertDecl, ConstNodeDecl, DeclKind, Declaration, NodeDecl, ParamDecl, Visibility,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::span::Spanned;
use crate::syntax::token::{ContextualKeyword, Token};

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
                let definition = self.parse_node_definition()?;
                let (_, semi_span) = self.expect(Token::Semicolon)?;
                (
                    DeclKind::Node(NodeDecl {
                        visibility: Visibility::Private,
                        name,
                        type_ann,
                        definition,
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
            doc: None,
            attributes: vec![],
            kind: decl_kind,
            span,
        })
    }

    /// Only `todo {` at the node-body boundary selects the marker. `todo()`
    /// and identifiers named `todo` continue through ordinary expression parsing.
    fn parse_node_definition(
        &mut self,
    ) -> Result<NodeDefinition<crate::syntax::ast::Expr, ScopedName>, ParseError> {
        if self.lexer.peek() != Some(&Token::ContextualKeyword(ContextualKeyword::Todo))
            || self.lexer.peek_second() != Some(&Token::LBrace)
        {
            return self.parse_expr().map(NodeDefinition::Formula);
        }
        let (_, start) = self.advance()?;
        self.expect(Token::LBrace)?;
        let dependencies = self.parse_comma_separated(Token::RBrace, |parser| {
            let (_, at) = parser.expect(Token::At)?;
            let path = parser.parse_ident_path()?;
            let (owner, member) = path.split_last();
            let name = ScopedName::qualified_path(
                owner
                    .iter()
                    .map(|part| ModuleAliasName::from_atom(part.name.clone())),
                DeclName::from_atom(member.name.clone()),
            );
            Ok(Spanned::new(name, at.merge(path.span())))
        })?;
        let (_, end) = self.expect(Token::RBrace)?;
        Ok(NodeDefinition::Todo(Spanned::new(
            dependencies,
            start.merge(end),
        )))
    }

    // --- assert declaration ---

    pub(super) fn parse_assert(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Assert)?;
        let name = self.parse_any_ident()?.into_spanned::<DeclName>();
        self.expect(Token::Eq)?;
        let first_expr = self.parse_expr()?;

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

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            doc: None,
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
