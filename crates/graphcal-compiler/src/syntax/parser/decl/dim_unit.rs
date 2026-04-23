use crate::syntax::ast::{BaseDimDecl, DeclKind, Declaration, DimDecl, Visibility};
use crate::syntax::names::{DimName, UnitName};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    // --- dimension and unit declarations ---

    /// Parse `base dim Name;`
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
            visibility: Visibility::Private,
            kind: DeclKind::BaseDimension(BaseDimDecl { name }),
            span,
            multi_decl_info: None,
        })
    }

    /// Parse a dimension declaration:
    /// - Derived: `dim Name = DimExpr;`
    /// - Required: `dim Name;` — the library requires a dimension
    ///   bound from outside.
    pub(super) fn parse_dimension_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dimension)?;
        let name = self.parse_any_ident()?.into_spanned::<DimName>();

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.expect(Token::Eq)?;
            Some(self.parse_dim_expr()?)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            visibility: Visibility::Private,
            kind: DeclKind::Dimension(DimDecl { name, definition }),
            span,
            multi_decl_info: None,
        })
    }

    /// Parse `unit Name: Dim = scale unit_expr;`.
    ///
    /// The no-body form `unit Name: Dim;` is now rejected — use
    /// `base unit Name: Dim;` (parsed via `parse_base_unit_decl`).
    pub(super) fn parse_unit_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(start_span, /*require_definition=*/ true)
    }

    /// Parse `const unit Name: Dim = scale unit_expr;`.
    pub(super) fn parse_const_unit(
        &mut self,
        const_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        let (_, _unit_span) = self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(const_span, /*require_definition=*/ true)
    }

    /// Parse `base unit Name: Dim;` — a base unit in its dimension.
    pub(super) fn parse_base_unit_decl(
        &mut self,
        base_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(base_span, /*require_definition=*/ false)
    }

    fn parse_unit_decl_inner(
        &mut self,
        start_span: crate::syntax::span::Span,
        require_definition: bool,
    ) -> Result<Declaration, ParseError> {
        let name = self.parse_any_ident()?.into_spanned::<UnitName>();
        self.expect(Token::Colon)?;
        let dim_type = self.parse_dim_expr()?;

        let definition = match self.lexer.peek() {
            Some(Token::Eq) => {
                self.lexer.next_token();
                Some(self.parse_unit_def()?)
            }
            _ => None,
        };

        if require_definition && definition.is_none() {
            // Non-base / non-const unit without a body: disallowed after A4.
            let err_span = name.span;
            return Err(self.unexpected_token(
                "`=` followed by a unit definition (use `base unit` for a no-body declaration)",
                ";",
                err_span,
            ));
        }

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            visibility: Visibility::Private,
            kind: DeclKind::Unit(crate::syntax::ast::UnitDecl {
                name,
                dim_type,
                definition,
            }),
            span,
            multi_decl_info: None,
        })
    }
}
