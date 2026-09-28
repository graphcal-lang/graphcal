use crate::syntax::ast::{DeclKind, Declaration, LayerDecl, Visibility};
use crate::syntax::token::Token;

use super::super::{CompositionKind, ParseError, Parser};

impl Parser<'_> {
    /// Parse a layer declaration: `layer name = { plots: [a, b], title: "..." };`
    pub(super) fn parse_layer(
        &mut self,
        visibility: Visibility,
    ) -> Result<Declaration, ParseError> {
        let parts = self.parse_composition_decl_parts(Token::Layer, CompositionKind::Layer)?;
        Ok(Declaration {
            doc: None,
            attributes: vec![],
            kind: DeclKind::Layer(LayerDecl {
                visibility,
                name: parts.name,
                plot_names: parts.plot_names,
                fields: parts.fields,
            }),
            span: parts.span,
        })
    }
}
