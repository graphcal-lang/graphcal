use crate::syntax::ast::{DagDecl, DeclKind, Declaration};
use crate::syntax::names::DeclName;
use crate::syntax::token::Token;

use super::super::{ParseError, Parser, is_lower_snake_case};

impl Parser<'_> {
    /// Parse a dag declaration: `dag name { declarations... }`
    ///
    /// The body is parsed as a list of declarations (same as file-level parsing).
    /// `dag` name must be `lower_snake_case`.
    pub(super) fn parse_dag_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dag)?;

        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<DeclName>();

        self.expect(Token::LBrace)?;

        // Parse declarations inside the block (same as file-level)
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
            is_pub: false,
            kind: DeclKind::Dag(DagDecl { name, body, span }),
            span,
        })
    }
}
