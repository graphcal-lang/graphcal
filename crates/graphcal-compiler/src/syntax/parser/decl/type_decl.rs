use crate::syntax::ast::{
    DeclKind, Declaration, FieldDecl, TypeDecl, UnionMember, UnionTypeDecl, Visibility,
};
use crate::syntax::names::{ConstructorName, FieldName, Spanned, StructTypeName};
use crate::syntax::span::Span;
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
                    visibility: Visibility::Private,
                    kind: DeclKind::Type(TypeDecl {
                        name,
                        generic_params,
                        fields: None,
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
        // Empty body: `type T {}` — treat as an empty record (existing
        // behavior, unchanged).
        if self.lexer.peek() == Some(&Token::RBrace) {
            let (_, end_span) = self.expect(Token::RBrace)?;
            let span = start_span.merge(end_span);
            return Ok(Declaration {
                attributes: vec![],
                visibility: Visibility::Private,
                kind: DeclKind::Type(TypeDecl {
                    name,
                    generic_params,
                    fields: Some(vec![]),
                }),
                span,
            });
        }

        // Consume the first identifier and peek at what follows to decide
        // record-form vs union-form.
        let first_ident = self.parse_any_ident()?;
        match self.lexer.peek() {
            Some(&Token::Colon) => {
                // Record form: `ident : Type, ...`
                self.lexer.next_token(); // consume `:`
                let first_type = self.parse_type_expr()?;
                let first_field = FieldDecl {
                    name: Spanned::new(FieldName::new(&first_ident.name), first_ident.span),
                    type_ann: first_type,
                };
                let fields = self.continue_field_list(first_field)?;
                let (_, end_span) = self.expect(Token::RBrace)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    attributes: vec![],
                    visibility: Visibility::Private,
                    kind: DeclKind::Type(TypeDecl {
                        name,
                        generic_params,
                        fields: Some(fields),
                    }),
                    span,
                })
            }
            Some(&Token::LParen | &Token::LBrace | &Token::Comma | &Token::RBrace) => {
                // Union form. Build the first constructor from `first_ident`,
                // then continue parsing the constructor list.
                let first_ctor = self.parse_constructor_tail(&first_ident)?;
                let members = self.continue_constructor_list(first_ctor)?;
                let (_, end_span) = self.expect(Token::RBrace)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    attributes: vec![],
                    visibility: Visibility::Private,
                    kind: DeclKind::UnionType(UnionTypeDecl {
                        name,
                        generic_params,
                        members,
                    }),
                    span,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "':' (record field), '(' or '{' (constructor payload), or ',' / '}' (unit constructor)",
                    &tok.to_string(),
                    span,
                ))
            }
        }
    }

    /// Parse the rest of a `FieldDecl` list after the first field has been
    /// parsed. Trailing comma allowed.
    fn continue_field_list(&mut self, first: FieldDecl) -> Result<Vec<FieldDecl>, ParseError> {
        let mut fields = vec![first];
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token();
            // Trailing comma — body ends here.
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            // The remaining entries must be record-form fields. If we see
            // a constructor-shape entry here, reject with a precise error.
            let ident = self.parse_any_ident()?;
            match self.lexer.peek() {
                Some(&Token::Colon) => {
                    self.lexer.next_token();
                    let type_ann = self.parse_type_expr()?;
                    fields.push(FieldDecl {
                        name: Spanned::new(FieldName::new(&ident.name), ident.span),
                        type_ann,
                    });
                }
                Some(&Token::LParen | &Token::LBrace | &Token::Comma | &Token::RBrace) => {
                    return Err(self.unexpected_token(
                        "':' (this body started as a record, every entry must be a field)",
                        "constructor-shaped entry",
                        ident.span,
                    ));
                }
                _ => {
                    let (tok, span) = self.advance()?;
                    return Err(self.unexpected_token("':'", &tok.to_string(), span));
                }
            }
        }
        Ok(fields)
    }

    /// Parse the payload (if any) following a constructor's identifier,
    /// producing a `UnionMember`. The identifier has already been consumed.
    fn parse_constructor_tail(
        &mut self,
        ident: &crate::syntax::ast::Ident,
    ) -> Result<UnionMember, ParseError> {
        let start_span = ident.span;
        let name = Spanned::new(ConstructorName::new(&ident.name), ident.span);

        let (payload, end_span) = match self.lexer.peek() {
            Some(&Token::LParen) => {
                self.lexer.next_token();
                // Empty payload `Ctor()` is allowed.
                let (fields, end_span) = if self.lexer.peek() == Some(&Token::RParen) {
                    let (_, end_span) = self.expect(Token::RParen)?;
                    (Vec::new(), end_span)
                } else {
                    let fields = self.parse_field_list_until(&Token::RParen)?;
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
                    let fields = self.parse_field_list_until(&Token::RBrace)?;
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
    fn parse_field_list_until(&mut self, terminator: &Token) -> Result<Vec<FieldDecl>, ParseError> {
        let mut fields = Vec::new();
        loop {
            let ident = self.parse_any_ident()?;
            self.expect(Token::Colon)?;
            let type_ann = self.parse_type_expr()?;
            fields.push(FieldDecl {
                name: Spanned::new(FieldName::new(&ident.name), ident.span),
                type_ann,
            });
            match self.lexer.peek() {
                Some(t) if t == terminator => break,
                Some(&Token::Comma) => {
                    self.lexer.next_token();
                    if let Some(t) = self.lexer.peek()
                        && t == terminator
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
