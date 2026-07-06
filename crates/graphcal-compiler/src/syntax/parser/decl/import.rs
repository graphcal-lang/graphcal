use crate::syntax::ast::DeclKind;
use crate::syntax::ast::Declaration;
use crate::syntax::ast::ImportKind;
use crate::syntax::ast::ModulePath;
use crate::syntax::ast::Visibility;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::NameAtom;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse an import declaration:
    ///   `import nasa.rocket;`
    ///   `import nasa.rocket as nr;`
    ///   `import nasa.rocket.{Orbit, compute_thrust as ct};`
    pub(super) fn parse_import_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Import)?;

        // `import plugin "path" as alias { ... }` — the contextual keyword
        // `plugin` followed by a string literal selects the extern-plugin
        // form; `import plugin.foo;` (a module path whose first segment is
        // spelled `plugin`) stays an ordinary import.
        if let Some((Token::Ident, ident_span)) =
            self.lexer.peek_with_span().map(|(tok, span)| (*tok, span))
            && self.lexer.slice_at(ident_span) == "plugin"
            && self.lexer.peek_second() == Some(&Token::StringLiteral)
        {
            return self.parse_plugin_import_decl(start_span);
        }

        let path = self.parse_module_path()?;

        // Reject param bindings on `import` — use `include` for DAG instantiation.
        if self.lexer.peek() == Some(&Token::LParen) {
            let (_, span) = self.advance()?;
            return Err(self.unexpected_token(
                "`{`, `as`, or `;` after path (`import` cannot have param bindings; use `include` for DAG instantiation)",
                "(",
                span,
            ));
        }

        let (kind, end_span) = self.parse_import_tail("`.{`, `as`, or `;` after path", true)?;
        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Import(crate::syntax::ast::ImportDecl {
                visibility: Visibility::Private,
                path,
                kind,
            }),
            span,
        })
    }

    /// Parse an extern plugin import (issue #943):
    ///   `import plugin "plugins/coolprop.wasm" as fluids {`
    ///   `    fn density(p: Pressure, t: Temperature) -> Density;`
    ///   `    fn smooth<D: Dim>(x: D, window: Dimensionless) -> D;`
    ///   `}`
    ///
    /// The caller has consumed `import` and verified the next tokens are the
    /// contextual keyword `plugin` followed by a string literal.
    fn parse_plugin_import_decl(
        &mut self,
        start_span: crate::syntax::span::Span,
    ) -> Result<Declaration, ParseError> {
        self.advance()?; // consume the contextual `plugin` keyword

        let (_, path_span) = self.expect(Token::StringLiteral)?;
        let raw = self.lexer.slice_at(path_span);
        // Strip surrounding quotes.
        let path = crate::syntax::plugin::PluginPath::new(&raw[1..raw.len() - 1]);

        // The alias is mandatory: extern functions are only callable
        // qualified through it.
        self.expect(Token::As)?;
        let alias = self.parse_any_ident()?.into_spanned::<ModuleAliasName>();

        self.expect(Token::LBrace)?;
        let mut functions = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            functions.push(self.parse_extern_fn_decl()?);
        }
        let (_, end_span) = self.expect(Token::RBrace)?;

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::PluginImport(crate::syntax::ast::PluginImportDecl {
                path: crate::syntax::span::Spanned::new(path, path_span),
                alias,
                functions,
            }),
            span: start_span.merge(end_span),
        })
    }

    /// Parse one extern function signature inside an `import plugin` block:
    ///   `fn smooth<D: Dim>(x: D, window: Dimensionless) -> D;`
    fn parse_extern_fn_decl(&mut self) -> Result<crate::syntax::ast::ExternFnDecl, ParseError> {
        // `fn` is a contextual keyword valid only inside plugin blocks.
        let fn_ident = match self.lexer.peek_with_span().map(|(tok, span)| (*tok, span)) {
            Some((Token::Ident, span)) if self.lexer.slice_at(span) == "fn" => {
                self.advance()?;
                span
            }
            Some((tok, _)) => {
                let tok_str = tok.to_string();
                let (_, span) = self.advance()?;
                return Err(self.unexpected_token(
                    "`fn` to declare an extern function, or `}` to close the plugin block",
                    &tok_str,
                    span,
                ));
            }
            None => {
                return Err(self.unexpected_eof("`fn` to declare an extern function, or `}`"));
            }
        };

        let name = self
            .parse_any_ident()?
            .into_spanned::<crate::syntax::function_name::FnName>();

        // Optional explicit generic binders: `<D1: Dim, D2: Dim>`. The
        // `name: constraint` form mirrors `generic_params` on `type`
        // declarations; extern signatures support the `Dim` constraint.
        let mut dim_vars = Vec::new();
        if self.lexer.peek() == Some(&Token::Lt) {
            self.advance()?;
            loop {
                let var = self.parse_any_ident()?;
                self.expect(Token::Colon)?;
                let constraint = self.parse_any_ident()?;
                match constraint.name.as_str() {
                    "Dim" => dim_vars.push(crate::syntax::span::Spanned::new(
                        crate::syntax::dimension::DimVarName::from_atom(var.name),
                        var.span,
                    )),
                    other => {
                        return Err(self.unexpected_token(
                            "`Dim` as the binder constraint (extern signatures support dimension variables)",
                            other,
                            constraint.span,
                        ));
                    }
                }
                match self.lexer.peek() {
                    Some(Token::Comma) => {
                        self.advance()?;
                        if self.lexer.peek() == Some(&Token::Gt) {
                            break; // trailing comma
                        }
                    }
                    _ => break,
                }
            }
            self.expect(Token::Gt)?;
        }

        self.expect(Token::LParen)?;
        let params = self.parse_comma_separated(Token::RParen, |p| {
            let name = p
                .parse_any_ident()?
                .into_spanned::<crate::syntax::function_name::FnParamName>();
            p.expect(Token::Colon)?;
            let type_ann = p.parse_type_expr()?;
            Ok(crate::syntax::ast::ExternFnParam { name, type_ann })
        })?;
        self.expect(Token::RParen)?;

        self.expect(Token::Arrow)?;
        let result = self.parse_type_expr()?;
        let (_, end_span) = self.expect(Token::Semicolon)?;

        Ok(crate::syntax::ast::ExternFnDecl {
            name,
            dim_vars,
            params,
            result,
            span: fn_ident.merge(end_span),
        })
    }

    /// Parse an include declaration:
    ///   `include nasa.rocket.compute_thrust(args);`
    ///   `include nasa.rocket.compute_thrust(args) as ct;`
    ///   `include nasa.rocket.compute_thrust(args).{thrust};`
    pub(super) fn parse_include_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Include)?;

        let path = self.parse_module_path()?;

        // Param bindings are required for `include`.
        let param_bindings = if self.lexer.peek() == Some(&Token::LParen) {
            self.parse_import_param_bindings()?
        } else {
            let found = self
                .lexer
                .peek()
                .map_or_else(|| "end of file".to_string(), ToString::to_string);
            return Err(self.unexpected_token("`(` to begin param bindings", &found, path.span));
        };

        let (kind, end_span) =
            self.parse_import_tail("`.{`, `as`, or `;` after param bindings", false)?;
        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Include(crate::syntax::ast::IncludeDecl {
                visibility: Visibility::Private,
                path,
                param_bindings,
                kind,
            }),
            span,
        })
    }

    /// Parse a dot-separated module path: `IDENT { "." IDENT }`.
    ///
    /// Stops before any `.` that is *not* immediately followed by an identifier;
    /// such a `.` belongs to the brace-list tail (`.{ ... }`).
    pub(in crate::syntax::parser) fn parse_module_path(
        &mut self,
    ) -> Result<ModulePath, ParseError> {
        let first = self.parse_any_ident()?;
        let path_start = first.span;
        let mut rest = Vec::new();

        while self.lexer.peek() == Some(&Token::Dot)
            && self.lexer.peek_second() == Some(&Token::Ident)
        {
            self.advance()?; // consume `.`
            let seg = self.parse_any_ident()?;
            rest.push(seg);
        }

        let segments = crate::syntax::non_empty::NonEmpty::new(first, rest);
        let path_end = segments.last().span;
        Ok(ModulePath {
            segments,
            span: path_start.merge(path_end),
        })
    }

    /// Parse the trailing portion of an import or include declaration:
    ///   `;`                    → bare module form
    ///   `as IDENT ;`           → aliased form
    ///   `.{ items, ... } ;`    → brace-list form
    fn parse_import_tail(
        &mut self,
        hint: &str,
        allow_type_marker: bool,
    ) -> Result<(ImportKind, crate::syntax::span::Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::Dot) => {
                // Brace-list form: `.{ X, Y as Z }`
                self.advance()?; // consume `.`
                if self.lexer.peek() != Some(&Token::LBrace) {
                    let found = self
                        .lexer
                        .peek()
                        .map_or_else(|| "end of file".to_string(), ToString::to_string);
                    let (_, span) = self.advance()?;
                    return Err(self.unexpected_token(
                        "`{` to begin a brace-list selector after `.`",
                        &found,
                        span,
                    ));
                }
                let names = self.parse_import_brace_list(allow_type_marker)?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((ImportKind::Selective(names), end_span))
            }
            Some(Token::As) => {
                self.lexer.next_token(); // consume `as`
                let alias = self.parse_any_ident()?.into_spanned::<ModuleAliasName>();
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((ImportKind::Module { alias: Some(alias) }, end_span))
            }
            Some(Token::Semicolon) => {
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((ImportKind::Module { alias: None }, end_span))
            }
            Some(tok) => {
                let tok_str = tok.to_string();
                let (_, span) = self.advance()?;
                Err(self.unexpected_token(hint, &tok_str, span))
            }
            None => Err(self.unexpected_eof(hint)),
        }
    }

    /// Parse the `{ X, Y as Z, ... }` body of a brace-list selector.
    fn parse_import_brace_list(
        &mut self,
        allow_type_marker: bool,
    ) -> Result<Vec<crate::syntax::ast::ImportItem>, ParseError> {
        self.expect(Token::LBrace)?;

        let names = self.parse_comma_separated(Token::RBrace, |p| {
            // Collect any leading attributes on this import item.
            let mut item_attributes = Vec::new();
            while p.lexer.peek() == Some(&Token::Hash) {
                item_attributes.push(p.parse_attribute()?);
            }

            // Optional `pub` prefix marks the item for re-export (issue #452).
            // `pub(bind)` is rejected — re-exports are use-sites, not declarations.
            let is_pub = if p.lexer.peek() == Some(&Token::Pub) {
                let (_, pub_span) = p.advance()?;
                if p.lexer.peek() == Some(&Token::LParen) {
                    return Err(p.unexpected_token(
                        "an identifier (`pub(bind)` is not allowed on import/include items — use `pub`)",
                        "(",
                        pub_span,
                    ));
                }
                true
            } else {
                false
            };

            let namespace = if p.lexer.peek() == Some(&Token::Type) {
                let (_, type_span) = p.advance()?;
                if !allow_type_marker {
                    return Err(p.unexpected_token(
                        "an identifier (`type` import items are only allowed in `import`, not `include`)",
                        "type",
                        type_span,
                    ));
                }
                crate::syntax::ast::ImportItemNamespace::Type
            } else {
                crate::syntax::ast::ImportItemNamespace::Default
            };

            // Accept any identifier (imports can be any casing).
            let (name_str, name_span) = match p.lexer.next_token() {
                Some((Token::Ident, span)) => (
                    NameAtom::new_unchecked_for_parser(p.lexer.slice_at(span).to_string()),
                    span,
                ),
                Some((tok, span)) => {
                    return Err(p.unexpected_token("an identifier", &tok.to_string(), span));
                }
                None => {
                    return Err(p.unexpected_eof("an identifier or `}`"));
                }
            };

            // Optional `as` alias.
            let alias = if p.lexer.peek() == Some(&Token::As) {
                p.lexer.next_token(); // consume `as`
                match p.lexer.next_token() {
                    Some((Token::Ident, alias_span)) => {
                        let alias_str = NameAtom::new_unchecked_for_parser(
                            p.lexer.slice_at(alias_span).to_string(),
                        );
                        Some(crate::syntax::ast::Ident {
                            name: alias_str,
                            span: alias_span,
                        })
                    }
                    Some((tok, span)) => {
                        return Err(p.unexpected_token(
                            "an identifier after `as`",
                            &tok.to_string(),
                            span,
                        ));
                    }
                    None => {
                        return Err(p.unexpected_eof("an identifier after `as`"));
                    }
                }
            } else {
                None
            };

            Ok(crate::syntax::ast::ImportItem {
                attributes: item_attributes,
                is_pub,
                namespace,
                name: crate::syntax::ast::Ident {
                    name: name_str,
                    span: name_span,
                },
                alias,
            })
        })?;

        self.expect(Token::RBrace)?;
        Ok(names)
    }

    /// Parse the `(name: expr, ...)` param bindings of an include declaration.
    pub(in crate::syntax::parser) fn parse_import_param_bindings(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::ParamBinding>, ParseError> {
        self.expect(Token::LParen)?;

        let bindings = self.parse_comma_separated(Token::RParen, |p| {
            let name_ident = p.parse_any_ident()?;
            let name_span = name_ident.span;
            p.expect(Token::Colon)?;
            let value = p.parse_expr()?;
            let binding_span = name_span.merge(value.span);
            Ok(crate::syntax::ast::ParamBinding {
                name: name_ident,
                value,
                span: binding_span,
            })
        })?;

        self.expect(Token::RParen)?;
        Ok(bindings)
    }
}
