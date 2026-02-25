use crate::ast::{DeclKind, Declaration, FieldDecl, TypeDecl, VariantDecl};
use crate::names::{FieldName, StructTypeName, VariantName};
use crate::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case, is_pascal_case};
use crate::names::Spanned;

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

        let (_, lbrace_span) = self.expect(Token::LBrace)?;

        // Disambiguate: empty type, struct sugar, or multi-variant
        let variants = if self.lexer.peek() == Some(&Token::RBrace) {
            // Empty type: `type ECI {}`
            Vec::new()
        } else if self.is_struct_sugar() {
            // Struct sugar: `type Foo { field: Type, ... }`
            // Desugar into a single variant with the same name as the type
            let fields = self.parse_field_list()?;
            let variant_span =
                lbrace_span.merge(fields.last().map_or(lbrace_span, |f| f.type_ann.span));
            vec![VariantDecl {
                name: Spanned::new(VariantName::new(name.value.as_str()), name.span),
                fields,
                span: variant_span,
            }]
        } else {
            // Multi-variant: `type Status { Nominal  Warning { message: Str } }`
            self.parse_variant_list()?
        };

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Type(TypeDecl {
                name,
                generic_params,
                derives: vec![],
                variants,
            }),
            span,
        })
    }

    /// Check if the type body is struct sugar (field list) vs variant list.
    /// Struct sugar starts with `lower_snake_case :` (field name followed by colon).
    /// Variant list starts with `PascalCase` (variant name).
    fn is_struct_sugar(&mut self) -> bool {
        if let Some((&Token::Ident, span)) = self.lexer.peek_with_span() {
            let name = self.lexer.slice_at(span);
            is_lower_snake_case(name)
        } else {
            false
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

    /// Parse a list of variant declarations (no separators between variants).
    /// `Nominal  Warning { message: Str }  Critical { message: Str, code: Int }`
    fn parse_variant_list(&mut self) -> Result<Vec<VariantDecl>, ParseError> {
        let mut variants = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            variants.push(self.parse_variant_decl()?);
        }
        if variants.is_empty() {
            let (tok, span) = self.advance()?;
            return Err(self.unexpected_token("a variant declaration", &tok.to_string(), span));
        }
        Ok(variants)
    }

    /// Parse a single variant: `Impulsive { delta_v: Velocity }` or bare `Nominal`
    fn parse_variant_decl(&mut self) -> Result<VariantDecl, ParseError> {
        let variant_ident = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
        let variant_name = Spanned::new(VariantName::new(&variant_ident.name), variant_ident.span);
        let start_span = variant_ident.span;

        if self.lexer.peek() == Some(&Token::LBrace) {
            // Variant with fields: `Impulsive { delta_v: Velocity }`
            self.lexer.next_token();
            let fields = if self.lexer.peek() == Some(&Token::RBrace) {
                Vec::new()
            } else {
                self.parse_field_list()?
            };
            let (_, end_span) = self.expect(Token::RBrace)?;
            Ok(VariantDecl {
                name: variant_name,
                fields,
                span: start_span.merge(end_span),
            })
        } else {
            // Bare variant: `Nominal`
            Ok(VariantDecl {
                name: variant_name,
                fields: Vec::new(),
                span: start_span,
            })
        }
    }
}
