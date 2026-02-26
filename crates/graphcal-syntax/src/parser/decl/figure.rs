use crate::ast::{DeclKind, Declaration, FigureDecl, PlotField};
use crate::names::DeclName;
use crate::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case};

impl Parser<'_> {
    /// Parse a figure declaration: `figure name = { plots: [a, b], title: "..." };`
    pub(super) fn parse_figure(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Figure)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();
        self.expect(Token::Eq)?;

        // Parse field block: { plots: [...], title: "...", ... }
        self.expect(Token::LBrace)?;
        let mut plot_names = Vec::new();
        let mut fields = Vec::new();

        while self.lexer.peek() != Some(&Token::RBrace) {
            let field_name = self.parse_any_ident()?;
            let field_start = field_name.span;
            self.expect(Token::Colon)?;

            if field_name.name == "plots" {
                // Parse plots: [name1, name2, ...]
                self.expect(Token::LBracket)?;
                while self.lexer.peek() != Some(&Token::RBracket) {
                    let plot_name = self
                        .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
                        .into_spanned::<DeclName>();
                    plot_names.push(plot_name);
                    if self.lexer.peek() == Some(&Token::Comma) {
                        self.expect(Token::Comma)?;
                    } else {
                        break;
                    }
                }
                self.expect(Token::RBracket)?;
            } else {
                // Parse regular field: name: expr
                let value = self.parse_expr()?;
                let field_end = value.span;
                fields.push(PlotField {
                    name: field_name,
                    value,
                    span: field_start.merge(field_end),
                });
            }
            self.expect(Token::Comma)?;
        }
        self.expect(Token::RBrace)?;

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Figure(FigureDecl {
                name,
                plot_names,
                fields,
            }),
            span,
        })
    }
}
