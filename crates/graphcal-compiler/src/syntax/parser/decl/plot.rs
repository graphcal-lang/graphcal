use crate::syntax::ast::{
    DeclKind, Declaration, Encoding, EncodingChannel, MarkSpec, MarkType, PlotDecl, PlotField,
};
use crate::syntax::names::DeclName;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case};

impl Parser<'_> {
    /// Parse a plot declaration: `plot name = { mark: type, encode: { ... }, title: "..." };`
    pub(super) fn parse_plot(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Plot)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Eq)?;

        // Parse the block: { mark: ..., encode: { ... }, ... }
        self.expect(Token::LBrace)?;

        let mut mark: Option<MarkSpec> = None;
        let mut encodings: Vec<Encoding> = Vec::new();
        let mut properties: Vec<PlotField> = Vec::new();

        while self.lexer.peek() != Some(&Token::RBrace) {
            let field_name = self.parse_any_ident()?;
            let field_start = field_name.span;
            self.expect(Token::Colon)?;

            match field_name.name.as_str() {
                "mark" => {
                    let mark_spec = self.parse_mark_spec(field_start)?;
                    mark = Some(mark_spec);
                }
                "encode" => {
                    encodings = self.parse_encode_block()?;
                }
                _ => {
                    let value = self.parse_expr()?;
                    let field_end = value.span;
                    properties.push(PlotField {
                        name: field_name,
                        value,
                        span: field_start.merge(field_end),
                    });
                }
            }
            self.expect(Token::Comma)?;
        }
        self.expect(Token::RBrace)?;

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);

        let Some(mark) = mark else {
            return Err(self.unexpected_token("`mark` field in plot declaration", "}", span));
        };

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Plot(PlotDecl {
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
        let mark_type = match mark_ident.name.as_str() {
            "point" => MarkType::Point,
            "line" => MarkType::Line,
            "bar" => MarkType::Bar,
            "area" => MarkType::Area,
            "rect" => MarkType::Rect,
            "tick" => MarkType::Tick,
            _ => {
                return Err(self.unexpected_token(
                    "`point`, `line`, `bar`, `area`, `rect`, or `tick`",
                    &mark_ident.name,
                    mark_type_span,
                ));
            }
        };

        // Optional mark properties: { stroke_width: 2.0, opacity: 0.5 }
        let mut properties = Vec::new();
        let end_span = if self.lexer.peek() == Some(&Token::LBrace) {
            self.expect(Token::LBrace)?;
            while self.lexer.peek() != Some(&Token::RBrace) {
                let prop_name = self.parse_any_ident()?;
                let prop_start = prop_name.span;
                self.expect(Token::Colon)?;
                let value = self.parse_expr()?;
                let prop_end = value.span;
                properties.push(PlotField {
                    name: prop_name,
                    value,
                    span: prop_start.merge(prop_end),
                });
                self.expect(Token::Comma)?;
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
            let channel = match channel_ident.name.as_str() {
                "x" => EncodingChannel::X,
                "y" => EncodingChannel::Y,
                "color" => EncodingChannel::Color,
                "size" => EncodingChannel::Size,
                "shape" => EncodingChannel::Shape,
                "opacity" => EncodingChannel::Opacity,
                "detail" => EncodingChannel::Detail,
                "text" => EncodingChannel::Text,
                "tooltip" => EncodingChannel::Tooltip,
                _ => {
                    return Err(self.unexpected_token(
                        "encoding channel (`x`, `y`, `color`, `size`, `shape`, `opacity`, `detail`, `text`, `tooltip`)",
                        &channel_ident.name,
                        channel_span,
                    ));
                }
            };
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            let value_span = value.span;
            encodings.push(Encoding {
                channel,
                channel_span,
                value,
                span: channel_span.merge(value_span),
            });
            self.expect(Token::Comma)?;
        }
        self.expect(Token::RBrace)?;

        Ok(encodings)
    }
}
