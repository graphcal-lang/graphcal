use crate::syntax::ast::{Attribute, AttributeArg, DeclKind, Declaration, Visibility};
use crate::syntax::span::Span;
use crate::syntax::token::Token;

use super::{ParseError, Parser};
use multi::SlotKind;

mod dag;
mod dim_unit;
mod figure;
mod import;
mod index;
mod layer;
mod multi;
mod plot;
#[cfg(test)]
mod tests;
mod type_decl;
mod value;

impl Parser<'_> {
    /// Parse one top-level declaration surface form, which may desugar into
    /// multiple declarations (multi-decl) or stay as a single declaration.
    #[expect(
        clippy::too_many_lines,
        reason = "single entry point dispatches across every declaration kind"
    )]
    pub(super) fn parse_declarations(&mut self) -> Result<Vec<Declaration>, ParseError> {
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

        // Reject `pub(bind)` on `import` / `include`. Use-sites are not
        // bindable (A5: B ≡ fixed). `pub` is legal as a re-export marker
        // per issue #452.
        if visibility == Visibility::PublicBind
            && matches!(self.lexer.peek(), Some(Token::Import | Token::Include))
            && let Some(vis_span) = visibility_span
        {
            return Err(self.unexpected_token(
                "`pub` (use-sites are not bindable — `pub(bind)` is only for declaration kinds)",
                "`pub(bind)`",
                vis_span,
            ));
        }

        // Reject `pub(bind)` on `node` / `const node`. Nodes are computed
        // values, not a bindable surface; `param` already plays that role.
        // `pub` on `node` is legal and controls projection visibility from
        // inline-dag call sites.
        if visibility == Visibility::PublicBind
            && matches!(self.lexer.peek(), Some(Token::Node | Token::Const))
            && let Some(vis_span) = visibility_span
        {
            return Err(self.unexpected_token(
                "`pub` (nodes are computed values — `pub(bind)` is not meaningful; use `param` to declare a bindable input)",
                "`pub(bind)`",
                vis_span,
            ));
        }

        let expected = "`param`, `node`, `const node`, `base dim`, `dim`, `unit`, `type`, `dag`, `index`, `import`, `include`, `assert`, `plot`, `figure`, or `layer`";

        // Value-declaration paths (`param`, `node`, `const node`) can be
        // either a single declaration or a multi-decl (issue #481). We
        // consume the kind keyword(s), parse the slot header, then peek
        // at the next token to decide.
        match self.lexer.peek() {
            Some(Token::Param) => {
                let (_, kind_span) = self.advance()?;
                return self.finish_value_decl_or_multi(
                    SlotKind::Param,
                    kind_span,
                    attributes,
                    visibility,
                    visibility_span,
                );
            }
            Some(Token::Node) => {
                let (_, kind_span) = self.advance()?;
                return self.finish_value_decl_or_multi(
                    SlotKind::Node,
                    kind_span,
                    attributes,
                    visibility,
                    visibility_span,
                );
            }
            Some(Token::Const) => {
                let (_, const_span) = self.advance()?;
                match self.lexer.peek() {
                    Some(Token::Node) => {
                        let (_, node_span) = self.advance()?;
                        return self.finish_value_decl_or_multi(
                            SlotKind::ConstNode,
                            const_span.merge(node_span),
                            attributes,
                            visibility,
                            visibility_span,
                        );
                    }
                    Some(Token::Unit) => {
                        // const unit: single decl only (no multi-decl sugar).
                        let mut decl = self.parse_const_unit(const_span)?;
                        decl.visibility = visibility;
                        if let Some(ps) = visibility_span {
                            decl.span = ps.merge(decl.span);
                        }
                        if let Some(first_attr) = attributes.first() {
                            decl.span = first_attr.span.merge(decl.span);
                        }
                        decl.attributes = attributes;
                        return Ok(vec![decl]);
                    }
                    Some(_) => {
                        let (tok, span) = self.advance()?;
                        return Err(self.unexpected_token(
                            "`node` or `unit` after `const`",
                            &tok.to_string(),
                            span,
                        ));
                    }
                    None => {
                        return Err(self.unexpected_eof("`node` or `unit` after `const`"));
                    }
                }
            }
            _ => {}
        }

        let mut decl = match self.lexer.peek() {
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

        // Mutual exclusion for re-exports (issue #452 / spec §4.1):
        // `pub import "X" { pub items };` mixes whole-module and selective
        // re-export forms. Reject at parse so the semantics of a single
        // re-export construct stays unambiguous.
        if decl.visibility == Visibility::Public
            && let Some(vis_span) = visibility_span
        {
            let selective_items = match &decl.kind {
                DeclKind::Import(d) => match &d.kind {
                    crate::syntax::ast::ImportKind::Selective(items) => Some(items.as_slice()),
                    crate::syntax::ast::ImportKind::Module { .. } => None,
                },
                DeclKind::Include(d) => match &d.kind {
                    crate::syntax::ast::ImportKind::Selective(items) => Some(items.as_slice()),
                    crate::syntax::ast::ImportKind::Module { .. } => None,
                },
                _ => None,
            };
            if let Some(items) = selective_items
                && items.iter().any(|it| it.is_pub)
            {
                return Err(self.unexpected_token(
                    "either `pub include/import \"X\" ...` (whole-module re-export) or `include/import \"X\" { pub items }` (selective re-export), not both",
                    "`pub`",
                    vis_span,
                ));
            }
        }

        // Extend the declaration span to include `pub` / `pub(bind)` prefix
        if let Some(ps) = visibility_span {
            decl.span = ps.merge(decl.span);
        }

        // Extend the declaration span to include the attributes
        if let Some(first_attr) = attributes.first() {
            decl.span = first_attr.span.merge(decl.span);
        }

        decl.attributes = attributes;

        Ok(vec![decl])
    }

    /// Complete parsing of a `param` / `node` / `const node` declaration
    /// starting from after the kind keyword, dispatching to either the
    /// single-decl path or the multi-decl path based on the first
    /// post-type-annotation token.
    fn finish_value_decl_or_multi(
        &mut self,
        kind: SlotKind,
        kind_span: Span,
        attributes: Vec<Attribute>,
        visibility: Visibility,
        visibility_span: Option<Span>,
    ) -> Result<Vec<Declaration>, ParseError> {
        let header = self.parse_slot_header_tail(kind, kind_span)?;

        if self.lexer.peek() == Some(&Token::Comma) {
            // Multi-decl. Attributes and visibility are not allowed.
            if let Some(first_attr) = attributes.first() {
                return Err(self.unexpected_token(
                    "no attributes on multi-decl (attributes are forbidden on multi-decl surface forms in v1)",
                    "`#[...]`",
                    first_attr.span,
                ));
            }
            if let Some(vis_span) = visibility_span {
                let found = match visibility {
                    Visibility::Public => "`pub`",
                    Visibility::PublicBind => "`pub(bind)`",
                    Visibility::Private => "visibility annotation",
                };
                return Err(self.unexpected_token(
                    "no visibility annotation on multi-decl (apply visibility to each slot in a future extension)",
                    found,
                    vis_span,
                ));
            }
            return self.parse_multi_decl_rest(header);
        }

        // Single decl. Continue with the existing param/node/const-node path.
        let decl = self.finish_single_value_decl(header)?;
        let mut decl = decl;
        decl.visibility = visibility;
        if let Some(ps) = visibility_span {
            decl.span = ps.merge(decl.span);
        }
        if let Some(first_attr) = attributes.first() {
            decl.span = first_attr.span.merge(decl.span);
        }
        decl.attributes = attributes;
        Ok(vec![decl])
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
