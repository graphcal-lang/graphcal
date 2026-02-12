use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::ast::{
    BinOp, ConstDecl, DeclKind, Declaration, DimDecl, DimExpr, DimExprItem, DimTerm, Expr,
    ExprKind, FieldDecl, FieldInit, File, Ident, LetBinding, MulDivOp, NodeDecl, ParamDecl,
    TypeDecl, TypeExpr, TypeExprKind, UnaryOp, UnitDecl, UnitDef, UnitExpr, UnitExprItem,
};
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
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            lexer: Lexer::new(source),
            source: Arc::new(source.to_string()),
            source_name: "input".to_string(),
        }
    }

    #[must_use]
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

    fn unexpected_token(&self, expected: &str, found: &str, span: Span) -> ParseError {
        ParseError::UnexpectedToken {
            expected: expected.to_string(),
            found: found.to_string(),
            src: self.named_source(),
            span: span.into(),
        }
    }

    fn unexpected_eof(&mut self, expected: &str) -> ParseError {
        ParseError::UnexpectedEof {
            expected: expected.to_string(),
            src: self.named_source(),
            span: Span::new(self.lexer.current_offset(), 0).into(),
        }
    }

    /// Parse the full source file into a [`File`] AST node.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] if the source contains invalid syntax.
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
            Some(Token::Dimension) => self.parse_dimension_decl(),
            Some(Token::Unit) => self.parse_unit_decl(),
            Some(Token::Type) => self.parse_type_decl(),
            Some(_) => {
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
                Err(self.unexpected_token(
                    "`param`, `node`, `const`, `dimension`, `unit`, or `type`",
                    &tok.to_string(),
                    span,
                ))
            }
            None => {
                Err(self.unexpected_eof("`param`, `node`, `const`, `dimension`, `unit`, or `type`"))
            }
        }
    }

    // --- param/node/const with required type annotation ---

    fn parse_param(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Param)?;
        let name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Param(ParamDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    fn parse_node(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Node)?;
        let name = self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Node(NodeDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    fn parse_const(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Const)?;
        let name = self.parse_ident_with_casing("UPPER_SNAKE_CASE", is_upper_snake_case)?;
        self.expect(Token::Colon)?;
        let type_ann = self.parse_type_expr()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expr()?;
        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Const(ConstDecl {
                name,
                type_ann,
                value,
            }),
            span,
        })
    }

    // --- dimension and unit declarations ---

    fn parse_dimension_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Dimension)?;
        let name = self.parse_any_ident()?;

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            Some(self.parse_dim_expr()?)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Dimension(DimDecl { name, definition }),
            span,
        })
    }

    fn parse_unit_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Unit)?;
        let name = self.parse_any_ident()?;
        self.expect(Token::Colon)?;
        let dim_type = self.parse_dim_expr()?;

        let definition = if self.lexer.peek() == Some(&Token::Eq) {
            self.lexer.next_token();
            let def = self.parse_unit_def()?;
            Some(def)
        } else {
            None
        };

        let (_, semi_span) = self.expect(Token::Semicolon)?;
        let span = start_span.merge(semi_span);
        Ok(Declaration {
            kind: DeclKind::Unit(UnitDecl {
                name,
                dim_type,
                definition,
            }),
            span,
        })
    }

    // --- type declaration ---

    fn parse_type_decl(&mut self) -> Result<Declaration, ParseError> {
        let (_, start_span) = self.expect(Token::Type)?;
        let name = self.parse_ident_with_casing("PascalCase", is_pascal_case)?;
        self.expect(Token::LBrace)?;

        // Parse field list: at least one field required
        let mut fields = Vec::new();
        loop {
            // Check for closing brace (empty struct or after trailing comma)
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let field_name =
                self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
            self.expect(Token::Colon)?;
            let type_ann = self.parse_type_expr()?;
            fields.push(FieldDecl {
                name: field_name,
                type_ann,
            });
            // Expect comma or closing brace
            if self.lexer.peek() == Some(&Token::Comma) {
                self.lexer.next_token();
            } else {
                break;
            }
        }

        if fields.is_empty() {
            let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
            return Err(self.unexpected_token("at least one field", &tok.to_string(), span));
        }

        let (_, end_span) = self.expect(Token::RBrace)?;
        let span = start_span.merge(end_span);
        Ok(Declaration {
            kind: DeclKind::Type(TypeDecl { name, fields }),
            span,
        })
    }

    /// Parse the RHS of a unit definition: `NUMBER UNIT_EXPR`
    /// E.g., `1000 m`, `1 kg * m / s^2`, `(PI / 180) rad`
    fn parse_unit_def(&mut self) -> Result<UnitDef, ParseError> {
        // Parse the scale expression: either a plain number or `(expr)`
        let (scale, scale_span) = self.parse_unit_scale()?;
        let unit_expr = self.parse_unit_expr()?;
        let span = scale_span.merge(unit_expr.span);
        Ok(UnitDef {
            scale,
            unit_expr,
            span,
        })
    }

    /// Parse the scale part of a unit definition.
    /// Supports: `1000`, `0.001`, `(PI / 180)`, `(expr)`
    fn parse_unit_scale(&mut self) -> Result<(f64, Span), ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                    ParseError::InvalidNumber {
                        reason: e.to_string(),
                        src: self.named_source(),
                        span: span.into(),
                    }
                })?;
                Ok((value, span))
            }
            Some(Token::LParen) => {
                // Parenthesized const expression: evaluate later
                // For now, parse as expression and evaluate at compile time
                // We parse the expression and store it; the scale will be resolved later.
                // However, UnitDef.scale is f64 -- we need to handle this.
                // For Phase 1, the only non-trivial case is `(PI / 180)`.
                // We'll parse the paren expression, evaluate if it's a simple const expr.
                let (_, lp_span) = self.lexer.next_token().expect("peek confirmed Some");
                let expr = self.parse_expr()?;
                let (_, rp_span) = self.expect(Token::RParen)?;
                let span = lp_span.merge(rp_span);
                // Try to evaluate simple constant expressions
                let scale = self.eval_const_expr(&expr)?;
                Ok((scale, span))
            }
            Some(_) => {
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
                Err(self.unexpected_token("number or `(`", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("number or `(`")),
        }
    }

    /// Evaluate a simple constant expression at parse time (for unit definitions).
    /// Only supports: numbers, PI, E, +, -, *, /, ^, unary -.
    fn eval_const_expr(&self, expr: &Expr) -> Result<f64, ParseError> {
        match &expr.kind {
            ExprKind::Number(n) => Ok(*n),
            ExprKind::ConstRef(ident) => match ident.name.as_str() {
                "PI" => Ok(std::f64::consts::PI),
                "E" => Ok(std::f64::consts::E),
                _ => Err(self.unexpected_token(
                    "PI or E",
                    &format!("constant `{}`", ident.name),
                    ident.span,
                )),
            },
            ExprKind::BinOp { op, lhs, rhs } => {
                let l = self.eval_const_expr(lhs)?;
                let r = self.eval_const_expr(rhs)?;
                Ok(match op {
                    BinOp::Add => l + r,
                    BinOp::Sub => l - r,
                    BinOp::Mul => l * r,
                    BinOp::Div => l / r,
                    BinOp::Pow => l.powf(r),
                    _ => {
                        return Err(self.unexpected_token(
                            "arithmetic operator",
                            &format!("`{op:?}`"),
                            expr.span,
                        ));
                    }
                })
            }
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => Ok(-self.eval_const_expr(operand)?),
            _ => Err(self.unexpected_token("constant expression", "complex expression", expr.span)),
        }
    }

    // --- Type expressions ---

    /// Parse a type expression: `Dimensionless` or a dimension expression.
    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        // Peek to see if it's `Dimensionless`
        if let Some((Token::Ident, span)) = self.lexer.peek_with_span() {
            let text = self.lexer.slice_at(span);
            if text == "Dimensionless" {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                return Ok(TypeExpr {
                    kind: TypeExprKind::Dimensionless,
                    span,
                });
            }
        }
        let dim_expr = self.parse_dim_expr()?;
        let span = dim_expr.span;
        Ok(TypeExpr {
            kind: TypeExprKind::DimExpr(dim_expr),
            span,
        })
    }

    /// Parse a dimension expression: `DimTerm (("*" | "/") DimTerm)*`
    fn parse_dim_expr(&mut self) -> Result<DimExpr, ParseError> {
        let first_term = self.parse_dim_term()?;
        let start_span = first_term.span;
        let mut terms = vec![DimExprItem {
            op: MulDivOp::Mul,
            term: first_term,
        }];

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let term = self.parse_dim_term()?;
                    terms.push(DimExprItem {
                        op: MulDivOp::Mul,
                        term,
                    });
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let term = self.parse_dim_term()?;
                    terms.push(DimExprItem {
                        op: MulDivOp::Div,
                        term,
                    });
                }
                _ => break,
            }
        }

        let end_span = terms.last().expect("at least one term").term.span;
        Ok(DimExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse a single dimension term: `IDENT ("^" INTEGER)?`
    fn parse_dim_term(&mut self) -> Result<DimTerm, ParseError> {
        let name = self.parse_any_ident()?;
        let mut end_span = name.span;

        let power = if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (neg, value, span) = self.parse_integer_literal()?;
            end_span = span;
            Some(if neg { -value } else { value })
        } else {
            None
        };

        Ok(DimTerm {
            span: name.span.merge(end_span),
            name,
            power,
        })
    }

    // --- Unit expressions ---

    /// Parse a unit expression: `IDENT (("*" | "/") IDENT ("^" INTEGER)?)*`
    fn parse_unit_expr(&mut self) -> Result<UnitExpr, ParseError> {
        let first_name = self.parse_any_ident()?;
        let start_span = first_name.span;
        let mut end_span = first_name.span;

        let first_power = if self.lexer.peek() == Some(&Token::Caret) {
            self.lexer.next_token();
            let (neg, value, span) = self.parse_integer_literal()?;
            end_span = span;
            Some(if neg { -value } else { value })
        } else {
            None
        };

        let mut terms = vec![UnitExprItem {
            op: MulDivOp::Mul,
            name: first_name,
            power: first_power,
        }];

        loop {
            match self.lexer.peek() {
                Some(Token::Star) => {
                    self.lexer.next_token();
                    let name = self.parse_any_ident()?;
                    end_span = name.span;
                    let power = if self.lexer.peek() == Some(&Token::Caret) {
                        self.lexer.next_token();
                        let (neg, value, span) = self.parse_integer_literal()?;
                        end_span = span;
                        Some(if neg { -value } else { value })
                    } else {
                        None
                    };
                    terms.push(UnitExprItem {
                        op: MulDivOp::Mul,
                        name,
                        power,
                    });
                }
                Some(Token::Slash) => {
                    self.lexer.next_token();
                    let name = self.parse_any_ident()?;
                    end_span = name.span;
                    let power = if self.lexer.peek() == Some(&Token::Caret) {
                        self.lexer.next_token();
                        let (neg, value, span) = self.parse_integer_literal()?;
                        end_span = span;
                        Some(if neg { -value } else { value })
                    } else {
                        None
                    };
                    terms.push(UnitExprItem {
                        op: MulDivOp::Div,
                        name,
                        power,
                    });
                }
                _ => break,
            }
        }

        Ok(UnitExpr {
            terms,
            span: start_span.merge(end_span),
        })
    }

    /// Parse an integer literal, possibly preceded by `-`.
    /// Returns `(is_negative, absolute_value, span)`.
    fn parse_integer_literal(&mut self) -> Result<(bool, i32, Span), ParseError> {
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

    // --- Expression parsing ---
    // Precedence (lowest to highest):
    //   0. -> (conversion, lowest)
    //   1. if/else (conditional)
    //   2. || (or)
    //   3. && (and)
    //   4. ==, !=, <, >, <=, >= (comparison, non-chaining)
    //   5. +, - (additive)
    //   6. *, / (multiplicative)
    //   7. unary -, ! (prefix)
    //   8. ^ (power, right-associative)
    //   9. atoms (including NUMBER UNIT_EXPR)

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_convert()
    }

    /// Parse conversion: `expr -> unit_expr` (lowest precedence).
    fn parse_convert(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_conditional()?;

        if self.lexer.peek() == Some(&Token::Arrow) {
            self.lexer.next_token();
            let target = self.parse_unit_expr()?;
            let span = expr.span.merge(target.span);
            Ok(Expr {
                kind: ExprKind::Convert {
                    expr: Box::new(expr),
                    target,
                },
                span,
            })
        } else {
            Ok(expr)
        }
    }

    fn parse_conditional(&mut self) -> Result<Expr, ParseError> {
        if self.lexer.peek() == Some(&Token::If) {
            let (_, if_span) = self.lexer.next_token().expect("peek confirmed Some");
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
                let (_, op_span) = self.lexer.next_token().expect("peek confirmed Some");
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
                let (_, op_span) = self.lexer.next_token().expect("peek confirmed Some");
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
        let base = self.parse_postfix()?;
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

    /// Parse postfix operators (field access `.field`), highest precedence after atoms.
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_atom()?;
        while self.lexer.peek() == Some(&Token::Dot) {
            self.lexer.next_token(); // consume '.'
            let field = self.parse_any_ident()?;
            let span = expr.span.merge(field.span);
            expr = Expr {
                kind: ExprKind::FieldAccess {
                    expr: Box::new(expr),
                    field,
                },
                span,
            };
        }
        Ok(expr)
    }

    #[expect(clippy::too_many_lines)] // one match arm per atom kind; splitting would obscure
    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.lexer.peek() {
            Some(Token::Number) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                let text = self.lexer.slice_at(span).replace('_', "");
                let value: f64 = text.parse().map_err(|e: std::num::ParseFloatError| {
                    ParseError::InvalidNumber {
                        reason: e.to_string(),
                        src: self.named_source(),
                        span: span.into(),
                    }
                })?;

                // Check if followed by an identifier (unit literal): `400 km`, `9.80665 m/s^2`
                // A unit literal is NUMBER immediately followed by IDENT with no operator.
                if self.lexer.peek() == Some(&Token::Ident) {
                    let unit_expr = self.parse_unit_expr()?;
                    let full_span = span.merge(unit_expr.span);
                    Ok(Expr {
                        kind: ExprKind::UnitLiteral {
                            value,
                            unit: unit_expr,
                        },
                        span: full_span,
                    })
                } else {
                    Ok(Expr {
                        kind: ExprKind::Number(value),
                        span,
                    })
                }
            }
            Some(Token::True) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                Ok(Expr {
                    kind: ExprKind::Bool(true),
                    span,
                })
            }
            Some(Token::False) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                Ok(Expr {
                    kind: ExprKind::Bool(false),
                    span,
                })
            }
            Some(Token::At) => {
                let (_, at_span) = self.lexer.next_token().expect("peek confirmed Some");
                let ident =
                    self.parse_ident_with_casing("lower_snake_case", is_lower_snake_case)?;
                let span = at_span.merge(ident.span);
                Ok(Expr {
                    kind: ExprKind::GraphRef(ident),
                    span,
                })
            }
            Some(Token::Ident) => {
                let (_, span) = self.lexer.next_token().expect("peek confirmed Some");
                let name = self.lexer.slice_at(span).to_string();

                if is_upper_snake_case(&name) {
                    // Const reference: PI, E, G0, UPPER_NAME
                    Ok(Expr {
                        kind: ExprKind::ConstRef(Ident { name, span }),
                        span,
                    })
                } else if is_pascal_case(&name) && self.lexer.peek() == Some(&Token::LBrace) {
                    // Struct construction: TypeName { field1: expr, field2 }
                    self.parse_struct_construction(Ident { name, span })
                } else if self.lexer.peek() == Some(&Token::LParen) {
                    // Function call: name(args...)
                    self.lexer.next_token(); // consume '('
                    let mut args = Vec::new();
                    if self.lexer.peek() != Some(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        while self.lexer.peek() == Some(&Token::Comma) {
                            self.lexer.next_token();
                            args.push(self.parse_expr()?);
                        }
                    }
                    let (_, rparen_span) = self.expect(Token::RParen)?;
                    let call_span = span.merge(rparen_span);
                    Ok(Expr {
                        kind: ExprKind::FnCall {
                            name: Ident { name, span },
                            args,
                        },
                        span: call_span,
                    })
                } else {
                    // Bare lowercase identifier -> LocalRef (let binding reference)
                    // Semantic validation happens in resolve/dim_check.
                    Ok(Expr {
                        kind: ExprKind::LocalRef(Ident { name, span }),
                        span,
                    })
                }
            }
            Some(Token::LBrace) => {
                // Block expression: { let a = ...; let b = ...; expr }
                self.parse_block()
            }
            Some(Token::LParen) => {
                self.lexer.next_token();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(_) => {
                let (tok, span) = self.lexer.next_token().expect("peek confirmed Some");
                Err(self.unexpected_token("expression", &tok.to_string(), span))
            }
            None => Err(self.unexpected_eof("expression")),
        }
    }

    // --- Block and let binding parsing ---

    /// Parse a block expression: `{ let_binding* expr }`
    fn parse_block(&mut self) -> Result<Expr, ParseError> {
        let (_, start_span) = self.expect(Token::LBrace)?;
        let mut stmts = Vec::new();

        // Parse let bindings while we see `let`
        while self.lexer.peek() == Some(&Token::Let) {
            stmts.push(self.parse_let_binding()?);
        }

        // Parse the final (return) expression
        let expr = self.parse_expr()?;

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

    /// Parse a let binding: `let IDENT (: TypeExpr)? = Expr ;`
    fn parse_let_binding(&mut self) -> Result<LetBinding, ParseError> {
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

    // --- Struct construction ---

    /// Parse struct construction after the type name has been consumed:
    /// `{ field1: expr, field2, ... }`
    fn parse_struct_construction(&mut self, type_name: Ident) -> Result<Expr, ParseError> {
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();

        loop {
            if self.lexer.peek() == Some(&Token::RBrace) {
                break;
            }
            let field_name = self.parse_any_ident()?;

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
            kind: ExprKind::StructConstruction { type_name, fields },
            span,
        })
    }

    // --- Helper methods ---

    #[expect(clippy::needless_pass_by_value)] // Token is small and the API is cleaner with by-value
    fn expect(&mut self, expected: Token) -> Result<(Token, Span), ParseError> {
        let expected_str = format!("`{expected}`");
        match self.lexer.next_token() {
            Some((tok, span)) if tok == expected => Ok((tok, span)),
            Some((tok, span)) => Err(self.unexpected_token(&expected_str, &tok.to_string(), span)),
            None => Err(self.unexpected_eof(&expected_str)),
        }
    }

    /// Parse an identifier and check that it matches the expected casing.
    fn parse_ident_with_casing(
        &mut self,
        casing_desc: &str,
        check: fn(&str) -> bool,
    ) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => {
                let name = self.lexer.slice_at(span).to_string();
                if check(&name) {
                    Ok(Ident { name, span })
                } else {
                    Err(self.unexpected_token(
                        &format!("{casing_desc} identifier"),
                        &format!("identifier `{name}`"),
                        span,
                    ))
                }
            }
            Some((tok, span)) => Err(self.unexpected_token(
                &format!("{casing_desc} identifier"),
                &tok.to_string(),
                span,
            )),
            None => Err(self.unexpected_eof(&format!("{casing_desc} identifier"))),
        }
    }

    /// Parse any identifier regardless of casing.
    fn parse_any_ident(&mut self) -> Result<Ident, ParseError> {
        match self.lexer.next_token() {
            Some((Token::Ident, span)) => Ok(Ident {
                name: self.lexer.slice_at(span).to_string(),
                span,
            }),
            Some((tok, span)) => Err(self.unexpected_token("identifier", &tok.to_string(), span)),
            None => Err(self.unexpected_eof("identifier")),
        }
    }
}

fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_lower_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `PascalCase`: starts with uppercase, contains at least one lowercase letter
/// (to distinguish from `UPPER_SNAKE_CASE` like `GRAVITY`).
fn is_pascal_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars().any(|c| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    // --- Phase 1 declaration tests ---

    #[test]
    fn parse_param_with_type() {
        let file = Parser::new("param x: Dimensionless = 42.0;")
            .parse_file()
            .unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.name, "x");
                assert!(matches!(p.type_ann.kind, TypeExprKind::Dimensionless));
                assert!(
                    matches!(p.value.kind, ExprKind::Number(n) if (n - 42.0).abs() < f64::EPSILON)
                );
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_param_with_dim_type() {
        let file = Parser::new("param alt: Length = 400 km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => {
                assert_eq!(p.name.name, "alt");
                match &p.type_ann.kind {
                    TypeExprKind::DimExpr(d) => {
                        assert_eq!(d.terms.len(), 1);
                        assert_eq!(d.terms[0].term.name.name, "Length");
                    }
                    TypeExprKind::Dimensionless => panic!("expected DimExpr"),
                }
                assert!(matches!(p.value.kind, ExprKind::UnitLiteral { .. }));
            }
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_node_with_compound_dim_type() {
        let file = Parser::new("node gm: Length^3 / Time^2 = 3.98e14 m^3/s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => {
                assert_eq!(n.name.name, "gm");
                match &n.type_ann.kind {
                    TypeExprKind::DimExpr(d) => {
                        assert_eq!(d.terms.len(), 2);
                        assert_eq!(d.terms[0].term.name.name, "Length");
                        assert_eq!(d.terms[0].term.power, Some(3));
                        assert_eq!(d.terms[1].op, MulDivOp::Div);
                        assert_eq!(d.terms[1].term.name.name, "Time");
                        assert_eq!(d.terms[1].term.power, Some(2));
                    }
                    TypeExprKind::Dimensionless => panic!("expected DimExpr"),
                }
            }
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_const_with_type() {
        let file = Parser::new("const G0: Dimensionless = 9.80665;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Const(c) => {
                assert_eq!(c.name.name, "G0");
                assert!(matches!(c.type_ann.kind, TypeExprKind::Dimensionless));
            }
            _ => panic!("expected const"),
        }
    }

    // --- Dimension declarations ---

    #[test]
    fn parse_base_dimension() {
        let file = Parser::new("dimension Length;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Dimension(d) => {
                assert_eq!(d.name.name, "Length");
                assert!(d.definition.is_none());
            }
            _ => panic!("expected dimension"),
        }
    }

    #[test]
    fn parse_derived_dimension() {
        let file = Parser::new("dimension Velocity = Length / Time;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Dimension(d) => {
                assert_eq!(d.name.name, "Velocity");
                let def = d.definition.as_ref().unwrap();
                assert_eq!(def.terms.len(), 2);
                assert_eq!(def.terms[0].term.name.name, "Length");
                assert_eq!(def.terms[1].op, MulDivOp::Div);
                assert_eq!(def.terms[1].term.name.name, "Time");
            }
            _ => panic!("expected dimension"),
        }
    }

    // --- Unit declarations ---

    #[test]
    fn parse_base_unit() {
        let file = Parser::new("unit m: Length;").parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.name, "m");
                assert_eq!(u.dim_type.terms[0].term.name.name, "Length");
                assert!(u.definition.is_none());
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_derived_unit() {
        let file = Parser::new("unit km: Length = 1000 m;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.name, "km");
                let def = u.definition.as_ref().unwrap();
                assert!((def.scale - 1000.0).abs() < f64::EPSILON);
                assert_eq!(def.unit_expr.terms.len(), 1);
                assert_eq!(def.unit_expr.terms[0].name.name, "m");
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_compound_unit_decl() {
        let file = Parser::new("unit N: Force = 1 kg * m / s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.name, "N");
                let def = u.definition.as_ref().unwrap();
                assert!((def.scale - 1.0).abs() < f64::EPSILON);
                assert_eq!(def.unit_expr.terms.len(), 3);
                assert_eq!(def.unit_expr.terms[0].name.name, "kg");
                assert_eq!(def.unit_expr.terms[1].op, MulDivOp::Mul);
                assert_eq!(def.unit_expr.terms[1].name.name, "m");
                assert_eq!(def.unit_expr.terms[2].op, MulDivOp::Div);
                assert_eq!(def.unit_expr.terms[2].name.name, "s");
                assert_eq!(def.unit_expr.terms[2].power, Some(2));
            }
            _ => panic!("expected unit"),
        }
    }

    #[test]
    fn parse_unit_decl_with_paren_expr() {
        let file = Parser::new("unit deg: Angle = (PI / 180) rad;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Unit(u) => {
                assert_eq!(u.name.name, "deg");
                let def = u.definition.as_ref().unwrap();
                assert!(
                    (def.scale - std::f64::consts::PI / 180.0).abs() < 1e-10,
                    "scale = {}",
                    def.scale
                );
                assert_eq!(def.unit_expr.terms[0].name.name, "rad");
            }
            _ => panic!("expected unit"),
        }
    }

    // --- Unit literals ---

    #[test]
    fn parse_unit_literal() {
        let file = Parser::new("param alt: Length = 400 km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Param(p) => match &p.value.kind {
                ExprKind::UnitLiteral { value, unit } => {
                    assert!((value - 400.0).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 1);
                    assert_eq!(unit.terms[0].name.name, "km");
                }
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn parse_compound_unit_literal() {
        let file = Parser::new("const G0: Acceleration = 9.80665 m/s^2;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Const(c) => match &c.value.kind {
                ExprKind::UnitLiteral { value, unit } => {
                    assert!((value - 9.80665).abs() < f64::EPSILON);
                    assert_eq!(unit.terms.len(), 2);
                    assert_eq!(unit.terms[0].name.name, "m");
                    assert_eq!(unit.terms[1].op, MulDivOp::Div);
                    assert_eq!(unit.terms[1].name.name, "s");
                    assert_eq!(unit.terms[1].power, Some(2));
                }
                _ => panic!("expected UnitLiteral"),
            },
            _ => panic!("expected const"),
        }
    }

    // --- Conversion ---

    #[test]
    fn parse_conversion() {
        let file = Parser::new("node speed_kmh: Velocity = @speed -> km/hour;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { expr, target } => {
                    assert!(matches!(&expr.kind, ExprKind::GraphRef(id) if id.name == "speed"));
                    assert_eq!(target.terms.len(), 2);
                    assert_eq!(target.terms[0].name.name, "km");
                    assert_eq!(target.terms[1].op, MulDivOp::Div);
                    assert_eq!(target.terms[1].name.name, "hour");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_convert_binds_loosely() {
        // @a + @b -> km should be (@a + @b) -> km
        let file = Parser::new("node x: Length = @a + @b -> km;")
            .parse_file()
            .unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::Convert { expr, target } => {
                    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
                    assert_eq!(target.terms[0].name.name, "km");
                }
                _ => panic!("expected Convert"),
            },
            _ => panic!("expected node"),
        }
    }

    // --- Expression parsing (preserved from Phase 0) ---

    /// Helper: parse a single node declaration and return its expression.
    fn parse_node_expr(input: &str) -> Expr {
        let full = format!("node x: Dimensionless = {input};");
        let file = Parser::new(&full).parse_file().unwrap();
        match file.declarations.into_iter().next().unwrap().kind {
            DeclKind::Node(n) => n.value,
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_arithmetic_precedence() {
        let expr = parse_node_expr("1.0 + 2.0 * 3.0");
        assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
        if let ExprKind::BinOp { rhs, .. } = &expr.kind {
            assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Mul, .. }));
        }
    }

    #[test]
    fn parse_left_associative_add() {
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

    // --- Error tests ---

    #[test]
    fn parse_error_missing_semicolon() {
        let result = Parser::new("param x: Dimensionless = 1.0").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_unexpected_token() {
        let result = Parser::new("+ 1.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_with_comments() {
        let input = "// this is a comment\nparam x: Dimensionless = 1.0;\n// another comment";
        let file = Parser::new(input).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
    }

    #[test]
    fn parse_error_bad_param_casing() {
        let result = Parser::new("param BadName: Dimensionless = 1.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_bad_const_casing() {
        let result = Parser::new("const bad_name: Dimensionless = 42.0;").parse_file();
        assert!(result.is_err());
    }

    // --- Milestone: orbital.ksr syntax ---

    #[test]
    fn parse_orbital_milestone_syntax() {
        let source = r"
dimension Velocity = Length / Time;

param alt: Length = 400 km;
param period: Time = 90 min;
const R_EARTH: Length = 6371 km;

node circumference: Length = 2.0 * PI * (R_EARTH + @alt);
node speed: Velocity = @circumference / @period;
node speed_kmh: Velocity = @speed -> km/hour;
";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 7);

        let names: Vec<&str> = file
            .declarations
            .iter()
            .map(|d| match &d.kind {
                DeclKind::Param(p) => p.name.name.as_str(),
                DeclKind::Node(n) => n.name.name.as_str(),
                DeclKind::Const(c) => c.name.name.as_str(),
                DeclKind::Dimension(d) => d.name.name.as_str(),
                DeclKind::Unit(u) => u.name.name.as_str(),
                DeclKind::Type(t) => t.name.name.as_str(),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "Velocity",
                "alt",
                "period",
                "R_EARTH",
                "circumference",
                "speed",
                "speed_kmh"
            ]
        );
    }

    // --- Phase 2 type declaration tests ---

    #[test]
    fn parse_type_decl_single_field() {
        let source = "type Orbit { sma: Length }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.name, "Orbit");
                assert_eq!(t.fields.len(), 1);
                assert_eq!(t.fields[0].name.name, "sma");
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_multiple_fields() {
        let source = "type TransferResult { dv1: Velocity, dv2: Velocity }";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 1);
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.name.name, "TransferResult");
                assert_eq!(t.fields.len(), 2);
                assert_eq!(t.fields[0].name.name, "dv1");
                assert_eq!(t.fields[1].name.name, "dv2");
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_trailing_comma() {
        let source = "type TransferResult { dv1: Velocity, dv2: Velocity, }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.fields.len(), 2);
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_empty_struct_error() {
        let source = "type Empty {}";
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_type_decl_uppercase_name_error() {
        // UPPER_SNAKE_CASE should be rejected (not PascalCase)
        let source = "type ORBIT { sma: Length }";
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_type_decl_lowercase_name_error() {
        let source = "type orbit { sma: Length }";
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parse_type_decl_with_dim_expr_field() {
        // Field type is a composite dimension expression
        let source = "type TransferResult { dv: Length / Time }";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Type(t) => {
                assert_eq!(t.fields.len(), 1);
                assert_eq!(t.fields[0].name.name, "dv");
                // The type_ann should be a DimExpr with division
                match &t.fields[0].type_ann.kind {
                    TypeExprKind::DimExpr(_) => {} // ok
                    other => panic!("expected DimExpr, got {other:?}"),
                }
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn parse_type_decl_mixed_with_other_decls() {
        let source = r"
dimension Velocity = Length / Time;
type TransferResult { dv1: Velocity, dv2: Velocity }
param alt: Length = 400 km;
";
        let file = Parser::new(source).parse_file().unwrap();
        assert_eq!(file.declarations.len(), 3);
        assert!(matches!(&file.declarations[0].kind, DeclKind::Dimension(_)));
        assert!(matches!(&file.declarations[1].kind, DeclKind::Type(_)));
        assert!(matches!(&file.declarations[2].kind, DeclKind::Param(_)));
    }

    #[test]
    fn is_pascal_case_examples() {
        assert!(is_pascal_case("TransferResult"));
        assert!(is_pascal_case("Orbit"));
        assert!(is_pascal_case("Ab"));
        assert!(!is_pascal_case("ORBIT"));
        assert!(!is_pascal_case("UPPER_SNAKE"));
        assert!(!is_pascal_case("orbit"));
        assert!(!is_pascal_case("lower_snake"));
        assert!(!is_pascal_case(""));
    }

    // --- Phase 2 block / let / LocalRef tests ---

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
                    // Final expression references two LocalRefs via BinOp
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
        // Block with no let bindings, just an expression
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
        // A bare lowercase identifier in expression position parses as LocalRef
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

    // --- Phase 2 struct construction and field access tests ---

    #[test]
    fn parse_struct_construction_explicit_fields() {
        let source = "node t: Dimensionless = TransferResult { dv1: @a + @b, dv2: @c };";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::StructConstruction { type_name, fields } => {
                    assert_eq!(type_name.name, "TransferResult");
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name.name, "dv1");
                    assert!(fields[0].value.is_some());
                    assert_eq!(fields[1].name.name, "dv2");
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
                    ExprKind::StructConstruction { type_name, fields } => {
                        assert_eq!(type_name.name, "TransferResult");
                        assert_eq!(fields.len(), 2);
                        // Shorthand: value is None
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
    fn parse_field_access() {
        let source = "node x: Dimensionless = @transfer.dv1;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::FieldAccess { expr, field } => {
                    assert!(
                        matches!(&expr.kind, ExprKind::GraphRef(ident) if ident.name == "transfer")
                    );
                    assert_eq!(field.name, "dv1");
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_chained_field_access() {
        let source = "node x: Dimensionless = @mission.transfer.dv1;";
        let file = Parser::new(source).parse_file().unwrap();
        match &file.declarations[0].kind {
            DeclKind::Node(n) => match &n.value.kind {
                ExprKind::FieldAccess { expr, field } => {
                    assert_eq!(field.name, "dv1");
                    // Inner should be another FieldAccess
                    match &expr.kind {
                        ExprKind::FieldAccess {
                            expr: inner,
                            field: mid_field,
                        } => {
                            assert_eq!(mid_field.name, "transfer");
                            assert!(
                                matches!(&inner.kind, ExprKind::GraphRef(ident) if ident.name == "mission")
                            );
                        }
                        other => panic!("expected inner FieldAccess, got {other:?}"),
                    }
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            _ => panic!("expected node"),
        }
    }

    #[test]
    fn parse_field_access_in_arithmetic() {
        // Field access should bind tighter than arithmetic
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
