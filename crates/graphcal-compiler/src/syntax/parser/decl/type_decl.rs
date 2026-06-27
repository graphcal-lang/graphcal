use crate::syntax::ast::{
    BindableVisibility, DeclKind, Declaration, FieldDecl, TypeDecl, TypeDeclBody, UnionMember,
};
use crate::syntax::names::{ConstructorName, FieldName, StructTypeName};
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    // --- type declaration ---

    pub(super) fn parse_type_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Type)?;
        let name = self.parse_any_ident()?.into_spanned::<StructTypeName>();

        // Optional generic params: <D: Dim, F: Type>
        let generic_params = if self.lexer.peek() == Some(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        match self.lexer.peek() {
            Some(&Token::LBrace) => {
                // Unified body: either record (fields) or union (constructors).
                self.lexer.next_token(); // consume `{`
                self.parse_unified_type_body(name, generic_params, start_span)
            }
            Some(&Token::Semicolon) => {
                // Required type: `type Foo;` — bound from outside via include.
                let (_, end_span) = self.expect(Token::Semicolon)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    attributes: vec![],
                    kind: DeclKind::Type(TypeDecl {
                        visibility: BindableVisibility::Private,
                        name,
                        generic_params,
                        body: TypeDeclBody::Required,
                    }),
                    span,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("'{' or ';'", &tok.to_string(), span))
            }
        }
    }

    /// Parse the body inside `type T { ... }` after the opening brace has
    /// been consumed. Distinguishes record-form (`ident : Type`) from
    /// union-form (`ident ( ... )`, `ident { ... }`, `ident` followed by
    /// `,` or `}`) by one-token structural lookahead — never by identifier
    /// casing. All entries must agree on form; mixing produces a precise
    /// syntax error.
    fn parse_unified_type_body(
        &mut self,
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
            return Err(self.unexpected_token(
                "at least one constructor (`type T { T }` for a unit marker, `type T;` for a required type)",
                "empty body",
                start_span.merge(end_span),
            ));
        }

        let first_ident = self.parse_any_ident()?;
        match self.lexer.peek() {
            Some(&Token::Colon) => {
                // Record-shaped entry. Reject with a precise diagnostic
                // pointing at the explicit single-variant form.
                Err(self.unexpected_token(
                    "a constructor — write `type T { T(x: U, ...) }` instead of a field list",
                    "record-style field",
                    first_ident.span,
                ))
            }
            Some(&Token::LParen | &Token::LBrace | &Token::Comma | &Token::RBrace) => {
                let first_ctor = self.parse_constructor_tail(&first_ident)?;
                let members = self.continue_constructor_list(first_ctor)?;
                let (_, end_span) = self.expect(Token::RBrace)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    attributes: vec![],
                    kind: DeclKind::Type(TypeDecl {
                        visibility: BindableVisibility::Private,
                        name,
                        generic_params,
                        body: TypeDeclBody::Constructors(members),
                    }),
                    span,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "'(' or '{' (constructor payload), or ',' / '}' (unit constructor)",
                    &tok.to_string(),
                    span,
                ))
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
        let name = Spanned::new(ConstructorName::from_atom(ident.name.clone()), ident.span);

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
            Some(&Token::LBrace) => {
                self.lexer.next_token();
                let (fields, end_span) = if self.lexer.peek() == Some(&Token::RBrace) {
                    let (_, end_span) = self.expect(Token::RBrace)?;
                    (Vec::new(), end_span)
                } else {
                    let fields = self.parse_field_list_until(Token::RBrace)?;
                    let (_, end_span) = self.expect(Token::RBrace)?;
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
                name: Spanned::new(FieldName::from_atom(ident.name), ident.span),
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
                    return Err(self.unexpected_token("',' or terminator", &tok.to_string(), span));
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
                return Err(self.unexpected_token(
                    "constructor (this body started as a tagged union, every entry must be a constructor)",
                    "record-style field",
                    ident.span,
                ));
            }
            members.push(self.parse_constructor_tail(&ident)?);
        }
        Ok(members)
    }
}
