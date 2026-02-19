use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::ast::{
    Attribute, BinOp, ConstDecl, DeclKind, Declaration, DeriveOp, DimDecl, DimExpr, DimExprItem,
    DimTerm, Expr, ExprKind, FieldDecl, FieldInit, File, FnBody, FnDecl, FnParam, ForBinding,
    GenericConstraint, GenericParam, Ident, IndexArg, IndexDecl, IndexDeclKind, LetBinding,
    MapEntry, MatchArm, MatchPattern, MulDivOp, NodeDecl, ParamDecl, PatternBinding, TypeDecl,
    TypeExpr, TypeExprKind, UnaryOp, UnitDecl, UnitDef, UnitExpr, UnitExprItem, VariantDecl,
};
use crate::lexer::Lexer;
use crate::names::{
    DeclName, DimName, FieldName, FnName, GenericParamName, IndexName, Spanned, StructTypeName,
    UnitName, VariantName,
};
use crate::span::Span;
use crate::token::Token;

/// Rich parse error with miette diagnostics.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum ParseError {
    #[error("unexpected token `{found}`")]
    #[diagnostic(code(graphcal::P001), help("expected {expected}"))]
    UnexpectedToken {
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("unexpected end of file")]
    #[diagnostic(code(graphcal::P002), help("expected {expected}"))]
    UnexpectedEof {
        expected: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("invalid number literal")]
    #[diagnostic(code(graphcal::P003))]
    InvalidNumber {
        reason: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("{reason}")]
        span: SourceSpan,
    },
}

pub struct Parser<'src> {
    lexer: Lexer<'src>,
    source: Arc<String>,
    source_name: String,
}

impl<'src> Parser<'src> {
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: "input".to_string(),
        }
    }

    #[must_use]
    pub fn with_name(source: &'src str, name: &str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: name.to_string(),
        }
    }

    fn named_source(&self) -> NamedSource<Arc<String>> {
        NamedSource::new(&self.source_name, Arc::clone(&self.source))
    }

    fn unexpected_token(&self, expected: &str, found: &str, span: Span) -> ParseError {
        ParseError::UnexpectedToken {
            expected: expected.to_string(),
            found: found.to_string(),
            src: self.named_source(),
            span: span.into(),
        }
    }

    fn unexpected_eof(&self, expected: &str) -> ParseError {
        ParseError::UnexpectedEof {
            expected: expected.to_string(),
            src: self.named_source(),
            span: Span::new(self.lexer.current_offset(), 0).into(),
        }
    }

    /// Parse a single expression from the source string.
    ///
    /// Expects the entire input to be consumed; returns an error if there
    /// are trailing tokens after the expression.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source is not a valid expression
    /// or if there are unexpected trailing tokens.
    pub fn parse_single_expr(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_expr()?;
        if let Some(tok) = self.lexer.peek().cloned() {
            let span = Span::new(self.lexer.current_offset(), 0);
            return Err(self.unexpected_token("end of input", &format!("{tok:?}"), span));
        }
        Ok(expr)
    }

    /// Parse the full source file into a [`File`] AST node.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source contains invalid syntax.
    pub fn parse_file(&mut self) -> Result<File, ParseError> {
        let mut declarations = Vec::new();
        while self.lexer.peek().is_some() {
            declarations.push(self.parse_declaration()?);
        }
        Ok(File { declarations })
    }

    fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        // Collect any leading attributes: #[name] or #[name(arg1, arg2)]
        let mut attributes = Vec::new();
        while self.lexer.peek() == Some(&Token::Hash) {
            attributes.push(self.parse_attribute()?);
        }

        let expected =
            "`param`, `node`, `const`, `dimension`, `unit`, `type`, `fn`, `index`, or `use`";
        let mut decl = match self.lexer.peek() {
            Some(Token::Param) => self.parse_param(),
            Some(Token::Node) => self.parse_node(),
            Some(Token::Const) => self.parse_const(),
            Some(Token::Dimension) => self.parse_dimension_decl(),
            Some(Token::Unit) => self.parse_unit_decl(),
            Some(Token::Type) => self.parse_type_decl(),
            Some(Token::Fn) => self.parse_fn_decl(),
            Some(Token::Index) => self.parse_index_decl(),
            Some(Token::Use) => self.parse_use_decl(),
            Some(_) => {
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
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
            kind: DeclKind::Unit(UnitDecl {
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
    fn parse_field_list(&mut self) -> Result<Vec<FieldDecl>, ParseError> {
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
            let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
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
            let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
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

    // --- use declaration ---

    /// Parse a use declaration:
    /// `use "./path/to/file.gcl" { name1, name2 };`
    fn parse_use_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Use)?;

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
                        Some(Ident {
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

            names.push(crate::ast::UseItem {
                name: Ident {
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
        let (_, end_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Use(crate::ast::UseDecl {
                path,
                path_span,
                names,
            }),
            span,
        })
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
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
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
            let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
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

    // --- function declaration ---

    /// Parse a function declaration:
    /// `fn NAME<GENERICS>(PARAMS) -> TYPE = EXPR;`
    /// `fn NAME<GENERICS>(PARAMS) -> TYPE { STMTS EXPR }`
    fn parse_fn_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Fn)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<FnName>();

        // Optional generic params: <D: Dim, E: Dim>
        let generic_params = if self.lexer.peek() == Some(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        // Parameter list: (param, param, ...)
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            params.push(self.parse_fn_param()?);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;

        // Return type: -> TypeExpr
        self.expect(Token::Arrow)?;
        let return_type = self.parse_type_expr()?;

        // Body: either `= expr;` (short) or `{ stmts expr }` (block)
        let (body, end_span) = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            let expr = self.parse_expr()?;
            let (_, semi_span) = self.expect(Token::Semicolon)?;
            (FnBody::Short(expr), semi_span)
        } else {
            let (_, lbrace_span) = self.expect(Token::LBrace)?;
            let (stmts, expr) = self.parse_block_contents()?;
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            let _ = lbrace_span; // span captured by rbrace
            (
                FnBody::Block {
                    stmts,
                    expr: Box::new(expr),
                },
                rbrace_span,
            )
        };

        let span = start_span.merge(end_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Fn(FnDecl {
                name,
                generic_params,
                params,
                return_type,
                body,
            }),
            span,
        })
    }

    /// Parse generic parameters: `<D: Dim, E: Dim>`
    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>, ParseError> {
        self.expect(Token::Lt)?;
        let mut params = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::Gt) {
                break;
            }
            let name = self.parse_any_ident()?.into_spanned::<GenericParamName>();
            self.expect(Token::Colon)?;
            let constraint_ident = self.parse_any_ident()?;
            let constraint = match constraint_ident.name.as_str() {
                "Dim" => GenericConstraint::Dim,
                "Index" => GenericConstraint::Index,
                "Type" => GenericConstraint::Type,
                _ => {
                    return Err(self.unexpected_token(
                        "`Dim`, `Index`, or `Type`",
                        &constraint_ident.name,
                        constraint_ident.span,
                    ));
                }
            };
            // Optional default: `= TypeExpr`
            let default = if self.lexer.peek() == Some(&Token::Eq) {
                self.lexer.next_token(); // consume `=`
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            params.push(GenericParam {
                name,
                constraint,
                default,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::Gt)?;
        Ok(params)
    }

    /// Parse a derive clause: `derive(Add, Sub, Neg)`
    fn parse_derive_clause(&mut self) -> Result<Vec<Spanned<DeriveOp>>, ParseError> {
        // Consume the `derive` identifier
        self.lexer.next_token();
        self.expect(Token::LParen)?;
        let mut derives = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            let op_ident = self.parse_any_ident()?;
            let op = match op_ident.name.as_str() {
                "Add" => DeriveOp::Add,
                "Sub" => DeriveOp::Sub,
                "Neg" => DeriveOp::Neg,
                _ => {
                    return Err(self.unexpected_token(
                        "`Add`, `Sub`, or `Neg`",
                        &op_ident.name,
                        op_ident.span,
                    ));
                }
            };
            derives.push(Spanned::new(op, op_ident.span));
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;
        Ok(derives)
    }

    /// Parse a function parameter: `name: TypeExpr`
    fn parse_fn_param(&mut self) -> Result<FnParam, ParseError> {
        let name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        Ok(FnParam { name, type_ann })
    }

    /// Parse the contents of a block: `let` bindings followed by a final expression.
    /// Shared between block expressions and function block bodies.
    fn parse_block_contents(&mut self) -> Result<(Vec<LetBinding>, Expr), ParseError> {
        let mut stmts = Vec::new();
        while self.lexer.peek() == Some(&Token::Let) {
            stmts.push(self.parse_let_binding()?);
        }
        let expr = self.parse_expr()?;
        Ok((stmts, expr))
    }

    /// Parse the RHS of a unit definition: `NUMBER UNIT_EXPR`
    /// E.g., `1000 m`, `1 kg * m / s^2`, `(PI / 180) rad`
    fn parse_unit_def(&mut self) -> Result<UnitDef, ParseError> {
        // Parse the scale expression: either a plain number or `(expr)`
        let (scale, scale_span) = self.parse_unit_scale()?;
        let unit_expr = self.parse_unit_expr()?;
        let span = scale_span.merge(unit_expr.span);
        Ok(UnitDef {
            scale,
            unit_expr,
            span,
        })
    }

    /// Parse the scale part of a unit definition.
    /// Supports: `1000`, `0.001`, `(PI / 180)`, `(expr)`
    fn parse_unit_scale(&mut self) -> Result<(f64, Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                    ParseError::InvalidNumber {
                        reason: e.to_string(),
                        src: self.named_source(),
                        span: span.into(),
                    }
                })?;
                Ok((value, span))
            }
            Some(Token::LParen) => {
                // Parenthesized const expression: evaluate later
                // For now, parse as expression and evaluate at compile time
                // We parse the expression and store it; the scale will be resolved later.
                // However, UnitDef.scale is f64 -- we need to handle this.
                // For Phase 1, the only non-trivial case is `(PI / 180)`.
                // We'll parse the paren expression, evaluate if it's a simple const expr.
                let (_, lp_span) = self.lexer.next_token().expect("peek confirmed Some");
                let expr = self.parse_expr()?;
                let (_, rp_span) = self.expect(Token::RParen)?;
                let span = lp_span.merge(rp_span);
                // Try to evaluate simple constant expressions
                let scale = self.eval_const_expr(&expr)?;
                Ok((scale, span))
            }
            Some(_) => {
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
                Err(self.unexpected_token("number or `(`", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("number or `(`")),
        }
    }

    /// Evaluate a simple constant expression at parse time (for unit definitions).
    /// Only supports: numbers, PI, E, +, -, *, /, ^, unary -.
    fn eval_const_expr(&self, expr: &Expr) -> Result<f64, ParseError> {
        match &expr.kind {
            ExprKind::Number(n) => Ok(*n),
            #[expect(clippy::cast_precision_loss, reason = "unit scale constant expression")]
            ExprKind::Integer(n) => Ok(*n as f64),
            ExprKind::ConstRef(ident) => match ident.value.as_str() {
                "PI" => Ok(std::f64::consts::PI),
                "E" => Ok(std::f64::consts::E),
                _ => Err(self.unexpected_token(
                    "PI or E",
                    &format!("constant `{}`", ident.value),
                    ident.span,
                )),
            },
            ExprKind::BinOp { op, lhs, rhs } => {
                let l = self.eval_const_expr(lhs)?;
                let r = self.eval_const_expr(rhs)?;
                Ok(match op {
                    BinOp::Add => l + r,
                    BinOp::Sub => l - r,
                    BinOp::Mul => l * r,
                    BinOp::Div => l / r,
                    BinOp::Pow => l.powf(r),
                    _ => {
                        return Err(self.unexpected_token(
                            "arithmetic operator",
                            &format!("`{op:?}`"),
                            expr.span,
                        ));
                    }
                })
            }
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => Ok(-self.eval_const_expr(operand)?),
            _ => Err(self.unexpected_token("constant expression", "complex expression", expr.span)),
        }
    }

    // --- Type expressions ---

    /// Parse a type expression: `Dimensionless` or a dimension expression.
    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        // Parse the base type first
        let mut base = if let Some((Token::Ident, span)) = self.lexer.peek_with_span() {
            let text = self.lexer.slice_at(span);
            if text == "Dimensionless" {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                TypeExpr {
                    kind: TypeExprKind::Dimensionless,
                    span,
                }
            } else if text == "Bool" {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                TypeExpr {
                    kind: TypeExprKind::Bool,
                    span,
                }
            } else if text == "Int" {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                TypeExpr {
                    kind: TypeExprKind::Int,
                    span,
                }
            } else if is_pascal_case(text) && self.is_lt_after_ident(span) {
                // Type application: Vec3<Length, ECI>
                let ident = self.parse_any_ident()?;
                let type_args = self.parse_type_arg_list()?;
                let end_span = type_args.last().map_or(ident.span, |a| a.span);
                let span = ident.span.merge(end_span);
                TypeExpr {
                    kind: TypeExprKind::TypeApplication {
                        name: ident,
                        type_args,
                    },
                    span,
                }
            } else {
                let dim_expr = self.parse_dim_expr()?;
                let span = dim_expr.span;
                TypeExpr {
                    kind: TypeExprKind::DimExpr(dim_expr),
                    span,
                }
            }
        } else {
            let dim_expr = self.parse_dim_expr()?;
            let span = dim_expr.span;
            TypeExpr {
                kind: TypeExprKind::DimExpr(dim_expr),
                span,
            }
        };

        // Check for optional `[Index, ...]` suffix
        if self.lexer.peek() == Some(&Token::LBracket) {
            let (_, _bracket_span) = self.lexer.next_token().expect("peek confirmed Some");
            let mut indexes = Vec::new();
            loop {
                if self.lexer.peek() == Some(&Token::RBracket) {
                    break;
                }
                let idx_name =
                    self.parse_ident_with_casing("PascalCase identifier", is_uppercase_starting)?;
                indexes.push(idx_name);
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            let (_, end_span) = self.expect(Token::RBracket)?;
            let span = base.span.merge(end_span);
            base = TypeExpr {
                kind: TypeExprKind::Indexed {
                    base: Box::new(base),
                    indexes,
                },
                span,
            };
        }

        Ok(base)
    }

    /// Parse a dimension expression: `DimTerm (("*" | "/") DimTerm)*`
    fn parse_dim_expr(&mut self) -> Result<DimExpr, ParseError> {
        let first_term = self.parse_dim_term()?;
        let start_span = first_term.span;
        let mut terms = vec![DimExprItem {
            op: MulDivOp::Mul,
            term: first_term,
        }];

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let term = self.parse_dim_term()?;
                    terms.push(DimExprItem {
                        op: MulDivOp::Mul,
                        term,
                    });
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let term = self.parse_dim_term()?;
                    terms.push(DimExprItem {
                        op: MulDivOp::Div,
                        term,
                    });
                }
                _ => break,
            }
        }

        let end_span = terms.last().expect("at least one term").term.span;
        Ok(DimExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single dimension term: `IDENT ("^" INTEGER)?`
    fn parse_dim_term(&mut self) -> Result<DimTerm, ParseError> {
        let name = self.parse_any_ident()?;
        let mut end_span = name.span;

        let power = if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (neg, value, span) = self.parse_integer_literal()?;
            end_span = span;
            Some(if neg { -value } else { value })
        } else {
            None
        };

        Ok(DimTerm {
            span: name.span.merge(end_span),
            name,
            power,
        })
    }

    // --- Unit expressions ---

    /// Parse a unit expression: `IDENT (("*" | "/") IDENT ("^" INTEGER)?)*`
    fn parse_unit_expr(&mut self) -> Result<UnitExpr, ParseError> {
        let first_ident = self.parse_any_ident()?;
        let start_span = first_ident.span;
        let mut end_span = first_ident.span;
        let first_name = first_ident.into_spanned::<UnitName>();

        let first_power = if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (neg, value, span) = self.parse_integer_literal()?;
            end_span = span;
            Some(if neg { -value } else { value })
        } else {
            None
        };

        let mut terms = vec![UnitExprItem {
            op: MulDivOp::Mul,
            name: first_name,
            power: first_power,
        }];

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let ident = self.parse_any_ident()?;
                    end_span = ident.span;
                    let name = ident.into_spanned::<UnitName>();
                    let power = if self.lexer.peek() == Some(&Token::Caret) {
                        self.lexer.next_token();
                        let (neg, value, span) = self.parse_integer_literal()?;
                        end_span = span;
                        Some(if neg { -value } else { value })
                    } else {
                        None
                    };
                    terms.push(UnitExprItem {
                        op: MulDivOp::Mul,
                        name,
                        power,
                    });
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let ident = self.parse_any_ident()?;
                    end_span = ident.span;
                    let name = ident.into_spanned::<UnitName>();
                    let power = if self.lexer.peek() == Some(&Token::Caret) {
                        self.lexer.next_token();
                        let (neg, value, span) = self.parse_integer_literal()?;
                        end_span = span;
                        Some(if neg { -value } else { value })
                    } else {
                        None
                    };
                    terms.push(UnitExprItem {
                        op: MulDivOp::Div,
                        name,
                        power,
                    });
                }
                _ => break,
            }
        }

        Ok(UnitExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse an integer literal, possibly preceded by `-`.
    /// Returns `(is_negative, absolute_value, span)`.
    fn parse_integer_literal(&mut self) -> Result<(bool, i32, Span), ParseError> {
        let neg = if self.lexer.peek() == Some(&Token::Minus) {
            self.lexer.next_token();
            true
        } else {
            false
        };

        match self.lexer.next_token() {
            Some((Token::Number, span)) => {
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: i32 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected integer".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok((neg, value, span))
            }
            Some((tok, span)) => Err(self.unexpected_token("integer", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("integer")),
        }
    }

    // --- Expression parsing ---
    // Precedence (lowest to highest):
    //   0. -> (conversion, lowest)
    //   1. if/else (conditional)
    //   2. || (or)
    //   3. && (and)
    //   4. ==, !=, <, >, <=, >= (comparison, non-chaining)
    //   5. +, - (additive)
    //   6. *, / (multiplicative)
    //   7. unary -, ! (prefix)
    //   8. ^ (power, right-associative)
    //   9. atoms (including NUMBER UNIT_EXPR)

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_convert()
    }

    /// Parse conversion: `expr -> unit_expr` (lowest precedence).
    fn parse_convert(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_conditional()?;

        if self.lexer.peek() == Some(&Token::Arrow) {
            self.lexer.next_token();
            let target = self.parse_unit_expr()?;
            let span = expr.span.merge(target.span);
            Ok(Expr {
                kind: ExprKind::Convert {
                    expr: Box::new(expr),
                    target,
                },
                span,
            })
        } else if self.lexer.peek() == Some(&Token::As) {
            self.lexer.next_token();
            let target_type = self.parse_type_expr()?;
            let span = expr.span.merge(target_type.span);
            Ok(Expr {
                kind: ExprKind::AsCast {
                    expr: Box::new(expr),
                    target_type,
                },
                span,
            })
        } else {
            Ok(expr)
        }
    }

    fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
        if self.lexer.peek() == Some(&Token::If) {
            let (_, if_span) = self.lexer.next_token().expect("peek confirmed Some");
            let condition = self.parse_expr()?;
            self.expect(Token::LBrace)?;
            let then_branch = self.parse_expr()?;
            self.expect(Token::RBrace)?;
            self.expect(Token::Else)?;
            self.expect(Token::LBrace)?;
            let else_branch = self.parse_expr()?;
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            let span = if_span.merge(rbrace_span);
            Ok(Expr {
                kind: ExprKind::If {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
                span,
            })
        } else {
            self.parse_or()
        }
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.lexer.peek() == Some(&Token::PipePipe) {
            self.lexer.next_token();
            let rhs = self.parse_and()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_comparison()?;
        while self.lexer.peek() == Some(&Token::AmpAmp) {
            self.lexer.next_token();
            let rhs = self.parse_comparison()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = match self.lexer.peek() {
            Some(Token::EqEq) => Some(BinOp::Eq),
            Some(Token::BangEq) => Some(BinOp::Ne),
            Some(Token::Lt) => Some(BinOp::Lt),
            Some(Token::Gt) => Some(BinOp::Gt),
            Some(Token::LtEq) => Some(BinOp::Le),
            Some(Token::GtEq) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.lexer.next_token();
            let rhs = self.parse_add()?;
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.lexer.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.lexer.next_token();
            let rhs = self.parse_mul()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.lexer.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                Some(Token::Percent) => BinOp::Mod,
                _ => break,
            };
            self.lexer.next_token();
            let rhs = self.parse_unary()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Minus) => {
                let (_, op_span) = self.lexer.next_token().expect("peek confirmed Some");
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                })
            }
            Some(Token::Bang) => {
                let (_, op_span) = self.lexer.next_token().expect("peek confirmed Some");
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                })
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Expr, ParseError> {
        let base = self.parse_postfix()?;
        if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            // Right-associative: recurse into parse_unary, not parse_power
            let exp = self.parse_unary()?;
            let span = base.span.merge(exp.span);
            Ok(Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Pow,
                    lhs: Box::new(base),
                    rhs: Box::new(exp),
                },
                span,
            })
        } else {
            Ok(base)
        }
    }

    /// Parse postfix operators (field access `.field`, index access `[i]`).
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_atom()?;
        loop {
            match self.lexer.peek() {
                Some(Token::Dot) => {
                    self.lexer.next_token(); // consume '.'
                    let field_ident = self.parse_any_ident()?;
                    let span = expr.span.merge(field_ident.span);
                    expr = Expr {
                        kind: ExprKind::FieldAccess {
                            expr: Box::new(expr),
                            field: field_ident.into_spanned::<FieldName>(),
                        },
                        span,
                    };
                }
                Some(Token::LBracket) => {
                    self.lexer.next_token(); // consume '['
                    let mut args = Vec::new();
                    loop {
                        if self.lexer.peek() == Some(&Token::RBracket) {
                            break;
                        }
                        args.push(self.parse_index_arg()?);
                        if self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                        } else {
                            break;
                        }
                    }
                    let (_, end_span) = self.expect(Token::RBracket)?;
                    let span = expr.span.merge(end_span);
                    expr = Expr {
                        kind: ExprKind::IndexAccess {
                            expr: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one match arm per atom kind; splitting would obscure"
    )]
    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                let text = self.lexer.slice_at(span).replace('_', "");
                let is_integer = !text.contains('.') && !text.contains('e') && !text.contains('E');

                if is_integer {
                    // Integer literal: no decimal point or scientific notation
                    if self.lexer.peek() == Some(&Token::Ident) {
                        // Integer followed by unit is an error: must use float
                        return Err(ParseError::InvalidNumber {
                            reason: format!(
                                "integer literal cannot have units; write `{text}.0` instead"
                            ),
                            src: self.named_source(),
                            span: span.into(),
                        });
                    }
                    let value: i64 = text.parse().map_err(|e: std::num::ParseIntError| {
                        ParseError::InvalidNumber {
                            reason: e.to_string(),
                            src: self.named_source(),
                            span: span.into(),
                        }
                    })?;
                    Ok(Expr {
                        kind: ExprKind::Integer(value),
                        span,
                    })
                } else {
                    // Float literal: has decimal point or scientific notation
                    let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                        ParseError::InvalidNumber {
                            reason: e.to_string(),
                            src: self.named_source(),
                            span: span.into(),
                        }
                    })?;

                    // Check if followed by an identifier (unit literal): `400.0 km`
                    if self.lexer.peek() == Some(&Token::Ident) {
                        let unit_expr = self.parse_unit_expr()?;
                        let full_span = span.merge(unit_expr.span);
                        Ok(Expr {
                            kind: ExprKind::UnitLiteral {
                                value,
                                unit: unit_expr,
                            },
                            span: full_span,
                        })
                    } else {
                        Ok(Expr {
                            kind: ExprKind::Number(value),
                            span,
                        })
                    }
                }
            }
            Some(Token::True) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    span,
                })
            }
            Some(Token::False) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    span,
                })
            }
            Some(Token::At) => {
                let (_, at_span) = self.lexer.next_token().expect("peek confirmed Some");
                let ident =
                    self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
                let span = at_span.merge(ident.span);
                Ok(Expr {
                    kind: ExprKind::GraphRef(ident.into_spanned::<DeclName>()),
                    span,
                })
            }
            Some(Token::Ident) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                let name = self.lexer.slice_at(span).to_string();

                if is_upper_snake_case(&name) {
                    // Const reference: PI, E, G0, UPPER_NAME
                    Ok(Expr {
                        kind: ExprKind::ConstRef(Spanned::new(DeclName::new(name), span)),
                        span,
                    })
                } else if is_pascal_case(&name)
                    && self.lexer.peek() == Some(&Token::Lt)
                    && self.is_type_args_followed_by_brace()
                {
                    // Generic struct construction: Vec3<Length, ECI> { x: 1 km, ... }
                    let type_args = self.parse_type_arg_list()?;
                    self.parse_struct_construction_with_type_args(
                        Spanned::new(StructTypeName::new(name), span),
                        type_args,
                    )
                } else if is_pascal_case(&name) && self.lexer.peek() == Some(&Token::LBrace) {
                    // Struct/variant construction with fields: TypeName { field1: expr, field2 }
                    self.parse_struct_construction(Spanned::new(StructTypeName::new(name), span))
                } else if is_pascal_case(&name) {
                    // Bare variant construction (no fields): `Nominal`
                    Ok(Expr {
                        kind: ExprKind::StructConstruction {
                            type_name: Spanned::new(StructTypeName::new(name), span),
                            type_args: Vec::new(),
                            fields: Vec::new(),
                        },
                        span,
                    })
                } else if name == "scan" && self.lexer.peek() == Some(&Token::LParen) {
                    // Scan expression: scan(source, init, |acc, val| body)
                    self.parse_scan(&Ident { name, span })
                } else if name == "unfold" && self.lexer.peek() == Some(&Token::LParen) {
                    // Unfold expression: unfold(init, |prev_i, i| body)
                    self.parse_unfold(&Ident { name, span })
                } else if self.lexer.peek() == Some(&Token::LParen) {
                    // Function call: name(args...)
                    self.lexer.next_token(); // consume '('
                    let mut args = Vec::new();
                    if self.lexer.peek() != Some(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        while self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                            args.push(self.parse_expr()?);
                        }
                    }
                    let (_, rparen_span) = self.expect(Token::RParen)?;
                    let call_span = span.merge(rparen_span);
                    Ok(Expr {
                        kind: ExprKind::FnCall {
                            name: Spanned::new(FnName::new(name), span),
                            args,
                        },
                        span: call_span,
                    })
                } else {
                    // Bare lowercase identifier -> LocalRef (let binding reference)
                    // Semantic validation happens in resolve/dim_check.
                    Ok(Expr {
                        kind: ExprKind::LocalRef(Ident { name, span }),
                        span,
                    })
                }
            }
            Some(Token::For) => {
                // For comprehension: for m: Maneuver { expr }
                self.parse_for_comp()
            }
            Some(Token::LBrace) => {
                // Disambiguate: map literal vs block expression
                // Consume '{' and peek at what follows
                let (_, start_span) = self.lexer.next_token().expect("peek confirmed Some");
                if let Some((Token::Ident, ident_span)) = self.lexer.peek_with_span() {
                    let text = self.lexer.slice_at(ident_span);
                    if is_pascal_case(text) {
                        // Could be map literal (PascalCase :: ...) or struct in block
                        // Save the ident text to check further
                        let saved_text = text.to_string();
                        // Consume the ident to peek at what's next
                        let (_, saved_span) = self.lexer.next_token().expect("peek confirmed");
                        if self.lexer.peek() == Some(&Token::ColonColon) {
                            // Map literal: { Ident :: Variant : expr, ... }
                            self.parse_map_literal_after_first_ident(
                                start_span,
                                Spanned::new(IndexName::new(saved_text), saved_span),
                            )
                        } else {
                            // Not a map literal — reparse as block with already-consumed tokens
                            // The consumed ident is the start of an expression in the block
                            self.parse_block_after_open_brace_and_ident(
                                start_span,
                                &saved_text,
                                saved_span,
                            )
                        }
                    } else {
                        // lowercase ident or other — block expression
                        self.parse_block_after_open_brace(start_span)
                    }
                } else {
                    // Not an ident after { — could be `{ let ...` or `{ expr }`
                    self.parse_block_after_open_brace(start_span)
                }
            }
            Some(Token::Match) => self.parse_match_expr(),
            Some(Token::LParen) => {
                self.lexer.next_token();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(_) => {
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
                Err(self.unexpected_token("expression", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("expression")),
        }
    }

    // --- Block and let binding parsing ---

    /// Parse a let binding: `let IDENT (: TypeExpr)? = Expr ;`
    fn parse_let_binding(&mut self) -> Result<LetBinding, ParseError> {
        let (_, let_span) = self.expect(Token::Let)?;
        let name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;

        // Optional type annotation
        let type_ann = if self.lexer.peek() == Some(&Token::Colon) {
            self.lexer.next_token(); // consume ':'
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = let_span.merge(semi_span);

        Ok(LetBinding {
            name,
            type_ann,
            value,
            span,
        })
    }

    // --- Match expression ---

    /// Parse a match expression:
    /// `match expr { Variant1 { field } => body, Variant2 => body }`
    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::Match)?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(Token::LBrace)?;

        let mut arms = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            arms.push(self.parse_match_arm()?);
            // Optional comma between arms
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            }
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Match { scrutinee, arms },
            span,
        })
    }

    /// Parse a single match arm: `VariantName { field1, field2: _ } => expr`
    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_match_pattern()?;
        self.expect(Token::FatArrow)?;
        let body = self.parse_expr()?;
        let span = pattern.span.merge(body.span);
        Ok(MatchArm {
            pattern,
            body,
            span,
        })
    }

    /// Parse a match pattern: `VariantName { field1, field2: binding }` or bare `VariantName`
    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let variant_ident = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
        let variant_name = Spanned::new(VariantName::new(&variant_ident.name), variant_ident.span);
        let start_span = variant_ident.span;

        let (bindings, end_span) = if self.lexer.peek() == Some(&Token::LBrace) {
            self.lexer.next_token(); // consume '{'
            let mut bindings = Vec::new();
            loop {
                if self.lexer.peek() == Some(&Token::RBrace) {
                    break;
                }
                bindings.push(self.parse_pattern_binding()?);
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            (bindings, rbrace_span)
        } else {
            // Bare variant: no bindings
            (Vec::new(), start_span)
        };

        Ok(MatchPattern {
            variant_name,
            bindings,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single pattern binding:
    /// - `field_name` (shorthand: bind to same name)
    /// - `field_name: var_name` (rename)
    /// - `field_name: _` (wildcard)
    fn parse_pattern_binding(&mut self) -> Result<PatternBinding, ParseError> {
        let field_ident = self.parse_any_ident()?;
        let field = Spanned::new(FieldName::new(&field_ident.name), field_ident.span);

        if self.lexer.peek() == Some(&Token::Colon) {
            self.lexer.next_token(); // consume ':'
            // Check for wildcard `_`
            if self.lexer.peek() == Some(&Token::Underscore) {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                return Ok(PatternBinding::Wildcard { field, span });
            }
            // Renamed binding: `field_name: var_name`
            let var = self.parse_any_ident()?;
            Ok(PatternBinding::Bind { field, var })
        } else {
            // Shorthand: bind to same name as field
            Ok(PatternBinding::Bind {
                field,
                var: field_ident,
            })
        }
    }

    // --- Struct construction ---

    fn parse_struct_construction(
        &mut self,
        type_name: Spanned<StructTypeName>,
    ) -> Result<Expr, ParseError> {
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();

        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let field_name = self.parse_any_ident()?.into_spanned::<FieldName>();

            // Check for `:` (explicit value) or shorthand (just name)
            let value = if self.lexer.peek() == Some(&Token::Colon) {
                self.lexer.next_token(); // consume ':'
                Some(self.parse_expr()?)
            } else {
                None // shorthand: field name matches variable name
            };

            fields.push(FieldInit {
                name: field_name,
                value,
            });

            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = type_name.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::StructConstruction {
                type_name,
                type_args: Vec::new(),
                fields,
            },
            span,
        })
    }

    /// Parse struct construction with explicit type args: `Vec3<Length, ECI> { x: 1 km, ... }`
    /// Called after the type args have already been parsed.
    fn parse_struct_construction_with_type_args(
        &mut self,
        type_name: Spanned<StructTypeName>,
        type_args: Vec<TypeExpr>,
    ) -> Result<Expr, ParseError> {
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();

        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let field_name = self.parse_any_ident()?.into_spanned::<FieldName>();

            let value = if self.lexer.peek() == Some(&Token::Colon) {
                self.lexer.next_token();
                Some(self.parse_expr()?)
            } else {
                None
            };

            fields.push(FieldInit {
                name: field_name,
                value,
            });

            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = type_name.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::StructConstruction {
                type_name,
                type_args,
                fields,
            },
            span,
        })
    }

    /// Check if `<` follows the current ident token (used for type application detection).
    /// Scans the raw source after the ident span to find `<` (skipping whitespace).
    fn is_lt_after_ident(&self, ident_span: Span) -> bool {
        let bytes = self.source.as_bytes();
        let mut pos = ident_span.offset + ident_span.len;
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        pos < bytes.len() && bytes[pos] == b'<'
    }

    /// Look ahead to check if `<...>` is followed by `{`.
    /// Used to disambiguate `Vec3<Length, ECI> { ... }` (struct construction with type args)
    /// from `Foo < bar` (comparison).
    ///
    /// Scans the raw source string from the current position to find matching angle brackets.
    fn is_type_args_followed_by_brace(&mut self) -> bool {
        // Get the byte offset where `<` starts
        let Some((&Token::Lt, lt_span)) = self.lexer.peek_with_span() else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let mut pos = lt_span.offset + lt_span.len; // byte after `<`
        let mut depth: usize = 1;
        while pos < bytes.len() {
            match bytes[pos] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        // Skip whitespace after `>`
                        let mut p = pos + 1;
                        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                            p += 1;
                        }
                        return p < bytes.len() && bytes[p] == b'{';
                    }
                }
                _ => {}
            }
            pos += 1;
        }
        false
    }

    /// Parse a type argument list: `<TypeExpr, TypeExpr, ...>`
    fn parse_type_arg_list(&mut self) -> Result<Vec<TypeExpr>, ParseError> {
        self.expect(Token::Lt)?;
        let mut args = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::Gt) {
                break;
            }
            args.push(self.parse_type_expr()?);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::Gt)?;
        Ok(args)
    }

    // --- For comprehension ---

    /// Parse a for comprehension: `for m: Maneuver, n: Phase { expr }`
    fn parse_for_comp(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::For)?;
        let mut bindings = Vec::new();
        loop {
            let var = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
            self.expect(Token::Colon)?;
            let index = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<IndexName>();
            bindings.push(ForBinding { var, index });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::LBrace)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::ForComp {
                bindings,
                body: Box::new(body),
            },
            span,
        })
    }

    // --- Map literal ---

    /// Parse a map literal after `{` and the first `Index` ident have been consumed.
    /// The `::` is the next token to consume.
    fn parse_map_literal_after_first_ident(
        &mut self,
        brace_span: Span,
        first_index: Spanned<IndexName>,
    ) -> Result<Expr, ParseError> {
        // Consume `::` (we already peeked and confirmed it)
        self.expect(Token::ColonColon)?;
        let variant = self
            .parse_ident_with_casing("PascalCase", is_pascal_case)?
            .into_spanned::<VariantName>();
        self.expect(Token::Colon)?;
        let value = self.parse_expr()?;
        let mut entries = vec![MapEntry {
            index: first_index,
            variant,
            value,
        }];
        // Parse remaining entries
        while self.lexer.peek() == Some(&Token::Comma) {
            self.lexer.next_token(); // consume ','
            if self.lexer.peek() == Some(&Token::RBrace) {
                break; // trailing comma
            }
            let index = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<IndexName>();
            self.expect(Token::ColonColon)?;
            let variant = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<VariantName>();
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            entries.push(MapEntry {
                index,
                variant,
                value,
            });
        }
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = brace_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::MapLiteral { entries },
            span,
        })
    }

    /// Parse a block expression after `{` has been consumed.
    fn parse_block_after_open_brace(&mut self, start_span: Span) -> Result<Expr, ParseError> {
        let (stmts, expr) = self.parse_block_contents()?;
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Block {
                stmts,
                expr: Box::new(expr),
            },
            span,
        })
    }

    /// Parse a block expression after `{` and a `PascalCase` ident have been consumed.
    /// The ident was consumed during map literal disambiguation but turned out not
    /// to be a map literal (no `::` followed). Reconstruct parsing state.
    fn parse_block_after_open_brace_and_ident(
        &mut self,
        start_span: Span,
        ident_name: &str,
        ident_span: Span,
    ) -> Result<Expr, ParseError> {
        // The consumed PascalCase ident is the start of an expression in the block.
        // It could be a struct construction (PascalCase { ... }) or a ConstRef.
        let first_expr = if is_pascal_case(ident_name) && self.lexer.peek() == Some(&Token::LBrace)
        {
            self.parse_struct_construction(Spanned::new(
                StructTypeName::new(ident_name),
                ident_span,
            ))?
        } else {
            // Treat as ConstRef (UPPER_SNAKE_CASE or PascalCase used as const)
            Expr {
                kind: ExprKind::ConstRef(Spanned::new(DeclName::new(ident_name), ident_span)),
                span: ident_span,
            }
        };
        // Now continue parsing the rest of the expression (binary ops, etc.)
        // and then close the block. This is the tricky part — the block might
        // have just this one expression: `{ SomeExpr }` or it might continue
        // with operators. We need to continue parsing the expression.
        // Since we already parsed an atom, we need to handle postfix + binary ops.
        // The simplest approach: treat `first_expr` as already parsed and
        // continue with `parse_expr_continued` or wrap the block.
        // Actually, for blocks the pattern is `{ let ...; expr }` or `{ expr }`.
        // Since we consumed an ident that's not a `let`, this must be `{ expr }`.
        // We need to finish parsing the expression (postfix, binary ops).
        // Unfortunately we can't easily resume mid-expression parsing.
        // The practical cases are: `{ StructName { ... } }` or `{ CONST_REF op ... }`.
        // Let's handle it by continuing to parse operators on first_expr.
        let expr = self.continue_parsing_expr(first_expr)?;
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Block {
                stmts: vec![],
                expr: Box::new(expr),
            },
            span,
        })
    }

    /// Continue parsing an expression from an already-parsed left-hand side.
    /// Handles postfix operations and binary operators.
    fn continue_parsing_expr(&mut self, mut expr: Expr) -> Result<Expr, ParseError> {
        // Handle postfix (field access, index access)
        loop {
            match self.lexer.peek() {
                Some(Token::Dot) => {
                    self.lexer.next_token();
                    let field_ident = self.parse_any_ident()?;
                    let span = expr.span.merge(field_ident.span);
                    expr = Expr {
                        kind: ExprKind::FieldAccess {
                            expr: Box::new(expr),
                            field: field_ident.into_spanned::<FieldName>(),
                        },
                        span,
                    };
                }
                Some(Token::LBracket) => {
                    self.lexer.next_token();
                    let mut args = Vec::new();
                    loop {
                        if self.lexer.peek() == Some(&Token::RBracket) {
                            break;
                        }
                        args.push(self.parse_index_arg()?);
                        if self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                        } else {
                            break;
                        }
                    }
                    let (_, end_span) = self.expect(Token::RBracket)?;
                    let span = expr.span.merge(end_span);
                    expr = Expr {
                        kind: ExprKind::IndexAccess {
                            expr: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        // For simplicity, don't handle binary operators here.
        // This path is only hit for `{ PascalCase ... }` blocks which are rare
        // and typically just `{ StructName { ... } }`.
        Ok(expr)
    }

    // --- Index access ---

    /// Parse an index argument: either `Index::Variant` or a loop variable `m`.
    fn parse_index_arg(&mut self) -> Result<IndexArg, ParseError> {
        let (_, span) = self
            .lexer
            .next_token()
            .ok_or_else(|| self.unexpected_eof("index argument"))?;
        let name = self.lexer.slice_at(span).to_string();

        if is_pascal_case(&name) && self.lexer.peek() == Some(&Token::ColonColon) {
            // Qualified variant: Index::Variant
            self.lexer.next_token(); // consume '::'
            let variant = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<VariantName>();
            Ok(IndexArg::Variant {
                index: Spanned::new(IndexName::new(name), span),
                variant,
            })
        } else if is_lower_snake_case(&name) {
            // Loop variable
            Ok(IndexArg::Var(Ident { name, span }))
        } else {
            Err(self.unexpected_token("loop variable or `Index::Variant`", &name, span))
        }
    }

    // --- Scan expression ---

    /// Parse a scan expression: `scan(source, init, |acc, val| body)` → `ExprKind::Scan`
    fn parse_scan(&mut self, name_ident: &Ident) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let first_expr = self.parse_expr()?;
        self.expect(Token::Comma)?;
        let init = self.parse_expr()?;
        self.expect(Token::Comma)?;
        // Parse lambda: |acc, val| body
        self.expect(Token::Pipe)?;
        let acc_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Comma)?;
        let val_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Pipe)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        let span = name_ident.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Scan {
                source: Box::new(first_expr),
                init: Box::new(init),
                acc_name,
                val_name,
                body: Box::new(body),
            },
            span,
        })
    }

    // --- Unfold expression ---

    /// Parse an unfold expression: `unfold(init, |prev_i, i| body)` → `ExprKind::Unfold`
    fn parse_unfold(&mut self, name_ident: &Ident) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let init = self.parse_expr()?;
        self.expect(Token::Comma)?;
        self.expect(Token::Pipe)?;
        let prev_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Comma)?;
        let curr_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Pipe)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        let span = name_ident.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Unfold {
                init: Box::new(init),
                prev_name,
                curr_name,
                body: Box::new(body),
            },
            span,
        })
    }

    // --- Helper methods ---

    #[expect(
        clippy::needless_pass_by_value,
        reason = "Token is small and the API is cleaner with by-value"
    )]
    fn expect(&mut self, expected: Token) -> Result<(Token, Span), ParseError> {
        let expected_str = format!("`{expected}`");
        match self.lexer.next_token() {
            Some((tok, span)) if tok == expected => Ok((tok, span)),
            Some((tok, span)) => Err(self.unexpected_token(&expected_str, &tok.to_string(), span)),
            None => Err(self.unexpected_eof(&expected_str)),
        }
    }

    /// Parse an identifier and check that it matches the expected casing.
    fn parse_ident_with_casing(
        &mut self,
        casing_desc: &str,
        check: fn(&str) -> bool,
    ) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => {
                let name = self.lexer.slice_at(span).to_string();
                if check(&name) {
                    Ok(Ident { name, span })
                } else {
                    Err(self.unexpected_token(
                        &format!("{casing_desc} identifier"),
                        &format!("identifier `{name}`"),
                        span,
                    ))
                }
            }
            Some((tok, span)) => Err(self.unexpected_token(
                &format!("{casing_desc} identifier"),
                &tok.to_string(),
                span,
            )),
            None => Err(self.unexpected_eof(&format!("{casing_desc} identifier"))),
        }
    }

    /// Parse any identifier regardless of casing.
    fn parse_any_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => Ok(Ident {
                name: self.lexer.slice_at(span).to_string(),
                span,
            }),
            Some((tok, span)) => Err(self.unexpected_token("identifier", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("identifier")),
        }
    }
}

fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_lower_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `PascalCase`: starts with uppercase, contains at least one lowercase letter
/// (to distinguish from `UPPER_SNAKE_CASE` like `GRAVITY`).
fn is_pascal_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars().any(|c| c.is_ascii_lowercase())
}

/// Uppercase-starting identifier: `PascalCase` names or single-letter generic params like `I`.
/// Used where both concrete index names (`Maneuver`) and generic params (`I`) are valid.
fn is_uppercase_starting(s: &str) -> bool {
    !s.is_empty() && s.starts_with(|c: char| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;

    // --- Phase 1 declaration tests ---

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
                    matches!(p.value.kind, ExprKind::Number(n) if (n - 42.0).abs() < f64::EPSILON)
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
                assert!(matches!(p.value.kind, ExprKind::UnitLiteral { .. }));
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

    // --- Dimension declarations ---

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

    // --- Unit declarations ---

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

    // --- Unit literals ---

    #[test]
    fn parse_unit_literal() {
        let file = Parser::new("param alt: Length = 400.0 km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.kind {
                ExprKind::UnitLiteral { value, unit } => {
                    assert!((value - 400.0).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 1);
                    assert_eq!(unit.terms[0].name.value.as_str(), "km");
                }
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_compound_unit_literal() {
        let file = Parser::new("const G0: Acceleration = 9.80665 m/s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Const(c) => match &c.value.kind {
                ExprKind::UnitLiteral { value, unit } => {
                    assert!((value - 9.80665).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 2);
                    assert_eq!(unit.terms[0].name.value.as_str(), "m");
                    assert_eq!(unit.terms[1].op, MulDivOp::Div);
                    assert_eq!(unit.terms[1].name.value.as_str(), "s");
                    assert_eq!(unit.terms[1].power, Some(2));
                }
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected const"),
        }
    }

    // --- Conversion ---

    #[test]
    fn parse_conversion() {
        let file = Parser::new("node speed_kmh: Velocity = @speed -> km/hour;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { expr, target } => {
                    assert!(
                        matches!(&expr.kind, ExprKind::GraphRef(id) if id.value.as_str() == "speed")
                    );
                    assert_eq!(target.terms.len(), 2);
                    assert_eq!(target.terms[0].name.value.as_str(), "km");
                    assert_eq!(target.terms[1].op, MulDivOp::Div);
                    assert_eq!(target.terms[1].name.value.as_str(), "hour");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_convert_binds_loosely() {
        // @a + @b -> km should be (@a + @b) -> km
        let file = Parser::new("node x: Length = @a + @b -> km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { expr, target } => {
                    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
                    assert_eq!(target.terms[0].name.value.as_str(), "km");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_as_cast() {
        // @v as Vec3<Length, Eci> should parse as AsCast
        let source = r"
            type Eci {}
            type Vec3<D: Dim, F: Type> { x: D, y: D, z: D, }
            node x: Vec3<Length, Eci> = @v as Vec3<Length, Eci>;
        ";
        let file = Parser::new(source).parse_file().unwrap();
        // The node is the 3rd declaration (after type Eci, type Vec3)
        match &file.declarations[2].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::AsCast { expr, target_type } => {
                    assert!(matches!(expr.kind, ExprKind::GraphRef(_)));
                    match &target_type.kind {
                        TypeExprKind::TypeApplication { name, type_args } => {
                            assert_eq!(name.name.as_str(), "Vec3");
                            assert_eq!(type_args.len(), 2);
                        }
                        other => panic!("expected TypeApplication, got {other:?}"),
                    }
                }
                other => panic!("expected AsCast, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_as_cast_binds_loosely() {
        // @a + @b as Vec3<Length, Eci> should be (@a + @b) as Vec3<Length, Eci>
        let source = r"
            type Eci {}
            type Vec3<D: Dim, F: Type> { x: D, y: D, z: D, }
            node x: Vec3<Length, Eci> = @a + @b as Vec3<Length, Eci>;
        ";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[2].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::AsCast { expr, target_type } => {
                    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
                    match &target_type.kind {
                        TypeExprKind::TypeApplication { name, .. } => {
                            assert_eq!(name.name.as_str(), "Vec3");
                        }
                        other => panic!("expected TypeApplication, got {other:?}"),
                    }
                }
                other => panic!("expected AsCast, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    // --- Expression parsing (preserved from Phase 0) ---

    /// Helper: parse a single node declaration and return its expression.
    fn parse_node_expr(input: &str) -> Expr {
        let full = format!("node x: Dimensionless = {input};");
        let file = Parser::new(&full).parse_file().unwrap();
        match file.declarations.into_iter().next().unwrap().kind {
            DeclKind::Node(n) => n.value,
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_arithmetic_precedence() {
        let expr = parse_node_expr("1.0 + 2.0 * 3.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        if let ExprKind::BinOp { rhs, .. } = &expr.kind {
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Mul, .. }));
        }
    }

    #[test]
    fn parse_left_associative_add() {
        let expr = parse_node_expr("1.0 - 2.0 - 3.0");
        if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Sub);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Sub, .. }));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_power_right_assoc() {
        let expr = parse_node_expr("2.0 ^ 3.0 ^ 2.0");
        if let ExprKind::BinOp { op, rhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Pow);
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Pow, .. }));
        } else {
            panic!("expected Pow");
        }
    }

    #[test]
    fn parse_neg_power_precedence() {
        let expr = parse_node_expr("-@x ^ 2.0");
        if let ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } = &expr.kind
        {
            assert!(matches!(
                operand.kind,
                ExprKind::BinOp { op: BinOp::Pow, .. }
            ));
        } else {
            panic!("expected Neg(Pow(...))");
        }
    }

    #[test]
    fn parse_graph_ref() {
        let expr = parse_node_expr("@x + 1.0");
        if let ExprKind::BinOp { lhs, .. } = &expr.kind {
            assert!(matches!(&lhs.kind, ExprKind::GraphRef(id) if id.value.as_str() == "x"));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_const_ref() {
        let expr = parse_node_expr("PI * 2.0");
        if let ExprKind::BinOp { lhs, .. } = &expr.kind {
            assert!(matches!(&lhs.kind, ExprKind::ConstRef(id) if id.value.as_str() == "PI"));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_function_call_one_arg() {
        let expr = parse_node_expr("sqrt(@x)");
        if let ExprKind::FnCall { name, args } = &expr.kind {
            assert_eq!(name.value.as_str(), "sqrt");
            assert_eq!(args.len(), 1);
            assert!(matches!(&args[0].kind, ExprKind::GraphRef(id) if id.value.as_str() == "x"));
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_function_call_two_args() {
        let expr = parse_node_expr("atan2(@a, @b)");
        if let ExprKind::FnCall { name, args } = &expr.kind {
            assert_eq!(name.value.as_str(), "atan2");
            assert_eq!(args.len(), 2);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_function_call_zero_args() {
        let expr = parse_node_expr("foo()");
        if let ExprKind::FnCall { name, args } = &expr.kind {
            assert_eq!(name.value.as_str(), "foo");
            assert_eq!(args.len(), 0);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_if_else() {
        let expr = parse_node_expr("if @x > 0.0 { @x } else { 0.0 }");
        if let ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } = &expr.kind
        {
            assert!(matches!(
                condition.kind,
                ExprKind::BinOp { op: BinOp::Gt, .. }
            ));
            assert!(matches!(
                &then_branch.kind,
                ExprKind::GraphRef(id) if id.value.as_str() == "x"
            ));
            assert!(matches!(else_branch.kind, ExprKind::Number(_)));
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn parse_nested_parens() {
        let expr = parse_node_expr("(1.0 + 2.0) * 3.0");
        if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Mul);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        } else {
            panic!("expected Mul");
        }
    }

    #[test]
    fn parse_boolean_and() {
        let expr = parse_node_expr("@a > 0.0 && @b > 0.0");
        if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::And);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
        } else {
            panic!("expected And");
        }
    }

    #[test]
    fn parse_boolean_or() {
        let expr = parse_node_expr("@a > 0.0 || @b > 0.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Or, .. }));
    }

    #[test]
    fn parse_unary_neg() {
        let expr = parse_node_expr("-1.0");
        assert!(matches!(
            expr.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    #[test]
    fn parse_unary_not() {
        let expr = parse_node_expr("!true");
        assert!(matches!(
            expr.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn parse_complex_expression() {
        let expr = parse_node_expr("@v_exhaust * ln(@mass_ratio)");
        if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::Mul);
            assert!(
                matches!(&lhs.kind, ExprKind::GraphRef(id) if id.value.as_str() == "v_exhaust")
            );
            assert!(
                matches!(&rhs.kind, ExprKind::FnCall { name, .. } if name.value.as_str() == "ln")
            );
        } else {
            panic!("expected Mul");
        }
    }

    #[test]
    fn parse_comparison_eq() {
        let expr = parse_node_expr("@x == 1.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Eq, .. }));
    }

    #[test]
    fn parse_comparison_ne() {
        let expr = parse_node_expr("@x != 1.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Ne, .. }));
    }

    // --- Error tests ---

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

    // --- Milestone: orbital.gcl syntax ---

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
                DeclKind::Use(_) => "<use>",
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

    // --- Phase 2 type declaration tests ---

    #[test]
    fn parse_type_decl_single_field() {
        let source = "type Orbit { sma: Length }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Orbit");
                // Struct sugar desugars to single variant with same name
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
        // Empty type (marker type) — zero variants
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
        // UPPER_SNAKE_CASE should be rejected (not PascalCase)
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
        // Field type is a composite dimension expression
        let source = "type TransferResult { dv: Length / Time }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.variants.len(), 1);
                assert_eq!(t.variants[0].fields.len(), 1);
                assert_eq!(t.variants[0].fields[0].name.value.as_str(), "dv");
                // The type_ann should be a DimExpr with division
                match &t.variants[0].fields[0].type_ann.kind {
                    TypeExprKind::DimExpr(_) => {} // ok
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
                assert_eq!(t.variants.len(), 1); // struct sugar
                assert_eq!(t.variants[0].fields.len(), 3);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_no_generics_empty() {
        // Empty marker type without generics
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
        // Multi-variant type with generic params
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
                // First param: D: Dim (no default)
                assert_eq!(t.generic_params[0].name.value.as_str(), "D");
                assert_eq!(t.generic_params[0].constraint, GenericConstraint::Dim);
                assert!(t.generic_params[0].default.is_none());
                // Second param: F: Type = Unframed
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
        // All params without defaults — default field should be None
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
    fn parse_type_decl_derive_clause() {
        let source = "type Vec3<D: Dim, F: Type> derive(Add, Sub, Neg) { x: D, y: D, z: D }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.value.as_str(), "Vec3");
                assert_eq!(t.generic_params.len(), 2);
                assert_eq!(t.derives.len(), 3);
                assert_eq!(t.derives[0].value, DeriveOp::Add);
                assert_eq!(t.derives[1].value, DeriveOp::Sub);
                assert_eq!(t.derives[2].value, DeriveOp::Neg);
                assert_eq!(t.variants.len(), 1);
            }
            _ => panic!("expected type declaration"),
        }
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

    // --- TypeApplication and generic struct construction tests ---

    /// Helper to extract the dimension name from a single-term `DimExpr` type expression.
    fn dim_expr_name(te: &TypeExpr) -> &str {
        match &te.kind {
            TypeExprKind::DimExpr(dim) => {
                assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
                dim.terms[0].term.name.name.as_str()
            }
            other => panic!("expected DimExpr, got {other:?}"),
        }
    }

    #[test]
    fn parse_type_application_in_annotation() {
        // Type annotation with type args: `Vec3<Length, ECI>`
        let source = "param v: Vec3<Length, ECI> = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.name.as_str(), "Vec3");
                    assert_eq!(type_args.len(), 2);
                    assert_eq!(dim_expr_name(&type_args[0]), "Length");
                    assert_eq!(dim_expr_name(&type_args[1]), "ECI");
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_application_single_arg() {
        let source = "param t: Timestamp<UTC> = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.name.as_str(), "Timestamp");
                    assert_eq!(type_args.len(), 1);
                    assert_eq!(dim_expr_name(&type_args[0]), "UTC");
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_non_generic_type_still_works() {
        // Non-generic type annotation should still parse as DimExpr, not TypeApplication
        let source = "param v: Length = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::DimExpr(_)));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_generic_struct_construction() {
        let source = "node v: Vec3<Length, ECI> = Vec3<Length, ECI> { x: 1.0, y: 2.0, z: 3.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction {
                    type_name,
                    type_args,
                    fields,
                } => {
                    assert_eq!(type_name.value.as_str(), "Vec3");
                    assert_eq!(type_args.len(), 2);
                    assert_eq!(dim_expr_name(&type_args[0]), "Length");
                    assert_eq!(dim_expr_name(&type_args[1]), "ECI");
                    assert_eq!(fields.len(), 3);
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_non_generic_struct_construction_still_works() {
        // Non-generic struct construction: no type args
        let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction {
                    type_name,
                    type_args,
                    fields,
                } => {
                    assert_eq!(type_name.value.as_str(), "TransferResult");
                    assert!(type_args.is_empty());
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn is_pascal_case_examples() {
        assert!(is_pascal_case("TransferResult"));
        assert!(is_pascal_case("Orbit"));
        assert!(is_pascal_case("Ab"));
        assert!(!is_pascal_case("ORBIT"));
        assert!(!is_pascal_case("UPPER_SNAKE"));
        assert!(!is_pascal_case("orbit"));
        assert!(!is_pascal_case("lower_snake"));
        assert!(!is_pascal_case(""));
    }

    // --- Phase 2 block / let / LocalRef tests ---

    #[test]
    fn parse_block_simple() {
        let source = "node x: Dimensionless = { let a = 1.0; a + 2.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, expr } => {
                    assert_eq!(stmts.len(), 1);
                    assert_eq!(stmts[0].name.name, "a");
                    assert!(stmts[0].type_ann.is_none());
                    assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_block_multiple_lets() {
        let source = "node x: Dimensionless = { let r1 = @a + @b; let r2 = @c; r1 + r2 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, expr } => {
                    assert_eq!(stmts.len(), 2);
                    assert_eq!(stmts[0].name.name, "r1");
                    assert_eq!(stmts[1].name.name, "r2");
                    // Final expression references two LocalRefs via BinOp
                    assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_block_let_with_type_ann() {
        let source = "node x: Dimensionless = { let a: Dimensionless = 1.0; a };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, .. } => {
                    assert_eq!(stmts.len(), 1);
                    assert!(stmts[0].type_ann.is_some());
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_block_no_lets() {
        // Block with no let bindings, just an expression
        let source = "node x: Dimensionless = { 1.0 + 2.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, .. } => {
                    assert_eq!(stmts.len(), 0);
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_local_ref() {
        // A bare lowercase identifier in expression position parses as LocalRef
        let source = "node x: Dimensionless = { let a = 1.0; a };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { expr, .. } => {
                    assert!(matches!(&expr.kind, ExprKind::LocalRef(ident) if ident.name == "a"));
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    // --- Phase 2 struct construction and field access tests ---

    #[test]
    fn parse_struct_construction_explicit_fields() {
        let source = "node t: Dimensionless = TransferResult { dv1: @a + @b, dv2: @c };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction {
                    type_name, fields, ..
                } => {
                    assert_eq!(type_name.value.as_str(), "TransferResult");
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name.value.as_str(), "dv1");
                    assert!(fields[0].value.is_some());
                    assert_eq!(fields[1].name.value.as_str(), "dv2");
                    assert!(fields[1].value.is_some());
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_struct_construction_shorthand() {
        let source =
            "node t: Dimensionless = { let dv1 = @a; let dv2 = @b; TransferResult { dv1, dv2 } };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { expr, .. } => match &expr.kind {
                    ExprKind::StructConstruction {
                        type_name, fields, ..
                    } => {
                        assert_eq!(type_name.value.as_str(), "TransferResult");
                        assert_eq!(fields.len(), 2);
                        // Shorthand: value is None
                        assert!(fields[0].value.is_none());
                        assert!(fields[1].value.is_none());
                    }
                    other => panic!("expected StructConstruction, got {other:?}"),
                },
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_struct_construction_trailing_comma() {
        let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0, };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction { fields, .. } => {
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_field_access() {
        let source = "node x: Dimensionless = @transfer.dv1;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::FieldAccess { expr, field } => {
                    assert!(
                        matches!(&expr.kind, ExprKind::GraphRef(ident) if ident.value.as_str() == "transfer")
                    );
                    assert_eq!(field.value.as_str(), "dv1");
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_chained_field_access() {
        let source = "node x: Dimensionless = @mission.transfer.dv1;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::FieldAccess { expr, field } => {
                    assert_eq!(field.value.as_str(), "dv1");
                    // Inner should be another FieldAccess
                    match &expr.kind {
                        ExprKind::FieldAccess {
                            expr: inner,
                            field: mid_field,
                        } => {
                            assert_eq!(mid_field.value.as_str(), "transfer");
                            assert!(
                                matches!(&inner.kind, ExprKind::GraphRef(ident) if ident.value.as_str() == "mission")
                            );
                        }
                        other => panic!("expected inner FieldAccess, got {other:?}"),
                    }
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_field_access_in_arithmetic() {
        // Field access should bind tighter than arithmetic
        let source = "node x: Dimensionless = @t.dv1 + @t.dv2;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::BinOp { op, lhs, rhs } => {
                    assert!(matches!(op, BinOp::Add));
                    assert!(matches!(&lhs.kind, ExprKind::FieldAccess { .. }));
                    assert!(matches!(&rhs.kind, ExprKind::FieldAccess { .. }));
                }
                other => panic!("expected BinOp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    // Phase 3: fn declaration tests

    #[test]
    fn parse_fn_short_form() {
        let source = "fn double(x: Dimensionless) -> Dimensionless = x * 2.0;";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "double");
                assert!(f.generic_params.is_empty());
                assert_eq!(f.params.len(), 1);
                assert_eq!(f.params[0].name.name, "x");
                assert!(matches!(f.body, FnBody::Short(_)));
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_block_form() {
        let source = "fn add_one(x: Dimensionless) -> Dimensionless { let one = 1.0; x + one }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "add_one");
                match &f.body {
                    FnBody::Block { stmts, expr } => {
                        assert_eq!(stmts.len(), 1);
                        assert_eq!(stmts[0].name.name, "one");
                        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                    }
                    FnBody::Short(_) => panic!("expected block body"),
                }
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_with_generics() {
        let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "lerp");
                assert_eq!(f.generic_params.len(), 1);
                assert_eq!(f.generic_params[0].name.value.as_str(), "D");
                assert_eq!(f.generic_params[0].constraint, GenericConstraint::Dim);
                assert_eq!(f.params.len(), 3);
                assert_eq!(f.params[0].name.name, "a");
                assert_eq!(f.params[1].name.name, "b");
                assert_eq!(f.params[2].name.name, "t");
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_multiple_generics() {
        let source = "fn convert<A: Dim, B: Dim>(x: A, y: B) -> A = x;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.generic_params.len(), 2);
                assert_eq!(f.generic_params[0].name.value.as_str(), "A");
                assert_eq!(f.generic_params[1].name.value.as_str(), "B");
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_zero_args() {
        let source = "fn pi_val() -> Dimensionless = PI;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "pi_val");
                assert!(f.params.is_empty());
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_trailing_comma() {
        let source = "fn add(x: Dimensionless, y: Dimensionless,) -> Dimensionless = x + y;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.params.len(), 2);
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_dim_expr_type() {
        let source = "fn speed(d: Length, t: Time) -> Length / Time = d / t;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.params.len(), 2);
                // Return type is a compound dim expr
                assert!(matches!(f.return_type.kind, TypeExprKind::DimExpr(_)));
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_block_no_lets() {
        let source = "fn identity(x: Dimensionless) -> Dimensionless { x }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => match &f.body {
                FnBody::Block { stmts, .. } => assert!(stmts.is_empty()),
                FnBody::Short(_) => panic!("expected block body"),
            },
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_mixed_with_other_decls() {
        let source = r"
            const TWO: Dimensionless = 2.0;
            fn double(x: Dimensionless) -> Dimensionless = x * TWO;
            param val: Dimensionless = 5.0;
            node result: Dimensionless = double(@val);
        ";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 4);
        assert!(matches!(file.declarations[0].kind, DeclKind::Const(_)));
        assert!(matches!(file.declarations[1].kind, DeclKind::Fn(_)));
        assert!(matches!(file.declarations[2].kind, DeclKind::Param(_)));
        assert!(matches!(file.declarations[3].kind, DeclKind::Node(_)));
    }

    // --- Phase 5: Indexed Values ---

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
    fn parse_indexed_type() {
        let source = "param dv: Velocity[Maneuver] = 1.0 m/s;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "dv");
                match &p.type_ann.kind {
                    TypeExprKind::Indexed { base, indexes } => {
                        assert!(matches!(base.kind, TypeExprKind::DimExpr(_)));
                        assert_eq!(indexes.len(), 1);
                        assert_eq!(indexes[0].name, "Maneuver");
                    }
                    other => panic!("expected Indexed type, got {other:?}"),
                }
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_multi_indexed_type() {
        let source = "param matrix: Dimensionless[Row, Col] = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::Indexed { indexes, .. } => {
                    assert_eq!(indexes.len(), 2);
                    assert_eq!(indexes[0].name, "Row");
                    assert_eq!(indexes[1].name, "Col");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_for_comprehension() {
        let source = "node fuel: Mass[Maneuver] = for m: Maneuver { 1.0 kg };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { bindings, body } => {
                    assert_eq!(bindings.len(), 1);
                    assert_eq!(bindings[0].var.name, "m");
                    assert_eq!(bindings[0].index.value.as_str(), "Maneuver");
                    assert!(matches!(body.kind, ExprKind::UnitLiteral { .. }));
                }
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_for_multi_binding() {
        let source = "node x: Dimensionless[Row, Col] = for r: Row, c: Col { 0.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { bindings, .. } => {
                    assert_eq!(bindings.len(), 2);
                    assert_eq!(bindings[0].var.name, "r");
                    assert_eq!(bindings[0].index.value.as_str(), "Row");
                    assert_eq!(bindings[1].var.name, "c");
                    assert_eq!(bindings[1].index.value.as_str(), "Col");
                }
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_index_access_with_variant() {
        let source = "node x: Velocity = @dv[Maneuver::Departure];";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::IndexAccess { expr, args } => {
                    assert!(matches!(expr.kind, ExprKind::GraphRef(_)));
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        IndexArg::Variant { index, variant } => {
                            assert_eq!(index.value.as_str(), "Maneuver");
                            assert_eq!(variant.value.as_str(), "Departure");
                        }
                        other @ IndexArg::Var(_) => panic!("expected Variant, got {other:?}"),
                    }
                }
                other => panic!("expected IndexAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_index_access_with_loop_var() {
        let source = "node y: Velocity[Maneuver] = for m: Maneuver { @dv[m] };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { body, .. } => match &body.kind {
                    ExprKind::IndexAccess { args, .. } => {
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            IndexArg::Var(ident) => assert_eq!(ident.name, "m"),
                            other @ IndexArg::Variant { .. } => {
                                panic!("expected Var, got {other:?}")
                            }
                        }
                    }
                    other => panic!("expected IndexAccess, got {other:?}"),
                },
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_map_literal() {
        let source = "param dv: Velocity[Maneuver] = { Maneuver::Departure: 2.0 km/s, Maneuver::Correction: 0.05 km/s };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.kind {
                ExprKind::MapLiteral { entries } => {
                    assert_eq!(entries.len(), 2);
                    assert_eq!(entries[0].index.value.as_str(), "Maneuver");
                    assert_eq!(entries[0].variant.value.as_str(), "Departure");
                    assert_eq!(entries[1].index.value.as_str(), "Maneuver");
                    assert_eq!(entries[1].variant.value.as_str(), "Correction");
                }
                other => panic!("expected MapLiteral, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_scan_expression() {
        let source = "node cum: Velocity[Maneuver] = scan(@dv, 0.0 m/s, |acc, val| acc + val);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Scan {
                    acc_name, val_name, ..
                } => {
                    assert_eq!(acc_name.name, "acc");
                    assert_eq!(val_name.name, "val");
                }
                other => panic!("expected Scan, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_unfold_expression() {
        let source =
            "node x: Dimensionless[TimeStep] = unfold(1.0, |prev_t, t| { @x[prev_t] * 2.0 });";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Unfold {
                    prev_name,
                    curr_name,
                    ..
                } => {
                    assert_eq!(prev_name.name, "prev_t");
                    assert_eq!(curr_name.name, "t");
                }
                other => panic!("expected Unfold, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_generic_fn_with_index_constraint() {
        let source = "fn total<D: Dim, I: Index>(values: D) -> D = values;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.generic_params.len(), 2);
                assert_eq!(f.generic_params[0].name.value.as_str(), "D");
                assert_eq!(f.generic_params[0].constraint, GenericConstraint::Dim);
                assert_eq!(f.generic_params[1].name.value.as_str(), "I");
                assert_eq!(f.generic_params[1].constraint, GenericConstraint::Index);
            }
            _ => panic!("expected fn"),
        }
    }

    // --- parse_single_expr tests ---

    #[test]
    fn single_expr_unit_literal() {
        let expr = Parser::new("450.0 s").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
    }

    #[test]
    fn single_expr_integer_with_unit_errors() {
        let result = Parser::new("450 s").parse_single_expr();
        assert!(
            result.is_err(),
            "integer literal with unit should be an error"
        );
    }

    #[test]
    fn single_expr_number() {
        let expr = Parser::new("3.0").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::Number(n) if (n - 3.0).abs() < f64::EPSILON));
    }

    #[test]
    fn single_expr_compound_unit() {
        let expr = Parser::new("9.80665 m/s^2").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
    }

    #[test]
    fn single_expr_arithmetic_with_const() {
        let expr = Parser::new("2.0 * PI").parse_single_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
    }

    #[test]
    fn single_expr_trailing_tokens_error() {
        let result = Parser::new("450.0 s; extra").parse_single_expr();
        assert!(result.is_err());
    }

    #[test]
    fn parse_use_no_alias() {
        let file = Parser::new(r#"use "./helper.gcl" { x, Y };"#)
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        let DeclKind::Use(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        assert_eq!(u.path, "./helper.gcl");
        assert_eq!(u.names.len(), 2);
        assert_eq!(u.names[0].name.name, "x");
        assert!(u.names[0].alias.is_none());
        assert_eq!(u.names[0].local_name(), "x");
        assert_eq!(u.names[1].name.name, "Y");
        assert!(u.names[1].alias.is_none());
        assert_eq!(u.names[1].local_name(), "Y");
    }

    #[test]
    fn parse_use_with_alias() {
        let file = Parser::new(r#"use "./helper.gcl" { x as y };"#)
            .parse_file()
            .unwrap();
        let DeclKind::Use(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        assert_eq!(u.names.len(), 1);
        assert_eq!(u.names[0].name.name, "x");
        assert_eq!(u.names[0].alias.as_ref().unwrap().name, "y");
        assert_eq!(u.names[0].local_name(), "y");
    }

    #[test]
    fn parse_use_mixed_alias() {
        let file = Parser::new(r#"use "./f.gcl" { x, Y as Z, w };"#)
            .parse_file()
            .unwrap();
        let DeclKind::Use(u) = &file.declarations[0].kind else {
            panic!("expected Use");
        };
        assert_eq!(u.names.len(), 3);
        assert_eq!(u.names[0].name.name, "x");
        assert!(u.names[0].alias.is_none());
        assert_eq!(u.names[1].name.name, "Y");
        assert_eq!(u.names[1].alias.as_ref().unwrap().name, "Z");
        assert_eq!(u.names[1].local_name(), "Z");
        assert_eq!(u.names[2].name.name, "w");
        assert!(u.names[2].alias.is_none());
    }

    #[test]
    fn parse_use_alias_missing_name_error() {
        let result = Parser::new(r#"use "./f.gcl" { x as };"#).parse_file();
        assert!(result.is_err());
    }

    // --- Attribute tests ---

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
        assert_eq!(attr.args[0].name, "pressure_safe");
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
        assert_eq!(attr.args[0].name, "pressure_safe");
        assert_eq!(attr.args[1].name, "temp_bounded");
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
        // Declaration span should start at '#' (offset 0), not at 'node'
        assert_eq!(file.declarations[0].span.offset, 0);
    }
}
