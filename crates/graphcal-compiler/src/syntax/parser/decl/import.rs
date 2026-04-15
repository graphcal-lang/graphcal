use crate::syntax::ast::DeclKind;
use crate::syntax::ast::Declaration;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    // --- import declaration ---

    /// Parse an import declaration (compile-time definition import, no param bindings):
    /// `import "./path/to/file.gcl" { name1, name2 };`
    /// `import "./path/to/file.gcl" as alias;`
    /// `import "./path/to/file.gcl";`
    pub(super) fn parse_import_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Import)?;

        // `import` requires 2-segment module paths (e.g., `package/module`),
        // but also accepts `..` for parent scope access inside DAGs.
        // Single-segment paths are only valid for `include` (inline DAG names).
        let path = self.parse_import_path(2)?;

        // Reject cross-file DAG paths on `import` — use `include` for DAG instantiation.
        if path.is_cross_file_dag() {
            return Err(self.unexpected_token(
                "a file path or module path (`import` cannot reference cross-file DAGs; use `include` for DAG instantiation)",
                &format!("\"...\"/{}", path.display_path().rsplit('/').next().unwrap_or("")),
                path.span(),
            ));
        }

        // Reject param bindings on `import` — use `include` for DAG instantiation.
        if self.lexer.peek() == Some(&Token::LParen) {
            let (_, span) = self.advance()?;
            return Err(self.unexpected_token(
                "`{`, `as`, or `;` after path (`import` cannot have param bindings; use `include` for DAG instantiation)",
                "(",
                span,
            ));
        }

        // Determine the kind of import based on the next token.
        let (kind, end_span) = self.parse_import_or_include_kind("`{`, `as`, or `;` after path")?;

        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            is_pub: false,
            kind: DeclKind::Import(crate::syntax::ast::ImportDecl { path, kind }),
            span,
        })
    }

    // --- include declaration ---

    /// Parse an include declaration (DAG embedding with optional param bindings):
    /// `include "./rocket.gcl"(dry_mass: 800.0 kg) { delta_v };`
    /// `include "./rocket.gcl"(dry_mass: 800.0 kg) as stage;`
    /// `include "./rocket.gcl" { delta_v };`
    pub(super) fn parse_include_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Include)?;

        // `include` allows 1-segment module paths for inline DAG references.
        let path = self.parse_import_path(1)?;

        // Optional param bindings: `(name: expr, ...)`
        let param_bindings = if self.lexer.peek() == Some(&Token::LParen) {
            self.parse_import_param_bindings()?
        } else {
            Vec::new()
        };

        // Determine the kind based on the next token.
        let after_hint = if param_bindings.is_empty() {
            "`(`, `{`, `as`, or `;` after path"
        } else {
            "`{`, `as`, or `;` after param bindings"
        };
        let (kind, end_span) = self.parse_import_or_include_kind(after_hint)?;

        let span = start_span.merge(end_span);

        Ok(Declaration {
            attributes: vec![],
            is_pub: false,
            kind: DeclKind::Include(crate::syntax::ast::IncludeDecl {
                path,
                param_bindings,
                kind,
            }),
            span,
        })
    }

    // --- shared helpers ---

    /// Parse an import/include path: string literal, bare module path, or `..` parent scope.
    ///
    /// `min_segments` controls the minimum number of segments for bare module paths.
    /// `import` requires 2 (e.g., `package/module`), `include` allows 1 (for inline DAG names).
    fn parse_import_path(
        &mut self,
        min_segments: usize,
    ) -> Result<crate::syntax::ast::ImportPath, ParseError> {
        match self.lexer.peek() {
            Some(Token::StringLiteral) => {
                let (_, span) = self.advance()?;
                let raw = self.lexer.slice_at(span);
                let path_str = raw[1..raw.len() - 1].to_string();

                // Check for cross-file DAG path: `"./file.gcl"/dag_name`
                if self.lexer.peek() == Some(&Token::Slash) {
                    self.lexer.next_token(); // consume `/`
                    let dag_ident = self.parse_any_ident()?;
                    let full_span = span.merge(dag_ident.span);
                    return Ok(crate::syntax::ast::ImportPath::CrossFileDag {
                        file_path: path_str,
                        dag_name: dag_ident,
                        span: full_span,
                    });
                }

                Ok(crate::syntax::ast::ImportPath::FilePath {
                    path: path_str,
                    span,
                })
            }
            Some(Token::DotDot) => {
                // Parent scope path: `..` or `../..` or `../../..`
                let (_, start_span) = self.advance()?;
                let mut levels: u32 = 1;
                let mut end_span = start_span;

                // Parse additional `/..` pairs for deeper traversal.
                // After `..`, only `/..` is valid for continued parent traversal.
                while self.lexer.peek() == Some(&Token::Slash) {
                    self.lexer.next_token(); // consume `/`
                    if self.lexer.peek() == Some(&Token::DotDot) {
                        let (_, span) = self.advance()?;
                        levels += 1;
                        end_span = span;
                    } else if let Some(tok) = self.lexer.peek() {
                        // `/` followed by something other than `..` — parse error
                        let tok_str = tok.to_string();
                        let (_, span) = self.advance()?;
                        return Err(self.unexpected_token("`..` after `/`", &tok_str, span));
                    } else {
                        return Err(self.unexpected_eof("`..` after `/`"));
                    }
                }

                Ok(crate::syntax::ast::ImportPath::ParentScope {
                    levels,
                    span: start_span.merge(end_span),
                })
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

                // Enforce minimum segment count
                if segments.len() < min_segments {
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
                Ok(crate::syntax::ast::ImportPath::ModulePath {
                    segments,
                    span: path_start.merge(path_end),
                })
            }
            Some(tok) => {
                let tok_str = tok.to_string();
                let (_, span) = self.advance()?;
                Err(self.unexpected_token("a string literal, module path, or `..`", &tok_str, span))
            }
            None => Err(self.unexpected_eof("a string literal, module path, or `..`")),
        }
    }

    /// Parse the kind portion of an import/include declaration:
    /// `{ name1, name2 as alias };` or `as alias;` or `;`
    fn parse_import_or_include_kind(
        &mut self,
        hint: &str,
    ) -> Result<(crate::syntax::ast::ImportKind, crate::syntax::span::Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::LBrace) => {
                let names = self.parse_import_selective_body()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((crate::syntax::ast::ImportKind::Selective(names), end_span))
            }
            Some(Token::As) => {
                self.lexer.next_token(); // consume `as`
                let alias = self.parse_any_ident()?;
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((
                    crate::syntax::ast::ImportKind::Module { alias: Some(alias) },
                    end_span,
                ))
            }
            Some(Token::Semicolon) => {
                let (_, end_span) = self.expect(Token::Semicolon)?;
                Ok((
                    crate::syntax::ast::ImportKind::Module { alias: None },
                    end_span,
                ))
            }
            Some(tok) => {
                let tok_str = tok.to_string();
                let (_, span) = self.advance()?;
                Err(self.unexpected_token(hint, &tok_str, span))
            }
            None => Err(self.unexpected_eof(hint)),
        }
    }

    /// Parse the `{ name1, name2 as alias, ... }` body of a selective import/include.
    fn parse_import_selective_body(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::ImportItem>, ParseError> {
        self.expect(Token::LBrace)?;

        let names = self.parse_comma_separated(Token::RBrace, |p| {
            // Collect any leading attributes on this import item
            let mut item_attributes = Vec::new();
            while p.lexer.peek() == Some(&Token::Hash) {
                item_attributes.push(p.parse_attribute()?);
            }

            // Accept any identifier (imports can be any casing)
            let (name_str, name_span) = match p.lexer.next_token() {
                Some((Token::Ident, span)) => (p.lexer.slice_at(span).to_string(), span),
                Some((tok, span)) => {
                    return Err(p.unexpected_token("an identifier", &tok.to_string(), span));
                }
                None => {
                    return Err(p.unexpected_eof("an identifier or `}`"));
                }
            };

            // Check for optional `as` alias
            let alias = if p.lexer.peek() == Some(&Token::As) {
                p.lexer.next_token(); // consume `as`
                match p.lexer.next_token() {
                    Some((Token::Ident, alias_span)) => {
                        let alias_str = p.lexer.slice_at(alias_span).to_string();
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
    fn parse_import_param_bindings(
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
}
