use crate::syntax::ast::{DeclKind, Declaration, IndexDecl, IndexDeclKind};
use crate::syntax::names::{IndexName, VariantName};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser, is_pascal_case};

impl Parser<'_> {
    /// Parse a unified index declaration:
    /// - `index Maneuver = { Departure, Correction, Insertion };` (named with variants)
    /// - `index TimeStep = linspace(0.0 s, 100.0 s, step: 0.1 s);` (range / linspace)
    /// - `index Foo;` (required named — must be bound via parameterized import)
    /// - `index Foo: Time;` (required range — must be bound via parameterized import)
    pub(super) fn parse_index_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Index)?;
        let name = self
            .parse_ident_with_casing("PascalCase", is_pascal_case)?
            .into_spanned::<IndexName>();

        // Required named index: `index Foo;`
        if self.lexer.peek() == Some(&Token::Semicolon) {
            let (_, end_span) = self.expect(Token::Semicolon)?;
            let span = start_span.merge(end_span);
            return Ok(Declaration {
                attributes: vec![],
                kind: DeclKind::Index(IndexDecl {
                    name,
                    kind: IndexDeclKind::RequiredNamed,
                }),
                span,
            });
        }

        // Required range index: `index Foo: Time;`
        if self.lexer.peek() == Some(&Token::Colon) {
            self.expect(Token::Colon)?;
            let dimension = self.parse_dim_expr()?;
            let (_, end_span) = self.expect(Token::Semicolon)?;
            let span = start_span.merge(end_span);
            return Ok(Declaration {
                attributes: vec![],
                kind: DeclKind::Index(IndexDecl {
                    name,
                    kind: IndexDeclKind::RequiredRange { dimension },
                }),
                span,
            });
        }

        // Both named and linspace forms require `=` next
        self.expect(Token::Eq)?;

        // Determine which form based on what follows `=`
        match self.lexer.peek() {
            // Named index: `index Phase = { V1, V2, V3 };`
            Some(&Token::LBrace) => {
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
                    return Err(self.unexpected_token(
                        "at least one variant",
                        &tok.to_string(),
                        span,
                    ));
                }

                let (_, end_span) = self.expect(Token::RBrace)?;
                self.expect(Token::Semicolon)?;
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
            // Range/linspace index: `index TimeStep = linspace(0.0 s, 100.0 s, step: 0.1 s);`
            Some(&Token::Linspace) => {
                self.expect(Token::Linspace)?;
                self.expect(Token::LParen)?;
                let start = self.parse_expr()?;
                self.expect(Token::Comma)?;
                let end = self.parse_expr()?;
                self.expect(Token::Comma)?;
                self.expect(Token::Step)?;
                self.expect(Token::Colon)?;
                let step = self.parse_expr()?;
                let (_, end_span) = self.expect(Token::RParen)?;
                self.expect(Token::Semicolon)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
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
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("`{` or `linspace`", &tok.to_string(), span))
            }
        }
    }
}
