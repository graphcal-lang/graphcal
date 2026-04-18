use crate::syntax::ast::{Attribute, AttributeArg, Declaration, Visibility};
use crate::syntax::token::Token;

use super::{ParseError, Parser};

mod dag;
mod dim_unit;
mod figure;
mod import;
mod index;
mod layer;
mod plot;
#[cfg(test)]
mod tests;
mod type_decl;
mod value;

impl Parser<'_> {
    pub(super) fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        // Collect any leading attributes: #[name] or #[name(arg1, arg2)]
        let mut attributes = Vec::new();
        while self.lexer.peek() == Some(&Token::Hash) {
            attributes.push(self.parse_attribute()?);
        }

        // Optional `pub` or `pub(bind)` visibility modifier.
        //
        // `bind` is a contextual keyword parsed as a literal identifier
        // inside the parens; it is not a reserved token so it remains a
        // valid identifier elsewhere.
        let (visibility, visibility_span) = if self.lexer.peek() == Some(&Token::Pub) {
            let (_, pub_span) = self.advance()?;
            if self.lexer.peek() == Some(&Token::LParen) {
                self.expect(Token::LParen)?;
                let (bind_tok, bind_span) = self.advance()?;
                if bind_tok != Token::Ident || self.lexer.slice_at(bind_span) != "bind" {
                    return Err(self.unexpected_token("`bind`", &bind_tok.to_string(), bind_span));
                }
                let (_, rparen_span) = self.expect(Token::RParen)?;
                (Visibility::PublicBind, Some(pub_span.merge(rparen_span)))
            } else {
                (Visibility::Public, Some(pub_span))
            }
        } else {
            (Visibility::Private, None)
        };

        // Reject `pub` / `pub(bind)` on `param` at parse time. The spec
        // (visibility-bindability axioms §4.0) says `param` is
        // annotation-free: it is inherently visible + bindable, and any
        // annotation conveys no information. Catching this here keeps
        // the grammar surface itself compliant without deferring to the
        // resolver.
        let found = match visibility {
            Visibility::Private => None,
            Visibility::Public => Some("`pub`"),
            Visibility::PublicBind => Some("`pub(bind)`"),
        };
        if let Some(found) = found
            && self.lexer.peek() == Some(&Token::Param)
            && let Some(vis_span) = visibility_span
        {
            return Err(self.unexpected_token(
                "no visibility annotation (params are always visible and bindable)",
                found,
                vis_span,
            ));
        }

        let expected = "`param`, `node`, `const node`, `base dim`, `dim`, `unit`, `type`, `dag`, `index`, `import`, `include`, `assert`, `plot`, `figure`, or `layer`";
        let mut decl = match self.lexer.peek() {
            Some(Token::Param) => self.parse_param(),
            Some(Token::Node) => self.parse_node(),
            Some(Token::Const) => {
                let (_, const_span) = self.advance()?;
                match self.lexer.peek() {
                    Some(Token::Node) => self.parse_const_node(const_span),
                    Some(Token::Unit) => self.parse_const_unit(const_span),
                    Some(_) => {
                        let (tok, span) = self.advance()?;
                        Err(self.unexpected_token(
                            "`node` or `unit` after `const`",
                            &tok.to_string(),
                            span,
                        ))
                    }
                    None => Err(self.unexpected_eof("`node` or `unit` after `const`")),
                }
            }
            Some(Token::Base) => {
                let (_, base_span) = self.advance()?;
                match self.lexer.peek() {
                    Some(Token::Dimension) => self.parse_base_dimension_decl(base_span),
                    Some(Token::Unit) => self.parse_base_unit_decl(base_span),
                    Some(_) => {
                        let (tok, span) = self.advance()?;
                        Err(self.unexpected_token(
                            "`dim` or `unit` after `base`",
                            &tok.to_string(),
                            span,
                        ))
                    }
                    None => Err(self.unexpected_eof("`dim` or `unit` after `base`")),
                }
            }
            Some(Token::Dimension) => self.parse_dimension_decl(),
            Some(Token::Unit) => self.parse_unit_decl(),
            Some(Token::Type) => self.parse_type_decl(),
            Some(Token::Index) => self.parse_index_decl(),
            Some(Token::Import) => self.parse_import_decl(),
            Some(Token::Include) => self.parse_include_decl(),
            Some(Token::Dag) => self.parse_dag_decl(),
            Some(Token::Assert) => self.parse_assert(),
            Some(Token::Plot) => self.parse_plot(),
            Some(Token::Figure) => self.parse_figure(),
            Some(Token::Layer) => self.parse_layer(),
            Some(_) => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(expected, &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof(expected)),
        }?;

        // Set visibility
        decl.visibility = visibility;

        // Extend the declaration span to include `pub` / `pub(bind)` prefix
        if let Some(ps) = visibility_span {
            decl.span = ps.merge(decl.span);
        }

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
}
