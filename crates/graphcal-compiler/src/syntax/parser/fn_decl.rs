use crate::syntax::ast::{Declaration, GenericConstraint, GenericParam, LetBinding};
use crate::syntax::names::GenericParamName;
use crate::syntax::token::Token;

use super::{ParseError, Parser};

impl Parser<'_> {
    /// Emit an error when the `fn` keyword is encountered.
    /// `fn` declarations are no longer supported; users should use `dag` blocks instead.
    pub(super) fn parse_fn_error(&mut self) -> Result<Declaration, ParseError> {
        let (_, span) = self.expect(Token::Fn)?;
        // Attempt error recovery: consume tokens up to `;` or a `{...}` block.
        let mut brace_depth = 0u32;
        loop {
            match self.lexer.peek() {
                None => break,
                Some(Token::Semicolon) if brace_depth == 0 => {
                    self.lexer.next_token(); // consume ';'
                    break;
                }
                Some(Token::LBrace) => {
                    brace_depth += 1;
                    self.lexer.next_token();
                }
                Some(Token::RBrace) => {
                    if brace_depth <= 1 {
                        self.lexer.next_token(); // consume final '}'
                        break;
                    }
                    brace_depth -= 1;
                    self.lexer.next_token();
                }
                _ => {
                    self.lexer.next_token();
                }
            }
        }
        Err(self.unexpected_token(
            "fn is no longer supported; use dag blocks instead",
            "fn",
            span,
        ))
    }

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

    /// Parse the contents of a block: `let` bindings followed by a final expression.
    /// Shared between block expressions and function block bodies.
    pub(super) fn parse_block_contents(
        &mut self,
    ) -> Result<(Vec<LetBinding>, crate::syntax::ast::Expr), ParseError> {
        let mut stmts = Vec::new();
        while self.lexer.peek() == Some(&Token::Let) {
            stmts.push(self.parse_let_binding()?);
        }
        let expr = self.parse_expr()?;
        Ok((stmts, expr))
    }
}
