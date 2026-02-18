use crate::names::{
    DeclName, DimName, FieldName, FnName, GenericParamName, IndexName, Spanned, StructTypeName,
    UnitName, VariantName,
};
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
    Type(TypeDecl),
    Fn(FnDecl),
    Index(IndexDecl),
    Use(UseDecl),
}

/// Import declaration: `use "./path/to/file.gcl" { name1, name2 as alias };`
#[derive(Debug, Clone)]
pub struct UseDecl {
    /// The file path (quotes stripped, relative to the importing file).
    pub path: String,
    /// The path literal's span (for diagnostics).
    pub path_span: Span,
    /// The items to import (each optionally aliased with `as`).
    pub names: Vec<UseItem>,
}

/// A single item in a `use` declaration, optionally aliased.
///
/// Example: `name1 as local_name` → `UseItem { name: "name1", alias: Some("local_name") }`
/// Example: `name1` → `UseItem { name: "name1", alias: None }`
#[derive(Debug, Clone)]
pub struct UseItem {
    /// The original name from the imported file.
    pub name: Ident,
    /// Optional local alias (introduced by `as`).
    pub alias: Option<Ident>,
}

impl UseItem {
    /// The name that this import introduces into the local scope.
    /// Returns the alias if present, otherwise the original name.
    #[must_use]
    pub fn local_name(&self) -> &str {
        self.alias.as_ref().map_or(&self.name.name, |a| &a.name)
    }

    /// The span of the local name (alias span if aliased, otherwise original name span).
    #[must_use]
    pub fn local_span(&self) -> Span {
        self.alias.as_ref().map_or(self.name.span, |a| a.span)
    }
}

#[derive(Debug, Clone)]
pub struct ParamDecl {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct NodeDecl {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

/// Dimension declaration: `dimension Velocity = Length / Time;`
#[derive(Debug, Clone)]
pub struct DimDecl {
    pub name: Spanned<DimName>,
    /// `None` for base dimensions: `dimension Length;`
    pub definition: Option<DimExpr>,
}

/// Unit declaration: `unit km: Length = 1000 m;`
#[derive(Debug, Clone)]
pub struct UnitDecl {
    pub name: Spanned<UnitName>,
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

/// Type declaration: structs, tagged unions, and empty marker types.
///
/// Struct sugar: `type TransferResult { dv1: Velocity, dv2: Velocity }`
///   → desugars to a single variant named `TransferResult`.
///
/// Tagged union: `type Status { Nominal  Warning { message: Str } }`
///
/// Empty marker type: `type ECI {}`
#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: Spanned<StructTypeName>,
    pub generic_params: Vec<GenericParam>,
    pub derives: Vec<Spanned<DeriveOp>>,
    pub variants: Vec<VariantDecl>,
}

/// An operator that can be derived for a struct type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeriveOp {
    Add,
    Sub,
    Neg,
}

/// A variant in a type declaration: `Impulsive { delta_v: Velocity }` or bare `Nominal`.
#[derive(Debug, Clone)]
pub struct VariantDecl {
    pub name: Spanned<VariantName>,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

/// A field in a variant or struct type declaration.
#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: Spanned<FieldName>,
    pub type_ann: TypeExpr,
}

///// The kind of an index declaration.
#[derive(Debug, Clone)]
pub enum IndexDeclKind {
    /// Named variants: `{ Departure, Correction, Insertion }`
    Named { variants: Vec<Spanned<VariantName>> },
    /// Numeric range: `range(start, end, step: step)`
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        step: Box<Expr>,
    },
}

/// Index declaration: `index Maneuver = { Departure, Correction, Insertion }`
/// or `index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);`
#[derive(Debug, Clone)]
pub struct IndexDecl {
    pub name: Spanned<IndexName>,
    pub kind: IndexDeclKind,
}

/// Function declaration: `fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;`
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: Spanned<FnName>,
    pub generic_params: Vec<GenericParam>,
    pub params: Vec<FnParam>,
    pub return_type: TypeExpr,
    pub body: FnBody,
}

/// A generic parameter: `D: Dim`
#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Spanned<GenericParamName>,
    pub constraint: GenericConstraint,
    /// Optional default type, e.g. `F: Type = Unframed`.
    pub default: Option<TypeExpr>,
}

/// Constraint on a generic parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericConstraint {
    /// `D: Dim` -- the generic stands for a dimension.
    Dim,
    /// `I: Index` -- the generic stands for an index.
    Index,
    /// `F: Type` -- the generic stands for any type (unconstrained phantom parameter).
    Type,
}

/// A function parameter: `x: Length`, `t: Dimensionless`
#[derive(Debug, Clone)]
pub struct FnParam {
    pub name: Ident,
    pub type_ann: TypeExpr,
}

/// The body of a function declaration.
#[derive(Debug, Clone)]
pub enum FnBody {
    /// Short form: `= expr;`
    Short(Expr),
    /// Block form: `{ let a = ...; let b = ...; expr }`
    Block {
        stmts: Vec<LetBinding>,
        expr: Box<Expr>,
    },
}

/// An identifier with its source span.
#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    /// Convert this identifier into a `Spanned<T>`, consuming the name and span.
    #[must_use]
    pub fn into_spanned<T: From<String>>(self) -> Spanned<T> {
        Spanned::new(T::from(self.name), self.span)
    }

    /// Interpret this identifier as a declaration name (const/param/node).
    #[must_use]
    pub fn as_decl_name(&self) -> DeclName {
        DeclName::new(&self.name)
    }

    /// Interpret this identifier as a dimension name.
    #[must_use]
    pub fn as_dim_name(&self) -> DimName {
        DimName::new(&self.name)
    }

    /// Interpret this identifier as a unit name.
    #[must_use]
    pub fn as_unit_name(&self) -> UnitName {
        UnitName::new(&self.name)
    }

    /// Interpret this identifier as a struct type name.
    #[must_use]
    pub fn as_struct_type_name(&self) -> StructTypeName {
        StructTypeName::new(&self.name)
    }

    /// Interpret this identifier as an index name.
    #[must_use]
    pub fn as_index_name(&self) -> IndexName {
        IndexName::new(&self.name)
    }

    /// Interpret this identifier as a function name.
    #[must_use]
    pub fn as_fn_name(&self) -> FnName {
        FnName::new(&self.name)
    }

    /// Interpret this identifier as a struct field name.
    #[must_use]
    pub fn as_field_name(&self) -> FieldName {
        FieldName::new(&self.name)
    }

    /// Interpret this identifier as an index variant name.
    #[must_use]
    pub fn as_variant_name(&self) -> VariantName {
        VariantName::new(&self.name)
    }

    /// Interpret this identifier as a generic parameter name.
    #[must_use]
    pub fn as_generic_param_name(&self) -> GenericParamName {
        GenericParamName::new(&self.name)
    }
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
    /// `Bool`
    Bool,
    /// `Int`
    Int,
    /// A dimension expression like `Length`, `Length^2`, `Mass * Length / Time^2`
    DimExpr(DimExpr),
    /// An indexed type like `Velocity[Maneuver]` or `Dimensionless[A, B]`
    Indexed {
        base: Box<TypeExpr>,
        indexes: Vec<Ident>,
    },
    /// A generic type application like `Vec3<Length, ECI>` or `Timestamp<UTC>`
    TypeApplication {
        name: Ident,
        type_args: Vec<TypeExpr>,
    },
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
    pub name: Spanned<UnitName>,
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
    /// Numeric literal: `1200.0`, `3.98e5`, `200_000.0`
    Number(f64),
    /// Integer literal: `42`, `1_000`
    Integer(i64),
    /// Boolean literal: `true`, `false`
    Bool(bool),
    /// Graph reference: `@lower_name`
    GraphRef(Spanned<DeclName>),
    /// Const or built-in constant reference: `UPPER_NAME`, `PI`, `E`
    ConstRef(Spanned<DeclName>),
    /// Binary operation: `a + b`, `a * b`, `a ^ b`, `a && b`, etc.
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Unary operation: `-x`, `!x`
    UnaryOp { op: UnaryOp, operand: Box<Expr> },
    /// Function call: `sqrt(x)`, `atan2(y, x)`
    FnCall {
        name: Spanned<FnName>,
        args: Vec<Expr>,
    },
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
    /// Phantom type cast: `expr as TypeExpr`
    AsCast {
        expr: Box<Expr>,
        target_type: TypeExpr,
    },
    /// Local variable reference (bare name in block scope): `r1`, `dv1`
    LocalRef(Ident),
    /// Block expression: `{ let a = ...; let b = ...; expr }`
    Block {
        stmts: Vec<LetBinding>,
        expr: Box<Expr>,
    },
    /// Field access: `@transfer.dv1`, `@mission.transfer.dv1`
    FieldAccess {
        expr: Box<Expr>,
        field: Spanned<FieldName>,
    },
    /// Struct construction: `TransferResult { dv1, dv2: a + b, total_dv: dv1 + dv2 }`
    /// or with type args: `Vec3<Length, ECI> { x: 1 km, y: 0 km, z: 0 km }`
    StructConstruction {
        type_name: Spanned<StructTypeName>,
        type_args: Vec<TypeExpr>,
        fields: Vec<FieldInit>,
    },
    /// Map literal: `{ Maneuver::Departure: 2.46 km/s, Maneuver::Correction: 0.05 km/s }`
    MapLiteral { entries: Vec<MapEntry> },
    /// For comprehension: `for m: Maneuver { @delta_v[m] + 1.0 }`
    ForComp {
        bindings: Vec<ForBinding>,
        body: Box<Expr>,
    },
    /// Index access: `@delta_v[m]`, `@delta_v[Maneuver::Departure]`, `@P[a, b]`
    IndexAccess {
        expr: Box<Expr>,
        args: Vec<IndexArg>,
    },
    /// Scan: `scan(source, init, |acc, val| body)`
    Scan {
        source: Box<Expr>,
        init: Box<Expr>,
        acc_name: Ident,
        val_name: Ident,
        body: Box<Expr>,
    },
    /// Unfold: `unfold(init, |prev_i, i| body)`
    ///
    /// Generates an indexed value from a seed by iterating over a range index.
    /// The closure receives `(prev_i, i)` bindings for the previous and current
    /// step indices, and the body can reference `@node_name[prev_i]`.
    Unfold {
        init: Box<Expr>,
        prev_name: Ident,
        curr_name: Ident,
        body: Box<Expr>,
    },
    /// Match expression: `match @status { Nominal => ..., Warning { message } => ... }`
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

/// A `let` binding inside a block body.
#[derive(Debug, Clone)]
pub struct LetBinding {
    pub name: Ident,
    /// Optional type annotation: `let r1: Length = ...`
    pub type_ann: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

/// An entry in a map literal: `Maneuver::Departure: 2.46 km/s`
#[derive(Debug, Clone)]
pub struct MapEntry {
    pub index: Spanned<IndexName>,
    pub variant: Spanned<VariantName>,
    pub value: Expr,
}

/// A binding in a `for` comprehension: `m: Maneuver`
#[derive(Debug, Clone)]
pub struct ForBinding {
    pub var: Ident,
    pub index: Spanned<IndexName>,
}

/// An argument in an index access: either a qualified variant or a loop variable.
#[derive(Debug, Clone)]
pub enum IndexArg {
    /// Qualified variant: `Maneuver::Departure`
    Variant {
        index: Spanned<IndexName>,
        variant: Spanned<VariantName>,
    },
    /// Loop variable: `m`
    Var(Ident),
}

/// A field initializer in struct construction.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Spanned<FieldName>,
    /// `None` means shorthand: `{ dv1 }` is equivalent to `{ dv1: dv1 }`
    pub value: Option<Expr>,
}

/// One arm of a `match` expression: `Impulsive { delta_v } => expr`
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Expr,
    pub span: Span,
}

/// A match pattern: `Impulsive { delta_v }`, `Nominal`, `Warning { message: _ }`
#[derive(Debug, Clone)]
pub struct MatchPattern {
    pub variant_name: Spanned<VariantName>,
    pub bindings: Vec<PatternBinding>,
    pub span: Span,
}

/// A binding in a match pattern.
#[derive(Debug, Clone)]
pub enum PatternBinding {
    /// Bind a field to a variable: `delta_v` (shorthand) or `message: msg` (rename)
    Bind {
        field: Spanned<FieldName>,
        var: Ident,
    },
    /// Wildcard: `message: _`
    Wildcard {
        field: Spanned<FieldName>,
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
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
                    name: Spanned::new(DeclName::new("x"), Span::new(6, 1)),
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

    #[test]
    fn construct_fn_decl_short() {
        let s = Span::new(0, 1);
        let decl = FnDecl {
            name: Spanned::new(FnName::new("double"), s),
            generic_params: vec![GenericParam {
                name: Spanned::new(GenericParamName::new("D"), s),
                constraint: GenericConstraint::Dim,
                default: None,
            }],
            params: vec![FnParam {
                name: Ident {
                    name: "x".into(),
                    span: s,
                },
                type_ann: TypeExpr {
                    kind: TypeExprKind::DimExpr(DimExpr {
                        terms: vec![DimExprItem {
                            op: MulDivOp::Mul,
                            term: DimTerm {
                                name: Ident {
                                    name: "D".into(),
                                    span: s,
                                },
                                power: None,
                                span: s,
                            },
                        }],
                        span: s,
                    }),
                    span: s,
                },
            }],
            return_type: TypeExpr {
                kind: TypeExprKind::DimExpr(DimExpr {
                    terms: vec![DimExprItem {
                        op: MulDivOp::Mul,
                        term: DimTerm {
                            name: Ident {
                                name: "D".into(),
                                span: s,
                            },
                            power: None,
                            span: s,
                        },
                    }],
                    span: s,
                }),
                span: s,
            },
            body: FnBody::Short(Expr {
                kind: ExprKind::BinOp {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr {
                        kind: ExprKind::Number(2.0),
                        span: s,
                    }),
                    rhs: Box::new(Expr {
                        kind: ExprKind::LocalRef(Ident {
                            name: "x".into(),
                            span: s,
                        }),
                        span: s,
                    }),
                },
                span: s,
            }),
        };
        assert_eq!(decl.generic_params.len(), 1);
        assert_eq!(decl.params.len(), 1);
        assert_eq!(decl.generic_params[0].constraint, GenericConstraint::Dim);
    }

    #[test]
    fn construct_fn_decl_block() {
        let s = Span::new(0, 1);
        let decl = FnDecl {
            name: Spanned::new(FnName::new("add_one"), s),
            generic_params: vec![],
            params: vec![FnParam {
                name: Ident {
                    name: "x".into(),
                    span: s,
                },
                type_ann: TypeExpr {
                    kind: TypeExprKind::Dimensionless,
                    span: s,
                },
            }],
            return_type: TypeExpr {
                kind: TypeExprKind::Dimensionless,
                span: s,
            },
            body: FnBody::Block {
                stmts: vec![LetBinding {
                    name: Ident {
                        name: "one".into(),
                        span: s,
                    },
                    type_ann: None,
                    value: Expr {
                        kind: ExprKind::Number(1.0),
                        span: s,
                    },
                    span: s,
                }],
                expr: Box::new(Expr {
                    kind: ExprKind::BinOp {
                        op: BinOp::Add,
                        lhs: Box::new(Expr {
                            kind: ExprKind::LocalRef(Ident {
                                name: "x".into(),
                                span: s,
                            }),
                            span: s,
                        }),
                        rhs: Box::new(Expr {
                            kind: ExprKind::LocalRef(Ident {
                                name: "one".into(),
                                span: s,
                            }),
                            span: s,
                        }),
                    },
                    span: s,
                }),
            },
        };
        assert_eq!(decl.generic_params.len(), 0);
        assert_eq!(decl.params.len(), 1);
        match &decl.body {
            FnBody::Block { stmts, .. } => assert_eq!(stmts.len(), 1),
            FnBody::Short(_) => panic!("expected block body"),
        }
    }
}
