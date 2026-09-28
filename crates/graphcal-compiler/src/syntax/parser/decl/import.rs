use crate::syntax::ast::DeclKind;
use crate::syntax::ast::Declaration;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::ast::ImportDecl;
use crate::syntax::ast::ImportKind;
use crate::syntax::ast::ModulePath;
use crate::syntax::ast::Visibility;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::span::Spanned;
use crate::syntax::token::{ContextualKeyword, Token};

use super::super::{Expected, Found, ParseError, ParseErrorKind, Parser};
use super::visibility::VisibilityPrefix;

impl Parser<'_> {
    /// Parse an import declaration:
    ///   `import nasa.rocket;`
    ///   `import nasa.rocket as nr;`
    ///   `import nasa.rocket::{type Orbit, compute_thrust as ct};`
    pub(super) fn parse_import_decl(
        &mut self,
        prefix: VisibilityPrefix,
    ) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Import)?;

        // `import plugin "path" as alias { ... }` — the contextual keyword
        // `plugin` followed by a string literal selects the extern-plugin
        // form; `import plugin.foo;` (a module path whose first segment is
        // spelled `plugin`) stays an ordinary import.
        if self.lexer.peek() == Some(&Token::ContextualKeyword(ContextualKeyword::Plugin))
            && self.lexer.peek_second() == Some(&Token::StringLiteral)
        {
            let decl = self.parse_plugin_import_decl(start_span)?;
            prefix.accept_non_dag_import()?;
            return Ok(decl);
        }

        let path = self.parse_module_path()?;

        // Reject param bindings on `import` — use `include` for DAG instantiation.
        if self.lexer.peek() == Some(&Token::LParen) {
            let (token, span) = self.advance()?;
            return Err(Self::unexpected(
                Expected::ImportTailWithoutBindings,
                token,
                span,
            ));
        }

        let (kind, end_span) = self.parse_import_tail(Expected::ImportTail)?;
        let span = start_span.merge(end_span);
        let import = match kind {
            ImportKind::Module { alias } => ImportDecl::Module {
                visibility: prefix.accept_public(Expected::NonBindableVisibility)?,
                path,
                alias,
            },
            ImportKind::Selective(items) => {
                prefix.accept_non_dag_import()?;
                ImportDecl::Selective { path, items }
            }
        };

        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Import(import),
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
        let alias: Spanned<ModuleAliasName> = self.parse_any_ident()?.classify();

        self.expect(Token::LBrace)?;
        let mut functions = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            functions.push(self.parse_extern_fn_decl()?);
        }
        let (_, end_span) = self.expect(Token::RBrace)?;

        Ok(Declaration {
            doc: None,
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
            Some((Token::ContextualKeyword(ContextualKeyword::Fn), span)) => {
                self.advance()?;
                span
            }
            Some(_) | None => {
                return Err(self.unexpected_next(Expected::ExternFunctionOrBlockClose));
            }
        };

        let name = self
            .parse_any_ident()?
            .classify::<crate::syntax::function_name::FnNameNamespace>();

        // Optional explicit generic binders: `<D: Dim, I: Index>`. The
        // `name: constraint` form mirrors `generic_params` on `type`
        // declarations; extern signatures support `Dim` and `Index`.
        let mut generics = Vec::new();
        if self.lexer.peek() == Some(&Token::Lt) {
            self.advance()?;
            loop {
                let var = self.parse_any_ident()?;
                self.expect(Token::Colon)?;
                let constraint = self.parse_any_ident()?;
                let binder = match GenericConstraint::parse(constraint.name.as_str()) {
                    Some(GenericConstraint::Dim) => crate::syntax::ast::ExternGenericBinder::Dim(
                        crate::syntax::span::Spanned::new(
                            crate::syntax::dimension::DimVarName::classify(var.name.into_atom()),
                            var.span,
                        ),
                    ),
                    Some(GenericConstraint::Index) => {
                        crate::syntax::ast::ExternGenericBinder::Index(
                            crate::syntax::span::Spanned::new(
                                crate::syntax::index_name::IndexVarName::classify(
                                    var.name.into_atom(),
                                ),
                                var.span,
                            ),
                        )
                    }
                    Some(GenericConstraint::Nat | GenericConstraint::Type) | None => {
                        return Err(Self::unexpected_token(
                            Expected::ExternBinderConstraint,
                            Found::Name(constraint.name),
                            constraint.span,
                        ));
                    }
                };
                generics.push(binder);
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
                .classify::<crate::syntax::function_name::FnParamNameNamespace>();
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
            generics,
            params,
            result,
            span: fn_ident.merge(end_span),
        })
    }

    /// Parse an include declaration:
    ///   `include nasa.rocket.compute_thrust(args);`
    ///   `include nasa.rocket.compute_thrust(args) as ct;`
    ///   `include nasa.rocket.compute_thrust(args)::{thrust};`
    pub(super) fn parse_include_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Include)?;

        let path = self.parse_module_path()?;

        // Param bindings are required for `include`.
        let param_bindings = if self.lexer.peek() == Some(&Token::LParen) {
            self.parse_import_param_bindings()?
        } else {
            let found = Found::token_or_end(self.lexer.peek().copied());
            return Err(Self::unexpected_token(
                Expected::ParamBindingsOpen,
                found,
                path.span,
            ));
        };

        let (kind, end_span) = self.parse_import_tail(Expected::IncludeTail)?;
        let span = start_span.merge(end_span);

        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Include(crate::syntax::ast::IncludeDecl {
                path,
                param_bindings,
                kind,
            }),
            span,
        })
    }

    /// Parse a dot-separated module path: `IDENT { "." IDENT }`.
    ///
    /// Dots are always package/DAG path separators in this production. The
    /// selective member boundary is the distinct `::{ ... }` token sequence.
    fn parse_module_path(&mut self) -> Result<ModulePath, ParseError> {
        let first = self.parse_any_ident()?;
        let path_start = first.span;
        let mut rest = Vec::new();

        while self.lexer.peek() == Some(&Token::Dot)
            && self
                .lexer
                .peek_second()
                .is_some_and(|token| token.is_identifier())
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
    ///   `::{ items, ... } ;`   → brace-list form
    fn parse_import_tail(
        &mut self,
        hint: Expected,
    ) -> Result<(ImportKind, crate::syntax::span::Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::DoubleColon) => {
                self.advance()?;
                if self.lexer.peek() != Some(&Token::LBrace) {
                    let found = Found::token_or_end(self.lexer.peek().copied());
                    let (_, span) = self.advance()?;
                    return Err(Self::unexpected_token(Expected::SelectorBrace, found, span));
                }
                let names = self.parse_import_brace_list()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((ImportKind::Selective(names), end_span))
            }
            Some(Token::As) => {
                self.lexer.next_token(); // consume `as`
                let alias: Spanned<ModuleAliasName> = self.parse_any_ident()?.classify();
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((ImportKind::Module { alias: Some(alias) }, end_span))
            }
            Some(Token::Semicolon) => {
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((ImportKind::Module { alias: None }, end_span))
            }
            Some(_) | None => Err(self.unexpected_next(hint)),
        }
    }

    /// Parse the `{ X, Y as Z, ... }` body of a brace-list selector.
    fn parse_import_brace_list(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::ImportItem>, ParseError> {
        self.expect(Token::LBrace)?;

        let names = self.parse_non_empty_comma_separated(Token::RBrace, |p| {
            // Collect any leading attributes on this import item.
            let mut item_attributes = Vec::new();
            while p.lexer.peek() == Some(&Token::Hash) {
                item_attributes.push(p.parse_attribute()?);
            }

            // Optional `pub` prefix marks the item for re-export (issue #452).
            // `pub(bind)` is rejected — re-exports are use-sites, not declarations.
            let visibility = if p.lexer.peek() == Some(&Token::Pub) {
                let (_, pub_span) = p.advance()?;
                if p.lexer.peek() == Some(&Token::LParen) {
                    return Err(Self::unexpected(
                        Expected::ImportItemName,
                        Token::LParen,
                        pub_span,
                    ));
                }
                Visibility::Public
            } else {
                Visibility::Private
            };

            let namespace = match p.lexer.peek() {
                Some(Token::Type) => {
                    let (_, span) = p.advance()?;
                    let _ = span;
                    crate::syntax::ast::ImportItemNamespace::Type
                }
                Some(Token::Dimension) => {
                    let (_, span) = p.advance()?;
                    let _ = span;
                    crate::syntax::ast::ImportItemNamespace::Dimension
                }
                Some(Token::Unit) => {
                    let (_, span) = p.advance()?;
                    let _ = span;
                    crate::syntax::ast::ImportItemNamespace::Unit
                }
                Some(Token::Index) => {
                    let (_, span) = p.advance()?;
                    let _ = span;
                    crate::syntax::ast::ImportItemNamespace::Index
                }
                _ => crate::syntax::ast::ImportItemNamespace::Term,
            };

            // Accept any identifier (imports can be any casing).
            let name = p.parse_any_ident()?;

            // Optional `as` alias::
            let alias = if p.lexer.peek() == Some(&Token::As) {
                p.lexer.next_token(); // consume `as`
                Some(p.parse_any_ident()?)
            } else {
                None
            };

            Ok(crate::syntax::ast::ImportItem {
                attributes: item_attributes,
                visibility,
                namespace,
                name,
                alias,
            })
        })?;

        self.expect(Token::RBrace)?;
        Ok(names.into_vec())
    }

    /// Parse categorized DAG bindings. Unmarked targets are parameters;
    /// `type`, `dim`, and `index` select Static targets. `param` is invalid.
    pub(in crate::syntax::parser) fn parse_import_param_bindings(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::ParamBinding>, ParseError> {
        self.expect(Token::LParen)?;

        let bindings = self.parse_comma_separated(Token::RParen, |p| {
            let (category, marker_span) = match p.lexer.peek() {
                Some(Token::Type) => {
                    let (_, span) = p.advance()?;
                    (crate::syntax::ast::InputBindingCategory::Type, Some(span))
                }
                Some(Token::Dimension) => {
                    let (_, span) = p.advance()?;
                    (
                        crate::syntax::ast::InputBindingCategory::Dimension,
                        Some(span),
                    )
                }
                Some(Token::Index) => {
                    let (_, span) = p.advance()?;
                    (crate::syntax::ast::InputBindingCategory::Index, Some(span))
                }
                _ => (crate::syntax::ast::InputBindingCategory::Unmarked, None),
            };
            let name_ident = p.parse_any_ident()?;
            let name_span = name_ident.span;
            p.expect(Token::Colon)?;
            let value = p.parse_expr()?;
            let binding_span = marker_span.unwrap_or(name_span).merge(value.span);
            Ok(crate::syntax::ast::ParamBinding {
                category,
                name: name_ident,
                value,
                span: binding_span,
            })
        })?;

        self.expect(Token::RParen)?;
        let mut declared = std::collections::HashMap::new();
        for binding in &bindings {
            if let Some(first) =
                declared.insert((binding.category, &binding.name.name), binding.name.span)
            {
                return Err(ParseError::new(
                    ParseErrorKind::DuplicateDagBinding {
                        name: binding.name.name.atom().clone(),
                        first,
                    },
                    binding.name.span,
                ));
            }
        }
        Ok(bindings)
    }
}
