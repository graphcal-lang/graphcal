use crate::syntax::ast::{
    DeclKind, Declaration, Encoding, EncodingChannel, MarkSpec, MarkType, PlotDecl, PlotField,
    Visibility,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::Spanned;
use crate::syntax::token::{ContextualKeyword, Token};

use super::super::{Expected, Found, ParseError, ParseErrorKind, Parser, PlotFieldContext};

impl Parser<'_> {
    /// Parse a plot declaration: `plot name = { mark: type, encode: { ... }, title: "..." };`
    pub(super) fn parse_plot(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Plot)?;
        let name: Spanned<DeclName> = self.parse_any_ident()?.classify();
        self.expect(Token::Eq)?;

        // Parse the block: { mark: ..., encode: { ... }, ... }
        self.expect(Token::LBrace)?;

        let mut mark: Option<MarkSpec> = None;
        let mut encode_seen = false;
        let mut encodings: Vec<Encoding> = Vec::new();
        let mut properties: Vec<PlotField> = Vec::new();

        while self.lexer.peek() != Some(&Token::RBrace) {
            let field_keyword = self.peek_contextual_keyword();
            let field_name = self.parse_any_ident()?;
            let field_start = field_name.span;
            self.expect(Token::Colon)?;

            // Duplicate fields would silently shadow each other with
            // implementation-defined precedence; reject them (#844).
            match field_keyword {
                Some(ContextualKeyword::Mark) => {
                    if mark.is_some() {
                        return Err(Self::duplicate_plot_field(
                            field_name.name,
                            PlotFieldContext::PlotDeclaration,
                            field_start,
                        ));
                    }
                    let mark_spec = self.parse_mark_spec(field_start)?;
                    mark = Some(mark_spec);
                }
                Some(ContextualKeyword::Encode) => {
                    if encode_seen {
                        return Err(Self::duplicate_plot_field(
                            field_name.name,
                            PlotFieldContext::PlotDeclaration,
                            field_start,
                        ));
                    }
                    encode_seen = true;
                    encodings = self.parse_encode_block()?;
                }
                _ => {
                    if properties
                        .iter()
                        .any(|p| p.name.value.as_str() == field_name.name.as_str())
                    {
                        return Err(Self::duplicate_plot_field(
                            field_name.name,
                            PlotFieldContext::PlotDeclaration,
                            field_start,
                        ));
                    }
                    let value = self.parse_expr()?;
                    let field_end = value.span;
                    properties.push(PlotField {
                        name: field_name.classify(),
                        value,
                        span: field_start.merge(field_end),
                    });
                }
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

        let Some(mark) = mark else {
            return Err(Self::unexpected(Expected::MarkField, Token::RBrace, span));
        };

        // A plot with no encoding channels renders an empty chart — almost
        // certainly a mistake; require at least one channel, symmetric with
        // the required `mark` field (#844).
        if encodings.is_empty() {
            return Err(ParseError::new(ParseErrorKind::MissingPlotEncoding, span));
        }

        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Plot(PlotDecl {
                visibility: Visibility::Private,
                name,
                mark,
                encodings,
                properties,
            }),
            span,
        })
    }

    /// Parse a mark specification: `point`, `line { stroke_width: 2.0 }`, etc.
    fn parse_mark_spec(
        &mut self,
        start_span: crate::syntax::span::Span,
    ) -> Result<MarkSpec, ParseError> {
        let mark_ident = self.parse_any_ident()?;
        let mark_type_span = mark_ident.span;
        let Some(mark_type) = MarkType::parse(mark_ident.name.as_str()) else {
            return Err(Self::unexpected_token(
                Expected::MarkType,
                Found::Name(mark_ident.name),
                mark_type_span,
            ));
        };

        // Optional mark properties: { stroke_width: 2.0, opacity: 0.5 }
        let mut properties = Vec::new();
        let end_span = if self.lexer.peek() == Some(&Token::LBrace) {
            self.expect(Token::LBrace)?;
            while self.lexer.peek() != Some(&Token::RBrace) {
                let prop_name = self.parse_any_ident()?;
                let prop_start = prop_name.span;
                self.expect(Token::Colon)?;
                if properties
                    .iter()
                    .any(|p: &PlotField| p.name.value.as_str() == prop_name.name.as_str())
                {
                    return Err(Self::duplicate_plot_field(
                        prop_name.name,
                        PlotFieldContext::MarkProperties,
                        prop_start,
                    ));
                }
                let value = self.parse_expr()?;
                let prop_end = value.span;
                properties.push(PlotField {
                    name: prop_name.classify(),
                    value,
                    span: prop_start.merge(prop_end),
                });
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.expect(Token::Comma)?;
                } else {
                    break;
                }
            }
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            rbrace_span
        } else {
            mark_type_span
        };

        Ok(MarkSpec {
            mark_type,
            mark_type_span,
            properties,
            span: start_span.merge(end_span),
        })
    }

    /// Parse an encode block: `{ x: expr, y: expr, color: expr, ... }`
    fn parse_encode_block(&mut self) -> Result<Vec<Encoding>, ParseError> {
        self.expect(Token::LBrace)?;
        let mut encodings = Vec::new();

        while self.lexer.peek() != Some(&Token::RBrace) {
            let channel_ident = self.parse_any_ident()?;
            let channel_span = channel_ident.span;
            let Some(channel) = EncodingChannel::parse(channel_ident.name.as_str()) else {
                return Err(Self::unexpected_token(
                    Expected::EncodingChannel,
                    Found::Name(channel_ident.name),
                    channel_span,
                ));
            };
            self.expect(Token::Colon)?;
            if encodings.iter().any(|e: &Encoding| e.channel == channel) {
                return Err(Self::duplicate_plot_field(
                    channel_ident.name,
                    PlotFieldContext::EncodeBlock,
                    channel_span,
                ));
            }
            let value = self.parse_expr()?;
            let value_span = value.span;
            encodings.push(Encoding {
                channel,
                channel_span,
                value,
                span: channel_span.merge(value_span),
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.expect(Token::Comma)?;
            } else {
                break;
            }
        }
        self.expect(Token::RBrace)?;

        Ok(encodings)
    }
}
