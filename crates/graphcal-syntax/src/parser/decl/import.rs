use crate::ast::DeclKind;
use crate::ast::Declaration;
use crate::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    // --- import declaration ---

    /// Parse an import declaration:
    /// `import "./path/to/file.gcl" { name1, name2 };`
    pub(super) fn parse_import_decl(&mut self) -> Result<Declaration, ParseError> {
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

        // Optional param bindings: `(name: expr, ...)`
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

            // Collect any leading attributes on this import item
            let mut item_attributes = Vec::new();
            while self.lexer.peek() == Some(&Token::Hash) {
                item_attributes.push(self.parse_attribute()?);
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
                attributes: item_attributes,
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

    /// Parse the `(name: expr, ...)` param bindings of an instantiated import.
    fn parse_import_param_bindings(&mut self) -> Result<Vec<crate::ast::ParamBinding>, ParseError> {
        self.expect(Token::LParen)?;

        let mut bindings = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            let name_ident = self.parse_any_ident()?;
            let name_span = name_ident.span;
            self.expect(Token::Colon)?;
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
}
