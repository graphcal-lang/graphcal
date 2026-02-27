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

/// An attribute annotation on a declaration: `#[name]` or `#[name(arg1, arg2)]`.
#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Ident,
    pub args: Vec<AttributeArg>,
    pub span: Span,
}

/// An argument inside an attribute's parenthesized list.
///
/// Supports plain identifiers (`pressure_safe`), qualified paths
/// (`Index::Variant`), and parenthesized groups (`(Mode::Boost, Phase::Launch)`).
#[derive(Debug, Clone)]
pub enum AttributeArg {
    /// A path of one or more `::` separated segments: `foo`, `Index::Variant`.
    Path { segments: Vec<Ident>, span: Span },
    /// A parenthesized group of args: `(Index::A, Index::B)`.
    Group { elements: Vec<Self>, span: Span },
}

impl AttributeArg {
    /// Returns the span of this argument.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Path { span, .. } | Self::Group { span, .. } => *span,
        }
    }

    /// If this is a single-segment `Path`, return the identifier.
    ///
    /// Used for backward-compatible access where attributes expect plain identifiers.
    #[must_use]
    pub fn as_single_ident(&self) -> Option<&Ident> {
        match self {
            Self::Path { segments, .. } if segments.len() == 1 => Some(&segments[0]),
            _ => None,
        }
    }
}

/// A top-level declaration.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub attributes: Vec<Attribute>,
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
    Import(ImportDecl),
    Assert(AssertDecl),
    Plot(PlotDecl),
    Figure(FigureDecl),
}

/// Assert declaration: `assert name = <expr>;`
///
/// The body must evaluate to `Bool`. No type annotation (it's always Bool).
/// Assert declarations are leaf nodes — they are evaluated after the entire graph.
#[derive(Debug, Clone)]
pub struct AssertDecl {
    pub name: Spanned<DeclName>,
    pub body: AssertBody,
}

/// The body of an assert declaration.
#[derive(Debug, Clone)]
pub enum AssertBody {
    /// Plain boolean expression: `assert name = expr;`
    Expr(Expr),
    /// Tolerance: `assert name = actual ~= expected +/- tolerance;`
    Tolerance {
        /// The actual value expression (left of `~=`).
        actual: Box<Expr>,
        /// The expected value expression (right of `~=`).
        expected: Box<Expr>,
        /// The tolerance expression (right of `+/-`).
        tolerance: Box<Expr>,
        /// Whether the tolerance is relative (`%`).
        is_relative: bool,
    },
}

/// The chart type in a plot declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartType {
    Line,
    Scatter,
    Bar,
    Heatmap,
}

impl std::fmt::Display for ChartType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line => write!(f, "line"),
            Self::Scatter => write!(f, "scatter"),
            Self::Bar => write!(f, "bar"),
            Self::Heatmap => write!(f, "heatmap"),
        }
    }
}

/// A named field in a plot declaration body.
///
/// Example: `x: for m: OpMode { @total_power[m] }`
#[derive(Debug, Clone)]
pub struct PlotField {
    /// The field name (e.g., "x", "y", "title").
    pub name: Ident,
    /// The field value expression.
    pub value: Expr,
    pub span: Span,
}

/// Plot declaration: `plot name = line { x: ..., y: ..., title: "..." };`
///
/// Plots are leaf declarations that depend on params/nodes via `@`-references.
/// They produce a plot specification, not a runtime `Value`.
#[derive(Debug, Clone)]
pub struct PlotDecl {
    pub name: Spanned<DeclName>,
    pub chart_type: ChartType,
    pub chart_type_span: Span,
    pub fields: Vec<PlotField>,
}

/// Figure declaration: `figure name = { plots: [a, b], title: "..." };`
///
/// Figures group multiple plot declarations into a single combined chart
/// with subplots. Like plots, they are leaf declarations.
#[derive(Debug, Clone)]
pub struct FigureDecl {
    pub name: Spanned<DeclName>,
    /// The plot names referenced by this figure (from the `plots: [...]` field).
    pub plot_names: Vec<Spanned<DeclName>>,
    /// Additional fields (e.g., `title`).
    pub fields: Vec<PlotField>,
}

/// The kind of an `import` declaration.
#[derive(Debug, Clone)]
pub enum ImportKind {
    /// Selective import: `import "path" { name1, name2 as alias };`
    Selective(Vec<ImportItem>),
    /// Module import: `import "path";` or `use "path" as alias;`
    Module { alias: Option<Ident> },
}

/// The path in an `import` declaration.
#[derive(Debug, Clone)]
pub enum ImportPath {
    /// Quoted string path: `"./path/to/file.gcl"`
    FilePath { path: String, span: Span },
    /// Bare identifier path: `nasa/rocket`
    ModulePath { segments: Vec<Ident>, span: Span },
}

impl ImportPath {
    /// Returns the span of this import path.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::FilePath { span, .. } | Self::ModulePath { span, .. } => *span,
        }
    }

    /// Human-readable path string for diagnostics.
    #[must_use]
    pub fn display_path(&self) -> String {
        match self {
            Self::FilePath { path, .. } => path.clone(),
            Self::ModulePath { segments, .. } => segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("/"),
        }
    }
}

/// Import declaration.
///
/// Supports file paths (`import "./file.gcl" { ... };`) and bare module paths
/// (`import nasa/rocket { ... };`). Optionally instantiated with param bindings:
/// `import "./rocket.gcl"(dry_mass = 800.0 kg) { delta_v };`.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// The import path (file-based or bare module path).
    pub path: ImportPath,
    /// Param bindings for module instantiation (empty = non-instantiated).
    pub param_bindings: Vec<ParamBinding>,
    /// The kind of import (selective or module).
    pub kind: ImportKind,
}

/// A param binding in a module instantiation: `name = expr`.
///
/// Used in `import "path"(name = expr, ...) { ... };`
#[derive(Debug, Clone)]
pub struct ParamBinding {
    /// The param name in the imported file.
    pub name: Ident,
    /// The value expression (evaluated in the importer's scope).
    pub value: Expr,
    /// Span covering the entire `name = expr`.
    pub span: Span,
}

/// A single item in an `import` declaration, optionally aliased.
///
/// Example: `name1 as local_name` → `ImportItem { name: "name1", alias: Some("local_name") }`
/// Example: `name1` → `ImportItem { name: "name1", alias: None }`
#[derive(Debug, Clone)]
pub struct ImportItem {
    /// The original name from the imported file.
    pub name: Ident,
    /// Optional local alias (introduced by `as`).
    pub alias: Option<Ident>,
}

impl ImportItem {
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
    /// The default value expression. `None` for required params (no default).
    pub value: Option<Expr>,
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
    pub scale_expr: Expr,
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

/// The kind of a domain constraint bound: `min` or `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainBoundKind {
    Min,
    Max,
}

impl std::fmt::Display for DomainBoundKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Min => write!(f, "min"),
            Self::Max => write!(f, "max"),
        }
    }
}

/// A domain constraint bound on a type expression: `min: expr` or `max: expr`.
///
/// Used in `Type(min: 100 kg, max: 2000 kg)` to declare valid value ranges.
#[derive(Debug, Clone)]
pub struct DomainBound {
    /// The bound kind (`min` or `max`).
    pub kind: DomainBoundKind,
    /// The span of the keyword (`min` or `max`).
    pub kind_span: Span,
    /// The bound value expression.
    pub value: Expr,
    pub span: Span,
}

/// A type expression (dimension annotation on declarations).
/// E.g., `Length`, `Dimensionless`, `Length^3 / Time^2`
///
/// Optionally carries domain constraints: `Mass(min: 100 kg, max: 2000 kg)`.
#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    /// Optional domain constraints on the type.
    pub constraints: Vec<DomainBound>,
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
    /// `Datetime` (bare, without time scale parameter — defaults to UTC)
    Datetime,
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
    /// String literal: `"hello"` (used as arguments to `datetime()`, `epoch()`, etc.)
    StringLiteral(String),
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
    /// Timezone display: `expr -> "America/New_York"` (datetime only)
    DisplayTimezone { expr: Box<Expr>, timezone: String },
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
    /// Table literal: `table[Phase, Maneuver] { ... }`
    /// Semantically equivalent to `MapLiteral` but preserves tabular structure for formatting.
    TableLiteral {
        indexes: Vec<Spanned<IndexName>>,
        entries: Vec<MapEntry>,
    },
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
    /// Tuple match expression: `match (a, b) { (X, Y) => expr, _ => fallback }`
    ///
    /// Preserved in the AST for formatting and tooling. Desugared to nested
    /// `If` / `BinOp(Eq)` chains before evaluation.
    TupleMatch {
        scrutinees: Vec<Expr>,
        arms: Vec<TupleMatchArm>,
    },
    /// Standalone index variant reference: `Maneuver::Departure`
    /// Used in comparisons with loop variables: `m == Maneuver::Departure`
    VariantLiteral {
        index: Spanned<IndexName>,
        variant: Spanned<VariantName>,
    },
    /// Module-qualified graph reference: `@module::name`
    QualifiedGraphRef {
        module: Ident,
        name: Spanned<DeclName>,
    },
    /// Module-qualified const reference: `module::CONST_NAME`
    QualifiedConstRef {
        module: Ident,
        name: Spanned<DeclName>,
    },
    /// Module-qualified function call: `module::fn_name(args...)`
    QualifiedFnCall {
        module: Ident,
        name: Spanned<FnName>,
        args: Vec<Expr>,
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

/// A single key in a map literal entry: `Index::Variant`
#[derive(Debug, Clone)]
pub struct MapEntryKey {
    pub index: Spanned<IndexName>,
    pub variant: Spanned<VariantName>,
}

/// An entry in a map literal.
///
/// Single-axis: `Maneuver::Departure: 2.46 km/s` (keys has 1 element)
/// Multi-axis:  `(Phase::Launch, Maneuver::Departure): 2.46 km/s` (keys has 2+ elements)
#[derive(Debug, Clone)]
pub struct MapEntry {
    pub keys: Vec<MapEntryKey>,
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

/// One arm of a tuple `match` expression: `(X, Y) => expr` or `_ => fallback`
#[derive(Debug, Clone)]
pub struct TupleMatchArm {
    /// `None` for the wildcard `_` arm.
    pub patterns: Option<Vec<Expr>>,
    pub body: Expr,
    pub span: Span,
}

/// A match pattern: `Impulsive { delta_v }`, `Nominal`, `Maneuver::Departure`
#[derive(Debug, Clone)]
pub struct MatchPattern {
    /// For index variant match: `Maneuver::Departure` → `Some(Spanned<IndexName>)`
    /// For tagged union match: `Nominal { ... }` → `None`
    pub qualified_index: Option<Spanned<IndexName>>,
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

// ---------------------------------------------------------------------------
// Desugaring: TupleMatch → nested If / BinOp(Eq)
// ---------------------------------------------------------------------------

/// Desugar all `TupleMatch` nodes in a file to nested `If`/`BinOp(Eq)` chains.
///
/// This must be called before evaluation, dim-checking, and dependency analysis,
/// which only understand the desugared form. The formatter and LSP symbol table
/// operate on the original AST (before desugaring) so they see `TupleMatch`.
pub fn desugar_tuple_matches(file: &mut File) {
    for decl in &mut file.declarations {
        match &mut decl.kind {
            DeclKind::Param(p) => {
                if let Some(v) = &mut p.value {
                    desugar_expr(v);
                }
            }
            DeclKind::Node(n) => desugar_expr(&mut n.value),
            DeclKind::Const(c) => desugar_expr(&mut c.value),
            DeclKind::Unit(u) => {
                if let Some(def) = &mut u.definition {
                    desugar_expr(&mut def.scale_expr);
                }
            }
            DeclKind::Fn(f) => match &mut f.body {
                FnBody::Short(e) => desugar_expr(e),
                FnBody::Block { stmts, expr } => {
                    for s in stmts {
                        desugar_expr(&mut s.value);
                    }
                    desugar_expr(expr);
                }
            },
            DeclKind::Assert(a) => match &mut a.body {
                AssertBody::Expr(e) => desugar_expr(e),
                AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    desugar_expr(actual);
                    desugar_expr(expected);
                    desugar_expr(tolerance);
                }
            },
            DeclKind::Plot(p) => {
                for field in &mut p.fields {
                    desugar_expr(&mut field.value);
                }
            }
            DeclKind::Figure(f) => {
                for field in &mut f.fields {
                    desugar_expr(&mut field.value);
                }
            }
            DeclKind::Dimension(_)
            | DeclKind::Index(_)
            | DeclKind::Type(_)
            | DeclKind::Import(_) => {}
        }
    }
}

/// Recursively desugar `TupleMatch` inside a single expression.
fn desugar_expr(expr: &mut Expr) {
    // First, recurse into children.
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::VariantLiteral { .. }
        | ExprKind::QualifiedGraphRef { .. }
        | ExprKind::QualifiedConstRef { .. }
        // TupleMatch is handled below after recursing into children.
        | ExprKind::TupleMatch { .. } => {}
        ExprKind::BinOp { lhs, rhs, .. } => {
            desugar_expr(lhs);
            desugar_expr(rhs);
        }
        ExprKind::UnaryOp { operand, .. } => desugar_expr(operand),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for a in args {
                desugar_expr(a);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            desugar_expr(condition);
            desugar_expr(then_branch);
            desugar_expr(else_branch);
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. }
        | ExprKind::FieldAccess { expr: inner, .. }
        | ExprKind::IndexAccess { expr: inner, .. } => desugar_expr(inner),
        ExprKind::Block { stmts, expr: body } => {
            for s in stmts {
                desugar_expr(&mut s.value);
            }
            desugar_expr(body);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for f in fields {
                if let Some(v) = &mut f.value {
                    desugar_expr(v);
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for e in entries {
                desugar_expr(&mut e.value);
            }
        }
        ExprKind::ForComp { body, .. } => desugar_expr(body),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            desugar_expr(source);
            desugar_expr(init);
            desugar_expr(body);
        }
        ExprKind::Unfold { init, body, .. } => {
            desugar_expr(init);
            desugar_expr(body);
        }
        ExprKind::Match { scrutinee, arms } => {
            desugar_expr(scrutinee);
            for arm in arms {
                desugar_expr(&mut arm.body);
            }
        }
    }

    // Now desugar TupleMatch at this node.
    if let ExprKind::TupleMatch { scrutinees, arms } = &mut expr.kind {
        // Recurse into children first.
        for s in scrutinees.iter_mut() {
            desugar_expr(s);
        }
        for arm in arms.iter_mut() {
            if let Some(patterns) = &mut arm.patterns {
                for p in patterns {
                    desugar_expr(p);
                }
            }
            desugar_expr(&mut arm.body);
        }

        // Take ownership of arms (scrutinees are borrowed).
        let arms = std::mem::take(arms);
        let span = expr.span;

        expr.kind = desugar_tuple_match(scrutinees, arms, span);
    }
}

/// Build a nested `if` / `BinOp(Eq)` chain from tuple match scrutinees and arms.
///
/// For `match (a, b) { (X, Y) => e1, (P, Q) => e2, _ => e3 }`:
/// ```text
/// if a == X && b == Y { e1 }
/// else if a == P && b == Q { e2 }
/// else { e3 }
/// ```
fn desugar_tuple_match(scrutinees: &[Expr], arms: Vec<TupleMatchArm>, span: Span) -> ExprKind {
    let false_expr = Expr {
        kind: ExprKind::Bool(false),
        span,
    };

    // Build the chain from last arm to first.
    let mut result: Option<Expr> = None;

    for arm in arms.into_iter().rev() {
        match arm.patterns {
            None => {
                // Wildcard arm becomes the else branch.
                result = Some(arm.body);
            }
            Some(patterns) => {
                // Build `scrutinee[0] == pattern[0] && scrutinee[1] == pattern[1] && ...`
                let condition = build_conjunction(scrutinees, &patterns, arm.span);
                let else_branch = result.unwrap_or_else(|| false_expr.clone());
                result = Some(Expr {
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(arm.body),
                        else_branch: Box::new(else_branch),
                    },
                    span: arm.span,
                });
            }
        }
    }

    result.unwrap_or(false_expr).kind
}

/// Build `a == X && b == Y && ...` from parallel scrutinee/pattern slices.
///
/// # Panics
///
/// Panics if `scrutinees` is empty (parser guarantees at least one).
#[expect(
    clippy::unreachable,
    reason = "invariant: parser guarantees arity >= 1"
)]
fn build_conjunction(scrutinees: &[Expr], patterns: &[Expr], span: Span) -> Expr {
    scrutinees
        .iter()
        .zip(patterns.iter())
        .map(|(s, p)| Expr {
            kind: ExprKind::BinOp {
                op: BinOp::Eq,
                lhs: Box::new(s.clone()),
                rhs: Box::new(p.clone()),
            },
            span,
        })
        .reduce(|acc, eq| Expr {
            kind: ExprKind::BinOp {
                op: BinOp::And,
                lhs: Box::new(acc),
                rhs: Box::new(eq),
            },
            span,
        })
        // The parser guarantees at least one scrutinee.
        .unwrap_or_else(|| unreachable!("tuple match must have at least one scrutinee"))
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

    #[test]
    fn construct_ast_by_hand() {
        let file = File {
            declarations: vec![Declaration {
                attributes: vec![],
                kind: DeclKind::Param(ParamDecl {
                    name: Spanned::new(DeclName::new("x"), Span::new(6, 1)),
                    type_ann: TypeExpr {
                        kind: TypeExprKind::Dimensionless,
                        constraints: vec![],
                        span: Span::new(9, 15),
                    },
                    value: Some(Expr {
                        kind: ExprKind::Number(1.0),
                        span: Span::new(27, 3),
                    }),
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
                    constraints: vec![],
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
                constraints: vec![],
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
                    constraints: vec![],
                    span: s,
                },
            }],
            return_type: TypeExpr {
                kind: TypeExprKind::Dimensionless,
                constraints: vec![],
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
