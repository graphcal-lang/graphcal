use crate::syntax::ast::{DeclKind, Declaration, LayerDecl, Visibility};
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse a layer declaration: `layer name = { plots: [a, b], title: "..." };`
    pub(super) fn parse_layer(&mut self) -> Result<Declaration, ParseError> {
        let parts = self.parse_composition_decl_parts(Token::Layer, "layer")?;
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Layer(LayerDecl {
                visibility: Visibility::Private,
                name: parts.name,
                plot_names: parts.plot_names,
                fields: parts.fields,
            }),
            span: parts.span,
        })
    }
}
