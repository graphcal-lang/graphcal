use crate::ast::TypeExpr;
use crate::ast::{
    BinOp, Expr, ExprKind, FieldInit, ForBinding, Ident, LetBinding, MatchArm, MatchPattern,
    PatternBinding,
};
use crate::names::{DeclName, FieldName, IndexName, Spanned, StructTypeName, VariantName};
use crate::span::Span;
use crate::token::Token;

use super::expr::token_to_comparison_op;
use super::{ParseError, Parser, is_lower_snake_case, is_pascal_case};

impl Parser<'_> {
    // --- Block and let binding parsing ---

    /// Parse a let binding: `let IDENT (: TypeExpr)? = Expr ;`
    pub(super) fn parse_let_binding(&mut self) -> Result<LetBinding, ParseError> {
        let (_, let_span) = self.expect(Token::Let)?;
        let name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;

        // Optional type annotation
        let type_ann = if self.lexer.peek() == Some(&Token::Colon) {
            self.lexer.next_token(); // consume ':'
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = let_span.merge(semi_span);

        Ok(LetBinding {
            name,
            type_ann,
            value,
            span,
        })
    }

    // --- Match expression ---

    /// Parse a match expression:
    /// `match expr { Variant1 { field } => body, Variant2 => body }`
    pub(super) fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::Match)?;
        // Support tuple scrutinee syntax: `match (a, b) { ... }`
        // by desugaring to nested `if` expressions.
        if self.lexer.peek() == Some(&Token::LParen) {
            self.lexer.next_token(); // consume '('
            let first = self.parse_expr()?;
            if self.lexer.peek() == Some(&Token::Comma) {
                let mut tuple_scrutinee = vec![first];
                while self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                    tuple_scrutinee.push(self.parse_expr()?);
                }
                self.expect(Token::RParen)?;
                self.expect(Token::LBrace)?;
                let tuple_match = self.parse_tuple_match_arms(&tuple_scrutinee, start_span)?;
                let (_, end_span) = self.expect(Token::RBrace)?;
                let span = start_span.merge(end_span);
                return Ok(Expr {
                    kind: tuple_match.kind,
                    span,
                });
            }
            self.expect(Token::RParen)?;
            self.expect(Token::LBrace)?;
            let scrutinee = Box::new(first);
            let mut arms = Vec::new();
            loop {
                if self.lexer.peek() == Some(&Token::RBrace) {
                    break;
                }
                arms.push(self.parse_match_arm()?);
                // Optional comma between arms
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                }
            }
            let (_, end_span) = self.expect(Token::RBrace)?;
            let span = start_span.merge(end_span);
            return Ok(Expr {
                kind: ExprKind::Match { scrutinee, arms },
                span,
            });
        }
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(Token::LBrace)?;

        let mut arms = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            arms.push(self.parse_match_arm()?);
            // Optional comma between arms
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            }
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Match { scrutinee, arms },
            span,
        })
    }

    /// Parse tuple-match arms and lower them to nested `if` expressions.
    ///
    /// Supported form:
    /// `match (a, b) { (X, Y) => expr, _ => fallback }`
    fn parse_tuple_match_arms(
        &mut self,
        tuple_scrutinee: &[Expr],
        start_span: Span,
    ) -> Result<Expr, ParseError> {
        let arity = tuple_scrutinee.len();
        let mut cases: Vec<(Vec<Expr>, Expr)> = Vec::new();
        let mut fallback: Option<Expr> = None;

        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }

            if self.lexer.peek() == Some(&Token::Underscore) {
                self.lexer.next_token(); // consume '_'
                self.expect(Token::FatArrow)?;
                fallback = Some(self.parse_expr()?);
            } else {
                self.expect(Token::LParen)?;
                let mut pattern_elems = Vec::new();
                loop {
                    pattern_elems.push(self.parse_expr()?);
                    if self.lexer.peek() == Some(&Token::Comma) {
                        self.lexer.next_token();
                    } else {
                        break;
                    }
                }
                self.expect(Token::RParen)?;
                if pattern_elems.len() != arity {
                    return Err(self.unexpected_token(
                        &format!("tuple pattern of arity {arity}"),
                        &format!("tuple pattern of arity {}", pattern_elems.len()),
                        start_span,
                    ));
                }
                self.expect(Token::FatArrow)?;
                let body = self.parse_expr()?;
                cases.push((pattern_elems, body));
            }

            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            }
        }

        // Build nested if-chain in reverse order.
        let mut else_expr = if let Some(fallback_expr) = fallback {
            fallback_expr
        } else if let Some((_, last_body)) = cases.pop() {
            last_body
        } else {
            return Err(self.unexpected_eof("at least one tuple match arm"));
        };

        for (pattern, body) in cases.into_iter().rev() {
            let mut cond: Option<Expr> = None;
            for (scrutinee_elem, pat_elem) in tuple_scrutinee.iter().zip(pattern.iter()) {
                let eq = Expr {
                    kind: ExprKind::BinOp {
                        op: BinOp::Eq,
                        lhs: Box::new(scrutinee_elem.clone()),
                        rhs: Box::new(pat_elem.clone()),
                    },
                    span: scrutinee_elem.span.merge(pat_elem.span),
                };
                cond = Some(if let Some(prev) = cond {
                    Expr {
                        kind: ExprKind::BinOp {
                            op: BinOp::And,
                            lhs: Box::new(prev.clone()),
                            rhs: Box::new(eq.clone()),
                        },
                        span: prev.span.merge(eq.span),
                    }
                } else {
                    eq
                });
            }
            let condition = cond.ok_or_else(|| self.unexpected_eof("condition"))?;
            else_expr = Expr {
                kind: ExprKind::If {
                    condition: Box::new(condition.clone()),
                    then_branch: Box::new(body.clone()),
                    else_branch: Box::new(else_expr.clone()),
                },
                span: condition.span.merge(else_expr.span),
            };
        }

        Ok(Expr {
            kind: else_expr.kind,
            span: start_span.merge(else_expr.span),
        })
    }

    /// Parse a single match arm: `VariantName { field1, field2: _ } => expr`
    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_match_pattern()?;
        self.expect(Token::FatArrow)?;
        let body = self.parse_expr()?;
        let span = pattern.span.merge(body.span);
        Ok(MatchArm {
            pattern,
            body,
            span,
        })
    }

    /// Parse a match pattern:
    /// - Tagged union: `VariantName { field1, field2: binding }` or bare `VariantName`
    /// - Index variant: `Index::Variant` (qualified form)
    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let first_ident = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
        let start_span = first_ident.span;

        if self.lexer.peek() == Some(&Token::ColonColon) {
            // Qualified index variant pattern: Index::Variant
            self.lexer.next_token(); // consume '::'
            let variant_ident = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
            let end_span = variant_ident.span;
            return Ok(MatchPattern {
                qualified_index: Some(Spanned::new(
                    IndexName::new(&first_ident.name),
                    first_ident.span,
                )),
                variant_name: Spanned::new(
                    VariantName::new(&variant_ident.name),
                    variant_ident.span,
                ),
                bindings: vec![],
                span: start_span.merge(end_span),
            });
        }

        // Tagged union pattern: bare VariantName or VariantName { fields }
        let variant_name = Spanned::new(VariantName::new(&first_ident.name), first_ident.span);

        let (bindings, end_span) = if self.lexer.peek() == Some(&Token::LBrace) {
            self.lexer.next_token(); // consume '{'
            let mut bindings = Vec::new();
            loop {
                if self.lexer.peek() == Some(&Token::RBrace) {
                    break;
                }
                bindings.push(self.parse_pattern_binding()?);
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            let (_, rbrace_span) = self.expect(Token::RBrace)?;
            (bindings, rbrace_span)
        } else {
            // Bare variant: no bindings
            (Vec::new(), start_span)
        };

        Ok(MatchPattern {
            qualified_index: None,
            variant_name,
            bindings,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single pattern binding:
    /// - `field_name` (shorthand: bind to same name)
    /// - `field_name: var_name` (rename)
    /// - `field_name: _` (wildcard)
    fn parse_pattern_binding(&mut self) -> Result<PatternBinding, ParseError> {
        let field_ident = self.parse_any_ident()?;
        let field = Spanned::new(FieldName::new(&field_ident.name), field_ident.span);

        if self.lexer.peek() == Some(&Token::Colon) {
            self.lexer.next_token(); // consume ':'
            // Check for wildcard `_`
            if self.lexer.peek() == Some(&Token::Underscore) {
                let (_, span) = self.advance()?;
                return Ok(PatternBinding::Wildcard { field, span });
            }
            // Renamed binding: `field_name: var_name`
            let var = self.parse_any_ident()?;
            Ok(PatternBinding::Bind { field, var })
        } else {
            // Shorthand: bind to same name as field
            Ok(PatternBinding::Bind {
                field,
                var: field_ident,
            })
        }
    }

    // --- Struct construction ---

    pub(super) fn parse_struct_construction(
        &mut self,
        type_name: Spanned<StructTypeName>,
    ) -> Result<Expr, ParseError> {
        self.parse_struct_construction_with_type_args(type_name, Vec::new())
    }

    /// Parse struct construction with explicit type args: `Vec3<Length, ECI> { x: 1 km, ... }`
    /// Called after the type args have already been parsed.
    pub(super) fn parse_struct_construction_with_type_args(
        &mut self,
        type_name: Spanned<StructTypeName>,
        type_args: Vec<TypeExpr>,
    ) -> Result<Expr, ParseError> {
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();

        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let field_name = self.parse_any_ident()?.into_spanned::<FieldName>();

            // Check for `:` (explicit value) or shorthand (just name)
            let value = if self.lexer.peek() == Some(&Token::Colon) {
                self.lexer.next_token(); // consume ':'
                Some(self.parse_expr()?)
            } else {
                None // shorthand: field name matches variable name
            };

            fields.push(FieldInit {
                name: field_name,
                value,
            });

            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = type_name.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::StructConstruction {
                type_name,
                type_args,
                fields,
            },
            span,
        })
    }

    // --- For comprehension ---

    /// Parse a for comprehension: `for m: Maneuver, n: Phase { expr }`
    pub(super) fn parse_for_comp(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::For)?;
        let mut bindings = Vec::new();
        loop {
            let var = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
            self.expect(Token::Colon)?;
            let index = self
                .parse_ident_with_casing("PascalCase", is_pascal_case)?
                .into_spanned::<IndexName>();
            bindings.push(ForBinding { var, index });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        self.expect(Token::LBrace)?;
        // Optional tuple-key sugar:
        // `for p: Phase, m: Maneuver { (p, m) => expr }`
        if self.lexer.peek() == Some(&Token::LParen) {
            self.lexer.next_token(); // consume '('
            loop {
                self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
                if self.lexer.peek() == Some(&Token::Comma) {
                    self.lexer.next_token();
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            self.expect(Token::FatArrow)?;
        }
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::ForComp {
                bindings,
                body: Box::new(body),
            },
            span,
        })
    }

    // --- Scan expression ---

    /// Parse a scan expression: `scan(source, init, |acc, val| body)` → `ExprKind::Scan`
    pub(super) fn parse_scan(&mut self, name_ident: &Ident) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let first_expr = self.parse_expr()?;
        self.expect(Token::Comma)?;
        let init = self.parse_expr()?;
        self.expect(Token::Comma)?;
        // Parse lambda: |acc, val| body
        self.expect(Token::Pipe)?;
        let acc_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Comma)?;
        let val_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Pipe)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        let span = name_ident.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Scan {
                source: Box::new(first_expr),
                init: Box::new(init),
                acc_name,
                val_name,
                body: Box::new(body),
            },
            span,
        })
    }

    // --- Unfold expression ---

    /// Parse an unfold expression: `unfold(init, |prev_i, i| body)` → `ExprKind::Unfold`
    pub(super) fn parse_unfold(&mut self, name_ident: &Ident) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let init = self.parse_expr()?;
        self.expect(Token::Comma)?;
        self.expect(Token::Pipe)?;
        let prev_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Comma)?;
        let curr_name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Pipe)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        let span = name_ident.span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Unfold {
                init: Box::new(init),
                prev_name,
                curr_name,
                body: Box::new(body),
            },
            span,
        })
    }

    // --- Block disambiguation helpers ---

    /// Parse a block expression after `{` has been consumed.
    pub(super) fn parse_block_after_open_brace(
        &mut self,
        start_span: Span,
    ) -> Result<Expr, ParseError> {
        let (stmts, expr) = self.parse_block_contents()?;
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Block {
                stmts,
                expr: Box::new(expr),
            },
            span,
        })
    }

    /// Parse a block expression after `{` and a `PascalCase` ident have been consumed.
    /// The ident was consumed during map literal disambiguation but turned out not
    /// to be a map literal (no `::` followed). Reconstruct parsing state.
    pub(super) fn parse_block_after_open_brace_and_ident(
        &mut self,
        start_span: Span,
        ident_name: &str,
        ident_span: Span,
    ) -> Result<Expr, ParseError> {
        // The consumed PascalCase ident is the start of an expression in the block.
        // It could be a struct construction (PascalCase { ... }) or a ConstRef.
        let first_expr = if is_pascal_case(ident_name) && self.lexer.peek() == Some(&Token::LBrace)
        {
            self.parse_struct_construction(Spanned::new(
                StructTypeName::new(ident_name),
                ident_span,
            ))?
        } else {
            // Treat as ConstRef (UPPER_SNAKE_CASE or PascalCase used as const)
            Expr {
                kind: ExprKind::ConstRef(Spanned::new(DeclName::new(ident_name), ident_span)),
                span: ident_span,
            }
        };
        let expr = self.continue_parsing_expr(first_expr)?;
        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr {
            kind: ExprKind::Block {
                stmts: vec![],
                expr: Box::new(expr),
            },
            span,
        })
    }

    /// Continue parsing an expression from an already-parsed left-hand side.
    /// Handles postfix operations and binary operators.
    pub(super) fn continue_parsing_expr(&mut self, mut expr: Expr) -> Result<Expr, ParseError> {
        // Handle postfix (field access, index access)
        loop {
            match self.lexer.peek() {
                Some(Token::Dot) => {
                    self.lexer.next_token();
                    let field_ident = self.parse_any_ident()?;
                    let span = expr.span.merge(field_ident.span);
                    expr = Expr {
                        kind: ExprKind::FieldAccess {
                            expr: Box::new(expr),
                            field: field_ident.into_spanned::<FieldName>(),
                        },
                        span,
                    };
                }
                Some(Token::LBracket) => {
                    self.lexer.next_token();
                    let mut args = Vec::new();
                    loop {
                        if self.lexer.peek() == Some(&Token::RBracket) {
                            break;
                        }
                        args.push(self.parse_index_arg()?);
                        if self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                        } else {
                            break;
                        }
                    }
                    let (_, end_span) = self.expect(Token::RBracket)?;
                    let span = expr.span.merge(end_span);
                    expr = Expr {
                        kind: ExprKind::IndexAccess {
                            expr: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                _ => break,
            }
        }
        // Handle binary operators (comparison, arithmetic, logical).
        // Check for comparison operators (==, !=, <, >, <=, >=).
        let op = self.lexer.peek().and_then(token_to_comparison_op);
        if let Some(op) = op {
            self.lexer.next_token();
            let rhs = self.parse_expr()?;
            let span = expr.span.merge(rhs.span);
            expr = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(expr),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(expr)
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
    use crate::ast::{BinOp, DeclKind, ExprKind};

    fn dim_expr_name(te: &crate::ast::TypeExpr) -> &str {
        match &te.kind {
            crate::ast::TypeExprKind::DimExpr(dim) => {
                assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
                dim.terms[0].term.name.name.as_str()
            }
            other => panic!("expected DimExpr, got {other:?}"),
        }
    }

    #[test]
    fn parse_block_simple() {
        let source = "node x: Dimensionless = { let a = 1.0; a + 2.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, expr } => {
                    assert_eq!(stmts.len(), 1);
                    assert_eq!(stmts[0].name.name, "a");
                    assert!(stmts[0].type_ann.is_none());
                    assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_block_multiple_lets() {
        let source = "node x: Dimensionless = { let r1 = @a + @b; let r2 = @c; r1 + r2 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, expr } => {
                    assert_eq!(stmts.len(), 2);
                    assert_eq!(stmts[0].name.name, "r1");
                    assert_eq!(stmts[1].name.name, "r2");
                    assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_block_let_with_type_ann() {
        let source = "node x: Dimensionless = { let a: Dimensionless = 1.0; a };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, .. } => {
                    assert_eq!(stmts.len(), 1);
                    assert!(stmts[0].type_ann.is_some());
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_block_no_lets() {
        let source = "node x: Dimensionless = { 1.0 + 2.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { stmts, .. } => {
                    assert_eq!(stmts.len(), 0);
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_local_ref() {
        let source = "node x: Dimensionless = { let a = 1.0; a };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { expr, .. } => {
                    assert!(matches!(&expr.kind, ExprKind::LocalRef(ident) if ident.name == "a"));
                }
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_struct_construction_explicit_fields() {
        let source = "node t: Dimensionless = TransferResult { dv1: @a + @b, dv2: @c };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction {
                    type_name, fields, ..
                } => {
                    assert_eq!(type_name.value.as_str(), "TransferResult");
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name.value.as_str(), "dv1");
                    assert!(fields[0].value.is_some());
                    assert_eq!(fields[1].name.value.as_str(), "dv2");
                    assert!(fields[1].value.is_some());
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_struct_construction_shorthand() {
        let source =
            "node t: Dimensionless = { let dv1 = @a; let dv2 = @b; TransferResult { dv1, dv2 } };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Block { expr, .. } => match &expr.kind {
                    ExprKind::StructConstruction {
                        type_name, fields, ..
                    } => {
                        assert_eq!(type_name.value.as_str(), "TransferResult");
                        assert_eq!(fields.len(), 2);
                        assert!(fields[0].value.is_none());
                        assert!(fields[1].value.is_none());
                    }
                    other => panic!("expected StructConstruction, got {other:?}"),
                },
                other => panic!("expected Block, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_struct_construction_trailing_comma() {
        let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0, };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction { fields, .. } => {
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_generic_struct_construction() {
        let source = "node v: Vec3<Length, ECI> = Vec3<Length, ECI> { x: 1.0, y: 2.0, z: 3.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction {
                    type_name,
                    type_args,
                    fields,
                } => {
                    assert_eq!(type_name.value.as_str(), "Vec3");
                    assert_eq!(type_args.len(), 2);
                    assert_eq!(dim_expr_name(&type_args[0]), "Length");
                    assert_eq!(dim_expr_name(&type_args[1]), "ECI");
                    assert_eq!(fields.len(), 3);
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_non_generic_struct_construction_still_works() {
        let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction {
                    type_name,
                    type_args,
                    fields,
                } => {
                    assert_eq!(type_name.value.as_str(), "TransferResult");
                    assert!(type_args.is_empty());
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_for_comprehension() {
        let source = "node fuel: Mass[Maneuver] = for m: Maneuver { 1.0 kg };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { bindings, body } => {
                    assert_eq!(bindings.len(), 1);
                    assert_eq!(bindings[0].var.name, "m");
                    assert_eq!(bindings[0].index.value.as_str(), "Maneuver");
                    assert!(matches!(body.kind, ExprKind::UnitLiteral { .. }));
                }
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_for_multi_binding() {
        let source = "node x: Dimensionless[Row, Col] = for r: Row, c: Col { 0.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { bindings, .. } => {
                    assert_eq!(bindings.len(), 2);
                    assert_eq!(bindings[0].var.name, "r");
                    assert_eq!(bindings[0].index.value.as_str(), "Row");
                    assert_eq!(bindings[1].var.name, "c");
                    assert_eq!(bindings[1].index.value.as_str(), "Col");
                }
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_index_access_with_variant() {
        let source = "node x: Velocity = @dv[Maneuver::Departure];";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::IndexAccess { expr, args } => {
                    assert!(matches!(expr.kind, ExprKind::GraphRef(_)));
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        crate::ast::IndexArg::Variant { index, variant } => {
                            assert_eq!(index.value.as_str(), "Maneuver");
                            assert_eq!(variant.value.as_str(), "Departure");
                        }
                        other @ crate::ast::IndexArg::Var(_) => {
                            panic!("expected Variant, got {other:?}")
                        }
                    }
                }
                other => panic!("expected IndexAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_index_access_with_loop_var() {
        let source = "node y: Velocity[Maneuver] = for m: Maneuver { @dv[m] };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { body, .. } => match &body.kind {
                    ExprKind::IndexAccess { args, .. } => {
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            crate::ast::IndexArg::Var(ident) => assert_eq!(ident.name, "m"),
                            other @ crate::ast::IndexArg::Variant { .. } => {
                                panic!("expected Var, got {other:?}")
                            }
                        }
                    }
                    other => panic!("expected IndexAccess, got {other:?}"),
                },
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_scan_expression() {
        let source = "node cum: Velocity[Maneuver] = scan(@dv, 0.0 m/s, |acc, val| acc + val);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Scan {
                    acc_name, val_name, ..
                } => {
                    assert_eq!(acc_name.name, "acc");
                    assert_eq!(val_name.name, "val");
                }
                other => panic!("expected Scan, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_unfold_expression() {
        let source =
            "node x: Dimensionless[TimeStep] = unfold(1.0, |prev_t, t| { @x[prev_t] * 2.0 });";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Unfold {
                    prev_name,
                    curr_name,
                    ..
                } => {
                    assert_eq!(prev_name.name, "prev_t");
                    assert_eq!(curr_name.name, "t");
                }
                other => panic!("expected Unfold, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_field_access_in_arithmetic() {
        let source = "node x: Dimensionless = @t.dv1 + @t.dv2;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::BinOp { op, lhs, rhs } => {
                    assert!(matches!(op, BinOp::Add));
                    assert!(matches!(&lhs.kind, ExprKind::FieldAccess { .. }));
                    assert!(matches!(&rhs.kind, ExprKind::FieldAccess { .. }));
                }
                other => panic!("expected BinOp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }
}
