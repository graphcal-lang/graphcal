use crate::ast::{ChartType, DeclKind, Declaration, PlotDecl, PlotField};
use crate::names::DeclName;
use crate::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case};

impl Parser<'_> {
    /// Parse a plot declaration: `plot name = chart_type { field: expr, ... };`
    pub(super) fn parse_plot(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Plot)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Eq)?;

        // Parse chart type (identifier: line, scatter, bar, heatmap)
        let chart_type_ident = self.parse_any_ident()?;
        let chart_type_span = chart_type_ident.span;
        let chart_type = match chart_type_ident.name.as_str() {
            "line" => ChartType::Line,
            "scatter" => ChartType::Scatter,
            "bar" => ChartType::Bar,
            "heatmap" => ChartType::Heatmap,
            _ => {
                return Err(self.unexpected_token(
                    "`line`, `scatter`, `bar`, or `heatmap`",
                    &chart_type_ident.name,
                    chart_type_span,
                ));
            }
        };

        // Parse field block: { field: expr, ... }
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            let field_name = self.parse_any_ident()?;
            let field_start = field_name.span;
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            let field_end = value.span;
            fields.push(PlotField {
                name: field_name,
                value,
                span: field_start.merge(field_end),
            });
            self.expect(Token::Comma)?;
        }
        self.expect(Token::RBrace)?;

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Plot(PlotDecl {
                name,
                chart_type,
                chart_type_span,
                fields,
            }),
            span,
        })
    }
}
