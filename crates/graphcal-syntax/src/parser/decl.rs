use crate::ast::{
    AssertBody, AssertDecl, Attribute, ConstDecl, DeclKind, Declaration, DimDecl, FieldDecl,
    IndexDecl, IndexDeclKind, NodeDecl, ParamDecl, TypeDecl, VariantDecl,
};
use crate::names::{
    DeclName, DimName, FieldName, IndexName, Spanned, StructTypeName, UnitName, VariantName,
};
use crate::token::Token;

use super::{ParseError, Parser, is_lower_snake_case, is_pascal_case, is_upper_snake_case};

impl Parser<'_> {
    pub(super) fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        // Collect any leading attributes: #[name] or #[name(arg1, arg2)]
        let mut attributes = Vec::new();
        while self.lexer.peek() == Some(&Token::Hash) {
            attributes.push(self.parse_attribute()?);
        }

        let expected = "`param`, `node`, `const`, `dimension`, `unit`, `type`, `fn`, `index`, `import`, or `assert`";
        let mut decl = match self.lexer.peek() {
            Some(Token::Param) => self.parse_param(),
            Some(Token::Node) => self.parse_node(),
            Some(Token::Const) => self.parse_const(),
            Some(Token::Dimension) => self.parse_dimension_decl(),
            Some(Token::Unit) => self.parse_unit_decl(),
            Some(Token::Type) => self.parse_type_decl(),
            Some(Token::Fn) => self.parse_fn_decl(),
            Some(Token::Index) => self.parse_index_decl(),
            Some(Token::Import) => self.parse_import_decl(),
            Some(Token::Assert) => self.parse_assert(),
            Some(_) => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(expected, &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof(expected)),
        }?;

        // Extend the declaration span to include the attributes
        if let Some(first_attr) = attributes.first() {
            decl.span = first_attr.span.merge(decl.span);
        }
        decl.attributes = attributes;
        Ok(decl)
    }

    /// Parse a single attribute: `#[name]` or `#[name(arg1, arg2)]`
    fn parse_attribute(&mut self) -> Result<Attribute, ParseError> {
        let (_, start_span) = self.expect(Token::Hash)?;
        self.expect(Token::LBracket)?;
        let name = self.parse_any_ident()?;
        let mut args = Vec::new();
        if self.lexer.peek() == Some(&Token::LParen) {
            self.expect(Token::LParen)?;
            // Parse comma-separated identifiers
            if self.lexer.peek() != Some(&Token::RParen) {
                args.push(self.parse_any_ident()?);
                while self.lexer.peek() == Some(&Token::Comma) {
                    self.expect(Token::Comma)?;
                    // Allow trailing comma
                    if self.lexer.peek() == Some(&Token::RParen) {
                        break;
                    }
                    args.push(self.parse_any_ident()?);
                }
            }
            self.expect(Token::RParen)?;
        }
        let (_, end_span) = self.expect(Token::RBracket)?;
        let span = start_span.merge(end_span);
        Ok(Attribute { name, args, span })
    }

    // --- param/node/const with required type annotation ---

    fn parse_param(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Param)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Param(ParamDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    fn parse_node(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Node)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Node(NodeDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    fn parse_const(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Const)?;
        let name = self
            .parse_ident_with_casing("UPPER_SNAKE_CASE", is_upper_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Const(ConstDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    // --- assert declaration ---

    fn parse_assert(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Assert)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Eq)?;
        let first_expr = self.parse_expr()?;

        let body = if self.lexer.peek() == Some(&Token::TildeEq) {
            // Tolerance syntax: actual ~= expected +/- tolerance [%]
            self.lexer.next_token(); // consume ~=
            let expected = self.parse_expr()?;
            self.expect(Token::PlusMinus)?;
            // Parse tolerance as a unary expr (not full expr) so `%` isn't consumed as modulo
            let tolerance = self.parse_unary()?;
            let is_relative = if self.lexer.peek() == Some(&Token::Percent) {
                self.lexer.next_token(); // consume %
                true
            } else {
                false
            };
            AssertBody::Tolerance {
                actual: Box::new(first_expr),
                expected: Box::new(expected),
                tolerance: Box::new(tolerance),
                is_relative,
            }
        } else {
            AssertBody::Expr(first_expr)
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Assert(AssertDecl { name, body }),
            span,
        })
    }

    // --- dimension and unit declarations ---

    fn parse_dimension_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dimension)?;
        let name = self.parse_any_ident()?.into_spanned::<DimName>();

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            Some(self.parse_dim_expr()?)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Dimension(DimDecl { name, definition }),
            span,
        })
    }

    fn parse_unit_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Unit)?;
        let name = self.parse_any_ident()?.into_spanned::<UnitName>();
        self.expect(Token::Colon)?;
        let dim_type = self.parse_dim_expr()?;

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            let def = self.parse_unit_def()?;
            Some(def)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Unit(crate::ast::UnitDecl {
                name,
                dim_type,
                definition,
            }),
            span,
        })
    }

    // --- type declaration ---

    fn parse_type_decl(&mut self) -> Result<Declaration, ParseError> {
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

        // Optional derive clause: derive(Add, Sub, Neg)
        let derives = if self.lexer.peek() == Some(&Token::Ident) {
            let peeked = self.lexer.peek_with_span();
            if let Some((&Token::Ident, span)) = peeked {
                if self.lexer.slice_at(span) == "derive" {
                    self.parse_derive_clause()?
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
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
                derives,
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

    // --- import declaration ---

    /// Parse an import declaration:
    /// `import "./path/to/file.gcl" { name1, name2 };`
    fn parse_import_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Import)?;

        // Expect a string literal for the file path
        let (path, path_span) = match self.lexer.next_token() {
            Some((Token::StringLiteral, span)) => {
                let raw = self.lexer.slice_at(span);
                // Strip surrounding quotes
                let path = raw[1..raw.len() - 1].to_string();
                (path, span)
            }
            Some((tok, span)) => {
                return Err(self.unexpected_token("a string literal", &tok.to_string(), span));
            }
            None => {
                return Err(self.unexpected_eof("a string literal"));
            }
        };

        // Determine the kind of import based on the next token:
        //   `{`  → selective import (existing)
        //   `as` → module import with alias
        //   `;`  → module import with name derived from filename
        let (kind, end_span) = match self.lexer.peek() {
            Some(Token::LBrace) => {
                let names = self.parse_import_selective_body()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                (crate::ast::ImportKind::Selective(names), end_span)
            }
            Some(Token::As) => {
                self.lexer.next_token(); // consume `as`
                let alias = self.parse_any_ident()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                (
                    crate::ast::ImportKind::Module { alias: Some(alias) },
                    end_span,
                )
            }
            Some(Token::Semicolon) => {
                let (_, end_span) = self.expect(Token::Semicolon)?;
                (crate::ast::ImportKind::Module { alias: None }, end_span)
            }
            Some(tok) => {
                let tok_str = tok.to_string();
                let (_, span) = self.advance()?;
                return Err(self.unexpected_token("`{`, `as`, or `;` after path", &tok_str, span));
            }
            None => {
                return Err(self.unexpected_eof("`{`, `as`, or `;` after path"));
            }
        };

        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Import(crate::ast::ImportDecl {
                path,
                path_span,
                kind,
            }),
            span,
        })
    }

    /// Parse the `{ name1, name2 as alias, ... }` body of a selective import.
    fn parse_import_selective_body(&mut self) -> Result<Vec<crate::ast::ImportItem>, ParseError> {
        self.expect(Token::LBrace)?;

        let mut names = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            // Accept any identifier (imports can be any casing)
            let (name_str, name_span) = match self.lexer.next_token() {
                Some((Token::Ident, span)) => (self.lexer.slice_at(span).to_string(), span),
                Some((tok, span)) => {
                    return Err(self.unexpected_token("an identifier", &tok.to_string(), span));
                }
                None => {
                    return Err(self.unexpected_eof("an identifier or `}`"));
                }
            };

            // Check for optional `as` alias
            let alias = if self.lexer.peek() == Some(&Token::As) {
                self.lexer.next_token(); // consume `as`
                match self.lexer.next_token() {
                    Some((Token::Ident, alias_span)) => {
                        let alias_str = self.lexer.slice_at(alias_span).to_string();
                        Some(crate::ast::Ident {
                            name: alias_str,
                            span: alias_span,
                        })
                    }
                    Some((tok, span)) => {
                        return Err(self.unexpected_token(
                            "an identifier after `as`",
                            &tok.to_string(),
                            span,
                        ));
                    }
                    None => {
                        return Err(self.unexpected_eof("an identifier after `as`"));
                    }
                }
            } else {
                None
            };

            names.push(crate::ast::ImportItem {
                name: crate::ast::Ident {
                    name: name_str,
                    span: name_span,
                },
                alias,
            });

            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        self.expect(Token::RBrace)?;
        Ok(names)
    }

    // --- index declaration ---

    /// Parse an index declaration:
    /// `index Maneuver = { Departure, Correction, Insertion }`
    fn parse_index_decl(&mut self) -> Result<Declaration, ParseError> {
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
