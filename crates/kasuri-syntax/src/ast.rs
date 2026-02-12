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
    Dimension(DimDecl),
    Unit(UnitDecl),
}

#[derive(Debug, Clone)]
pub struct ParamDecl {
    pub name: Ident,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct NodeDecl {
    pub name: Ident,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: Ident,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

/// Dimension declaration: `dimension Velocity = Length / Time;`
#[derive(Debug, Clone)]
pub struct DimDecl {
    pub name: Ident,
    /// `None` for base dimensions: `dimension Length;`
    pub definition: Option<DimExpr>,
}

/// Unit declaration: `unit km: Length = 1000 m;`
#[derive(Debug, Clone)]
pub struct UnitDecl {
    pub name: Ident,
    /// The dimension this unit measures.
    pub dim_type: DimExpr,
    /// Scale definition: `(scale_value, base_unit_expr)`.
    /// `None` for base SI units: `unit m: Length;`
    pub definition: Option<UnitDef>,
}

/// The scale definition part of a unit declaration: `1000 m` or `1 kg * m / s^2`.
#[derive(Debug, Clone)]
pub struct UnitDef {
    pub scale: f64,
    pub unit_expr: UnitExpr,
    pub span: Span,
}

/// An identifier with its source span.
#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

// --- Type expressions ---

/// A type expression (dimension annotation on declarations).
/// E.g., `Length`, `Dimensionless`, `Length^3 / Time^2`
#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

/// The kind of a type expression.
#[derive(Debug, Clone)]
pub enum TypeExprKind {
    /// `Dimensionless`
    Dimensionless,
    /// A dimension expression like `Length`, `Length^2`, `Mass * Length / Time^2`
    DimExpr(DimExpr),
}

/// A dimension expression: product/quotient of dimension terms.
/// E.g., `Length^3 / Time^2`
#[derive(Debug, Clone)]
pub struct DimExpr {
    pub terms: Vec<DimExprItem>,
    pub span: Span,
}

/// One term in a dimension expression with its combining operator.
#[derive(Debug, Clone)]
pub struct DimExprItem {
    /// `Mul` for the first term and for `*`, `Div` for `/`.
    pub op: MulDivOp,
    pub term: DimTerm,
}

/// A single dimension term: `IDENT` or `IDENT ^ INTEGER`
#[derive(Debug, Clone)]
pub struct DimTerm {
    pub name: Ident,
    /// `None` means exponent 1.
    pub power: Option<i32>,
    pub span: Span,
}

// --- Unit expressions ---

/// A unit expression (for literals and conversion targets).
/// E.g., `km`, `m/s^2`, `kg * m / s^2`
#[derive(Debug, Clone)]
pub struct UnitExpr {
    pub terms: Vec<UnitExprItem>,
    pub span: Span,
}

/// One term in a unit expression.
#[derive(Debug, Clone)]
pub struct UnitExprItem {
    /// `Mul` for the first term and for `*`, `Div` for `/`.
    pub op: MulDivOp,
    pub name: Ident,
    /// `None` means exponent 1.
    pub power: Option<i32>,
}

/// Multiply or divide operator used in dimension/unit expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulDivOp {
    Mul,
    Div,
}

// --- Expressions ---

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
    /// Unit-annotated literal: `400 km`, `9.80665 m/s^2`
    UnitLiteral { value: f64, unit: UnitExpr },
    /// Conversion: `expr -> unit_expr`
    Convert { expr: Box<Expr>, target: UnitExpr },
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
                    type_ann: TypeExpr {
                        kind: TypeExprKind::Dimensionless,
                        span: Span::new(9, 15),
                    },
                    value: Expr {
                        kind: ExprKind::Number(1.0),
                        span: Span::new(27, 3),
                    },
                }),
                span: Span::new(0, 31),
            }],
        };
        assert_eq!(file.declarations.len(), 1);
    }
}
