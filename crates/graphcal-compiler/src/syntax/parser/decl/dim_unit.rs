use crate::syntax::ast::{BaseDimDecl, DeclKind, Declaration, DimDecl};
use crate::syntax::names::{DimName, UnitName};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    // --- dimension and unit declarations ---

    /// Parse `base dimension Name;`
    pub(super) fn parse_base_dimension_decl(
        &mut self,
        base_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        let (_, _dim_span) = self.expect(Token::Dimension)?;
        let name = self.parse_any_ident()?.into_spanned::<DimName>();
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = base_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::BaseDimension(BaseDimDecl { name }),
            span,
        })
    }

    /// Parse `dimension Name = DimExpr;` (derived dimensions only)
    pub(super) fn parse_dimension_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dimension)?;
        let name = self.parse_any_ident()?.into_spanned::<DimName>();

        self.expect(Token::Eq)?;
        let definition = self.parse_dim_expr()?;

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Dimension(DimDecl { name, definition }),
            span,
        })
    }

    pub(super) fn parse_unit_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(start_span)
    }

    pub(super) fn parse_const_unit(
        &mut self,
        const_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        let (_, _unit_span) = self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(const_span)
    }

    fn parse_unit_decl_inner(
        &mut self,
        start_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        let name = self.parse_any_ident()?.into_spanned::<UnitName>();
        self.expect(Token::Colon)?;
        let dim_type = self.parse_dim_expr()?;

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            let def = self.parse_unit_def()?;
            Some(def)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Unit(crate::syntax::ast::UnitDecl {
                name,
                dim_type,
                definition,
            }),
            span,
        })
    }
}
