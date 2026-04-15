use crate::syntax::ast::{GenericConstraint, GenericParam};
use crate::syntax::names::GenericParamName;
use crate::syntax::token::Token;

use super::{ParseError, Parser};

impl Parser<'_> {
    /// Parse generic parameters: `<D: Dim, E: Dim>`
    pub(super) fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>, ParseError> {
        self.expect(Token::Lt)?;
        let mut params = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::Gt) {
                break;
            }
            let name = self.parse_any_ident()?.into_spanned::<GenericParamName>();
            self.expect(Token::Colon)?;
            let constraint_ident = self.parse_any_ident()?;
            let constraint = match constraint_ident.name.as_str() {
                "Dim" => GenericConstraint::Dim,
                "Index" => GenericConstraint::Index,
                "Nat" => GenericConstraint::Nat,
                "Type" => GenericConstraint::Type,
                _ => {
                    return Err(self.unexpected_token(
                        "`Dim`, `Index`, `Nat`, or `Type`",
                        &constraint_ident.name,
                        constraint_ident.span,
                    ));
                }
            };
            // Optional default: `= TypeExpr`
            let default = if self.lexer.peek() == Some(&Token::Eq) {
                self.lexer.next_token(); // consume `=`
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            params.push(GenericParam {
                name,
                constraint,
                default,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::Gt)?;
        Ok(params)
    }
}
