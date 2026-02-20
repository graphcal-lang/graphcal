use crate::ast::{
    DeclKind, Declaration, DeriveOp, FnBody, FnDecl, FnParam, GenericConstraint, GenericParam,
    LetBinding,
};
use crate::names::{FnName, GenericParamName, Spanned};
use crate::token::Token;

use super::{ParseError, Parser, is_lower_snake_case};

impl Parser<'_> {
    /// Parse a function declaration:
    /// `fn NAME<GENERICS>(PARAMS) -> TYPE = EXPR;`
    /// `fn NAME<GENERICS>(PARAMS) -> TYPE { STMTS EXPR }`
    pub(super) fn parse_fn_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Fn)?;
        let name = self
            .parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?
            .into_spanned::<FnName>();

        // Optional generic params: <D: Dim, E: Dim>
        let generic_params = if self.lexer.peek() == Some(&Token::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        // Parameter list: (param, param, ...)
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            params.push(self.parse_fn_param()?);
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;

        // Return type: -> TypeExpr
        self.expect(Token::Arrow)?;
        let return_type = self.parse_type_expr()?;

        // Body: either `= expr;` (short) or `{ stmts expr }` (block)
        let (body, end_span) = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            let expr = self.parse_expr()?;
            let (_, semi_span) = self.expect(Token::Semicolon)?;
            (FnBody::Short(expr), semi_span)
        } else {
            let (_, lbrace_span) = self.expect(Token::LBrace)?;
            let (stmts, expr) = self.parse_block_contents()?;
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            let _ = lbrace_span; // span captured by rbrace
            (
                FnBody::Block {
                    stmts,
                    expr: Box::new(expr),
                },
                rbrace_span,
            )
        };

        let span = start_span.merge(end_span);
        Ok(Declaration {
            attributes: vec![],
            kind: DeclKind::Fn(FnDecl {
                name,
                generic_params,
                params,
                return_type,
                body,
            }),
            span,
        })
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
                "Type" => GenericConstraint::Type,
                _ => {
                    return Err(self.unexpected_token(
                        "`Dim`, `Index`, or `Type`",
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

    /// Parse a derive clause: `derive(Add, Sub, Neg)`
    pub(super) fn parse_derive_clause(&mut self) -> Result<Vec<Spanned<DeriveOp>>, ParseError> {
        // Consume the `derive` identifier
        self.lexer.next_token();
        self.expect(Token::LParen)?;
        let mut derives = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            let op_ident = self.parse_any_ident()?;
            let op = match op_ident.name.as_str() {
                "Add" => DeriveOp::Add,
                "Sub" => DeriveOp::Sub,
                "Neg" => DeriveOp::Neg,
                _ => {
                    return Err(self.unexpected_token(
                        "`Add`, `Sub`, or `Neg`",
                        &op_ident.name,
                        op_ident.span,
                    ));
                }
            };
            derives.push(Spanned::new(op, op_ident.span));
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;
        Ok(derives)
    }

    /// Parse a function parameter: `name: TypeExpr`
    fn parse_fn_param(&mut self) -> Result<FnParam, ParseError> {
        let name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        Ok(FnParam { name, type_ann })
    }

    /// Parse the contents of a block: `let` bindings followed by a final expression.
    /// Shared between block expressions and function block bodies.
    pub(super) fn parse_block_contents(
        &mut self,
    ) -> Result<(Vec<LetBinding>, crate::ast::Expr), ParseError> {
        let mut stmts = Vec::new();
        while self.lexer.peek() == Some(&Token::Let) {
            stmts.push(self.parse_let_binding()?);
        }
        let expr = self.parse_expr()?;
        Ok((stmts, expr))
    }
}
