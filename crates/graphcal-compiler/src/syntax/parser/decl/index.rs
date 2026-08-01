use crate::syntax::ast::{BindableVisibility, DeclKind, Declaration, IndexDecl, IndexDeclKind};
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::span::Span;
use crate::syntax::token::{ContextualKeyword, Token};

use super::super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse a unified index declaration:
    /// - `index Maneuver = { Departure, Correction, Insertion };` (named with variants)
    /// - `index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);` (exact increment)
    /// - `index Samples = linspace(0.0 s, 1.0 s, points: 11);` (exact count)
    /// - `index Foo;` (required named — must be bound via parameterized include)
    /// - `index Foo: Time;` (required coordinate — bound via parameterized include)
    pub(super) fn parse_index_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Index)?;
        let name = self.parse_any_ident()?.into_spanned::<IndexName>();
        let (kind, end_span) = match self.lexer.peek() {
            Some(&Token::Semicolon) => {
                let (_, end_span) = self.expect(Token::Semicolon)?;
                (IndexDeclKind::RequiredNamed, end_span)
            }
            Some(&Token::Colon) => {
                self.expect(Token::Colon)?;
                let dimension = self.parse_dim_expr()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                (IndexDeclKind::RequiredCoordinate { dimension }, end_span)
            }
            _ => {
                self.expect(Token::Eq)?;
                self.parse_concrete_index_kind()?
            }
        };
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Index(IndexDecl {
                visibility: BindableVisibility::Private,
                name,
                kind,
            }),
            span: start_span.merge(end_span),
        })
    }

    fn parse_concrete_index_kind(&mut self) -> Result<(IndexDeclKind, Span), ParseError> {
        match self.lexer.peek() {
            Some(&Token::LBrace) => self.parse_named_index_kind(),
            Some(&Token::ContextualKeyword(ContextualKeyword::Range)) => {
                self.parse_range_index_kind()
            }
            Some(&Token::ContextualKeyword(ContextualKeyword::Linspace)) => {
                self.parse_linspace_index_kind()
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("`{`, `range`, or `linspace`", &tok.to_string(), span))
            }
        }
    }

    fn parse_named_index_kind(&mut self) -> Result<(IndexDeclKind, Span), ParseError> {
        self.expect(Token::LBrace)?;
        let variants = self.parse_comma_separated(Token::RBrace, |parser| {
            Ok(parser.parse_any_ident()?.into_spanned::<IndexVariantName>())
        })?;
        if variants.is_empty() {
            let (token, span) = self.advance()?;
            return Err(self.unexpected_token("at least one variant", &token.to_string(), span));
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        self.expect(Token::Semicolon)?;
        Ok((IndexDeclKind::Named { variants }, end_span))
    }

    fn parse_range_index_kind(&mut self) -> Result<(IndexDeclKind, Span), ParseError> {
        self.expect(Token::ContextualKeyword(ContextualKeyword::Range))?;
        self.expect(Token::LParen)?;
        let start = self.parse_expr()?;
        self.expect(Token::Comma)?;
        let end = self.parse_expr()?;
        self.expect(Token::Comma)?;
        self.expect(Token::ContextualKeyword(ContextualKeyword::Step))?;
        self.expect(Token::Colon)?;
        let step = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        self.expect(Token::Semicolon)?;
        Ok((
            IndexDeclKind::Range {
                start: Box::new(start),
                end: Box::new(end),
                step: Box::new(step),
            },
            end_span,
        ))
    }

    fn parse_linspace_index_kind(&mut self) -> Result<(IndexDeclKind, Span), ParseError> {
        self.expect(Token::ContextualKeyword(ContextualKeyword::Linspace))?;
        self.expect(Token::LParen)?;
        let start = self.parse_expr()?;
        self.expect(Token::Comma)?;
        let end = self.parse_expr()?;
        self.expect(Token::Comma)?;
        self.expect(Token::ContextualKeyword(ContextualKeyword::Points))?;
        self.expect(Token::Colon)?;
        let points = self.parse_nat_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        self.expect(Token::Semicolon)?;
        Ok((
            IndexDeclKind::Linspace {
                start: Box::new(start),
                end: Box::new(end),
                points,
            },
            end_span,
        ))
    }
}
