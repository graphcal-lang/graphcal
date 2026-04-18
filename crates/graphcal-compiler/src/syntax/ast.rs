use crate::syntax::names::{
    DeclName, DimName, FieldName, FnName, GenericParamName, IndexName, Spanned, StructTypeName,
    UnitName, VariantName,
};
use crate::syntax::span::Span;

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

/// Visibility and bindability annotation on a declaration.
///
/// Tracks the two-axis split from the visibility / bindability axioms:
/// - `Private`: no annotation — the declaration is not visible outside the library.
/// - `Public`: `pub` — visible at the include boundary but not bindable.
/// - `PublicBind`: `pub(bind)` — visible AND bindable via include param bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
    PublicBind,
}

impl Visibility {
    /// Returns `true` for `Public` and `PublicBind`.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public | Self::PublicBind)
    }

    /// Returns `true` for `PublicBind`.
    #[must_use]
    pub const fn is_bindable(self) -> bool {
        matches!(self, Self::PublicBind)
    }
}

/// A top-level declaration.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub attributes: Vec<Attribute>,
    pub visibility: Visibility,
    pub kind: DeclKind,
    pub span: Span,
}

impl Declaration {
    /// Returns `true` if this declaration is visible (`pub` or `pub(bind)`).
    #[must_use]
    pub const fn is_pub(&self) -> bool {
        self.visibility.is_public()
    }

    /// Returns `true` if this declaration is bindable (`pub(bind)`).
    #[must_use]
    pub const fn is_bindable(&self) -> bool {
        self.visibility.is_bindable()
    }
}

#[derive(Debug, Clone)]
pub enum DeclKind {
    Param(ParamDecl),
    Node(NodeDecl),
    ConstNode(ConstNodeDecl),
    BaseDimension(BaseDimDecl),
    Dimension(DimDecl),
    Unit(UnitDecl),
    Type(TypeDecl),
    UnionType(UnionTypeDecl),
    Index(IndexDecl),
    Import(ImportDecl),
    Include(IncludeDecl),
    Dag(DagDecl),
    Assert(AssertDecl),
    Plot(PlotDecl),
    Figure(FigureDecl),
    Layer(LayerDecl),
}

impl DeclKind {
    /// Returns the declaration name as a string slice and its span, if the
    /// variant carries a name. `Import` and `Include` have no name and return
    /// `None`.
    #[must_use]
    pub fn name_and_span(&self) -> Option<(&str, Span)> {
        match self {
            Self::Param(p) => Some((p.name.value.as_str(), p.name.span)),
            Self::Node(n) => Some((n.name.value.as_str(), n.name.span)),
            Self::ConstNode(c) => Some((c.name.value.as_str(), c.name.span)),
            Self::BaseDimension(d) => Some((d.name.value.as_str(), d.name.span)),
            Self::Dimension(d) => Some((d.name.value.as_str(), d.name.span)),
            Self::Unit(u) => Some((u.name.value.as_str(), u.name.span)),
            Self::Type(t) => Some((t.name.value.as_str(), t.name.span)),
            Self::UnionType(u) => Some((u.name.value.as_str(), u.name.span)),
            Self::Index(i) => Some((i.name.value.as_str(), i.name.span)),
            Self::Dag(d) => Some((d.name.value.as_str(), d.name.span)),
            Self::Assert(a) => Some((a.name.value.as_str(), a.name.span)),
            Self::Plot(p) => Some((p.name.value.as_str(), p.name.span)),
            Self::Figure(f) => Some((f.name.value.as_str(), f.name.span)),
            Self::Layer(l) => Some((l.name.value.as_str(), l.name.span)),
            Self::Import(_) | Self::Include(_) => None,
        }
    }
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

/// The mark type in a plot declaration (Vega-Lite grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkType {
    Point,
    Line,
    Bar,
    Area,
    Rect,
    Tick,
}

impl std::fmt::Display for MarkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Point => write!(f, "point"),
            Self::Line => write!(f, "line"),
            Self::Bar => write!(f, "bar"),
            Self::Area => write!(f, "area"),
            Self::Rect => write!(f, "rect"),
            Self::Tick => write!(f, "tick"),
        }
    }
}

/// An encoding channel in a plot declaration (Vega-Lite grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingChannel {
    X,
    Y,
    Color,
    Size,
    Shape,
    Opacity,
    Detail,
    Text,
    Tooltip,
}

impl std::fmt::Display for EncodingChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X => write!(f, "x"),
            Self::Y => write!(f, "y"),
            Self::Color => write!(f, "color"),
            Self::Size => write!(f, "size"),
            Self::Shape => write!(f, "shape"),
            Self::Opacity => write!(f, "opacity"),
            Self::Detail => write!(f, "detail"),
            Self::Text => write!(f, "text"),
            Self::Tooltip => write!(f, "tooltip"),
        }
    }
}

/// The mark specification in a plot declaration: `mark: point` or `mark: line { stroke_width: 2.0 }`.
#[derive(Debug, Clone)]
pub struct MarkSpec {
    pub mark_type: MarkType,
    pub mark_type_span: Span,
    pub properties: Vec<PlotField>,
    pub span: Span,
}

/// An encoding channel mapping in a plot declaration.
///
/// Example: `x: for m: OpMode { @total_power[m] }`
#[derive(Debug, Clone)]
pub struct Encoding {
    pub channel: EncodingChannel,
    pub channel_span: Span,
    pub value: Expr,
    pub span: Span,
}

/// A named field in a plot or figure declaration body.
///
/// Example: `title: "My Chart"`
#[derive(Debug, Clone)]
pub struct PlotField {
    /// The field name (e.g., "title", "width", "height").
    pub name: Ident,
    /// The field value expression.
    pub value: Expr,
    pub span: Span,
}

/// Plot declaration: `plot name = { mark: point, encode: { x: ..., y: ... }, title: "..." };`
///
/// Plots are leaf declarations that depend on params/nodes via `@`-references.
/// They produce a plot specification, not a runtime `Value`.
#[derive(Debug, Clone)]
pub struct PlotDecl {
    pub name: Spanned<DeclName>,
    pub mark: MarkSpec,
    pub encodings: Vec<Encoding>,
    pub properties: Vec<PlotField>,
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

/// A layer declaration: overlays multiple plots on shared axes.
///
/// Syntax: `layer name = { plots: [a, b], title: "..." };`
///
/// Unlike `figure` (which tiles plots side-by-side), `layer` overlays
/// them on the same coordinate space. In Vega-Lite this maps to the
/// `"layer"` composition operator.
#[derive(Debug, Clone)]
pub struct LayerDecl {
    pub name: Spanned<DeclName>,
    /// The plot names to overlay (from the `plots: [...]` field).
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

/// The path in an `import` or `include` declaration.
#[derive(Debug, Clone)]
pub enum ImportPath {
    /// Quoted string path: `"./path/to/file.gcl"`
    FilePath { path: String, span: Span },
    /// Bare identifier path: `nasa/rocket` or `dag_name`
    ModulePath { segments: Vec<Ident>, span: Span },
    /// Parent scope path: `..` (1 level), `../..` (2 levels), etc.
    /// Only valid inside `dag` blocks to access the enclosing scope.
    ParentScope { levels: u32, span: Span },
    /// Cross-file DAG path: `"./file.gcl"/dag_name`
    /// References an inline DAG defined in another file.
    CrossFileDag {
        file_path: String,
        dag_name: Ident,
        span: Span,
    },
}

impl ImportPath {
    /// Returns the span of this import path.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::FilePath { span, .. }
            | Self::ModulePath { span, .. }
            | Self::ParentScope { span, .. }
            | Self::CrossFileDag { span, .. } => *span,
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
            Self::ParentScope { levels, .. } => {
                let mut path = "..".to_string();
                for _ in 1..*levels {
                    path.push_str("/..");
                }
                path
            }
            Self::CrossFileDag {
                file_path,
                dag_name,
                ..
            } => format!("\"{file_path}\"/{}", dag_name.name),
        }
    }

    /// Returns `true` if this is a `ParentScope` path (`..`, `../..`, etc.).
    #[must_use]
    pub const fn is_parent_scope(&self) -> bool {
        matches!(self, Self::ParentScope { .. })
    }

    /// Returns `true` if this is a `CrossFileDag` path (`"./file.gcl"/dag_name`).
    #[must_use]
    pub const fn is_cross_file_dag(&self) -> bool {
        matches!(self, Self::CrossFileDag { .. })
    }
}

/// Import declaration (compile-time definition import).
///
/// Supports file paths (`import "./file.gcl" { ... };`) and bare module paths
/// (`import nasa/rocket { ... };`). No param bindings — for DAG instantiation
/// with param bindings, use `include` instead.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// The import path (file-based or bare module path).
    pub path: ImportPath,
    /// The kind of import (selective or module).
    pub kind: ImportKind,
}

/// Include declaration (DAG embedding / instantiation).
///
/// Supports file paths and bare module paths with optional param/index bindings:
/// `include "./rocket.gcl"(dry_mass: 800.0 kg) { delta_v };`
/// `include "./rocket.gcl"(dry_mass: 800.0 kg) as rocket;`
#[derive(Debug, Clone)]
pub struct IncludeDecl {
    /// The import path (file-based or bare module path).
    pub path: ImportPath,
    /// Param/index bindings for DAG instantiation.
    pub param_bindings: Vec<ParamBinding>,
    /// The kind of include (selective or module).
    pub kind: ImportKind,
}

/// Inline DAG declaration: `dag name { ... }`
///
/// The body contains declarations (same as file-level). Semantics are not yet
/// implemented — this phase only parses the syntax.
#[derive(Debug, Clone)]
pub struct DagDecl {
    /// The DAG name.
    pub name: Spanned<DeclName>,
    /// Declarations inside the DAG block.
    pub body: Vec<Declaration>,
    /// Span covering the entire `dag name { ... }` block.
    pub span: Span,
}

/// A param binding in a module instantiation: `name: expr`.
///
/// Used in `include "path"(name: expr, ...) { ... };`
#[derive(Debug, Clone)]
pub struct ParamBinding {
    /// The param name in the imported file.
    pub name: Ident,
    /// The value expression (evaluated in the importer's scope).
    pub value: Expr,
    /// Span covering the entire `name: expr`.
    pub span: Span,
}

/// A single item in an `import` declaration, optionally aliased.
///
/// Example: `name1 as local_name` → `ImportItem { name: "name1", alias: Some("local_name") }`
/// Example: `name1` → `ImportItem { name: "name1", alias: None }`
/// Example: `pub name1` → re-exported at the importer (selective form).
#[derive(Debug, Clone)]
pub struct ImportItem {
    /// Attributes on this import item (e.g., `#[expected_fail(...)]`).
    pub attributes: Vec<Attribute>,
    /// Whether this item is re-exported (`pub` prefix) from the importer.
    pub is_pub: bool,
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

/// Runtime node declaration: `node name: Type = expr;`
#[derive(Debug, Clone)]
pub struct NodeDecl {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

/// Const node declaration: `const node name: Type = expr;`
#[derive(Debug, Clone)]
pub struct ConstNodeDecl {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr,
    pub value: Expr,
}

/// Base dimension declaration: `base dim Length;`
#[derive(Debug, Clone)]
pub struct BaseDimDecl {
    pub name: Spanned<DimName>,
}

/// Dimension declaration with a body or required.
///
/// Two forms:
/// - Derived: `dim Velocity = Length / Time;` — `definition: Some(...)`
/// - Required: `dim D;` — `definition: None`. The library requires a
///   dimension to be bound here from outside (via an include with
///   dim bindings). Treated like an opaque base dimension when the
///   library is compiled standalone.
#[derive(Debug, Clone)]
pub struct DimDecl {
    pub name: Spanned<DimName>,
    pub definition: Option<DimExpr>,
}

/// Unit declaration: `unit km: Length = 1000 m;`, `const unit km: Length = 1000 m;`,
/// or `base unit m: Length;`.
#[derive(Debug, Clone)]
pub struct UnitDecl {
    pub name: Spanned<UnitName>,
    /// The dimension this unit measures.
    pub dim_type: DimExpr,
    /// Scale definition: `(scale_value, base_unit_expr)`.
    /// `None` iff this is a base unit (`base unit m: Length;`).
    pub definition: Option<UnitDef>,
}

/// The scale definition part of a unit declaration: `1000 m` or `1 kg * m / s^2`.
#[derive(Debug, Clone)]
pub struct UnitDef {
    pub scale_expr: Expr,
    pub unit_expr: UnitExpr,
    pub span: Span,
}

/// Type declaration: record types and required types.
///
/// Forms:
/// - Record type: `type TransferResult { dv1: Velocity, dv2: Velocity }`
/// - Empty record type (unit-like marker): `type Eci {}`
/// - Required type: `type T;` — the library requires a type bound from
///   outside; no body at declaration.
#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: Spanned<StructTypeName>,
    pub generic_params: Vec<GenericParam>,
    /// Fields of the type:
    /// - `None` — required type (`type T;`, no body).
    /// - `Some(vec![])` — empty record (`type T {}`).
    /// - `Some(non-empty)` — record with fields.
    pub fields: Option<Vec<FieldDecl>>,
}

/// Union type declaration: `type ManeuverKind = Impulsive | Coasting;`
///
/// Members must reference previously defined record or unit types.
#[derive(Debug, Clone)]
pub struct UnionTypeDecl {
    pub name: Spanned<StructTypeName>,
    pub generic_params: Vec<GenericParam>,
    pub members: Vec<UnionMember>,
}

/// A member of a union type, optionally with type arguments.
///
/// E.g., `Ok<D>` in `type Result<D: Dim> = Ok<D> | Err;`
#[derive(Debug, Clone)]
pub struct UnionMember {
    pub name: Spanned<StructTypeName>,
    pub type_args: Vec<TypeExpr>,
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
    /// Numeric range: `linspace(start, end, step: step)`
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        step: Box<Expr>,
    },
    /// Required named index (no variants): `index Foo;`
    ///
    /// Must be bound via parameterized import.
    RequiredNamed,
    /// Required range index with dimension constraint: `index Foo: Time;`
    ///
    /// Must be bound via parameterized import.
    RequiredRange { dimension: DimExpr },
}

impl IndexDeclKind {
    /// Returns `true` for required index declarations that must be bound via import.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::RequiredNamed | Self::RequiredRange { .. })
    }
}

/// Index declaration: `index Maneuver = { Departure, Correction, Insertion };`
/// or `index TimeStep = linspace(0.0 s, 100.0 s, step: 0.1 s);`
#[derive(Debug, Clone)]
pub struct IndexDecl {
    pub name: Spanned<IndexName>,
    pub kind: IndexDeclKind,
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
    /// `N: Nat` -- the generic stands for a natural number (type-level).
    Nat,
    /// `F: Type` -- the generic stands for any type (unconstrained phantom parameter).
    Type,
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
/// An expression in index position of an indexed type.
///
/// In `Velocity[Maneuver]`, the `Maneuver` is an `IndexExpr::Name`.
/// In `Dimensionless[3, 4]`, `3` and `4` are `IndexExpr::NatLiteral`.
/// In `D[N + 1]`, `N + 1` is an `IndexExpr::NatExpr`.
#[derive(Debug, Clone)]
pub enum IndexExpr {
    /// A named index or generic parameter: `Maneuver`, `I`, `N`
    Name(Ident),
    /// An integer literal in index position: `3` (desugars to `range(3)` internally)
    NatLiteral(u64, Span),
    /// A compound Nat expression in index position: `N + 1`, `M + N`
    NatExpr(NatExpr),
}

impl IndexExpr {
    /// Get the source span of this index expression.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Name(ident) => ident.span,
            Self::NatLiteral(_, span) => *span,
            Self::NatExpr(nat_expr) => nat_expr.span(),
        }
    }
}

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
    /// An indexed type like `Velocity[Maneuver]`, `Dimensionless[3, 4]`, or `D[M, N]`
    Indexed {
        base: Box<TypeExpr>,
        indexes: Vec<IndexExpr>,
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
    /// Graph reference: `@name` (param, node, or const node)
    GraphRef(Spanned<DeclName>),
    /// Built-in constant reference: `PI`, `E`, `TAU`
    ConstRef(Spanned<DeclName>),
    /// Binary operation: `a + b`, `a * b`, `a ^ b`, `a && b`, etc.
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Unary operation: `-x`, `!x`
    UnaryOp { op: UnaryOp, operand: Box<Expr> },
    /// Function call: `sqrt(x)`, `atan2(y, x)`, `eye<3>()`
    FnCall {
        name: Spanned<FnName>,
        type_args: Vec<GenericArg>,
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
    /// Local variable reference (loop variable, function parameter, match binding, etc.)
    LocalRef(Ident),
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
    /// Table literal: `table[Phase, 3] { ... }`
    /// Semantically equivalent to `MapLiteral` but preserves tabular structure for formatting.
    /// Indexes can be named (`Phase`) or Nat range literals (`3`).
    TableLiteral {
        indexes: Vec<TableIndexSpec>,
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
    /// Module-qualified built-in constant reference: `module::CONST_NAME`
    QualifiedConstRef {
        module: Ident,
        name: Spanned<DeclName>,
    },
    /// Unresolved bare identifier reference.
    ///
    /// Produced by the parser when the meaning of a bare identifier cannot be
    /// determined from syntax alone. A name-resolution pass rewrites this to
    /// one of `ConstRef`, `LocalRef`, or `StructConstruction` (bare variant).
    NameRef(Ident),
    /// Unresolved qualified reference: `a::b`
    ///
    /// Produced by the parser when `ident::ident` appears without a following
    /// `(` (which would make it a qualified function call). A name-resolution
    /// pass rewrites this to `VariantLiteral` or `QualifiedConstRef`.
    QualifiedNameRef { qualifier: Ident, member: Ident },
}

/// An index specification in a table literal's bracket list: `table[Phase, 3]`
///
/// Named indexes reference declared index types, while Nat range literals
/// desugar to `range(N)` with synthetic variants `#0`, `#1`, etc.
#[derive(Debug, Clone)]
pub enum TableIndexSpec {
    /// A named index: `Phase`, `Maneuver`
    Named(Spanned<IndexName>),
    /// A Nat range literal: `3` (desugars to `range(3)`)
    NatRange(u64, Span),
}

impl TableIndexSpec {
    /// Get the source span of this table index specification.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Named(spanned) => spanned.span,
            Self::NatRange(_, span) => *span,
        }
    }

    /// Returns `true` if this is a Nat range index.
    #[must_use]
    pub const fn is_nat_range(&self) -> bool {
        matches!(self, Self::NatRange(..))
    }
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

/// A binding in a `for` comprehension: `m: Maneuver` or `i: range(3)`
#[derive(Debug, Clone)]
pub struct ForBinding {
    pub var: Ident,
    pub index: ForBindingIndex,
}

/// The index in a for binding: either a named index or a `range(...)` expression.
#[derive(Debug, Clone)]
pub enum ForBindingIndex {
    /// A named index: `for m: Maneuver { ... }`
    Named(Spanned<IndexName>),
    /// A range expression: `for i: range(3) { ... }` or `for i: range(N) { ... }`
    Range {
        /// The argument to `range(...)` — a nat literal or generic nat param.
        arg: NatExpr,
        /// Span of the entire `range(...)` expression.
        span: Span,
    },
}

/// A Nat expression (type-level natural number).
///
/// Supports literals, variables, addition (Level 1), and multiplication (Level 2).
#[derive(Debug, Clone)]
pub enum NatExpr {
    /// An integer literal, e.g., `3`
    Literal(u64, Span),
    /// A variable (generic Nat parameter), e.g., `N`
    Var(Ident),
    /// Addition of two nat expressions, e.g., `N + 1`, `M + N`
    Add(Box<Self>, Box<Self>, Span),
    /// Multiplication of two nat expressions, e.g., `N * 3`, `M * N`
    Mul(Box<Self>, Box<Self>, Span),
}

impl NatExpr {
    /// Get the source span.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Literal(_, span) | Self::Add(_, _, span) | Self::Mul(_, _, span) => *span,
            Self::Var(ident) => ident.span,
        }
    }
}

impl std::fmt::Display for NatExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Literal(n, _) => write!(f, "{n}"),
            Self::Var(ident) => f.write_str(&ident.name),
            Self::Add(lhs, rhs, _) => write!(f, "{lhs} + {rhs}"),
            Self::Mul(lhs, rhs, _) => write!(f, "{lhs} * {rhs}"),
        }
    }
}

/// A generic argument at a call site (turbofish syntax).
///
/// `eye<3>()` has one `GenericArg::Nat(NatExpr::Literal(3, ..))`.
/// `some_fn<Length>()` has one `GenericArg::Type(TypeExpr { kind: DimExpr(..), .. })`.
#[derive(Debug, Clone)]
pub enum GenericArg {
    /// A type expression (for Dim or Index generic params): `Length`, `Maneuver`
    Type(TypeExpr),
    /// A nat expression (for Nat generic params): `3`, `N + 1`
    Nat(NatExpr),
}

impl GenericArg {
    /// Get the source span of this generic argument.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Type(te) => te.span,
            Self::Nat(ne) => ne.span(),
        }
    }
}

/// An argument in an index access: a qualified variant, a loop variable, or an expression.
#[derive(Debug, Clone)]
pub enum IndexArg {
    /// Qualified variant: `Maneuver::Departure`
    Variant {
        index: Spanned<IndexName>,
        variant: Spanned<VariantName>,
    },
    /// Loop variable: `m`
    Var(Ident),
    /// Arbitrary expression: `i + 1`, `i - M`
    Expr(Box<Expr>),
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
            DeclKind::ConstNode(c) => desugar_expr(&mut c.value),
            DeclKind::Unit(u) => {
                if let Some(def) = &mut u.definition {
                    desugar_expr(&mut def.scale_expr);
                }
            }
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
                for encoding in &mut p.encodings {
                    desugar_expr(&mut encoding.value);
                }
                for prop in &mut p.mark.properties {
                    desugar_expr(&mut prop.value);
                }
                for prop in &mut p.properties {
                    desugar_expr(&mut prop.value);
                }
            }
            DeclKind::Figure(f) => {
                for field in &mut f.fields {
                    desugar_expr(&mut field.value);
                }
            }
            DeclKind::Layer(l) => {
                for field in &mut l.fields {
                    desugar_expr(&mut field.value);
                }
            }
            DeclKind::Dag(d) => {
                // Recursively desugar declarations inside the dag block
                let mut inner_file = File {
                    declarations: std::mem::take(&mut d.body),
                };
                desugar_tuple_matches(&mut inner_file);
                d.body = inner_file.declarations;
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Index(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_) => {}
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
        | ExprKind::NameRef(_)
        | ExprKind::QualifiedNameRef { .. }
        // TupleMatch is handled below after recursing into children.
        | ExprKind::TupleMatch { .. } => {}
        ExprKind::BinOp { lhs, rhs, .. } => {
            desugar_expr(lhs);
            desugar_expr(rhs);
        }
        ExprKind::UnaryOp { operand, .. } => desugar_expr(operand),
        ExprKind::FnCall { args, .. } => {
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
                visibility: Visibility::Private,
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
}
