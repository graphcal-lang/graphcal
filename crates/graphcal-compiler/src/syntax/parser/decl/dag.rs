use crate::syntax::ast::{DagDecl, DeclKind, Declaration, Visibility};
use crate::syntax::names::DeclName;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse a dag declaration: `dag name { declarations... }`
    ///
    /// The body is parsed as a list of declarations (same as file-level parsing).
    /// `dag` name must be `lower_snake_case`.
    pub(super) fn parse_dag_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dag)?;

        let name = self.parse_any_ident()?.into_spanned::<DeclName>();

        self.expect(Token::LBrace)?;

        // Parse declarations inside the block (same as file-level).
        let mut body = Vec::new();
        while self.lexer.peek() != Some(&Token::RBrace) {
            if self.lexer.peek().is_none() {
                return Err(self.unexpected_eof("`}` to close `dag` block"));
            }
            body.push(self.parse_declaration()?);
        }

        let (_, end_span) = self.expect(Token::RBrace)?;

        let span = start_span.merge(end_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Dag(DagDecl {
                visibility: Visibility::Private,
                name,
                body,
                span,
            }),
            span,
        })
    }
}
