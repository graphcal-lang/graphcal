use crate::ast::{
    AssertBody, AssertDecl, Attribute, AttributeArg, ConstDecl, DeclKind, Declaration, DeriveOp,
    DimDecl, FieldDecl, IndexDecl, IndexDeclKind, NodeDecl, ParamDecl, TypeDecl, VariantDecl,
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

        // Extract #[derive(...)] attribute for type declarations
        if let DeclKind::Type(ref mut type_decl) = decl.kind {
            let mut remaining_attrs = Vec::new();
            for attr in attributes {
                if attr.name.name == "derive" {
                    for arg in &attr.args {
                        let ident = arg.as_single_ident().ok_or_else(|| {
                            self.unexpected_token(
                                "`Add`, `Sub`, or `Neg`",
                                "<complex argument>",
                                arg.span(),
                            )
                        })?;
                        let op = match ident.name.as_str() {
                            "Add" => DeriveOp::Add,
                            "Sub" => DeriveOp::Sub,
                            "Neg" => DeriveOp::Neg,
                            _ => {
                                return Err(self.unexpected_token(
                                    "`Add`, `Sub`, or `Neg`",
                                    &ident.name,
                                    ident.span,
                                ));
                            }
                        };
                        type_decl.derives.push(Spanned::new(op, ident.span));
                    }
                } else {
                    remaining_attrs.push(attr);
                }
            }
            decl.attributes = remaining_attrs;
        } else {
            // For non-type declarations, check that no #[derive] attribute was used
            let derive_attr = attributes.iter().find(|a| a.name.name == "derive");
            if let Some(attr) = derive_attr {
                return Err(self.unexpected_token(
                    "a valid attribute for this declaration",
                    "derive",
                    attr.span,
                ));
            }
            decl.attributes = attributes;
        }

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
            if self.lexer.peek() != Some(&Token::RParen) {
                args.push(self.parse_attribute_arg()?);
                while self.lexer.peek() == Some(&Token::Comma) {
                    self.expect(Token::Comma)?;
                    if self.lexer.peek() == Some(&Token::RParen) {
                        break;
                    }
                    args.push(self.parse_attribute_arg()?);
                }
            }
            self.expect(Token::RParen)?;
        }
        let (_, end_span) = self.expect(Token::RBracket)?;
        let span = start_span.merge(end_span);
        Ok(Attribute { name, args, span })
    }

    /// Parse a single attribute argument: a path (`ident`, `Idx::Var`) or
    /// a parenthesized group (`(Idx::A, Idx::B)`).
    fn parse_attribute_arg(&mut self) -> Result<AttributeArg, ParseError> {
        if self.lexer.peek() == Some(&Token::LParen) {
            // Group: (arg, arg, ...)
            let (_, start_span) = self.expect(Token::LParen)?;
            let mut elements = Vec::new();
            if self.lexer.peek() != Some(&Token::RParen) {
                elements.push(self.parse_attribute_arg()?);
                while self.lexer.peek() == Some(&Token::Comma) {
                    self.expect(Token::Comma)?;
                    if self.lexer.peek() == Some(&Token::RParen) {
                        break;
                    }
                    elements.push(self.parse_attribute_arg()?);
                }
            }
            let (_, end_span) = self.expect(Token::RParen)?;
            Ok(AttributeArg::Group {
                elements,
                span: start_span.merge(end_span),
            })
        } else {
            // Path: ident or ident::ident::...
            let first = self.parse_any_ident()?;
            let start_span = first.span;
            let mut segments = vec![first];
            while self.lexer.peek() == Some(&Token::ColonColon) {
                self.expect(Token::ColonColon)?;
                segments.push(self.parse_any_ident()?);
            }
            let end_span = segments.last().map_or(start_span, |s| s.span);
            Ok(AttributeArg::Path {
                segments,
                span: start_span.merge(end_span),
            })
        }
    }

    // --- param/node/const with required type annotation ---

    fn parse_param(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Param)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        let value = if self.lexer.peek() == Some(&Token::Eq) {
            self.expect(Token::Eq)?;
            Some(self.parse_expr()?)
        } else {
            None
        };
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

    // --- import declaration ---

    /// Parse an import declaration:
    /// `import "./path/to/file.gcl" { name1, name2 };`
    fn parse_import_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Import)?;

        // Parse the import path: string literal or bare identifier path.
        let path = match self.lexer.peek() {
            Some(Token::StringLiteral) => {
                let (_, span) = self.advance()?;
                let raw = self.lexer.slice_at(span);
                let path_str = raw[1..raw.len() - 1].to_string();
                crate::ast::ImportPath::FilePath {
                    path: path_str,
                    span,
                }
            }
            Some(Token::Ident) => {
                // Bare module path: ident / ident / ...
                let first = self.parse_any_ident()?;
                let path_start = first.span;
                let mut segments = vec![first];

                // Parse subsequent `/ident` pairs
                while self.lexer.peek() == Some(&Token::Slash) {
                    self.lexer.next_token(); // consume `/`
                    let seg = self.parse_any_ident()?;
                    segments.push(seg);
                }

                // Require at least two segments (e.g., `nasa/rocket`, not just `nasa`)
                if segments.len() < 2 {
                    let found = self
                        .lexer
                        .peek()
                        .map_or_else(|| "end of file".to_string(), ToString::to_string);
                    let err_span = segments.last().map_or(path_start, |s| s.span);
                    return Err(self.unexpected_token(
                        "a `/` followed by a module name (bare import paths require at least two segments, e.g., `package/module`)",
                        &found,
                        err_span,
                    ));
                }

                let path_end = segments.last().map_or(path_start, |s| s.span);
                crate::ast::ImportPath::ModulePath {
                    segments,
                    span: path_start.merge(path_end),
                }
            }
            Some(tok) => {
                let tok_str = tok.to_string();
                let (_, span) = self.advance()?;
                return Err(self.unexpected_token(
                    "a string literal or module path",
                    &tok_str,
                    span,
                ));
            }
            None => {
                return Err(self.unexpected_eof("a string literal or module path"));
            }
        };

        // Optional param bindings: `(name = expr, ...)`
        let param_bindings = if self.lexer.peek() == Some(&Token::LParen) {
            self.parse_import_param_bindings()?
        } else {
            Vec::new()
        };

        // Determine the kind of import based on the next token:
        //   `{`  → selective import (existing)
        //   `as` → module import with alias
        //   `;`  → module import with name derived from filename
        let after_bindings_hint = if param_bindings.is_empty() {
            "`(`, `{`, `as`, or `;` after path"
        } else {
            "`{`, `as`, or `;` after param bindings"
        };
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
                return Err(self.unexpected_token(after_bindings_hint, &tok_str, span));
            }
            None => {
                return Err(self.unexpected_eof(after_bindings_hint));
            }
        };

        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Import(crate::ast::ImportDecl {
                path,
                param_bindings,
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

    /// Parse the `(name = expr, ...)` param bindings of an instantiated import.
    fn parse_import_param_bindings(&mut self) -> Result<Vec<crate::ast::ParamBinding>, ParseError> {
        self.expect(Token::LParen)?;

        let mut bindings = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            let name_ident = self.parse_any_ident()?;
            let name_span = name_ident.span;
            self.expect(Token::Eq)?;
            let value = self.parse_expr()?;
            let binding_span = name_span.merge(value.span);
            bindings.push(crate::ast::ParamBinding {
                name: name_ident,
                value,
                span: binding_span,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        if bindings.is_empty() {
            let (tok, span) = self.advance()?;
            return Err(self.unexpected_token(
                "at least one param binding",
                &tok.to_string(),
                span,
            ));
        }

        self.expect(Token::RParen)?;
        Ok(bindings)
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;
    use crate::ast::{
        DeclKind, ExprKind, GenericConstraint, IndexDeclKind, MulDivOp, TypeExprKind,
    };

    fn dim_expr_name(te: &crate::ast::TypeExpr) -> &str {
        match &te.kind {
            TypeExprKind::DimExpr(dim) => {
                assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
                dim.terms[0].term.name.name.as_str()
            }
            other => panic!("expected DimExpr, got {other:?}"),
        }
    }

    #[test]
    fn parse_param_with_type() {
        let file = Parser::new("param x: Dimensionless = 42.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "x");
                assert!(matches!(p.type_ann.kind, TypeExprKind::Dimensionless));
                assert!(
                    matches!(p.value.as_ref().unwrap().kind, ExprKind::Number(n) if (n - 42.0).abs() < f64::EPSILON)
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_param_with_dim_type() {
        let file = Parser::new("param alt: Length = 400.0 km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "alt");
                match &p.type_ann.kind {
                    TypeExprKind::DimExpr(d) => {
                        assert_eq!(d.terms.len(), 1);
                        assert_eq!(d.terms[0].term.name.name, "Length");
                    }
                    other => panic!("expected DimExpr, got {other:?}"),
                }
                assert!(matches!(
                    p.value.as_ref().unwrap().kind,
                    ExprKind::UnitLiteral { .. }
                ));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_param_required() {
        let file = Parser::new("param dry_mass: Mass;").parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "dry_mass");
                match &p.type_ann.kind {
                    TypeExprKind::DimExpr(d) => {
                        assert_eq!(d.terms.len(), 1);
                        assert_eq!(d.terms[0].term.name.name, "Mass");
                    }
                    other => panic!("expected DimExpr, got {other:?}"),
                }
                assert!(p.value.is_none());
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_node_with_compound_dim_type() {
        let file = Parser::new("node gm: Length^3 / Time^2 = 3.98e14 m^3/s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => {
                assert_eq!(n.name.value.as_str(), "gm");
                match &n.type_ann.kind {
                    TypeExprKind::DimExpr(d) => {
                        assert_eq!(d.terms.len(), 2);
                        assert_eq!(d.terms[0].term.name.name, "Length");
                        assert_eq!(d.terms[0].term.power, Some(3));
                        assert_eq!(d.terms[1].op, MulDivOp::Div);
                        assert_eq!(d.terms[1].term.name.name, "Time");
                        assert_eq!(d.terms[1].term.power, Some(2));
                    }
                    other => panic!("expected DimExpr, got {other:?}"),
                }
            }
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_const_with_type() {
        let file = Parser::new("const G0: Dimensionless = 9.80665;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Const(c) => {
                assert_eq!(c.name.value.as_str(), "G0");
                assert!(matches!(c.type_ann.kind, TypeExprKind::Dimensionless));
            }
            _ => panic!("expected const"),
        }
    }

    #[test]
    fn parse_base_dimension() {
        let file = Parser::new("dimension Length;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Dimension(d) => {
                assert_eq!(d.name.value.as_str(), "Length");
                assert!(d.definition.is_none());
            }
            _ => panic!("expected dimension"),
        }
    }

    #[test]
    fn parse_derived_dimension() {
        let file = Parser::new("dimension Velocity = Length / Time;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Dimension(d) => {
                assert_eq!(d.name.value.as_str(), "Velocity");
                let def = d.definition.as_ref().unwrap();
                assert_eq!(def.terms.len(), 2);
                assert_eq!(def.terms[0].term.name.name, "Length");
                assert_eq!(def.terms[1].op, MulDivOp::Div);
                assert_eq!(def.terms[1].term.name.name, "Time");
            }
            _ => panic!("expected dimension"),
        }
    }

    #[test]
    fn parse_base_unit() {
        let file = Parser::new("unit m: Length;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.value.as_str(), "m");
                assert_eq!(u.dim_type.terms[0].term.name.name, "Length");
                assert!(u.definition.is_none());
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_derived_unit() {
        let file = Parser::new("unit km: Length = 1000.0 m;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.value.as_str(), "km");
                let def = u.definition.as_ref().unwrap();
                assert!((def.scale - 1000.0).abs() < f64::EPSILON);
                assert_eq!(def.unit_expr.terms.len(), 1);
                assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "m");
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_compound_unit_decl() {
        let file = Parser::new("unit N: Force = 1.0 kg * m / s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.value.as_str(), "N");
                let def = u.definition.as_ref().unwrap();
                assert!((def.scale - 1.0).abs() < f64::EPSILON);
                assert_eq!(def.unit_expr.terms.len(), 3);
                assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "kg");
                assert_eq!(def.unit_expr.terms[1].op, MulDivOp::Mul);
                assert_eq!(def.unit_expr.terms[1].name.value.as_str(), "m");
                assert_eq!(def.unit_expr.terms[2].op, MulDivOp::Div);
                assert_eq!(def.unit_expr.terms[2].name.value.as_str(), "s");
                assert_eq!(def.unit_expr.terms[2].power, Some(2));
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_unit_decl_with_paren_expr() {
        let file = Parser::new("unit deg: Angle = (PI / 180) rad;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.value.as_str(), "deg");
                let def = u.definition.as_ref().unwrap();
                assert!(
                    (def.scale - std::f64::consts::PI / 180.0).abs() < 1e-10,
                    "scale = {}",
                    def.scale
                );
                assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "rad");
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_error_missing_semicolon() {
        let result = Parser::new("param x: Dimensionless = 1.0").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_unexpected_token() {
        let result = Parser::new("+ 1.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_with_comments() {
        let input = "// this is a comment\nparam x: Dimensionless = 1.0;\n// another comment";
        let file = Parser::new(input).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
    }

    #[test]
    fn parse_error_bad_param_casing() {
        let result = Parser::new("param BadName: Dimensionless = 1.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_bad_const_casing() {
        let result = Parser::new("const bad_name: Dimensionless = 42.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_orbital_milestone_syntax() {
        let source = r"
dimension Velocity = Length / Time;

param alt: Length = 400.0 km;
param period: Time = 90.0 min;
const R_EARTH: Length = 6371.0 km;

node circumference: Length = 2.0 * PI * (R_EARTH + @alt);
node speed: Velocity = @circumference / @period;
node speed_kmh: Velocity = @speed -> km/hour;
";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 7);

        let names: Vec<&str> = file
            .declarations
            .iter()
            .map(|d| match &d.kind {
                DeclKind::Param(p) => p.name.value.as_str(),
                DeclKind::Node(n) => n.name.value.as_str(),
                DeclKind::Const(c) => c.name.value.as_str(),
                DeclKind::Dimension(d) => d.name.value.as_str(),
                DeclKind::Unit(u) => u.name.value.as_str(),
                DeclKind::Type(t) => t.name.value.as_str(),
                DeclKind::Fn(f) => f.name.value.as_str(),
                DeclKind::Index(i) => i.name.value.as_str(),
                DeclKind::Import(_) => "<import>",
                DeclKind::Assert(a) => a.name.value.as_str(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "Velocity",
                "alt",
                "period",
                "R_EARTH",
                "circumference",
                "speed",
                "speed_kmh"
            ]
        );
    }

    #[test]
    fn parse_type_decl_single_field() {
        let source = "type Orbit { sma: Length }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Orbit");
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].name.value.as_str(), "Orbit");
                assert_eq!(t.variants[0].fields.len(), 1);
                assert_eq!(t.variants[0].fields[0].name.value.as_str(), "sma");
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_multiple_fields() {
        let source = "type TransferResult { dv1: Velocity, dv2: Velocity }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "TransferResult");
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].fields.len(), 2);
                assert_eq!(t.variants[0].fields[0].name.value.as_str(), "dv1");
                assert_eq!(t.variants[0].fields[1].name.value.as_str(), "dv2");
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_trailing_comma() {
        let source = "type TransferResult { dv1: Velocity, dv2: Velocity, }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].fields.len(), 2);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_empty_type() {
        let source = "type Eci {}";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Eci");
                assert_eq!(t.variants.len(), 0);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_uppercase_name_error() {
        let source = "type ORBIT { sma: Length }";
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_type_decl_lowercase_name_error() {
        let source = "type orbit { sma: Length }";
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_type_decl_with_dim_expr_field() {
        let source = "type TransferResult { dv: Length / Time }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].fields.len(), 1);
                assert_eq!(t.variants[0].fields[0].name.value.as_str(), "dv");
                match &t.variants[0].fields[0].type_ann.kind {
                    TypeExprKind::DimExpr(_) => {}
                    other => {
                        panic!("expected DimExpr, got {other:?}")
                    }
                }
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_mixed_with_other_decls() {
        let source = r"
dimension Velocity = Length / Time;
type TransferResult { dv1: Velocity, dv2: Velocity }
param alt: Length = 400.0 km;
";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 3);
        assert!(matches!(&file.declarations[0].kind, DeclKind::Dimension(_)));
        assert!(matches!(&file.declarations[1].kind, DeclKind::Type(_)));
        assert!(matches!(&file.declarations[2].kind, DeclKind::Param(_)));
    }

    #[test]
    fn parse_type_decl_generic_params() {
        let source = "type Vec3<D: Dim, F: Type> { x: D, y: D, z: D }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Vec3");
                assert_eq!(t.generic_params.len(), 2);
                assert_eq!(t.generic_params[0].name.value.as_str(), "D");
                assert_eq!(t.generic_params[0].constraint, GenericConstraint::Dim);
                assert_eq!(t.generic_params[1].name.value.as_str(), "F");
                assert_eq!(t.generic_params[1].constraint, GenericConstraint::Type);
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].fields.len(), 3);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_no_generics_empty() {
        let source = "type Eci {}";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Eci");
                assert!(t.generic_params.is_empty());
                assert_eq!(t.variants.len(), 0);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_generic_single_type_param() {
        let source = "type Timestamp<TZ: Type> { epoch_seconds: Time }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Timestamp");
                assert_eq!(t.generic_params.len(), 1);
                assert_eq!(t.generic_params[0].name.value.as_str(), "TZ");
                assert_eq!(t.generic_params[0].constraint, GenericConstraint::Type);
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].fields.len(), 1);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_generic_tagged_union() {
        let source = "type Result<D: Dim, E: Type> { Ok { value: D } Err }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Result");
                assert_eq!(t.generic_params.len(), 2);
                assert_eq!(t.variants.len(), 2);
                assert_eq!(t.variants[0].name.value.as_str(), "Ok");
                assert_eq!(t.variants[0].fields.len(), 1);
                assert_eq!(t.variants[1].name.value.as_str(), "Err");
                assert_eq!(t.variants[1].fields.len(), 0);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_generic_default_type_param() {
        let source = "type Vec3<D: Dim, F: Type = Unframed> { x: D, y: D, z: D }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Vec3");
                assert_eq!(t.generic_params.len(), 2);
                assert_eq!(t.generic_params[0].name.value.as_str(), "D");
                assert_eq!(t.generic_params[0].constraint, GenericConstraint::Dim);
                assert!(t.generic_params[0].default.is_none());
                assert_eq!(t.generic_params[1].name.value.as_str(), "F");
                assert_eq!(t.generic_params[1].constraint, GenericConstraint::Type);
                let default = t.generic_params[1].default.as_ref().unwrap();
                assert_eq!(dim_expr_name(default), "Unframed");
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_generic_no_default() {
        let source = "type Pair<A: Dim, B: Dim> { a: A, b: B }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.generic_params.len(), 2);
                assert!(t.generic_params[0].default.is_none());
                assert!(t.generic_params[1].default.is_none());
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_derive_attribute() {
        let source = "#[derive(Add, Sub, Neg)]\ntype Vec3<D: Dim, F: Type> { x: D, y: D, z: D }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        // #[derive(...)] should be extracted, not left in attributes
        assert!(file.declarations[0].attributes.is_empty());
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Vec3");
                assert_eq!(t.generic_params.len(), 2);
                assert_eq!(t.derives.len(), 3);
                assert_eq!(t.derives[0].value, crate::ast::DeriveOp::Add);
                assert_eq!(t.derives[1].value, crate::ast::DeriveOp::Sub);
                assert_eq!(t.derives[2].value, crate::ast::DeriveOp::Neg);
                assert_eq!(t.variants.len(), 1);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_derive_attribute_on_non_type_is_error() {
        let source = "#[derive(Add)]\nparam x: Dimensionless = 1.0;";
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_type_decl_no_derive() {
        let source = "type Eci {}";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert!(t.derives.is_empty());
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_index_decl() {
        let source = "index Maneuver = { Departure, Correction, Insertion }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Index(idx) => {
                assert_eq!(idx.name.value.as_str(), "Maneuver");
                match &idx.kind {
                    IndexDeclKind::Named { variants } => {
                        assert_eq!(variants.len(), 3);
                        assert_eq!(variants[0].value.as_str(), "Departure");
                        assert_eq!(variants[1].value.as_str(), "Correction");
                        assert_eq!(variants[2].value.as_str(), "Insertion");
                    }
                    IndexDeclKind::Range { .. } => panic!("expected named index"),
                }
            }
            _ => panic!("expected index declaration"),
        }
    }

    #[test]
    fn parse_index_decl_trailing_comma() {
        let source = "index Phase = { Boost, Coast, }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Index(idx) => {
                assert_eq!(idx.name.value.as_str(), "Phase");
                match &idx.kind {
                    IndexDeclKind::Named { variants } => {
                        assert_eq!(variants.len(), 2);
                    }
                    IndexDeclKind::Range { .. } => panic!("expected named index"),
                }
            }
            _ => panic!("expected index declaration"),
        }
    }

    #[test]
    fn parse_range_index_decl() {
        let source = "index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Index(idx) => {
                assert_eq!(idx.name.value.as_str(), "TimeStep");
                assert!(matches!(idx.kind, IndexDeclKind::Range { .. }));
            }
            _ => panic!("expected index declaration"),
        }
    }

    #[test]
    fn parse_import_no_alias() {
        let file = Parser::new(r#"import "./helper.gcl" { x, Y };"#)
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        assert_eq!(u.path.display_path(), "./helper.gcl");
        assert!(matches!(&u.path, crate::ast::ImportPath::FilePath { .. }));
        let crate::ast::ImportKind::Selective(names) = &u.kind else {
            panic!("expected Selective");
        };
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].name.name, "x");
        assert!(names[0].alias.is_none());
        assert_eq!(names[0].local_name(), "x");
        assert_eq!(names[1].name.name, "Y");
        assert!(names[1].alias.is_none());
        assert_eq!(names[1].local_name(), "Y");
    }

    #[test]
    fn parse_import_with_alias() {
        let file = Parser::new(r#"import "./helper.gcl" { x as y };"#)
            .parse_file()
            .unwrap();
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        let crate::ast::ImportKind::Selective(names) = &u.kind else {
            panic!("expected Selective");
        };
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].name.name, "x");
        assert_eq!(names[0].alias.as_ref().unwrap().name, "y");
        assert_eq!(names[0].local_name(), "y");
    }

    #[test]
    fn parse_import_mixed_alias() {
        let file = Parser::new(r#"import "./f.gcl" { x, Y as Z, w };"#)
            .parse_file()
            .unwrap();
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        let crate::ast::ImportKind::Selective(names) = &u.kind else {
            panic!("expected Selective");
        };
        assert_eq!(names.len(), 3);
        assert_eq!(names[0].name.name, "x");
        assert!(names[0].alias.is_none());
        assert_eq!(names[1].name.name, "Y");
        assert_eq!(names[1].alias.as_ref().unwrap().name, "Z");
        assert_eq!(names[1].local_name(), "Z");
        assert_eq!(names[2].name.name, "w");
        assert!(names[2].alias.is_none());
    }

    #[test]
    fn parse_import_alias_missing_name_error() {
        let result = Parser::new(r#"import "./f.gcl" { x as };"#).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_import_module_bare() {
        let file = Parser::new(r#"import "./constants.gcl";"#)
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        assert_eq!(u.path.display_path(), "./constants.gcl");
        let crate::ast::ImportKind::Module { alias } = &u.kind else {
            panic!("expected Module");
        };
        assert!(alias.is_none());
    }

    #[test]
    fn parse_import_module_with_alias() {
        let file = Parser::new(r#"import "./constants.gcl" as consts;"#)
            .parse_file()
            .unwrap();
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        assert_eq!(u.path.display_path(), "./constants.gcl");
        let crate::ast::ImportKind::Module { alias } = &u.kind else {
            panic!("expected Module");
        };
        assert_eq!(alias.as_ref().unwrap().name, "consts");
    }

    #[test]
    fn parse_import_module_missing_alias_ident_error() {
        let result = Parser::new(r#"import "./f.gcl" as;"#).parse_file();
        assert!(result.is_err());
    }

    // ---- Bare module path tests ----

    #[test]
    fn parse_import_bare_path_selective() {
        let file = Parser::new("import nasa/rocket { delta_v };")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Import");
        };
        let crate::ast::ImportPath::ModulePath { segments, .. } = &u.path else {
            panic!("expected ModulePath");
        };
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].name, "nasa");
        assert_eq!(segments[1].name, "rocket");
        assert_eq!(u.path.display_path(), "nasa/rocket");
        let crate::ast::ImportKind::Selective(names) = &u.kind else {
            panic!("expected Selective");
        };
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].name.name, "delta_v");
    }

    #[test]
    fn parse_import_bare_path_nested() {
        let file = Parser::new("import a/b/c/d;").parse_file().unwrap();
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Import");
        };
        let crate::ast::ImportPath::ModulePath { segments, .. } = &u.path else {
            panic!("expected ModulePath");
        };
        assert_eq!(segments.len(), 4);
        assert_eq!(u.path.display_path(), "a/b/c/d");
    }

    #[test]
    fn parse_import_bare_path_with_alias() {
        let file = Parser::new("import nasa/rocket as r;")
            .parse_file()
            .unwrap();
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Import");
        };
        assert!(matches!(&u.path, crate::ast::ImportPath::ModulePath { .. }));
        assert_eq!(u.path.display_path(), "nasa/rocket");
        let crate::ast::ImportKind::Module { alias } = &u.kind else {
            panic!("expected Module");
        };
        assert_eq!(alias.as_ref().unwrap().name, "r");
    }

    #[test]
    fn parse_import_bare_path_with_param_bindings() {
        let file = Parser::new("import nasa/rocket(dry_mass = 800.0 kg) as stage_1;")
            .parse_file()
            .unwrap();
        let DeclKind::Import(u) = &file.declarations[0].kind else {
            panic!("expected Import");
        };
        assert!(matches!(&u.path, crate::ast::ImportPath::ModulePath { .. }));
        assert_eq!(u.path.display_path(), "nasa/rocket");
        assert_eq!(u.param_bindings.len(), 1);
        assert_eq!(u.param_bindings[0].name.name, "dry_mass");
        let crate::ast::ImportKind::Module { alias } = &u.kind else {
            panic!("expected Module");
        };
        assert_eq!(alias.as_ref().unwrap().name, "stage_1");
    }

    #[test]
    fn parse_import_bare_path_single_segment_error() {
        // Single-segment bare paths are ambiguous; require at least pkg/module
        let result = Parser::new("import foo;").parse_file();
        // This should parse as a module import with a bare identifier... actually
        // our parser requires at least one `/` for bare paths, so a single bare
        // identifier after `import` that isn't followed by `/` should error.
        assert!(result.is_err(), "single-segment bare import should fail");
    }

    #[test]
    fn parse_attribute_no_args() {
        let file = Parser::new("#[lazy]\nnode x: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        assert_eq!(file.declarations[0].attributes.len(), 1);
        assert_eq!(file.declarations[0].attributes[0].name.name, "lazy");
        assert!(file.declarations[0].attributes[0].args.is_empty());
    }

    #[test]
    fn parse_attribute_with_one_arg() {
        let file = Parser::new("#[assumes(pressure_safe)]\nnode x: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations[0].attributes.len(), 1);
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.name.name, "assumes");
        assert_eq!(attr.args.len(), 1);
        assert_eq!(
            attr.args[0].as_single_ident().unwrap().name,
            "pressure_safe"
        );
    }

    #[test]
    fn parse_attribute_with_multiple_args() {
        let file =
            Parser::new("#[assumes(pressure_safe, temp_bounded)]\nnode x: Dimensionless = 1.0;")
                .parse_file()
                .unwrap();
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.name.name, "assumes");
        assert_eq!(attr.args.len(), 2);
        assert_eq!(
            attr.args[0].as_single_ident().unwrap().name,
            "pressure_safe"
        );
        assert_eq!(attr.args[1].as_single_ident().unwrap().name, "temp_bounded");
    }

    #[test]
    fn parse_attribute_trailing_comma() {
        let file = Parser::new("#[assumes(pressure_safe,)]\nnode x: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.args.len(), 1);
    }

    #[test]
    fn parse_multiple_attributes() {
        let file = Parser::new("#[lazy]\n#[assumes(x)]\nnode y: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations[0].attributes.len(), 2);
        assert_eq!(file.declarations[0].attributes[0].name.name, "lazy");
        assert_eq!(file.declarations[0].attributes[1].name.name, "assumes");
    }

    #[test]
    fn parse_attribute_on_param() {
        let file = Parser::new("#[assumes(x)]\nparam y: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations[0].attributes.len(), 1);
        assert!(matches!(file.declarations[0].kind, DeclKind::Param(_)));
    }

    #[test]
    fn parse_no_attributes_still_works() {
        let file = Parser::new("param x: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        assert!(file.declarations[0].attributes.is_empty());
    }

    #[test]
    fn parse_attribute_span_covers_hash_to_bracket() {
        let file = Parser::new("#[lazy]\nnode x: Dimensionless = 1.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations[0].span.offset(), 0);
    }

    #[test]
    fn parse_attribute_expected_fail_no_args() {
        let file = Parser::new("#[expected_fail]\nassert x = true;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations[0].attributes.len(), 1);
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.name.name, "expected_fail");
        assert!(attr.args.is_empty());
    }

    #[test]
    fn parse_attribute_qualified_path() {
        let file = Parser::new("#[expected_fail(Mode::Boost)]\nassert x = true;")
            .parse_file()
            .unwrap();
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.args.len(), 1);
        let AttributeArg::Path { segments, .. } = &attr.args[0] else {
            panic!("expected Path, got {:?}", attr.args[0]);
        };
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].name, "Mode");
        assert_eq!(segments[1].name, "Boost");
    }

    #[test]
    fn parse_attribute_multiple_qualified_paths() {
        let file = Parser::new("#[expected_fail(Mode::Boost, Mode::Eco)]\nassert x = true;")
            .parse_file()
            .unwrap();
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.args.len(), 2);
        let AttributeArg::Path { segments: s0, .. } = &attr.args[0] else {
            panic!("expected Path, got {:?}", attr.args[0]);
        };
        assert_eq!(s0[0].name, "Mode");
        assert_eq!(s0[1].name, "Boost");
        let AttributeArg::Path { segments: s1, .. } = &attr.args[1] else {
            panic!("expected Path, got {:?}", attr.args[1]);
        };
        assert_eq!(s1[0].name, "Mode");
        assert_eq!(s1[1].name, "Eco");
    }

    #[test]
    fn parse_attribute_group_arg() {
        let file = Parser::new("#[expected_fail((Mode::Boost, Phase::Launch))]\nassert x = true;")
            .parse_file()
            .unwrap();
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.args.len(), 1);
        let AttributeArg::Group { elements, .. } = &attr.args[0] else {
            panic!("expected Group, got {:?}", attr.args[0]);
        };
        assert_eq!(elements.len(), 2);
        let AttributeArg::Path { segments: s0, .. } = &elements[0] else {
            panic!("expected Path, got {:?}", elements[0]);
        };
        assert_eq!(s0[0].name, "Mode");
        assert_eq!(s0[1].name, "Boost");
        let AttributeArg::Path { segments: s1, .. } = &elements[1] else {
            panic!("expected Path, got {:?}", elements[1]);
        };
        assert_eq!(s1[0].name, "Phase");
        assert_eq!(s1[1].name, "Launch");
    }

    #[test]
    fn parse_attribute_multiple_groups() {
        let source = "#[expected_fail((Mode::Boost, Phase::Launch), (Mode::Eco, Phase::Cruise))]\nassert x = true;";
        let file = Parser::new(source).parse_file().unwrap();
        let attr = &file.declarations[0].attributes[0];
        assert_eq!(attr.args.len(), 2);
        assert!(
            matches!(&attr.args[0], AttributeArg::Group { elements, .. } if elements.len() == 2)
        );
        assert!(
            matches!(&attr.args[1], AttributeArg::Group { elements, .. } if elements.len() == 2)
        );
    }
}
