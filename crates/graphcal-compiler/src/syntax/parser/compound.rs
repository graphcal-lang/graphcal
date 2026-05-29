use crate::syntax::ast::{
    Expr, ExprKind, ForBinding, ForBindingIndex, IdentPath, MatchArm, MatchPattern, NatExpr,
    PatternBinding,
};
use crate::syntax::names::FieldName;
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;

use super::{ParseError, Parser};

impl Parser<'_> {
    // --- Match expression ---

    /// Parse a match expression:
    /// `match expr { Variant1(field) => body, Variant2 => body }`
    pub(super) fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::Match)?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(Token::LBrace)?;

        let arms = self.parse_match_arm_list()?;

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Expr::new(ExprKind::Match { scrutinee, arms }, span))
    }

    /// Parse a list of match arms until `}`.
    fn parse_match_arm_list(&mut self) -> Result<Vec<MatchArm>, ParseError> {
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
        Ok(arms)
    }

    /// Parse a single match arm: `VariantName(field1, field2: _) => expr`
    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_match_pattern()?;
        self.expect(Token::FatArrow)?;
        let body = self.parse_expr()?;
        let span = pattern.span().merge(body.span);
        Ok(MatchArm {
            pattern,
            body,
            span,
        })
    }

    /// Parse a match pattern as syntax first.
    ///
    /// The parser records the complete path and any constructor-style bindings,
    /// but deliberately does not decide whether the path denotes a union
    /// constructor or an index label. That categorization requires name
    /// resolution.
    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let first_ident = self.parse_any_ident()?;
        let start_span = first_ident.span;
        let mut rest = Vec::new();
        while self.lexer.peek() == Some(&Token::Dot) {
            self.lexer.next_token();
            rest.push(self.parse_any_ident()?);
        }
        let path = IdentPath::new(crate::syntax::non_empty::NonEmpty::new(first_ident, rest));

        let (bindings, end_span) = if self.lexer.peek() == Some(&Token::LParen) {
            self.lexer.next_token(); // consume '('
            let bindings =
                self.parse_comma_separated(Token::RParen, Self::parse_pattern_binding)?;
            let (_, rparen_span) = self.expect(Token::RParen)?;
            (bindings, rparen_span)
        } else {
            (Vec::new(), path.span())
        };

        Ok(MatchPattern::Path {
            path,
            bindings,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single pattern binding:
    /// - `field_name: var_name` (bind)
    /// - `field_name: _` (wildcard)
    fn parse_pattern_binding(&mut self) -> Result<PatternBinding, ParseError> {
        let field_ident = self.parse_any_ident()?;
        let field = Spanned::new(FieldName::new(&field_ident.name), field_ident.span);

        self.expect(Token::Colon)?;
        // Check for wildcard `_`
        if self.lexer.peek() == Some(&Token::Underscore) {
            let (_, span) = self.advance()?;
            return Ok(PatternBinding::Wildcard { field, span });
        }
        // Binding: `field_name: var_name`
        let var = self.parse_any_ident()?;
        Ok(PatternBinding::Bind { field, var })
    }

    // --- For comprehension ---

    /// Parse a for comprehension: `for m: Maneuver, i: range(3) { expr }`
    pub(super) fn parse_for_comp(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::For)?;
        let mut bindings = Vec::new();
        loop {
            let var = self.parse_any_ident()?.into_spanned();
            self.expect(Token::Colon)?;
            let index = self.parse_for_binding_index()?;
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
        // Use raw-byte lookahead to disambiguate from `(expr)` grouping.
        if self.is_tuple_key_sugar() {
            self.lexer.next_token(); // consume '('
            loop {
                self.parse_any_ident()?;
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
        Ok(Expr::new(
            ExprKind::ForComp {
                bindings,
                body: Box::new(body),
            },
            span,
        ))
    }

    /// Parse a for binding index: either a named index (`Maneuver`) or `range(N)`.
    fn parse_for_binding_index(&mut self) -> Result<ForBindingIndex, ParseError> {
        // Check if next token is the identifier "range"
        if let Some((Token::Ident, span)) = self.lexer.peek_with_span() {
            let text = self.lexer.slice_at(span);
            if text == "range" {
                let range_start = span;
                self.lexer.next_token(); // consume "range"
                self.expect(Token::LParen)?;
                // Parse the nat expression inside range(...)
                let nat_expr = self.parse_nat_expr()?;
                let (_, rparen_span) = self.expect(Token::RParen)?;
                let range_span = range_start.merge(rparen_span);
                return Ok(ForBindingIndex::Range {
                    arg: nat_expr,
                    span: range_span,
                });
            }
        }
        // Named index
        let first = self.parse_any_ident()?;
        let mut segments = vec![first];
        while self.lexer.peek() == Some(&Token::Dot) {
            self.lexer.next_token();
            segments.push(self.parse_any_ident()?);
        }
        Ok(ForBindingIndex::Named(Self::index_name_path_from_segments(
            &segments,
        )))
    }

    /// Parse a nat expression: supports literals, identifiers, addition, and multiplication.
    ///
    /// Grammar (with precedence: `*` binds tighter than `+`):
    /// ```text
    /// nat_expr := nat_mul_term ('+' nat_mul_term)*
    /// nat_mul_term := nat_atom ('*' nat_atom)*
    /// ```
    fn parse_nat_expr(&mut self) -> Result<NatExpr, ParseError> {
        let mut lhs = self.parse_nat_mul_term()?;
        while self.lexer.peek() == Some(&Token::Plus) {
            self.lexer.next_token(); // consume '+'
            let rhs = self.parse_nat_mul_term()?;
            let span = lhs.span().merge(rhs.span());
            lhs = NatExpr::Add(Box::new(lhs), Box::new(rhs), span);
        }
        Ok(lhs)
    }

    /// Parse a multiplicative nat term: `nat_atom ('*' nat_atom)*`
    fn parse_nat_mul_term(&mut self) -> Result<NatExpr, ParseError> {
        let mut lhs = self.parse_nat_atom()?;
        while self.lexer.peek() == Some(&Token::Star) {
            self.lexer.next_token(); // consume '*'
            let rhs = self.parse_nat_atom()?;
            let span = lhs.span().merge(rhs.span());
            lhs = NatExpr::Mul(Box::new(lhs), Box::new(rhs), span);
        }
        Ok(lhs)
    }

    /// Parse a single nat atom: an integer literal or an identifier (generic Nat param).
    fn parse_nat_atom(&mut self) -> Result<NatExpr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: u64 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected non-negative integer in range()".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok(NatExpr::Literal(value, span))
            }
            Some(Token::Ident) => {
                let ident = self.parse_any_ident()?;
                Ok(NatExpr::Var(ident))
            }
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "integer literal or Nat parameter name",
                    &tok.to_string(),
                    span,
                ))
            }
        }
    }

    // --- Scan expression ---

    /// Parse a scan expression: `scan(source, init, |acc, val| body)` → `ExprKind::Scan`
    ///
    /// The `scan` keyword has already been consumed; `keyword_span` is its span.
    pub(super) fn parse_scan(&mut self, keyword_span: Span) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let first_expr = self.parse_expr()?;
        self.expect(Token::Comma)?;
        let init = self.parse_expr()?;
        self.expect(Token::Comma)?;
        // Parse lambda: |acc, val| body
        self.expect(Token::Pipe)?;
        let acc_name = self.parse_any_ident()?.into_spanned();
        self.expect(Token::Comma)?;
        let val_name = self.parse_any_ident()?.into_spanned();
        self.expect(Token::Pipe)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        let span = keyword_span.merge(end_span);
        Ok(Expr::new(
            ExprKind::Scan {
                source: Box::new(first_expr),
                init: Box::new(init),
                acc_name,
                val_name,
                body: Box::new(body),
            },
            span,
        ))
    }

    // --- Unfold expression ---

    /// Parse an unfold expression: `unfold(init, |prev_i, i| body)` → `ExprKind::Unfold`
    ///
    /// The `unfold` keyword has already been consumed; `keyword_span` is its span.
    pub(super) fn parse_unfold(&mut self, keyword_span: Span) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let init = self.parse_expr()?;
        self.expect(Token::Comma)?;
        self.expect(Token::Pipe)?;
        let prev_name = self.parse_any_ident()?.into_spanned();
        self.expect(Token::Comma)?;
        let curr_name = self.parse_any_ident()?.into_spanned();
        self.expect(Token::Pipe)?;
        let body = self.parse_expr()?;
        let (_, end_span) = self.expect(Token::RParen)?;
        let span = keyword_span.merge(end_span);
        Ok(Expr::new(
            ExprKind::Unfold {
                init: Box::new(init),
                prev_name,
                curr_name,
                body: Box::new(body),
            },
            span,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{BinOp, DeclKind, ExprKind};

    fn dim_expr_name(te: &crate::syntax::ast::TypeExpr) -> &str {
        match &te.kind {
            crate::syntax::ast::TypeExprKind::DimExpr(dim) => {
                assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
                dim.terms[0].term.name.value.leaf().as_str()
            }
            other => panic!("expected DimExpr, got {other:?}"),
        }
    }

    #[test]
    fn parse_constructor_call_explicit_fields() {
        // Construction always uses the parens form `Ctor(field: expr, ...)`.
        let source = "node t: Dimensionless = TransferResult(dv1: @a + @b, dv2: @c);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ConstructorCall { callee, fields, .. } => {
                    assert_eq!(callee.as_bare().unwrap().name, "TransferResult");
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name.value.as_str(), "dv1");
                    assert_eq!(fields[1].name.value.as_str(), "dv2");
                }
                other => panic!("expected ConstructorCall, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_qualified_constructor_call_preserves_callee_path() {
        let source = "node t: Dimensionless = module.TransferResult(dv1: @a);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ConstructorCall { callee, fields, .. } => {
                    assert_eq!(callee.segments.len(), 2);
                    assert_eq!(callee.segments[0].name, "module");
                    assert_eq!(callee.segments[1].name, "TransferResult");
                    assert_eq!(fields.len(), 1);
                }
                other => panic!("expected ConstructorCall, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_constructor_call_trailing_comma() {
        let source = "node t: Dimensionless = TransferResult(dv1: 1.0, dv2: 2.0,);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ConstructorCall { fields, .. } => {
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected ConstructorCall, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_constructor_call_rejects_field_shorthand() {
        let source = "node t: Dimensionless = TransferResult(dv1: 1.0, dv2);";
        assert!(Parser::new(source).parse_file().is_err());
    }

    #[test]
    fn parse_generic_constructor_call() {
        let source = "node v: Vec3<Length, ECI> = Vec3<Length, ECI>(x: 1.0, y: 2.0, z: 3.0);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ConstructorCall {
                    callee,
                    generic_args,
                    fields,
                } => {
                    assert_eq!(callee.as_bare().unwrap().name, "Vec3");
                    assert_eq!(generic_args.len(), 2);
                    match &generic_args[0] {
                        crate::syntax::ast::GenericArg::Type(type_arg) => {
                            assert_eq!(dim_expr_name(type_arg), "Length");
                        }
                        other @ crate::syntax::ast::GenericArg::Nat(_) => {
                            panic!("expected type generic arg, got {other:?}")
                        }
                    }
                    match &generic_args[1] {
                        crate::syntax::ast::GenericArg::Type(type_arg) => {
                            assert_eq!(dim_expr_name(type_arg), "ECI");
                        }
                        other @ crate::syntax::ast::GenericArg::Nat(_) => {
                            panic!("expected type generic arg, got {other:?}")
                        }
                    }
                    assert_eq!(fields.len(), 3);
                }
                other => panic!("expected ConstructorCall, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_constructor_call_preserves_nat_generic_arg() {
        let source = "node v: Dimensionless = FixedVec<3>(x: 1.0);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ConstructorCall { generic_args, .. } => {
                    assert_eq!(generic_args.len(), 1);
                    assert!(matches!(
                        generic_args[0],
                        crate::syntax::ast::GenericArg::Nat(crate::syntax::ast::NatExpr::Literal(
                            3,
                            _
                        ))
                    ));
                }
                other => panic!("expected ConstructorCall, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_brace_form_construction_rejected() {
        // The legacy brace-form `Ctor { field: val }` no longer parses.
        let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0 };";
        assert!(Parser::new(source).parse_file().is_err());
    }

    #[test]
    fn parse_for_comprehension() {
        let source = "node fuel: Mass[Maneuver] = for m: Maneuver { 1.0 kg };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::ForComp { bindings, body } => {
                    assert_eq!(bindings.len(), 1);
                    assert_eq!(bindings[0].var.value.as_str(), "m");
                    let ForBindingIndex::Named(spanned) = &bindings[0].index else {
                        panic!("expected Named")
                    };
                    assert_eq!(spanned.value.leaf().as_str(), "Maneuver");
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
                    assert_eq!(bindings[0].var.value.as_str(), "r");
                    let ForBindingIndex::Named(spanned) = &bindings[0].index else {
                        panic!("expected Named")
                    };
                    assert_eq!(spanned.value.leaf().as_str(), "Row");
                    assert_eq!(bindings[1].var.value.as_str(), "c");
                    let ForBindingIndex::Named(spanned) = &bindings[1].index else {
                        panic!("expected Named")
                    };
                    assert_eq!(spanned.value.leaf().as_str(), "Col");
                }
                other => panic!("expected ForComp, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_index_access_with_variant() {
        let source = "node x: Velocity = @dv[Maneuver.Departure];";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::IndexAccess { expr, args } => {
                    assert!(matches!(expr.kind, ExprKind::GraphRef(_)));
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        crate::syntax::ast::IndexArg::Variant { index, variant } => {
                            assert_eq!(index.value.leaf().as_str(), "Maneuver");
                            assert_eq!(variant.value.as_str(), "Departure");
                        }
                        other @ (crate::syntax::ast::IndexArg::Var(_)
                        | crate::syntax::ast::IndexArg::Expr(_)) => {
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
                            crate::syntax::ast::IndexArg::Var(ident) => assert_eq!(ident.name, "m"),
                            other @ (crate::syntax::ast::IndexArg::Variant { .. }
                            | crate::syntax::ast::IndexArg::Expr(_)) => {
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
                    assert_eq!(acc_name.value.as_str(), "acc");
                    assert_eq!(val_name.value.as_str(), "val");
                }
                other => panic!("expected Scan, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_unfold_expression() {
        let source = "node x: Dimensionless[TimeStep] = unfold(1.0, |prev_t, t| @x[prev_t] * 2.0);";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Unfold {
                    prev_name,
                    curr_name,
                    ..
                } => {
                    assert_eq!(prev_name.value.as_str(), "prev_t");
                    assert_eq!(curr_name.value.as_str(), "t");
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

    #[test]
    fn parse_match_constructor_pattern_with_parens() {
        let source = "node fuel: Force = match @maneuver { LowThrust(thrust: thrust, duration: _) => thrust, Coast => 0.0 N };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Match { arms, .. } => {
                    assert_eq!(arms.len(), 2);
                    let MatchPattern::Path { path, bindings, .. } = &arms[0].pattern else {
                        panic!("expected syntactic path pattern");
                    };
                    assert_eq!(path.as_bare().unwrap().name, "LowThrust");
                    assert_eq!(bindings.len(), 2);
                    match &bindings[0] {
                        PatternBinding::Bind { field, var } => {
                            assert_eq!(field.value.as_str(), "thrust");
                            assert_eq!(var.name, "thrust");
                        }
                        other @ PatternBinding::Wildcard { .. } => {
                            panic!("expected bind, got {other:?}")
                        }
                    }
                    match &bindings[1] {
                        PatternBinding::Wildcard { field, .. } => {
                            assert_eq!(field.value.as_str(), "duration");
                        }
                        other @ PatternBinding::Bind { .. } => {
                            panic!("expected wildcard, got {other:?}")
                        }
                    }
                    let MatchPattern::Path { bindings, .. } = &arms[1].pattern else {
                        panic!("expected syntactic path pattern");
                    };
                    assert!(bindings.is_empty());
                }
                other => panic!("expected Match, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_match_constructor_pattern_rejects_field_shorthand() {
        let source = "node fuel: Force = match @maneuver { LowThrust(thrust, duration: _) => thrust, Coast => 0.0 N };";
        assert!(Parser::new(source).parse_file().is_err());
    }

    #[test]
    fn parse_dotted_match_pattern_with_bindings_remains_syntactic() {
        let source = "node x: Dimensionless = match @phase { Phase.Launch(label: value) => 1.0 };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Match { arms, .. } => {
                    let MatchPattern::Path { path, bindings, .. } = &arms[0].pattern else {
                        panic!("expected syntactic path pattern");
                    };
                    assert_eq!(path.segments.len(), 2);
                    assert_eq!(bindings.len(), 1);
                }
                other => panic!("expected Match, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_tuple_match_rejected() {
        let source = "node x: Dimensionless = match (Phase.Launch, Mode.Nominal) { (Phase.Launch, Mode.Nominal) => 1.0, _ => 0.0 };";
        assert!(Parser::new(source).parse_file().is_err());
    }
}
