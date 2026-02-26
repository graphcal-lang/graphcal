use crate::ast::{Attribute, AttributeArg, DeclKind, Declaration, DeriveOp};
use crate::names::Spanned;
use crate::token::Token;

use super::{ParseError, Parser};

mod dim_unit;
mod figure;
mod import;
mod index;
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

        let expected = "`param`, `node`, `const`, `dimension`, `unit`, `type`, `fn`, `index`, `import`, `assert`, `plot`, or `figure`";
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
            Some(Token::Plot) => self.parse_plot(),
            Some(Token::Figure) => self.parse_figure(),
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
}
