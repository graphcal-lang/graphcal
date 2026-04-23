use crate::syntax::ast::{
    DeclKind, Declaration, FieldDecl, TypeDecl, UnionMember, UnionTypeDecl, Visibility,
};
use crate::syntax::names::{FieldName, StructTypeName};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};
use crate::syntax::names::Spanned;

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

        // Disambiguate: record type, required type, or union type.
        // After the visibility-bindability axioms rework, `type T;` is a
        // REQUIRED type; the empty record / unit-like marker is `type T {}`.
        match self.lexer.peek() {
            Some(&Token::LBrace) => {
                // Record type: `type Foo { field: Type, ... }` or empty `type Foo {}`
                self.lexer.next_token();
                let fields = if self.lexer.peek() == Some(&Token::RBrace) {
                    Vec::new()
                } else {
                    self.parse_field_list()?
                };
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
                    multi_decl_surface_span: None,
                })
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
                    multi_decl_surface_span: None,
                })
            }
            Some(&Token::Eq) => {
                // Union type: `type Foo = A | B | C;`
                self.lexer.next_token();
                let members = self.parse_union_members()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
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
                    multi_decl_surface_span: None,
                })
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("'{', ';', or '='", &tok.to_string(), span))
            }
        }
    }

    /// Parse a comma-separated field list: `field: Type, field: Type, ...`
    pub(super) fn parse_field_list(&mut self) -> Result<Vec<FieldDecl>, ParseError> {
        let fields = self.parse_comma_separated(Token::RBrace, |p| {
            let field_name = p.parse_any_ident()?.into_spanned::<FieldName>();
            p.expect(Token::Colon)?;
            let type_ann = p.parse_type_expr()?;
            Ok(FieldDecl {
                name: field_name,
                type_ann,
            })
        })?;
        if fields.is_empty() {
            let (tok, span) = self.advance()?;
            return Err(self.unexpected_token("at least one field", &tok.to_string(), span));
        }
        Ok(fields)
    }

    /// Parse union members: `A | B | C` or `A<D> | B`
    fn parse_union_members(&mut self) -> Result<Vec<UnionMember>, ParseError> {
        let mut members = Vec::new();
        members.push(self.parse_union_member()?);
        while self.lexer.peek() == Some(&Token::Pipe) {
            self.lexer.next_token();
            members.push(self.parse_union_member()?);
        }
        if members.len() < 2 {
            let span = members[0].span;
            return Err(self.unexpected_token("'|' followed by another union member", ";", span));
        }
        Ok(members)
    }

    /// Parse a single union member: `Foo` or `Foo<D, E>`
    fn parse_union_member(&mut self) -> Result<UnionMember, ParseError> {
        let ident = self.parse_any_ident()?;
        let name = Spanned::new(StructTypeName::new(&ident.name), ident.span);
        let start_span = ident.span;

        let (type_args, end_span) = if self.lexer.peek() == Some(&Token::Lt) {
            self.lexer.next_token();
            let mut args = Vec::new();
            args.push(self.parse_type_expr()?);
            while self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
                // Allow trailing comma
                if self.lexer.peek() == Some(&Token::Gt) {
                    break;
                }
                args.push(self.parse_type_expr()?);
            }
            let (_, gt_span) = self.expect(Token::Gt)?;
            (args, gt_span)
        } else {
            (Vec::new(), start_span)
        };

        Ok(UnionMember {
            name,
            type_args,
            span: start_span.merge(end_span),
        })
    }
}
