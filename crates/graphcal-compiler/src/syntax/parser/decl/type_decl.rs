use crate::syntax::ast::{
    BindableVisibility, DeclKind, Declaration, FieldDecl, TypeDecl, TypeDeclBody, UnionMember,
};
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;
use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

use super::super::{Expected, Found, ParseError, Parser};

impl Parser<'_> {
    // --- type declaration ---

    pub(super) fn parse_type_decl(
        &mut self,
        visibility: BindableVisibility,
    ) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Type)?;
        let name: Spanned<StructTypeName> = self.parse_any_ident()?.classify();

        // Optional generic params: <D: Dim, F: Type>
        let generic_params = if self.lexer.peek() == Some(&Token::Lt) {
            self.parse_generic_params()?.into_vec()
        } else {
            Vec::new()
        };

        match self.lexer.peek() {
            Some(&Token::LBrace) => {
                // Unified body: either record (fields) or union (constructors).
                self.lexer.next_token(); // consume `{`
                self.parse_unified_type_body(visibility, name, generic_params, start_span)
            }
            Some(&Token::Semicolon) => {
                // Required type: `type Foo;` — bound from outside via include.
                let (_, end_span) = self.expect(Token::Semicolon)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    doc: None,
                    attributes: vec![],
                    kind: DeclKind::Type(TypeDecl {
                        visibility,
                        name,
                        generic_params,
                        body: TypeDeclBody::Required,
                    }),
                    span,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(Self::unexpected(Expected::TypeBody, tok, span))
            }
        }
    }

    /// Parse the body inside `type T { ... }` after the opening brace has
    /// been consumed. Distinguishes record-form (`ident : Type`) from
    /// union-form (`ident ( ... )` or `ident` followed by `,` / `}`) by
    /// one-token structural lookahead — never by identifier casing. All entries
    /// must agree on form; mixing produces a precise syntax error.
    fn parse_unified_type_body(
        &mut self,
        visibility: BindableVisibility,
        name: Spanned<StructTypeName>,
        generic_params: Vec<crate::syntax::ast::GenericParam>,
        start_span: Span,
    ) -> Result<Declaration, ParseError> {
        // The body of a `type T { ... }` must be a constructor list:
        // every entry is a constructor of an n-variant tagged union. A
        // record-shaped declaration is written as a single-variant
        // tagged union whose sole constructor's name matches the
        // type's name (`type Position { Position(x: Length, ...) }`).
        if self.lexer.peek() == Some(&Token::RBrace) {
            // `type T {}` is rejected: there is no zero-variant tagged
            // union (it has no inhabitants and no purpose). The author
            // either meant `type T { T }` (single unit constructor) or
            // `type T;` (required, awaits include binding).
            let (_, end_span) = self.advance()?;
            return Err(Self::unexpected_token(
                Expected::TypeConstructor,
                Found::EmptyBody,
                start_span.merge(end_span),
            ));
        }

        let first_ident = self.parse_any_ident()?;
        match self.lexer.peek() {
            Some(&Token::Colon) => {
                // Record-shaped entry. Reject with a precise diagnostic
                // pointing at the explicit single-variant form.
                Err(Self::unexpected_token(
                    Expected::ConstructorNotField,
                    Found::RecordStyleField,
                    first_ident.span,
                ))
            }
            Some(&Token::LParen | &Token::Comma | &Token::RBrace) => {
                let first_ctor = self.parse_constructor_tail(&first_ident)?;
                let members = self.continue_constructor_list(first_ctor)?;
                let (_, end_span) = self.expect(Token::RBrace)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    doc: None,
                    attributes: vec![],
                    kind: DeclKind::Type(TypeDecl {
                        visibility,
                        name,
                        generic_params,
                        body: TypeDeclBody::Constructors(members),
                    }),
                    span,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(Self::unexpected(Expected::ConstructorTail, tok, span))
            }
        }
    }

    /// Parse the payload (if any) following a constructor's identifier,
    /// producing a `UnionMember`. The identifier has already been consumed.
    fn parse_constructor_tail(
        &mut self,
        ident: &crate::syntax::ast::Ident,
    ) -> Result<UnionMember, ParseError> {
        let start_span = ident.span;
        let name = Spanned::new(
            ConstructorName::classify(ident.name.atom().clone()),
            ident.span,
        );

        let (payload, end_span) = match self.lexer.peek() {
            Some(&Token::LParen) => {
                self.lexer.next_token();
                // Empty payload `Ctor()` is allowed.
                let (fields, end_span) = if self.lexer.peek() == Some(&Token::RParen) {
                    let (_, end_span) = self.expect(Token::RParen)?;
                    (Vec::new(), end_span)
                } else {
                    let fields = self.parse_field_list_until(Token::RParen)?;
                    let (_, end_span) = self.expect(Token::RParen)?;
                    (fields, end_span)
                };
                (Some(fields), end_span)
            }
            _ => (None, start_span),
        };

        Ok(UnionMember {
            name,
            payload,
            span: start_span.merge(end_span),
        })
    }

    /// Parse `field: Type, field: Type, ...` terminated by `terminator`
    /// (which is *not* consumed). Trailing comma allowed.
    fn parse_field_list_until(&mut self, terminator: Token) -> Result<Vec<FieldDecl>, ParseError> {
        let mut fields = Vec::new();
        loop {
            let ident = self.parse_any_ident()?;
            self.expect(Token::Colon)?;
            let type_ann = self.parse_type_expr()?;
            fields.push(FieldDecl {
                name: Spanned::new(FieldName::classify(ident.name.into_atom()), ident.span),
                type_ann,
            });
            match self.lexer.peek() {
                Some(t) if *t == terminator => break,
                Some(&Token::Comma) => {
                    self.lexer.next_token();
                    if let Some(t) = self.lexer.peek()
                        && *t == terminator
                    {
                        break;
                    }
                }
                _ => {
                    let (tok, span) = self.advance()?;
                    return Err(Self::unexpected(Expected::FieldSeparator, tok, span));
                }
            }
        }
        Ok(fields)
    }

    /// Parse the rest of a constructor list (`, Ctor, Ctor(...), ...`),
    /// stopping at the closing `}` (not consumed). Trailing comma allowed.
    fn continue_constructor_list(
        &mut self,
        first: UnionMember,
    ) -> Result<Vec<UnionMember>, ParseError> {
        let mut members = vec![first];
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token();
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let ident = self.parse_any_ident()?;
            // If we see a record-form field here (`:` follows the ident),
            // reject with a precise error rather than silently parsing it.
            if self.lexer.peek() == Some(&Token::Colon) {
                return Err(Self::unexpected_token(
                    Expected::UnionConstructor,
                    Found::RecordStyleField,
                    ident.span,
                ));
            }
            members.push(self.parse_constructor_tail(&ident)?);
        }
        Ok(members)
    }
}
