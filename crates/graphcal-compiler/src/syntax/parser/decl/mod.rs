use crate::syntax::ast::{Attribute, AttributeArg, Declaration, PlotField, SlotKind};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::{ContextualKeyword, Token};

use super::{
    CompositionKind, Expected, Found, InvalidNumberReason, ParseError, ParseErrorKind, Parser,
    PlotFieldContext,
};

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
mod visibility;

struct CompositionDeclParts {
    name: Spanned<DeclName>,
    plot_names: Vec<Spanned<ScopedName>>,
    fields: Vec<PlotField>,
    span: Span,
}

impl Parser<'_> {
    fn parse_composition_decl_parts(
        &mut self,
        token: Token,
        kind: CompositionKind,
    ) -> Result<CompositionDeclParts, ParseError> {
        let (_, start_span) = self.expect(token)?;
        let name: Spanned<DeclName> = self.parse_any_ident()?.classify();
        self.expect(Token::Eq)?;

        self.expect(Token::LBrace)?;
        let mut plots_seen = false;
        let mut plot_names = Vec::new();
        let mut fields = Vec::new();

        while self.lexer.peek() != Some(&Token::RBrace) {
            let field_keyword = self.peek_contextual_keyword();
            let field_name = self.parse_any_ident()?;
            let field_start = field_name.span;
            self.expect(Token::Colon)?;

            if field_keyword == Some(ContextualKeyword::Plots) {
                if plots_seen {
                    return Err(Self::duplicate_plot_field(
                        field_name.name,
                        PlotFieldContext::Composition(kind),
                        field_start,
                    ));
                }
                plots_seen = true;
                self.expect(Token::LBracket)?;
                while self.lexer.peek() != Some(&Token::RBracket) {
                    let plot_ident = self.parse_any_ident()?;
                    let plot_name = Spanned::new(
                        ScopedName::local(DeclName::classify(plot_ident.name.into_atom())),
                        plot_ident.span,
                    );
                    plot_names.push(plot_name);
                    if self.lexer.peek() == Some(&Token::Comma) {
                        self.expect(Token::Comma)?;
                    } else {
                        break;
                    }
                }
                self.expect(Token::RBracket)?;
            } else {
                if fields
                    .iter()
                    .any(|f: &PlotField| f.name.value.as_str() == field_name.name.as_str())
                {
                    return Err(Self::duplicate_plot_field(
                        field_name.name,
                        PlotFieldContext::Composition(kind),
                        field_start,
                    ));
                }
                let value = self.parse_expr()?;
                let field_end = value.span;
                fields.push(PlotField {
                    name: field_name.classify(),
                    value,
                    span: field_start.merge(field_end),
                });
            }
            if self.lexer.peek() == Some(&Token::Comma) {
                self.expect(Token::Comma)?;
            } else {
                break;
            }
        }
        self.expect(Token::RBrace)?;

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        if plot_names.is_empty() {
            return Err(ParseError::new(
                ParseErrorKind::EmptyCompositionPlots { kind },
                span,
            ));
        }

        Ok(CompositionDeclParts {
            name,
            plot_names,
            fields,
            span,
        })
    }

    /// Parse one declaration surface form. A multi-decl is represented as
    /// `DeclKind::Sugar(RawDeclSugar::Multi(_))` and expanded later by the desugar pass.
    pub(super) fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        self.with_nesting_budget(Self::parse_declaration_inner)
    }

    fn parse_declaration_inner(&mut self) -> Result<Declaration, ParseError> {
        // Collect any leading attributes: #[name] or #[name(arg1, arg2)]
        let mut attributes = Vec::new();
        while self.lexer.peek() == Some(&Token::Hash) {
            attributes.push(self.parse_attribute()?);
        }

        // Optional `pub` or `pub(bind)` prefix; each declaration kind below
        // accepts it as the visibility type it stores.
        let prefix = self.parse_visibility_prefix()?;

        // Value-declaration paths (`param`, `node`, `const node`) can be
        // either a single declaration or a multi-decl (issue #481). We
        // consume the kind keyword(s) — which also accepts the visibility
        // prefix for the kind — parse the slot header, then peek at the
        // next token to decide.
        let is_value_decl = match self.lexer.peek() {
            Some(Token::Param | Token::Node) => true,
            Some(Token::Const) => self.lexer.peek_second() == Some(&Token::Node),
            _ => false,
        };
        if is_value_decl {
            let (kind, kind_span) = self.parse_slot_kind(prefix)?;
            return self.finish_value_decl_or_multi(kind, kind_span, attributes, prefix.span());
        }

        let decl = match self.lexer.peek() {
            Some(Token::Const) => {
                let (_, const_span) = self.advance()?;
                match self.lexer.peek() {
                    Some(Token::Unit) => {
                        // `const unit`: single declaration only (no multi-decl sugar).
                        let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                        self.parse_const_unit(const_span, visibility)
                    }
                    Some(_) | None => Err(self.unexpected_next(Expected::AfterConst)),
                }
            }
            Some(Token::Base) => {
                let (_, base_span) = self.advance()?;
                match self.lexer.peek() {
                    Some(Token::Dimension) => {
                        let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                        self.parse_base_dimension_decl(base_span, visibility)
                    }
                    Some(Token::Unit) => {
                        let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                        self.parse_base_unit_decl(base_span, visibility)
                    }
                    Some(_) | None => Err(self.unexpected_next(Expected::AfterBase)),
                }
            }
            Some(Token::Dimension) => self.parse_dimension_decl(prefix.accept_bindable()),
            Some(Token::Unit) => {
                let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                self.parse_unit_decl(visibility)
            }
            Some(Token::Type) => self.parse_type_decl(prefix.accept_bindable()),
            Some(Token::Index) => self.parse_index_decl(prefix.accept_bindable()),
            Some(Token::Import) => self.parse_import_decl(prefix),
            Some(Token::Include) => {
                prefix.accept_none(Expected::IncludeWithoutVisibility)?;
                self.parse_include_decl()
            }
            Some(Token::Dag) => {
                let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                self.parse_dag_decl(visibility)
            }
            Some(Token::Assert) => {
                let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                self.parse_assert(visibility)
            }
            Some(Token::Plot) => {
                let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                self.parse_plot(visibility)
            }
            Some(Token::Figure) => {
                let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                self.parse_figure(visibility)
            }
            Some(Token::Layer) => {
                let visibility = prefix.accept_public(Expected::NonBindableVisibility)?;
                self.parse_layer(visibility)
            }
            Some(_) | None => Err(self.unexpected_next(Expected::Declaration)),
        }?;

        Ok(Self::with_prefix_and_attributes(
            decl,
            prefix.span(),
            attributes,
        ))
    }

    /// Extend `decl` over its visibility prefix and attributes, and attach the
    /// attributes.
    fn with_prefix_and_attributes(
        mut decl: Declaration,
        prefix_span: Option<Span>,
        attributes: Vec<Attribute>,
    ) -> Declaration {
        if let Some(prefix_span) = prefix_span {
            decl.span = prefix_span.merge(decl.span);
        }
        if let Some(first_attr) = attributes.first() {
            decl.span = first_attr.span.merge(decl.span);
        }
        decl.attributes = attributes;
        decl
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
        prefix_span: Option<Span>,
    ) -> Result<Declaration, ParseError> {
        let header = self.parse_slot_header_tail(kind, kind_span)?;

        if self.lexer.peek() == Some(&Token::Comma) {
            // Multi-decl. Attributes are still forbidden; visibility now
            // attaches to each slot, with the leading prefix consumed by
            // `parse_declaration` becoming the first slot's visibility.
            if let Some(first_attr) = attributes.first() {
                return Err(Self::unexpected_token(
                    Expected::MultiDeclWithoutAttributes,
                    Found::Attributes,
                    first_attr.span,
                ));
            }
            return self.parse_multi_decl_rest(header);
        }

        // Single decl. Continue with the existing param/node/const-node path.
        let decl = self.finish_single_value_decl(header)?;
        Ok(Self::with_prefix_and_attributes(
            decl,
            prefix_span,
            attributes,
        ))
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

    /// Parse a single attribute argument: a Term path, `Index#Label`, a
    /// finite position, or a parenthesized group.
    fn parse_attribute_arg(&mut self) -> Result<AttributeArg, ParseError> {
        self.with_nesting_budget(Self::parse_attribute_arg_inner)
    }

    fn parse_attribute_arg_inner(&mut self) -> Result<AttributeArg, ParseError> {
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
        } else if self.lexer.peek() == Some(&Token::Hash) {
            // Finite position: #N (matching table slice labels).
            let (_, hash_span) = self.expect(Token::Hash)?;
            let (_, num_span) = self.expect(Token::Number)?;
            let text = self.lexer.slice_at(num_span).replace('_', "");
            let position: u64 = text.parse().map_err(|_| {
                Self::invalid_number(InvalidNumberReason::AttributePosition, num_span)
            })?;
            Ok(AttributeArg::FinitePosition {
                position,
                span: hash_span.merge(num_span),
            })
        } else {
            let path = self.parse_ident_path()?;
            if self.lexer.peek() == Some(&Token::Hash) {
                self.expect(Token::Hash)?;
                let label: Spanned<IndexVariantName> = self.parse_any_ident()?.classify();
                let span = path.span().merge(label.span);
                Ok(AttributeArg::IndexLabel {
                    index: path.into_spanned_name_path(),
                    label,
                    span,
                })
            } else {
                Ok(AttributeArg::Path {
                    path: path.into_spanned_name_path(),
                })
            }
        }
    }
}
