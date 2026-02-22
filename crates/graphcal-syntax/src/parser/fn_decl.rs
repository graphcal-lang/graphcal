use crate::ast::{
    DeclKind, Declaration, FnBody, FnDecl, FnParam, GenericConstraint, GenericParam, LetBinding,
};
use crate::names::{FnName, GenericParamName};
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;
    use crate::ast::{DeclKind, ExprKind, FnBody, GenericConstraint, TypeExprKind};

    #[test]
    fn parse_fn_short_form() {
        let source = "fn double(x: Dimensionless) -> Dimensionless = x * 2.0;";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "double");
                assert!(f.generic_params.is_empty());
                assert_eq!(f.params.len(), 1);
                assert_eq!(f.params[0].name.name, "x");
                assert!(matches!(f.body, FnBody::Short(_)));
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_block_form() {
        let source = "fn add_one(x: Dimensionless) -> Dimensionless { let one = 1.0; x + one }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "add_one");
                match &f.body {
                    FnBody::Block { stmts, expr } => {
                        assert_eq!(stmts.len(), 1);
                        assert_eq!(stmts[0].name.name, "one");
                        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                    }
                    FnBody::Short(_) => panic!("expected block body"),
                }
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_with_generics() {
        let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "lerp");
                assert_eq!(f.generic_params.len(), 1);
                assert_eq!(f.generic_params[0].name.value.as_str(), "D");
                assert_eq!(f.generic_params[0].constraint, GenericConstraint::Dim);
                assert_eq!(f.params.len(), 3);
                assert_eq!(f.params[0].name.name, "a");
                assert_eq!(f.params[1].name.name, "b");
                assert_eq!(f.params[2].name.name, "t");
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_multiple_generics() {
        let source = "fn convert<A: Dim, B: Dim>(x: A, y: B) -> A = x;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.generic_params.len(), 2);
                assert_eq!(f.generic_params[0].name.value.as_str(), "A");
                assert_eq!(f.generic_params[1].name.value.as_str(), "B");
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_zero_args() {
        let source = "fn pi_val() -> Dimensionless = PI;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.name.value.as_str(), "pi_val");
                assert!(f.params.is_empty());
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_trailing_comma() {
        let source = "fn add(x: Dimensionless, y: Dimensionless,) -> Dimensionless = x + y;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.params.len(), 2);
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_dim_expr_type() {
        let source = "fn speed(d: Length, t: Time) -> Length / Time = d / t;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.params.len(), 2);
                assert!(matches!(f.return_type.kind, TypeExprKind::DimExpr(_)));
            }
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_block_no_lets() {
        let source = "fn identity(x: Dimensionless) -> Dimensionless { x }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => match &f.body {
                FnBody::Block { stmts, .. } => assert!(stmts.is_empty()),
                FnBody::Short(_) => panic!("expected block body"),
            },
            other => panic!("expected Fn, got {other:?}"),
        }
    }

    #[test]
    fn parse_fn_mixed_with_other_decls() {
        let source = r"
        const TWO: Dimensionless = 2.0;
        fn double(x: Dimensionless) -> Dimensionless = x * TWO;
        param val: Dimensionless = 5.0;
        node result: Dimensionless = double(@val);
    ";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 4);
        assert!(matches!(file.declarations[0].kind, DeclKind::Const(_)));
        assert!(matches!(file.declarations[1].kind, DeclKind::Fn(_)));
        assert!(matches!(file.declarations[2].kind, DeclKind::Param(_)));
        assert!(matches!(file.declarations[3].kind, DeclKind::Node(_)));
    }

    #[test]
    fn parse_generic_fn_with_index_constraint() {
        let source = "fn total<D: Dim, I: Index>(values: D) -> D = values;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Fn(f) => {
                assert_eq!(f.generic_params.len(), 2);
                assert_eq!(f.generic_params[0].name.value.as_str(), "D");
                assert_eq!(f.generic_params[0].constraint, GenericConstraint::Dim);
                assert_eq!(f.generic_params[1].name.value.as_str(), "I");
                assert_eq!(f.generic_params[1].constraint, GenericConstraint::Index);
            }
            _ => panic!("expected fn"),
        }
    }
}
