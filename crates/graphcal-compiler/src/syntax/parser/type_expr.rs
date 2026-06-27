use crate::dimension::Rational;
use crate::syntax::ast::{
    DimExpr, DimExprItem, DimTerm, Expr, ExprKind, GenericConstraint, GenericParam, IdentPath,
    IndexExpr, MulDivOp, NatExpr, TypeExpr, TypeExprKind, UnitDef, UnitExpr, UnitExprItem,
};
use crate::syntax::dimension::{UnitName, UnitRef};
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::span::Span;
use crate::syntax::span::Spanned;
use crate::syntax::token::Token;
use crate::syntax::type_name::GenericParamName;

use super::{ParseError, Parser};

#[derive(Debug, Clone)]
enum IndexExprAtom {
    Path(IdentPath),
    Nat(NatExpr),
}

impl Parser<'_> {
    // --- Type expressions ---

    /// Parse a type expression: `Dimensionless` or a dimension expression.
    pub(super) fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        self.with_depth(Self::parse_type_expr_inner)
    }

    fn parse_type_expr_inner(&mut self) -> Result<TypeExpr, ParseError> {
        // Parse the base type first. Identifier-shaped type references are
        // parsed as paths before any semantic categorization; bare built-ins
        // are recognized only when the path has exactly one segment.
        let mut base = if self.lexer.peek() == Some(&Token::Ident) {
            let path = self.parse_ident_path()?;
            let path_span = path.span();
            let bare_name = path.as_bare().map(|ident| ident.name.as_str().to_string());

            match bare_name.as_deref() {
                Some("Dimensionless") => TypeExpr {
                    kind: TypeExprKind::Dimensionless,
                    constraints: vec![],
                    span: path_span,
                },
                Some("Bool") => TypeExpr {
                    kind: TypeExprKind::Bool,
                    constraints: vec![],
                    span: path_span,
                },
                Some("Int") => TypeExpr {
                    kind: TypeExprKind::Int,
                    constraints: vec![],
                    span: path_span,
                },
                Some("Datetime") => {
                    if self.lexer.peek() == Some(&Token::Lt) {
                        // Datetime<TT> — built-in parameterized type, kept in
                        // its own variant so TIR resolution doesn't need to
                        // string-match the built-in type name.
                        let type_args = self.parse_type_arg_list()?;
                        let end_span = type_args.last().map_or(path_span, |a| a.span);
                        TypeExpr {
                            kind: TypeExprKind::DatetimeApplication { type_args },
                            constraints: vec![],
                            span: path_span.merge(end_span),
                        }
                    } else {
                        // Bare Datetime (= Datetime<UTC>)
                        TypeExpr {
                            kind: TypeExprKind::Datetime,
                            constraints: vec![],
                            span: path_span,
                        }
                    }
                }
                _ if self.lexer.peek() == Some(&Token::Lt) => {
                    // Type application: Vec3<Length, ECI> or module.Vec3<Length>.
                    // `<` cannot follow a complete dim expr in type position,
                    // so no casing heuristic is needed — the previous
                    // ASCII-uppercase check silently misparsed non-ASCII
                    // type names into dim expressions.
                    let name = path.into_spanned_name_path();
                    let type_args = self.parse_type_arg_list()?;
                    let end_span = type_args.last().map_or(name.span, |a| a.span);
                    let span = name.span.merge(end_span);
                    TypeExpr {
                        kind: TypeExprKind::TypeApplication { name, type_args },
                        constraints: vec![],
                        span,
                    }
                }
                _ => {
                    let dim_expr = self.parse_dim_expr_after_first_path(path)?;
                    let span = dim_expr.span;
                    TypeExpr {
                        kind: TypeExprKind::DimExpr(dim_expr),
                        constraints: vec![],
                        span,
                    }
                }
            }
        } else {
            let dim_expr = self.parse_dim_expr()?;
            let span = dim_expr.span;
            TypeExpr {
                kind: TypeExprKind::DimExpr(dim_expr),
                constraints: vec![],
                span,
            }
        };

        // Check for optional domain constraints: `(min: expr, max: expr)`
        if self.lexer.peek() == Some(&Token::LParen) {
            let constraints = self.parse_domain_constraints()?;
            if let Some(last) = constraints.last() {
                base.span = base.span.merge(last.span);
            }
            base.constraints = constraints;
        }

        // Check for optional `[Index, ...]` suffix
        // Supports named indexes (`Phase`), generic params (`I`, `N`), and nat literals (`3`)
        if self.lexer.peek() == Some(&Token::LBracket) {
            let (_, _bracket_span) = self.advance()?;
            let indexes = self.parse_comma_separated(Token::RBracket, Self::parse_index_expr)?;
            let (_, end_span) = self.expect(Token::RBracket)?;
            let span = base.span.merge(end_span);
            base = TypeExpr {
                kind: TypeExprKind::Indexed {
                    base: Box::new(base),
                    indexes,
                },
                constraints: vec![],
                span,
            };
        }

        Ok(base)
    }

    /// Parse domain constraints: `(min: expr, max: expr)`.
    ///
    /// Called when `(` is peeked after a base type expression.
    /// Each constraint is `name: expr` where `name` is an identifier like `min` or `max`.
    fn parse_domain_constraints(
        &mut self,
    ) -> Result<Vec<crate::syntax::ast::DomainBound>, ParseError> {
        let (_, _lparen_span) = self.expect(Token::LParen)?;
        let mut constraints = Vec::new();
        loop {
            if self.lexer.peek() == Some(&Token::RParen) {
                break;
            }
            let ident = self.parse_any_ident()?;
            let kind_span = ident.span;
            let kind = match ident.name.as_str() {
                "min" => crate::syntax::ast::DomainBoundKind::Min,
                "max" => crate::syntax::ast::DomainBoundKind::Max,
                _ => {
                    return Err(ParseError::InvalidDomainBoundKey {
                        key: ident.name.to_string(),
                        src: self.named_source(),
                        span: kind_span.into(),
                    });
                }
            };
            self.expect(Token::Colon)?;
            let value = self.parse_expr()?;
            let bound_span = kind_span.merge(value.span);
            constraints.push(crate::syntax::ast::DomainBound {
                kind,
                kind_span,
                value,
                span: bound_span,
            });
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }
        let (_, rparen_span) = self.expect(Token::RParen)?;
        // Update span of last constraint to include rparen for better error reporting
        if constraints.is_empty() {
            return Err(self.unexpected_token(
                "at least one domain constraint (e.g., `min: 0`)",
                ")",
                rparen_span,
            ));
        }
        Ok(constraints)
    }

    /// Parse a dimension expression: `DimTermOrGroup (("*" | "/") DimTermOrGroup)*`
    ///
    /// A term-or-group is either `ident_path ("^" INTEGER)?` or `"(" DimExpr ")" ("^" INTEGER)?`.
    /// Parenthesized groups are flattened: `(A * B / C)^2` becomes `A^2 * B^2 / C^2`,
    /// and `D / (A * B)` becomes `D / A / B`.
    pub(super) fn parse_dim_expr(&mut self) -> Result<DimExpr, ParseError> {
        let first_items = self.parse_dim_term_or_group()?;
        self.parse_dim_expr_after_first_items(first_items)
    }

    fn parse_dim_expr_after_first_path(&mut self, path: IdentPath) -> Result<DimExpr, ParseError> {
        let term = self.parse_dim_term_after_path(path)?;
        self.parse_dim_expr_after_first_items(vec![DimExprItem {
            op: MulDivOp::Mul,
            term,
        }])
    }

    fn parse_dim_expr_after_first_items(
        &mut self,
        first_items: Vec<DimExprItem>,
    ) -> Result<DimExpr, ParseError> {
        let start_span = first_items[0].term.span;
        let mut terms: Vec<DimExprItem> = first_items;

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let items = self.parse_dim_term_or_group()?;
                    for mut item in items {
                        item.op = Self::combine_ops(MulDivOp::Mul, item.op);
                        terms.push(item);
                    }
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let items = self.parse_dim_term_or_group()?;
                    for mut item in items {
                        item.op = Self::combine_ops(MulDivOp::Div, item.op);
                        terms.push(item);
                    }
                }
                _ => break,
            }
        }

        let end_span = terms
            .last()
            .ok_or_else(|| self.unexpected_eof("dimension term"))?
            .term
            .span;
        Ok(DimExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single dimension term or a parenthesized group, returning flattened items.
    ///
    /// - `ident_path ("^" INTEGER)?` → single item with op=Mul
    /// - `"(" DimExpr ")" ("^" INTEGER)?` → flattened items with powers multiplied
    fn parse_dim_term_or_group(&mut self) -> Result<Vec<DimExprItem>, ParseError> {
        if self.lexer.peek() == Some(&Token::LParen) {
            self.lexer.next_token();
            let inner = self.parse_dim_expr()?;
            self.expect(Token::RParen)?;

            let outer_power = self.parse_outer_power()?;

            // Flatten: distribute the outer power to each inner term
            inner
                .terms
                .into_iter()
                .map(|item| {
                    let inner_power = item.term.power.unwrap_or(Rational::ONE);
                    let combined =
                        (inner_power * outer_power).map_err(|_| ParseError::InvalidNumber {
                            reason: "dimension exponent overflows `i32`".to_string(),
                            src: self.named_source(),
                            span: item.term.span.into(),
                        })?;
                    Ok(DimExprItem {
                        op: item.op,
                        term: DimTerm {
                            name: item.term.name,
                            power: if combined == Rational::ONE {
                                None
                            } else {
                                Some(combined)
                            },
                            span: item.term.span,
                        },
                    })
                })
                .collect()
        } else {
            let term = self.parse_dim_term()?;
            Ok(vec![DimExprItem {
                op: MulDivOp::Mul,
                term,
            }])
        }
    }

    /// Parse a single dimension term: `ident_path ("^" INTEGER)?`
    fn parse_dim_term(&mut self) -> Result<DimTerm, ParseError> {
        let path = self.parse_ident_path()?;
        self.parse_dim_term_after_path(path)
    }

    fn parse_dim_term_after_path(&mut self, path: IdentPath) -> Result<DimTerm, ParseError> {
        let name_span = path.span();
        let mut end_span = name_span;

        let power = self.parse_term_power(&mut end_span)?;

        Ok(DimTerm {
            span: name_span.merge(end_span),
            name: path.into_spanned_name_path(),
            power,
        })
    }

    /// Parse an optional `^` power suffix, returning the exponent (defaulting to 1).
    ///
    /// Used for parenthesized groups: `(A * B)^2` → returns `2`.
    fn parse_outer_power(&mut self) -> Result<Rational, ParseError> {
        if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (value, _span) = self.parse_exponent_value()?;
            Ok(value)
        } else {
            Ok(Rational::ONE)
        }
    }

    /// Parse an optional `^` power suffix, returning `Some(power)` or `None`.
    ///
    /// Used for individual terms: `m^2` → `Some(2)`, `m` → `None`.
    /// Updates `end_span` to cover the power literal when present.
    fn parse_term_power(&mut self, end_span: &mut Span) -> Result<Option<Rational>, ParseError> {
        if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (value, span) = self.parse_exponent_value()?;
            *end_span = span;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// Parse the exponent value after a `^` in a dimension or unit expression:
    /// a signed integer (`2`, `-3`) or a parenthesized rational (`(1/2)`,
    /// `(-3/2)`, `(2)`). Returns the value and its end span.
    ///
    /// Zero exponents (including `0/n`) are rejected (#648 N3): a zero power
    /// erases its term, so it is never meaningful.
    fn parse_exponent_value(&mut self) -> Result<(Rational, Span), ParseError> {
        let (value, span) = if self.lexer.peek() == Some(&Token::LParen) {
            self.lexer.next_token();
            let (num_neg, num, num_span) = self.parse_integer_literal()?;
            let num = if num_neg { -num } else { num };
            let value = if self.lexer.peek() == Some(&Token::Slash) {
                self.lexer.next_token();
                let (den_neg, den, den_span) = self.parse_integer_literal()?;
                let den = if den_neg { -den } else { den };
                Rational::try_new(num, den).map_err(|_| ParseError::InvalidNumber {
                    reason: "exponent denominator must be a non-zero integer".to_string(),
                    src: self.named_source(),
                    span: den_span.into(),
                })?
            } else {
                Rational::from(num)
            };
            let (_, rparen_span) = self.expect(Token::RParen)?;
            (value, num_span.merge(rparen_span))
        } else {
            let (neg, value, span) = self.parse_integer_literal()?;
            (Rational::from(if neg { -value } else { value }), span)
        };
        if value.is_zero() {
            return Err(ParseError::ZeroExponent {
                src: self.named_source(),
                span: span.into(),
            });
        }
        Ok((value, span))
    }

    /// Combine two `MulDivOp`s: `Mul*Mul=Mul`, `Mul*Div=Div`, `Div*Mul=Div`, `Div*Div=Mul`.
    const fn combine_ops(outer: MulDivOp, inner: MulDivOp) -> MulDivOp {
        match (outer, inner) {
            (MulDivOp::Mul, MulDivOp::Mul) | (MulDivOp::Div, MulDivOp::Div) => MulDivOp::Mul,
            (MulDivOp::Mul, MulDivOp::Div) | (MulDivOp::Div, MulDivOp::Mul) => MulDivOp::Div,
        }
    }

    // --- Unit expressions ---

    /// Parse a unit expression:
    ///   `unit_term (("*" | "/") unit_term)*`
    /// where `unit_term` is `IDENT ["." IDENT] ["^" INTEGER]` or
    /// `"(" unit_expr ")" ["^" INTEGER]`. The optional dotted qualifier is a
    /// module alias (`u.mile`); deeper paths are rejected (P017).
    ///
    /// Parenthesized groups are flattened into the term list (operator
    /// combination and power distribution), so the AST stays flat.
    pub(super) fn parse_unit_expr(&mut self) -> Result<UnitExpr, ParseError> {
        self.with_depth(Self::parse_unit_expr_inner)
    }

    fn parse_unit_expr_inner(&mut self) -> Result<UnitExpr, ParseError> {
        // `1/unit` shorthand (#648 U2/N4): a literal `1` numerator contributes
        // nothing — the term after the slash is a plain division. This matches
        // the canonical display rendering (`min^-1` prints as `1/min`), so
        // displayed unit labels round-trip as source.
        let (first_terms, start_span, mut end_span) = if self.lexer.peek() == Some(&Token::Number)
            && self.lexer.peek_second() == Some(&Token::Slash)
        {
            let (_, one_span) = self.expect(Token::Number)?;
            let text = self.lexer.slice_at(one_span);
            if text != "1" {
                return Err(ParseError::InvalidNumber {
                    reason: "only `1` can appear as a unit numerator (e.g. `1/min`)".to_string(),
                    src: self.named_source(),
                    span: one_span.into(),
                });
            }
            self.expect(Token::Slash)?;
            let (terms, _, end) = self.parse_unit_term_or_group(MulDivOp::Div)?;
            (terms, one_span, end)
        } else {
            self.parse_unit_term_or_group(MulDivOp::Mul)?
        };

        let mut terms: Vec<UnitExprItem> = first_terms;

        while let Some(&Token::Star | &Token::Slash) = self.lexer.peek() {
            // Only continue the unit expression if the operator is followed by
            // an identifier or `(` (parenthesized group). Otherwise, leave the
            // operator for the expression parser to handle as arithmetic
            // (e.g., `459.3 W / (1.0 m^2)`).
            if !matches!(
                self.lexer.peek_second(),
                Some(&Token::Ident | &Token::LParen)
            ) {
                break;
            }

            // peek() confirmed a token exists, so next_token() will return Some.
            let Some((op_token, _)) = self.lexer.next_token() else {
                break;
            };
            let outer_op = if op_token == Token::Star {
                MulDivOp::Mul
            } else {
                MulDivOp::Div
            };

            let (new_terms, _, new_end) = self.parse_unit_term_or_group(outer_op)?;
            end_span = new_end;
            terms.extend(new_terms);
        }

        Ok(UnitExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single unit term or a parenthesized unit group.
    ///
    /// Returns `(items, start_span, end_span)`. For a parenthesized group the
    /// items are flattened with the given `outer_op` distributed across them.
    fn parse_unit_term_or_group(
        &mut self,
        outer_op: MulDivOp,
    ) -> Result<(Vec<UnitExprItem>, Span, Span), ParseError> {
        if self.lexer.peek() == Some(&Token::LParen) {
            let (_, lparen_span) = self.expect(Token::LParen)?;
            let inner = self.parse_unit_expr()?;
            let (_, rparen_span) = self.expect(Token::RParen)?;

            let outer_power = self.parse_outer_power()?;

            let end_span = rparen_span;
            let items = inner
                .terms
                .into_iter()
                .map(|item| {
                    let combined_power = (item.power.unwrap_or(Rational::ONE) * outer_power)
                        .map_err(|_| ParseError::InvalidNumber {
                            reason: "unit exponent overflows `i32`".to_string(),
                            src: self.named_source(),
                            span: item.name.span.into(),
                        })?;
                    Ok(UnitExprItem {
                        op: Self::combine_ops(outer_op, item.op),
                        name: item.name,
                        power: if combined_power == Rational::ONE {
                            None
                        } else {
                            Some(combined_power)
                        },
                    })
                })
                .collect::<Result<Vec<_>, ParseError>>()?;
            Ok((items, lparen_span, end_span))
        } else {
            let ident = self.parse_any_ident()?;
            let start_span = ident.span;
            let mut end_span = ident.span;
            let name = if self.lexer.peek() == Some(&Token::Dot)
                && matches!(self.lexer.peek_second(), Some(&Token::Ident))
            {
                self.expect(Token::Dot)?;
                let leaf = self.parse_any_ident()?;
                // Unit references are at most `alias.unit`: module aliases are
                // single segments, so a deeper path can never resolve.
                if self.lexer.peek() == Some(&Token::Dot)
                    && matches!(self.lexer.peek_second(), Some(&Token::Ident))
                {
                    return Err(ParseError::UnitReferenceTooDeep {
                        src: self.named_source(),
                        span: start_span.merge(leaf.span).into(),
                    });
                }
                let span = start_span.merge(leaf.span);
                end_span = leaf.span;
                Spanned::new(
                    UnitRef::qualified(
                        ModuleAliasName::from_atom(ident.name),
                        UnitName::from_atom(leaf.name),
                    ),
                    span,
                )
            } else {
                ident.into_spanned::<UnitRef>()
            };
            let power = self.parse_term_power(&mut end_span)?;
            Ok((
                vec![UnitExprItem {
                    op: outer_op,
                    name,
                    power,
                }],
                start_span,
                end_span,
            ))
        }
    }

    /// Parse an integer literal, possibly preceded by `-`.
    /// Returns `(is_negative, absolute_value, span)`.
    pub(super) fn parse_integer_literal(
        &mut self,
    ) -> Result<(bool, i32, crate::syntax::span::Span), ParseError> {
        let neg = if self.lexer.peek() == Some(&Token::Minus) {
            self.lexer.next_token();
            true
        } else {
            false
        };

        match self.lexer.next_token() {
            Some((Token::Number, span)) => {
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: i32 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected integer".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok((neg, value, span))
            }
            Some((tok, span)) => Err(self.unexpected_token("integer", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("integer")),
        }
    }

    /// Parse the RHS of a unit definition: `NUMBER UNIT_EXPR`
    /// E.g., `1000 m`, `1 kg * m / s^2`, `(PI / 180) rad`
    pub(super) fn parse_unit_def(&mut self) -> Result<UnitDef, ParseError> {
        // Parse the scale expression: either a plain number or `(expr)`
        let scale_expr = self.parse_unit_scale()?;
        let unit_expr = self.parse_unit_expr()?;
        let span = scale_expr.span.merge(unit_expr.span);
        Ok(UnitDef {
            scale_expr,
            unit_expr,
            span,
        })
    }

    /// Parse the scale part of a unit definition.
    /// Supports: `1000`, `0.001`, `(PI / 180)`, `(expr)`
    fn parse_unit_scale(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let value = self.parse_finite_f64_literal(&text, span)?;
                Ok(Expr::new(ExprKind::Number(value), span))
            }
            Some(Token::LParen) => {
                let (_, _lp_span) = self.advance()?;
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(_) => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token("number or `(`", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("number or `(`")),
        }
    }

    /// Parse a type argument list: `<TypeExpr, TypeExpr, ...>`
    pub(super) fn parse_type_arg_list(&mut self) -> Result<Vec<TypeExpr>, ParseError> {
        self.expect(Token::Lt)?;
        let args = self.parse_comma_separated(Token::Gt, Self::parse_type_expr)?;
        self.expect(Token::Gt)?;
        Ok(args)
    }

    /// Parse an index expression in type position.
    ///
    /// Supports names, literals, addition, and multiplication with correct precedence:
    /// `*` binds tighter than `+`, so `M + N * P` parses as `M + (N * P)`.
    ///
    /// - `Phase` / `module.Phase` → `IndexExpr::Name` (named index or generic param path)
    /// - `3` → `IndexExpr::NatExpr(NatExpr::Literal(..))` (desugars to `range(3)`)
    /// - `N + 1` → `IndexExpr::NatExpr` (compound Nat expression)
    /// - `M * N` → `IndexExpr::NatExpr` (multiplication)
    /// - `M * N + 1` → `IndexExpr::NatExpr` (mixed arithmetic)
    fn parse_index_expr(&mut self) -> Result<IndexExpr, ParseError> {
        // Parse the first multiplicative term
        let first_atom = self.parse_index_expr_atom()?;

        // Check if this is followed by an operator (* or +)
        let has_operator = matches!(self.lexer.peek(), Some(&Token::Star | &Token::Plus));

        if !has_operator {
            // Simple case: a path is an index/type-level name; a literal is a
            // nat expression. Semantic resolution later decides whether the
            // path names a concrete index or an in-scope generic parameter.
            return match first_atom {
                IndexExprAtom::Path(path) => Ok(IndexExpr::Name(path.into_spanned_name_path())),
                IndexExprAtom::Nat(nat_expr) => Ok(IndexExpr::NatExpr(nat_expr)),
            };
        }

        // Has operators: parse as a full nat additive expression. Arithmetic
        // nat expressions currently accept only bare generic Nat variables;
        // qualified paths remain syntactic names in non-arithmetic index
        // position and are rejected here rather than flattened.
        let first_nat = self.index_expr_atom_into_nat_expr(first_atom)?;
        let mut lhs = self.parse_nat_mul_continuation(first_nat)?;

        // Then parse additive continuation: `+ term + term + ...`
        while self.lexer.peek() == Some(&Token::Plus) {
            self.lexer.next_token(); // consume '+'
            let rhs = self.parse_nat_mul_term_in_index()?;
            let full_span = lhs.span().merge(rhs.span());
            lhs = NatExpr::Add(Box::new(lhs), Box::new(rhs), full_span);
        }

        Ok(IndexExpr::NatExpr(lhs))
    }

    /// Parse a multiplicative term in index position: `atom * atom * ...`
    ///
    /// This is a complete multiplicative term (starts by parsing an atom).
    fn parse_nat_mul_term_in_index(&mut self) -> Result<NatExpr, ParseError> {
        let atom = self.parse_index_expr_atom()?;
        let nat_expr = self.index_expr_atom_into_nat_expr(atom)?;
        self.parse_nat_mul_continuation(nat_expr)
    }

    /// Given an already-parsed left-hand atom, continue parsing `* atom * atom ...`
    fn parse_nat_mul_continuation(&mut self, first: NatExpr) -> Result<NatExpr, ParseError> {
        let mut lhs = first;
        while self.lexer.peek() == Some(&Token::Star) {
            self.lexer.next_token(); // consume '*'
            let rhs_atom = self.parse_index_expr_atom()?;
            let rhs = self.index_expr_atom_into_nat_expr(rhs_atom)?;
            let full_span = lhs.span().merge(rhs.span());
            lhs = NatExpr::Mul(Box::new(lhs), Box::new(rhs), full_span);
        }
        Ok(lhs)
    }

    /// Parse a single index-expression atom: a literal or identifier path.
    fn parse_index_expr_atom(&mut self) -> Result<IndexExprAtom, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.advance()?;
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: u64 = text.parse().map_err(|_| ParseError::InvalidNumber {
                    reason: "expected non-negative integer in index position".to_string(),
                    src: self.named_source(),
                    span: span.into(),
                })?;
                Ok(IndexExprAtom::Nat(NatExpr::Literal(value, span)))
            }
            Some(Token::Ident) => Ok(IndexExprAtom::Path(self.parse_ident_path()?)),
            _ => {
                let (tok, span) = self.advance()?;
                Err(self.unexpected_token(
                    "integer literal or type-level name path",
                    &tok.to_string(),
                    span,
                ))
            }
        }
    }

    fn index_expr_atom_into_nat_expr(&self, atom: IndexExprAtom) -> Result<NatExpr, ParseError> {
        match atom {
            IndexExprAtom::Nat(nat_expr) => Ok(nat_expr),
            IndexExprAtom::Path(path) => match path.into_bare() {
                Ok(ident) => Ok(NatExpr::Var(ident)),
                Err(path) => Err(self.unexpected_token(
                    "bare Nat parameter name in arithmetic index expression",
                    &path.display_path(),
                    path.span(),
                )),
            },
        }
    }

    /// Parse generic parameters: `<D: Dim, E: Dim>`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{DeclKind, TypeExprKind};

    fn dim_expr_name(te: &crate::syntax::ast::TypeExpr) -> &str {
        match &te.kind {
            TypeExprKind::DimExpr(dim) => {
                assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
                dim.terms[0].term.name.value.leaf().as_str()
            }
            other => panic!("expected DimExpr, got {other:?}"),
        }
    }

    #[test]
    fn dim_power_flattening_overflow_errors() {
        // Regression: distributing an outer power over a parenthesized
        // dimension group used unchecked `i32` multiplication — panic in
        // debug builds, silently wrong dimension in release builds.
        let source = "param x: (Length^2000000000)^2000000000 = 1.0;";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(err, ParseError::InvalidNumber { .. }), "{err:?}");
    }

    #[test]
    fn unit_power_flattening_overflow_errors() {
        let source = "param x: Length = 1.0 m/(s^2000000000)^2000000000;";
        let err = Parser::new(source).parse_file().unwrap_err();
        assert!(matches!(err, ParseError::InvalidNumber { .. }), "{err:?}");
    }

    #[test]
    fn parse_type_application_in_annotation() {
        let source = "param v: Vec3<Length, ECI> = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.value.leaf().as_str(), "Vec3");
                    assert_eq!(type_args.len(), 2);
                    assert_eq!(dim_expr_name(&type_args[0]), "Length");
                    assert_eq!(dim_expr_name(&type_args[1]), "ECI");
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_application_single_arg() {
        let source = "param t: Timestamp<UTC> = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.value.leaf().as_str(), "Timestamp");
                    assert_eq!(type_args.len(), 1);
                    assert_eq!(dim_expr_name(&type_args[0]), "UTC");
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_qualified_type_application_preserves_name_path() {
        let source = "param v: math.Vec3<Length> = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::TypeApplication { name, type_args } => {
                    assert_eq!(name.value.display_path(), "math.Vec3");
                    assert_eq!(name.value.leaf().as_str(), "Vec3");
                    assert_eq!(type_args.len(), 1);
                }
                other => panic!("expected TypeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_qualified_dim_term_preserves_name_path() {
        let source = "param v: physics.Length / Time = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::DimExpr(dim_expr) => {
                    assert_eq!(dim_expr.terms.len(), 2);
                    assert_eq!(
                        dim_expr.terms[0].term.name.value.display_path(),
                        "physics.Length"
                    );
                    assert_eq!(dim_expr.terms[1].term.name.value.display_path(), "Time");
                }
                other => panic!("expected DimExpr, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_qualified_index_expr_preserves_name_path() {
        let source = "param xs: Dimensionless[mesh.Row] = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::Indexed { indexes, .. } => {
                    assert_eq!(indexes.len(), 1);
                    let IndexExpr::Name(index_path) = &indexes[0] else {
                        panic!("expected Name")
                    };
                    assert_eq!(index_path.value.display_path(), "mesh.Row");
                    assert_eq!(index_path.value.leaf().as_str(), "Row");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_datetime_application_uses_dedicated_variant() {
        // The built-in `Datetime<...>` must not lower to `TypeApplication` —
        // downstream resolution dispatches on the variant rather than on the
        // type name string.
        let source = "param t: Datetime<TT> = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::DatetimeApplication { type_args } => {
                    assert_eq!(type_args.len(), 1);
                    assert_eq!(dim_expr_name(&type_args[0]), "TT");
                }
                TypeExprKind::TypeApplication { name, .. } => panic!(
                    "Datetime<...> must parse as DatetimeApplication, not TypeApplication (got name `{}`)",
                    name.value,
                ),
                other => panic!("expected DatetimeApplication, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_bare_datetime_stays_bare() {
        let source = "param t: Datetime = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::Datetime));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_non_generic_type_still_works() {
        let source = "param v: Length = 1.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::DimExpr(_)));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_indexed_type() {
        let source = "param dv: Velocity[Maneuver] = 1.0 m/s;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.value.as_str(), "dv");
                match &p.type_ann.kind {
                    TypeExprKind::Indexed { base, indexes } => {
                        assert!(matches!(base.kind, TypeExprKind::DimExpr(_)));
                        assert_eq!(indexes.len(), 1);
                        let IndexExpr::Name(ident) = &indexes[0] else {
                            panic!("expected Name")
                        };
                        assert_eq!(ident.value.leaf().as_str(), "Maneuver");
                    }
                    other => panic!("expected Indexed type, got {other:?}"),
                }
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_multi_indexed_type() {
        let source = "param matrix: Dimensionless[Row, Col] = 0.0;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::Indexed { indexes, .. } => {
                    assert_eq!(indexes.len(), 2);
                    let IndexExpr::Name(ident) = &indexes[0] else {
                        panic!("expected Name")
                    };
                    assert_eq!(ident.value.leaf().as_str(), "Row");
                    let IndexExpr::Name(ident) = &indexes[1] else {
                        panic!("expected Name")
                    };
                    assert_eq!(ident.value.leaf().as_str(), "Col");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_with_min_max() {
        let source = "param m: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::DimExpr(_)));
                assert_eq!(p.type_ann.constraints.len(), 2);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Min
                );
                assert_eq!(
                    p.type_ann.constraints[1].kind,
                    crate::syntax::ast::DomainBoundKind::Max
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_with_min_only() {
        let source = "param f: Force(min: 0.01 N) = 0.5 N;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.type_ann.constraints.len(), 1);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Min
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_dimensionless_with_constraint() {
        let source = "param e: Dimensionless(max: 1.0) = 0.85;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::Dimensionless));
                assert_eq!(p.type_ann.constraints.len(), 1);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Max
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_constrained_indexed_type() {
        let source = "param dv: Velocity(min: 0.0 m/s, max: 10000.0 m/s)[Maneuver] = 1.0 m/s;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.type_ann.kind {
                TypeExprKind::Indexed { base, indexes } => {
                    // Constraints are on the base type, not the outer Indexed
                    assert_eq!(base.constraints.len(), 2);
                    assert_eq!(
                        base.constraints[0].kind,
                        crate::syntax::ast::DomainBoundKind::Min
                    );
                    assert_eq!(
                        base.constraints[1].kind,
                        crate::syntax::ast::DomainBoundKind::Max
                    );
                    assert_eq!(indexes.len(), 1);
                    let IndexExpr::Name(ident) = &indexes[0] else {
                        panic!("expected Name")
                    };
                    assert_eq!(ident.value.leaf().as_str(), "Maneuver");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_int_with_constraints() {
        let source = "param n: Int(min: 1, max: 100) = 10;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(matches!(&p.type_ann.kind, TypeExprKind::Int));
                assert_eq!(p.type_ann.constraints.len(), 2);
                assert_eq!(
                    p.type_ann.constraints[0].kind,
                    crate::syntax::ast::DomainBoundKind::Min
                );
                assert_eq!(
                    p.type_ann.constraints[1].kind,
                    crate::syntax::ast::DomainBoundKind::Max
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_type_no_constraints_still_works() {
        let source = "param m: Mass = 1.0 kg;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert!(p.type_ann.constraints.is_empty());
            }
            _ => panic!("expected param"),
        }
    }
}
