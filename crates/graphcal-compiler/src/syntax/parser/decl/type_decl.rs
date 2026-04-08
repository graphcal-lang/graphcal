use crate::syntax::ast::{DeclKind, Declaration, FieldDecl, TypeDecl, UnionMember, UnionTypeDecl};
use crate::syntax::names::{FieldName, StructTypeName};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case, is_pascal_case};
use crate::syntax::names::Spanned;

impl Parser<'_> {
    // --- type declaration ---

    pub(super) fn parse_type_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Type)?;
        let name = self
            .parse_ident_with_casing("PascalCase", is_pascal_case)?
            .into_spanned::<StructTypeName>();

        // Optional generic params: <D: Dim, F: Type>
        let generic_params = if self.lexer.peek() == Some(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        // Disambiguate: record type, unit type, or union type
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
                    is_pub: false,
                    kind: DeclKind::Type(TypeDecl {
                        name,
                        generic_params,
                        fields,
                    }),
                    span,
                })
            }
            Some(&Token::Semicolon) => {
                // Unit type: `type Foo;`
                let (_, end_span) = self.expect(Token::Semicolon)?;
                let span = start_span.merge(end_span);
                Ok(Declaration {
                    attributes: vec![],
                    is_pub: false,
                    kind: DeclKind::Type(TypeDecl {
                        name,
                        generic_params,
                        fields: Vec::new(),
                    }),
                    span,
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
                    is_pub: false,
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
                Err(self.unexpected_token("'{', ';', or '='", &tok.to_string(), span))
            }
        }
    }

    /// Parse a comma-separated field list: `field: Type, field: Type, ...`
    pub(super) fn parse_field_list(&mut self) -> Result<Vec<FieldDecl>, ParseError> {
        let mut fields = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let field_name = self
                .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
                .into_spanned::<FieldName>();
            self.expect(Token::Colon)?;
            let type_ann = self.parse_type_expr()?;
            fields.push(FieldDecl {
                name: field_name,
                type_ann,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
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
        let ident = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
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
