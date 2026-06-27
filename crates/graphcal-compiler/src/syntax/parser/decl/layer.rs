use crate::syntax::ast::PlotPropertyName;
use crate::syntax::ast::{DeclKind, Declaration, LayerDecl, PlotField, Visibility};
use crate::syntax::decl_name::DeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse a layer declaration: `layer name = { plots: [a, b], title: "..." };`
    pub(super) fn parse_layer(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Layer)?;
        let name = self.parse_any_ident()?.into_spanned::<DeclName>();
        self.expect(Token::Eq)?;

        // Parse field block: { plots: [...], title: "...", ... }
        self.expect(Token::LBrace)?;
        let mut plots_seen = false;
        let mut plot_names = Vec::new();
        let mut fields = Vec::new();

        while self.lexer.peek() != Some(&Token::RBrace) {
            let field_name = self.parse_any_ident()?;
            let field_start = field_name.span;
            self.expect(Token::Colon)?;

            if field_name.name == "plots" {
                // Duplicate fields would silently shadow each other (#844).
                if plots_seen {
                    return Err(self.duplicate_plot_field(
                        "plots",
                        "layer declaration",
                        field_start,
                    ));
                }
                plots_seen = true;
                // Parse plots: [name1, name2, ...]
                self.expect(Token::LBracket)?;
                while self.lexer.peek() != Some(&Token::RBracket) {
                    let plot_name = self.parse_any_ident()?.into_spanned::<ScopedName>();
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
                    .any(|f: &PlotField| f.name.value.as_str() == field_name.name)
                {
                    return Err(self.duplicate_plot_field(
                        &field_name.name,
                        "layer declaration",
                        field_start,
                    ));
                }
                // Parse regular field: name: expr
                let value = self.parse_expr()?;
                let field_end = value.span;
                fields.push(PlotField {
                    name: field_name.into_spanned::<PlotPropertyName>(),
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

        // A layer with no plots renders an empty (non-renderable) layer
        // spec — always a mistake (#843).
        if plot_names.is_empty() {
            return Err(ParseError::EmptyCompositionPlots {
                kind: "layer",
                src: self.named_source(),
                span: span.into(),
            });
        }

        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Layer(LayerDecl {
                visibility: Visibility::Private,
                name,
                plot_names,
                fields,
            }),
            span,
        })
    }
}
