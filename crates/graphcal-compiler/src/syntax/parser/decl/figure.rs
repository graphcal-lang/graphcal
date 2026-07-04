use crate::syntax::ast::{DeclKind, Declaration, FigureDecl, Visibility};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse a figure declaration: `figure name = { plots: [a, b], title: "..." };`
    pub(super) fn parse_figure(&mut self) -> Result<Declaration, ParseError> {
        let parts = self.parse_composition_decl_parts(Token::Figure, "figure")?;
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Figure(FigureDecl {
                visibility: Visibility::Private,
                name: parts.name,
                plot_names: parts.plot_names,
                fields: parts.fields,
            }),
            span: parts.span,
        })
    }
}
