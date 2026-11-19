use crate::syntax::ast::{
    BaseDimDecl, BindableVisibility, DeclKind, Declaration, DimDecl, UnitBody, UnitConstness,
    UnitDecl, Visibility,
};
use crate::syntax::dimension::{DimName, UnitName};
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;

use super::super::{Expected, ParseError, Parser};

impl Parser<'_> {
    // --- dimension and unit declarations ---

    /// Parse `base dim Name;`
    pub(super) fn parse_base_dimension_decl(
        &mut self,
        base_span: crate::syntax::span::Span,
        visibility: Visibility,
    ) -> Result<Declaration, ParseError> {
        let (_, _dim_span) = self.expect(Token::Dimension)?;
        let name: Spanned<DimName> = self.parse_any_ident()?.classify();
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = base_span.merge(semi_span);
        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::BaseDimension(BaseDimDecl { visibility, name }),
            span,
        })
    }

    /// Parse a dimension declaration:
    /// - Derived: `dim Name = DimExpr;`
    /// - Required: `dim Name;` — the library requires a dimension
    ///   bound from outside.
    pub(super) fn parse_dimension_decl(
        &mut self,
        visibility: BindableVisibility,
    ) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dimension)?;
        let name: Spanned<DimName> = self.parse_any_ident()?.classify();

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.expect(Token::Eq)?;
            Some(self.parse_dim_expr()?)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Dimension(DimDecl {
                visibility,
                name,
                definition,
            }),
            span,
        })
    }

    /// Parse `unit Name: Dim = scale unit_expr;`.
    ///
    /// The no-body form `unit Name: Dim;` is rejected — use
    /// `base unit Name: Dim;` (parsed via `parse_base_unit_decl`).
    pub(super) fn parse_unit_decl(
        &mut self,
        visibility: Visibility,
    ) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(start_span, visibility, UnitConstness::Dynamic)
    }

    /// Parse `const unit Name: Dim = scale unit_expr;`.
    pub(super) fn parse_const_unit(
        &mut self,
        const_span: crate::syntax::span::Span,
        visibility: Visibility,
    ) -> Result<Declaration, ParseError> {
        self.expect(Token::Unit)?;
        self.parse_unit_decl_inner(const_span, visibility, UnitConstness::Const)
    }

    /// Parse `base unit Name: Dim;` — a base unit in its dimension.
    ///
    /// A base unit has no body: `base unit Name: Dim = …;` is rejected at the
    /// `=` rather than read as a `const unit`.
    pub(super) fn parse_base_unit_decl(
        &mut self,
        base_span: crate::syntax::span::Span,
        visibility: Visibility,
    ) -> Result<Declaration, ParseError> {
        self.expect(Token::Unit)?;
        let name: Spanned<UnitName> = self.parse_any_ident()?.classify();
        self.expect(Token::Colon)?;
        let dim_type = self.parse_dim_expr()?;
        let semi_span = match self.lexer.next_token() {
            Some((Token::Semicolon, span)) => span,
            Some((token, span)) => {
                return Err(Self::unexpected(Expected::BaseUnitEnd, token, span));
            }
            None => return Err(self.unexpected_eof(Expected::BaseUnitEnd)),
        };
        Ok(Self::unit_declaration(
            base_span.merge(semi_span),
            UnitDecl {
                visibility,
                name,
                dim_type,
                body: UnitBody::Base,
            },
        ))
    }

    /// Parse the rest of a `unit` / `const unit` declaration, whose scale
    /// body is mandatory.
    fn parse_unit_decl_inner(
        &mut self,
        start_span: crate::syntax::span::Span,
        visibility: Visibility,
        constness: UnitConstness,
    ) -> Result<Declaration, ParseError> {
        let name: Spanned<UnitName> = self.parse_any_ident()?.classify();
        self.expect(Token::Colon)?;
        let dim_type = self.parse_dim_expr()?;

        // Non-base units need a scale body; use `base unit` for a canonical unit.
        match self.lexer.next_token() {
            Some((Token::Eq, _)) => {}
            Some((token, span)) => {
                return Err(Self::unexpected(Expected::UnitDefinition, token, span));
            }
            None => return Err(self.unexpected_eof(Expected::UnitDefinition)),
        }
        let definition = self.parse_unit_def()?;
        let body = match constness {
            UnitConstness::Const => UnitBody::Const(definition),
            UnitConstness::Dynamic => UnitBody::Dynamic(definition),
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        Ok(Self::unit_declaration(
            start_span.merge(semi_span),
            UnitDecl {
                visibility,
                name,
                dim_type,
                body,
            },
        ))
    }

    const fn unit_declaration(span: crate::syntax::span::Span, unit: UnitDecl) -> Declaration {
        Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Unit(unit),
            span,
        }
    }
}
