use crate::span::Span;

/// A complete source file.
#[derive(Debug, Clone)]
pub struct File {
    pub declarations: Vec<Declaration>,
}

/// A top-level declaration.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub kind: DeclKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum DeclKind {
    Param(ParamDecl),
    Node(NodeDecl),
    Const(ConstDecl),
}

#[derive(Debug, Clone)]
pub struct ParamDecl {
    pub name: Ident,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct NodeDecl {
    pub name: Ident,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: Ident,
    pub value: Expr,
}

/// An identifier with its source span.
#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// An expression node.
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    /// Numeric literal: `1200.0`, `3.98e5`, `200_000`
    Number(f64),
    /// Boolean literal: `true`, `false`
    Bool(bool),
    /// Graph reference: `@lower_name`
    GraphRef(Ident),
    /// Const or built-in constant reference: `UPPER_NAME`, `PI`, `E`
    ConstRef(Ident),
    /// Binary operation: `a + b`, `a * b`, `a ^ b`, `a && b`, etc.
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Unary operation: `-x`, `!x`
    UnaryOp { op: UnaryOp, operand: Box<Expr> },
    /// Function call: `sqrt(x)`, `atan2(y, x)`
    FnCall { name: Ident, args: Vec<Expr> },
    /// Conditional: `if cond { then_expr } else { else_expr }`
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_ast_by_hand() {
        let file = File {
            declarations: vec![Declaration {
                kind: DeclKind::Param(ParamDecl {
                    name: Ident {
                        name: "x".into(),
                        span: Span::new(6, 1),
                    },
                    value: Expr {
                        kind: ExprKind::Number(1.0),
                        span: Span::new(10, 3),
                    },
                }),
                span: Span::new(0, 14),
            }],
        };
        assert_eq!(file.declarations.len(), 1);
    }
}
