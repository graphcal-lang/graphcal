use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::ast::*;
use crate::lexer::Lexer;
use crate::span::Span;
use crate::token::Token;

/// Rich parse error with miette diagnostics.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum ParseError {
    #[error("unexpected token `{found}`")]
    #[diagnostic(code(kasuri::P001), help("expected {expected}"))]
    UnexpectedToken {
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("unexpected end of file")]
    #[diagnostic(code(kasuri::P002), help("expected {expected}"))]
    UnexpectedEof {
        expected: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("invalid number literal")]
    #[diagnostic(code(kasuri::P003))]
    InvalidNumber {
        reason: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("{reason}")]
        span: SourceSpan,
    },
}

pub struct Parser<'src> {
    lexer: Lexer<'src>,
    source: Arc<String>,
    source_name: String,
}

impl<'src> Parser<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: "input".to_string(),
        }
    }

    pub fn with_name(source: &'src str, name: &str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: name.to_string(),
        }
    }

    fn named_source(&self) -> NamedSource<Arc<String>> {
        NamedSource::new(&self.source_name, Arc::clone(&self.source))
    }

    /// Create a `ParseError` for an unexpected token.
    fn unexpected_token(&self, expected: &str, found: &str, span: Span) -> ParseError {
        ParseError::UnexpectedToken {
            expected: expected.to_string(),
            found: found.to_string(),
            src: self.named_source(),
            span: span.into(),
        }
    }

    /// Create a `ParseError` for an unexpected EOF.
    fn unexpected_eof(&mut self, expected: &str) -> ParseError {
        ParseError::UnexpectedEof {
            expected: expected.to_string(),
            src: self.named_source(),
            span: Span::new(self.lexer.current_offset(), 0).into(),
        }
    }

    pub fn parse_file(&mut self) -> Result<File, ParseError> {
        let mut declarations = Vec::new();
        while self.lexer.peek().is_some() {
            declarations.push(self.parse_declaration()?);
        }
        Ok(File { declarations })
    }

    fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        match self.lexer.peek() {
            Some(Token::Param) => self.parse_param(),
            Some(Token::Node) => self.parse_node(),
            Some(Token::Const) => self.parse_const(),
            Some(_) => {
                let (tok, span) = self.lexer.next_token().unwrap();
                Err(self.unexpected_token("`param`, `node`, or `const`", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("`param`, `node`, or `const`")),
        }
    }

    fn parse_param(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Param)?;
        let name = self.parse_lower_ident()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Param(ParamDecl { name, value }),
            span,
        })
    }

    fn parse_node(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Node)?;
        let name = self.parse_lower_ident()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Node(NodeDecl { name, value }),
            span,
        })
    }

    fn parse_const(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Const)?;
        let name = self.parse_upper_ident()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Const(ConstDecl { name, value }),
            span,
        })
    }

    // --- Expression parsing ---
    // Precedence (lowest to highest):
    //   1. if/else (conditional)
    //   2. || (or)
    //   3. && (and)
    //   4. ==, !=, <, >, <=, >= (comparison, non-chaining)
    //   5. +, - (additive)
    //   6. *, / (multiplicative)
    //   7. unary -, ! (prefix)
    //   8. ^ (power, right-associative)
    //   9. atoms

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_conditional()
    }

    fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
        if self.lexer.peek() == Some(&Token::If) {
            let (_, if_span) = self.lexer.next_token().unwrap();
            let condition = self.parse_expr()?;
            self.expect(Token::LBrace)?;
            let then_branch = self.parse_expr()?;
            self.expect(Token::RBrace)?;
            self.expect(Token::Else)?;
            self.expect(Token::LBrace)?;
            let else_branch = self.parse_expr()?;
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            let span = if_span.merge(rbrace_span);
            Ok(Expr {
                kind: ExprKind::If {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
                span,
            })
        } else {
            self.parse_or()
        }
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.lexer.peek() == Some(&Token::PipePipe) {
            self.lexer.next_token();
            let rhs = self.parse_and()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_comparison()?;
        while self.lexer.peek() == Some(&Token::AmpAmp) {
            self.lexer.next_token();
            let rhs = self.parse_comparison()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = match self.lexer.peek() {
            Some(Token::EqEq) => Some(BinOp::Eq),
            Some(Token::BangEq) => Some(BinOp::Ne),
            Some(Token::Lt) => Some(BinOp::Lt),
            Some(Token::Gt) => Some(BinOp::Gt),
            Some(Token::LtEq) => Some(BinOp::Le),
            Some(Token::GtEq) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.lexer.next_token();
            let rhs = self.parse_add()?;
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.lexer.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.lexer.next_token();
            let rhs = self.parse_mul()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.lexer.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.lexer.next_token();
            let rhs = self.parse_unary()?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Minus) => {
                let (_, op_span) = self.lexer.next_token().unwrap();
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                })
            }
            Some(Token::Bang) => {
                let (_, op_span) = self.lexer.next_token().unwrap();
                let operand = self.parse_unary()?;
                let span = op_span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                })
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Expr, ParseError> {
        let base = self.parse_atom()?;
        if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            // Right-associative: recurse into parse_unary, not parse_power
            let exp = self.parse_unary()?;
            let span = base.span.merge(exp.span);
            Ok(Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Pow,
                    lhs: Box::new(base),
                    rhs: Box::new(exp),
                },
                span,
            })
        } else {
            Ok(base)
        }
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.lexer.next_token().unwrap();
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                    ParseError::InvalidNumber {
                        reason: e.to_string(),
                        src: self.named_source(),
                        span: span.into(),
                    }
                })?;
                Ok(Expr {
                    kind: ExprKind::Number(value),
                    span,
                })
            }
            Some(Token::True) => {
                let (_, span) = self.lexer.next_token().unwrap();
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    span,
                })
            }
            Some(Token::False) => {
                let (_, span) = self.lexer.next_token().unwrap();
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    span,
                })
            }
            Some(Token::At) => {
                let (_, at_span) = self.lexer.next_token().unwrap();
                let ident = self.parse_lower_ident()?;
                let span = at_span.merge(ident.span);
                Ok(Expr {
                    kind: ExprKind::GraphRef(ident),
                    span,
                })
            }
            Some(Token::UpperIdent) => {
                let (_, span) = self.lexer.next_token().unwrap();
                let name = self.lexer.slice_at(span).to_string();
                Ok(Expr {
                    kind: ExprKind::ConstRef(Ident { name, span }),
                    span,
                })
            }
            Some(Token::LowerIdent) => {
                // Function call: name(args...)
                let (_, name_span) = self.lexer.next_token().unwrap();
                let name = self.lexer.slice_at(name_span).to_string();
                self.expect(Token::LParen)?;
                let mut args = Vec::new();
                if self.lexer.peek() != Some(&Token::RParen) {
                    args.push(self.parse_expr()?);
                    while self.lexer.peek() == Some(&Token::Comma) {
                        self.lexer.next_token();
                        args.push(self.parse_expr()?);
                    }
                }
                let (_, rparen_span) = self.expect(Token::RParen)?;
                let span = name_span.merge(rparen_span);
                Ok(Expr {
                    kind: ExprKind::FnCall {
                        name: Ident {
                            name,
                            span: name_span,
                        },
                        args,
                    },
                    span,
                })
            }
            Some(Token::LParen) => {
                self.lexer.next_token();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(_) => {
                let (tok, span) = self.lexer.next_token().unwrap();
                Err(self.unexpected_token("expression", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("expression")),
        }
    }

    // --- Helper methods ---

    fn expect(&mut self, expected: Token) -> Result<(Token, Span), ParseError> {
        let expected_str = format!("`{expected}`");
        match self.lexer.next_token() {
            Some((tok, span)) if tok == expected => Ok((tok, span)),
            Some((tok, span)) => Err(self.unexpected_token(&expected_str, &tok.to_string(), span)),
            None => Err(self.unexpected_eof(&expected_str)),
        }
    }

    fn parse_lower_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::LowerIdent, span)) => Ok(Ident {
                name: self.lexer.slice_at(span).to_string(),
                span,
            }),
            Some((tok, span)) => {
                Err(self.unexpected_token("lower_snake_case identifier", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("lower_snake_case identifier")),
        }
    }

    fn parse_upper_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::UpperIdent, span)) => Ok(Ident {
                name: self.lexer.slice_at(span).to_string(),
                span,
            }),
            Some((tok, span)) => {
                Err(self.unexpected_token("UPPER_SNAKE_CASE identifier", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("UPPER_SNAKE_CASE identifier")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_param_literal() {
        let file = Parser::new("param x = 42.0;").parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.name, "x");
                assert!(
                    matches!(p.value.kind, ExprKind::Number(n) if (n - 42.0).abs() < f64::EPSILON)
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_node_literal() {
        let file = Parser::new("node y = 3.14;").parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Node(n) => {
                assert_eq!(n.name.name, "y");
                assert!(
                    matches!(n.value.kind, ExprKind::Number(v) if (v - 3.14).abs() < f64::EPSILON)
                );
            }
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_const_literal() {
        let file = Parser::new("const G0 = 9.80665;").parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Const(c) => {
                assert_eq!(c.name.name, "G0");
            }
            _ => panic!("expected const"),
        }
    }

    #[test]
    fn parse_multiple_declarations() {
        let input = "param x = 1.0;\nparam y = 2.0;\nnode z = 3.0;";
        let file = Parser::new(input).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 3);
    }

    #[test]
    fn parse_bool_literal() {
        let file = Parser::new("param b = true;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(p.value.kind, ExprKind::Bool(true)));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_scientific_notation() {
        let file = Parser::new("param x = 3.98e5;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(
                    matches!(p.value.kind, ExprKind::Number(n) if (n - 398_000.0).abs() < f64::EPSILON)
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_underscore_number() {
        let file = Parser::new("param x = 200_000;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(
                    matches!(p.value.kind, ExprKind::Number(n) if (n - 200_000.0).abs() < f64::EPSILON)
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_error_missing_semicolon() {
        let result = Parser::new("param x = 1.0").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_unexpected_token() {
        let result = Parser::new("+ 1.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_with_comments() {
        let input = "// this is a comment\nparam x = 1.0;\n// another comment";
        let file = Parser::new(input).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
    }

    #[test]
    fn parse_declaration_spans() {
        let input = "param x = 1.0;";
        let file = Parser::new(input).parse_file().unwrap();
        let decl = &file.declarations[0];
        assert_eq!(decl.span.offset, 0);
        assert_eq!(decl.span.len, 14);
    }

    // --- Expression parsing tests ---

    /// Helper: parse a single node declaration and return its expression.
    fn parse_node_expr(input: &str) -> Expr {
        let full = format!("node x = {input};");
        let file = Parser::new(&full).parse_file().unwrap();
        match file.declarations.into_iter().next().unwrap().kind {
            DeclKind::Node(n) => n.value,
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_arithmetic_precedence() {
        // 1.0 + 2.0 * 3.0 should be Add(1, Mul(2, 3))
        let expr = parse_node_expr("1.0 + 2.0 * 3.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        if let ExprKind::BinOp { rhs, .. } = &expr.kind {
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Mul, .. }));
        }
    }

    #[test]
    fn parse_left_associative_add() {
        // 1.0 - 2.0 - 3.0 should be Sub(Sub(1, 2), 3)
        let expr = parse_node_expr("1.0 - 2.0 - 3.0");
        if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Sub);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Sub, .. }));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_power_right_assoc() {
        // 2.0 ^ 3.0 ^ 2.0 should be Pow(2, Pow(3, 2))
        let expr = parse_node_expr("2.0 ^ 3.0 ^ 2.0");
        if let ExprKind::BinOp { op, rhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Pow);
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Pow, .. }));
        } else {
            panic!("expected Pow");
        }
    }

    #[test]
    fn parse_neg_power_precedence() {
        // -@x ^ 2.0 should be Neg(Pow(@x, 2))
        let expr = parse_node_expr("-@x ^ 2.0");
        if let ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } = &expr.kind
        {
            assert!(matches!(
                operand.kind,
                ExprKind::BinOp { op: BinOp::Pow, .. }
            ));
        } else {
            panic!("expected Neg(Pow(...))");
        }
    }

    #[test]
    fn parse_graph_ref() {
        let expr = parse_node_expr("@x + 1.0");
        if let ExprKind::BinOp { lhs, .. } = &expr.kind {
            assert!(matches!(&lhs.kind, ExprKind::GraphRef(id) if id.name == "x"));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_const_ref() {
        let expr = parse_node_expr("PI * 2.0");
        if let ExprKind::BinOp { lhs, .. } = &expr.kind {
            assert!(matches!(&lhs.kind, ExprKind::ConstRef(id) if id.name == "PI"));
        } else {
            panic!("expected BinOp");
        }
    }

    #[test]
    fn parse_function_call_one_arg() {
        let expr = parse_node_expr("sqrt(@x)");
        if let ExprKind::FnCall { name, args } = &expr.kind {
            assert_eq!(name.name, "sqrt");
            assert_eq!(args.len(), 1);
            assert!(matches!(&args[0].kind, ExprKind::GraphRef(id) if id.name == "x"));
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_function_call_two_args() {
        let expr = parse_node_expr("atan2(@a, @b)");
        if let ExprKind::FnCall { name, args } = &expr.kind {
            assert_eq!(name.name, "atan2");
            assert_eq!(args.len(), 2);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_function_call_zero_args() {
        // While no built-in has zero args, the syntax should support it
        let expr = parse_node_expr("foo()");
        if let ExprKind::FnCall { name, args } = &expr.kind {
            assert_eq!(name.name, "foo");
            assert_eq!(args.len(), 0);
        } else {
            panic!("expected FnCall");
        }
    }

    #[test]
    fn parse_if_else() {
        let expr = parse_node_expr("if @x > 0.0 { @x } else { 0.0 }");
        if let ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } = &expr.kind
        {
            assert!(matches!(
                condition.kind,
                ExprKind::BinOp { op: BinOp::Gt, .. }
            ));
            assert!(matches!(
                &then_branch.kind,
                ExprKind::GraphRef(id) if id.name == "x"
            ));
            assert!(matches!(else_branch.kind, ExprKind::Number(_)));
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn parse_nested_parens() {
        // (1.0 + 2.0) * 3.0 should be Mul(Add(1, 2), 3)
        let expr = parse_node_expr("(1.0 + 2.0) * 3.0");
        if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
            assert_eq!(*op, BinOp::Mul);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        } else {
            panic!("expected Mul");
        }
    }

    #[test]
    fn parse_boolean_and() {
        let expr = parse_node_expr("@a > 0.0 && @b > 0.0");
        if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::And);
            assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
        } else {
            panic!("expected And");
        }
    }

    #[test]
    fn parse_boolean_or() {
        let expr = parse_node_expr("@a > 0.0 || @b > 0.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Or, .. }));
    }

    #[test]
    fn parse_unary_neg() {
        let expr = parse_node_expr("-1.0");
        assert!(matches!(
            expr.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    #[test]
    fn parse_unary_not() {
        let expr = parse_node_expr("!true");
        assert!(matches!(
            expr.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn parse_complex_expression() {
        // @v_exhaust * ln(@mass_ratio) — from the milestone test
        let expr = parse_node_expr("@v_exhaust * ln(@mass_ratio)");
        if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
            assert_eq!(*op, BinOp::Mul);
            assert!(matches!(&lhs.kind, ExprKind::GraphRef(id) if id.name == "v_exhaust"));
            assert!(matches!(&rhs.kind, ExprKind::FnCall { name, .. } if name.name == "ln"));
        } else {
            panic!("expected Mul");
        }
    }

    #[test]
    fn parse_comparison_eq() {
        let expr = parse_node_expr("@x == 1.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Eq, .. }));
    }

    #[test]
    fn parse_comparison_ne() {
        let expr = parse_node_expr("@x != 1.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Ne, .. }));
    }

    // --- Integration tests: parse fixture files ---

    #[test]
    fn parse_rocket_ksr() {
        let source = include_str!("../../../tests/fixtures/rocket.ksr");
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 7);

        let names: Vec<&str> = file
            .declarations
            .iter()
            .map(|d| match &d.kind {
                DeclKind::Param(p) => p.name.name.as_str(),
                DeclKind::Node(n) => n.name.name.as_str(),
                DeclKind::Const(c) => c.name.name.as_str(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "dry_mass",
                "fuel_mass",
                "isp",
                "G0",
                "v_exhaust",
                "mass_ratio",
                "delta_v"
            ]
        );
    }

    #[test]
    fn parse_constants_ksr() {
        let source = include_str!("../../../tests/fixtures/constants.ksr");
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 7);

        let names: Vec<&str> = file
            .declarations
            .iter()
            .map(|d| match &d.kind {
                DeclKind::Param(p) => p.name.name.as_str(),
                DeclKind::Node(n) => n.name.name.as_str(),
                DeclKind::Const(c) => c.name.name.as_str(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "G0",
                "TWO_G0",
                "HALF_PI",
                "SQRT2",
                "radius",
                "circumference",
                "area"
            ]
        );
    }
}
