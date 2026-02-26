use crate::ast::{DeclKind, Declaration, IndexDecl, IndexDeclKind};
use crate::names::{IndexName, VariantName};
use crate::token::Token;

use super::super::{ParseError, Parser, is_pascal_case};

impl Parser<'_> {
    // --- index declaration ---

    /// Parse an index declaration:
    /// `index Maneuver = { Departure, Correction, Insertion }`
    pub(super) fn parse_index_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Index)?;
        let name = self
            .parse_ident_with_casing("PascalCase", is_pascal_case)?
            .into_spanned::<IndexName>();
        self.expect(Token::Eq)?;

        // Check whether this is a range index or a named index
        let is_range = if let Some((&Token::Ident, span)) = self.lexer.peek_with_span() {
            self.lexer.slice_at(span) == "range"
        } else {
            false
        };
        if is_range {
            // range(start, end, step: step)
            self.lexer.next_token(); // consume "range"
            self.expect(Token::LParen)?;
            let start = self.parse_expr()?;
            self.expect(Token::Comma)?;
            let end = self.parse_expr()?;
            self.expect(Token::Comma)?;
            // Expect `step:` keyword argument
            let is_step = if let Some((&Token::Ident, span)) = self.lexer.peek_with_span() {
                self.lexer.slice_at(span) == "step"
            } else {
                false
            };
            if is_step {
                self.lexer.next_token(); // consume "step"
            } else {
                let (tok, span) = self.advance()?;
                return Err(self.unexpected_token("`step`", &tok.to_string(), span));
            }
            self.expect(Token::Colon)?;
            let step = self.parse_expr()?;
            let (_, end_span) = self.expect(Token::RParen)?;
            self.expect(Token::Semicolon)?;
            let span = start_span.merge(end_span);
            return Ok(Declaration {
                attributes: vec![],
                kind: DeclKind::Index(IndexDecl {
                    name,
                    kind: IndexDeclKind::Range {
                        start: Box::new(start),
                        end: Box::new(end),
                        step: Box::new(step),
                    },
                }),
                span,
            });
        }

        self.expect(Token::LBrace)?;

        let mut variants = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let variant = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<VariantName>();
            variants.push(variant);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        if variants.is_empty() {
            let (tok, span) = self.advance()?;
            return Err(self.unexpected_token("at least one variant", &tok.to_string(), span));
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Index(IndexDecl {
                name,
                kind: IndexDeclKind::Named { variants },
            }),
            span,
        })
    }
}
