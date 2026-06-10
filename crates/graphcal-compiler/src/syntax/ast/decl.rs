use crate::syntax::ast::common::{
    Attribute, BindableVisibility, ImportKind, ModulePath, Visibility,
};
use crate::syntax::ast::value::{
    DimExpr, Expr, MapEntryKey, MultiDeclSharedAxes, ParamBinding, TypeExpr, UnitExpr,
};
use crate::syntax::names::{
    ConstructorName, DeclName, DimName, FieldName, GenericParamName, IndexName, IndexVariantName,
    PlotPropertyName, ScopedName, StructTypeName, UnitName,
};
use crate::syntax::phase::{Phase, Raw};
use crate::syntax::span::{Span, Spanned};

// ---------------------------------------------------------------------------
// Raw-only sugar variants
// ---------------------------------------------------------------------------

/// Declaration-level sugar — only legal in [`Raw`].
///
/// Each variant corresponds to a surface declaration form that is rewritten
/// into ordinary `DeclKind` variants by [`crate::desugar`]. After desugaring,
/// `DeclKind::Sugar(_)` carries [`core::convert::Infallible`] and these variants vanish from
/// the type system entirely.
#[derive(Debug, Clone)]
pub enum RawDeclSugar {
    /// Multi-declaration (issue #481): N parallel slots sharing one
    /// `table[…] {…}` initializer. Desugared into N separate
    /// `DeclKind::{Param, Node, ConstNode}` declarations.
    ///
    /// Pinned to `MultiDecl<Raw>` because multi-decl is by definition a
    /// raw-only construct — the desugar pass eliminates it.
    Multi(MultiDecl<Raw>),
}

impl RawDeclSugar {
    /// Returns the surface span of the sugar form.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Multi(m) => m.span,
        }
    }
}
/// A complete source file.
///
/// Generic over a [`Phase`] parameter that distinguishes the parser's raw
/// AST (carrying surface sugar) from the desugared AST consumed by name
/// resolution and below. Defaults to [`Raw`] so existing call sites — which
/// always handle the parser output — keep compiling unchanged.
#[derive(Debug, Clone)]
pub struct File<P: Phase = Raw> {
    pub declarations: Vec<Declaration<P>>,
}
/// A top-level declaration.
#[derive(Debug, Clone)]
pub struct Declaration<P: Phase = Raw> {
    pub attributes: Vec<Attribute>,
    pub kind: DeclKind<P>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum DeclKind<P: Phase = Raw> {
    Param(ParamDecl<P>),
    Node(NodeDecl<P>),
    ConstNode(ConstNodeDecl<P>),
    BaseDimension(BaseDimDecl),
    Dimension(DimDecl),
    Unit(UnitDecl<P>),
    Type(TypeDecl<P>),
    Index(IndexDecl<P>),
    Import(ImportDecl),
    Include(IncludeDecl<P>),
    Dag(DagDecl<P>),
    Assert(AssertDecl<P>),
    Plot(PlotDecl<P>),
    Figure(FigureDecl<P>),
    Layer(LayerDecl<P>),
    /// Phase-specific declaration sugar.
    ///
    /// In [`Raw`], this is [`crate::syntax::ast::RawDeclSugar`] and carries
    /// surface forms like multi-decl (issue #481) that are eliminated by the
    /// desugar pass. In [`Desugared`](Raw), the
    /// payload is [`core::convert::Infallible`] — the variant is statically
    /// unreachable, so post-desugar consumers handle it with
    /// [`crate::syntax::phase::never`].
    Sugar(P::DeclSugar),
}

impl<P: Phase> DeclKind<P> {
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
            Self::Index(i) => Some((i.name.value.as_str(), i.name.span)),
            Self::Dag(d) => Some((d.name.value.as_str(), d.name.span)),
            Self::Assert(a) => Some((a.name.value.as_str(), a.name.span)),
            Self::Plot(p) => Some((p.name.value.as_str(), p.name.span)),
            Self::Figure(f) => Some((f.name.value.as_str(), f.name.span)),
            Self::Layer(l) => Some((l.name.value.as_str(), l.name.span)),
            Self::Import(_) | Self::Include(_) | Self::Sugar(_) => None,
        }
    }
}

/// Assert declaration: `assert name = <expr>;`
///
/// The body must evaluate to `Bool`. No type annotation (it's always Bool).
/// Assert declarations are leaf nodes — they are evaluated after the entire graph.
#[derive(Debug, Clone)]
pub struct AssertDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub body: AssertBody<P>,
}

/// The body of an assert declaration.
#[derive(Debug, Clone)]
pub enum AssertBody<P: Phase = Raw> {
    /// Plain boolean expression: `assert name = expr;`
    Expr(Expr<P>),
    /// Tolerance: `assert name = actual ~= expected +/- tolerance;`
    Tolerance {
        /// The actual value expression (left of `~=`).
        actual: Box<Expr<P>>,
        /// The expected value expression (right of `~=`).
        expected: Box<Expr<P>>,
        /// The tolerance expression (right of `+/-`).
        tolerance: Box<Expr<P>>,
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
pub struct MarkSpec<P: Phase = Raw> {
    pub mark_type: MarkType,
    pub mark_type_span: Span,
    pub properties: Vec<PlotField<P>>,
    pub span: Span,
}

/// An encoding channel mapping in a plot declaration.
///
/// Example: `x: for m: OpMode { @total_power[m] }`
#[derive(Debug, Clone)]
pub struct Encoding<P: Phase = Raw> {
    pub channel: EncodingChannel,
    pub channel_span: Span,
    pub value: Expr<P>,
    pub span: Span,
}

/// A named field in a plot or figure declaration body.
///
/// Example: `title: "My Chart"`
#[derive(Debug, Clone)]
pub struct PlotField<P: Phase = Raw> {
    /// The field name (e.g., "title", "width", "height").
    pub name: Spanned<PlotPropertyName>,
    /// The field value expression.
    pub value: Expr<P>,
    pub span: Span,
}

/// Plot declaration: `plot name = { mark: point, encode: { x: ..., y: ... }, title: "..." };`
///
/// Plots are leaf declarations that depend on params/nodes via `@`-references.
/// They produce a plot specification, not a runtime `Value`.
#[derive(Debug, Clone)]
pub struct PlotDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub mark: MarkSpec<P>,
    pub encodings: Vec<Encoding<P>>,
    pub properties: Vec<PlotField<P>>,
}

/// Figure declaration: `figure name = { plots: [a, b], title: "..." };`
///
/// Figures group multiple plot declarations into a single combined chart
/// with subplots. Like plots, they are leaf declarations.
#[derive(Debug, Clone)]
pub struct FigureDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    /// The plot names referenced by this figure (from the `plots: [...]` field).
    pub plot_names: Vec<Spanned<ScopedName>>,
    /// Additional fields (e.g., `title`).
    pub fields: Vec<PlotField<P>>,
}

/// A layer declaration: overlays multiple plots on shared axes.
///
/// Syntax: `layer name = { plots: [a, b], title: "..." };`
///
/// Unlike `figure` (which tiles plots side-by-side), `layer` overlays
/// them on the same coordinate space. In Vega-Lite this maps to the
/// `"layer"` composition operator.
#[derive(Debug, Clone)]
pub struct LayerDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    /// The plot names to overlay (from the `plots: [...]` field).
    pub plot_names: Vec<Spanned<ScopedName>>,
    /// Additional fields (e.g., `title`).
    pub fields: Vec<PlotField<P>>,
}

/// Import declaration (compile-time name import).
///
/// `import nasa.rocket;` — brings the leaf module into scope.
/// `import nasa.rocket as nr;` — brings the leaf module under an alias.
/// `import nasa.rocket.{Orbit, compute_thrust};` — brings only the listed names.
///
/// No param bindings — for DAG instantiation with param bindings, use `include`.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub visibility: Visibility,
    pub path: ModulePath,
    pub kind: ImportKind,
}

/// Include declaration (DAG embedding / instantiation).
///
/// `include nasa.rocket.compute_thrust(args);` — bare form; instance alias is
/// the DAG's leaf name.
/// `include nasa.rocket.compute_thrust(args) as ct;` — explicit instance alias.
/// `include nasa.rocket.compute_thrust(args).{thrust};` — exposes selected
/// outputs as nodes in the including DAG.
#[derive(Debug, Clone)]
pub struct IncludeDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub path: ModulePath,
    pub param_bindings: Vec<ParamBinding<P>>,
    pub kind: ImportKind,
}

/// Inline DAG declaration: `dag name { ... }`
///
/// The body contains declarations (same as file-level). Semantics are not yet
/// implemented — this phase only parses the syntax.
#[derive(Debug, Clone)]
pub struct DagDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    /// The DAG name.
    pub name: Spanned<DeclName>,
    /// Declarations inside the DAG block.
    pub body: Vec<Declaration<P>>,
    /// Span covering the entire `dag name { ... }` block.
    pub span: Span,
}
#[derive(Debug, Clone)]
pub struct ParamDecl<P: Phase = Raw> {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    /// The default value expression. `None` for required params (no default).
    pub value: Option<Expr<P>>,
}

// ---------------------------------------------------------------------------
// Multi-declaration surface info (issue #481)
// ---------------------------------------------------------------------------
//
// A multi-decl is a single surface form — e.g.,
//
//     param a: T[I], const node b: U[I, J] = table[I, (_, J)] { : _, …; … };
//
// — represented in the AST as `DeclKind::Multi(MultiDecl)`. A dedicated
// desugar pass (`syntax::desugar::desugar_multi_decls_in_file`) expands
// each `Multi` into N parallel ordinary declarations before lowering;
// consumers that want the surface form (formatter, surface-aware LSP
// features) read the AST variant directly.

/// The surface form of a multi-decl: parallel declaration slots sharing a
/// single `table[…] {…}` initializer.
#[derive(Debug, Clone)]
pub struct MultiDecl<P: Phase = Raw> {
    /// Slot headers in declaration order. Length = number of declarations
    /// this multi-decl expanded into.
    pub slots: Vec<MultiDeclSlot<P>>,
    /// Shared axes from the bracket prefix `table[A, B, …, (…)]`.
    pub shared_axes: MultiDeclSharedAxes,
    /// Per-slot extra-axis annotation from the slot tuple. Same length
    /// as `slots`.
    pub slot_axes: Vec<MultiSlotAxis>,
    /// Body slices. Exactly one slice for single-shared-axis multi-decls;
    /// multiple slices for N-D shared-axis prefixes (v3).
    pub slices: Vec<MultiDeclSlice<P>>,
    /// Full surface span: from the first slot's kind keyword through the
    /// closing `;`.
    pub span: Span,
    /// Span of the `table[…] {…}` sub-expression.
    pub table_expr_span: Span,
}

/// One slot in a multi-decl: kind keyword, name, type annotation, visibility.
#[derive(Debug, Clone)]
pub struct MultiDeclSlot<P: Phase = Raw> {
    /// Visibility for this slot. The first slot inherits the leading
    /// `pub`/`pub(bind)` prefix consumed before the multi-decl was
    /// recognized; subsequent slots accept their own optional prefix
    /// before the kind keyword.
    pub visibility: Visibility,
    pub kind: MultiSlotKind,
    /// Span covering the kind keyword(s) (`param`, `node`, or `const node`).
    pub kind_span: Span,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    /// Span from kind keyword through end of the type annotation.
    pub header_span: Span,
}

/// Value-decl kinds that a multi-decl slot can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiSlotKind {
    Param,
    Node,
    ConstNode,
}

/// Per-slot entry in the slot tuple `(…)`.
#[derive(Debug, Clone)]
pub enum MultiSlotAxis {
    /// `_` — 1-D slot, typed `T[SharedAxis]`.
    Underscore,
    /// Named axis — 2-D slot, typed `T[SharedAxis, ExtraAxis]`.
    Axis(Spanned<IndexName>),
}

/// Where a slot's columns live within each slice's header row.
#[derive(Debug, Clone)]
pub enum MultiSlotColumnSpan {
    /// 1-D slot: one column at `col_idx`.
    Single(usize),
    /// 2-D slot: columns `start..end`, one per variant of `extra_axis`.
    Range {
        start: usize,
        end: usize,
        extra_axis: Spanned<IndexName>,
    },
}

/// One slice of a multi-decl body: optional slice-label prefix + header + rows.
#[derive(Debug, Clone)]
pub struct MultiDeclSlice<P: Phase = Raw> {
    /// Slice labels covering the shared-axis prefix except the row axis.
    /// Empty for single-shared-axis bodies.
    pub prefix_keys: Vec<MapEntryKey>,
    /// Header row cells, in left-to-right order.
    pub header_cells: Vec<MultiHeaderCell>,
    /// Span of the entire header row (`:` through `;`).
    pub header_span: Span,
    /// Per-slot column span into this slice's `header_cells` and `rows`
    /// values. Same length as `MultiDecl::slots`. May differ between
    /// slices if their header rows list variants in different orders.
    pub column_layout: Vec<MultiSlotColumnSpan>,
    /// Data rows for this slice.
    pub rows: Vec<MultiDataRow<P>>,
}

/// One cell of a multi-decl header row.
#[derive(Debug, Clone)]
pub enum MultiHeaderCell {
    Underscore {
        span: Span,
    },
    Variant {
        /// Axis qualifier, if the author wrote `Axis.Variant`.
        axis: Option<Spanned<IndexName>>,
        variant: Spanned<IndexVariantName>,
        span: Span,
    },
}

impl MultiHeaderCell {
    /// Returns the span of this cell.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Underscore { span } | Self::Variant { span, .. } => *span,
        }
    }
}

/// One data row of a multi-decl body: label + value per column.
#[derive(Debug, Clone)]
pub struct MultiDataRow<P: Phase = Raw> {
    pub label: Spanned<IndexVariantName>,
    pub values: Vec<Expr<P>>,
    pub span: Span,
}

/// Runtime node declaration: `node name: Type = expr;`
#[derive(Debug, Clone)]
pub struct NodeDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    pub value: Expr<P>,
}

/// Const node declaration: `const node name: Type = expr;`
#[derive(Debug, Clone)]
pub struct ConstNodeDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    pub value: Expr<P>,
}

/// Base dimension declaration: `base dim Length;`
#[derive(Debug, Clone)]
pub struct BaseDimDecl {
    pub visibility: Visibility,
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
    pub visibility: BindableVisibility,
    pub name: Spanned<DimName>,
    pub definition: Option<DimExpr>,
}

/// Unit declaration: `unit km: Length = 1000 m;` or `base unit m: Length;`.
#[derive(Debug, Clone)]
pub struct UnitDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<UnitName>,
    /// The dimension this unit measures.
    pub dim_type: DimExpr,
    /// Scale definition: `(scale_value, base_unit_expr)`.
    /// `None` iff this is a base unit (`base unit m: Length;`).
    pub definition: Option<UnitDef<P>>,
}

/// The scale definition part of a unit declaration: `1000 m` or `1 kg * m / s^2`.
#[derive(Debug, Clone)]
pub struct UnitDef<P: Phase = Raw> {
    pub scale_expr: Expr<P>,
    pub unit_expr: UnitExpr,
    pub span: Span,
}

/// Type declaration: required type stubs and tagged-union bodies.
///
/// Forms:
/// - Required type: `type T;` — the library requires a type bound from
///   outside; no body at declaration.
/// - Tagged union: `type Maneuver { Impulsive(delta_v: Velocity), Coast }`
/// - Record-shaped type: `type Position { Position(x: Length, y: Length) }`,
///   a single-variant union whose constructor name matches the type name.
#[derive(Debug, Clone)]
pub struct TypeDecl<P: Phase = Raw> {
    pub visibility: BindableVisibility,
    pub name: Spanned<StructTypeName>,
    pub generic_params: Vec<GenericParam<P>>,
    pub body: TypeDeclBody<P>,
}

/// Body of a `type` declaration.
#[derive(Debug, Clone)]
pub enum TypeDeclBody<P: Phase = Raw> {
    /// Required type with no body: `type T;`.
    Required,
    /// Tagged-union constructor list: `type T { Ctor, Other(x: U) }`.
    Constructors(Vec<UnionMember<P>>),
}

/// A member of a type declaration body: a constructor with an optional payload.
///
/// Forms:
/// - Unit: `Coast` — `payload` is `None`.
/// - Record-payload (parens): `Impulsive(delta_v: Velocity)` —
///   `payload` is `Some(vec![…])`.
/// - Record-payload (braces): `LowThrust { thrust: Force, duration: Time }`
///   — `payload` is `Some(vec![…])`. The brace/paren choice is purely
///   surface syntax; both produce the same AST.
#[derive(Debug, Clone)]
pub struct UnionMember<P: Phase = Raw> {
    /// The constructor's name. Lives in the constructor namespace —
    /// distinct from the type namespace.
    pub name: Spanned<ConstructorName>,
    /// Inline payload fields, or `None` for unit constructors.
    pub payload: Option<Vec<FieldDecl<P>>>,
    pub span: Span,
}

/// A field in a variant or struct type declaration.
#[derive(Debug, Clone)]
pub struct FieldDecl<P: Phase = Raw> {
    pub name: Spanned<FieldName>,
    pub type_ann: TypeExpr<P>,
}

///// The kind of an index declaration.
#[derive(Debug, Clone)]
pub enum IndexDeclKind<P: Phase = Raw> {
    /// Named variants: `{ Departure, Correction, Insertion }`
    Named {
        variants: Vec<Spanned<IndexVariantName>>,
    },
    /// Numeric range: `linspace(start, end, step: step)`
    Range {
        start: Box<Expr<P>>,
        end: Box<Expr<P>>,
        step: Box<Expr<P>>,
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

impl<P: Phase> IndexDeclKind<P> {
    /// Returns `true` for required index declarations that must be bound via import.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::RequiredNamed | Self::RequiredRange { .. })
    }
}

/// Index declaration: `index Maneuver = { Departure, Correction, Insertion };`
/// or `index TimeStep = linspace(0.0 s, 100.0 s, step: 0.1 s);`
#[derive(Debug, Clone)]
pub struct IndexDecl<P: Phase = Raw> {
    pub visibility: BindableVisibility,
    pub name: Spanned<IndexName>,
    pub kind: IndexDeclKind<P>,
}

/// A generic parameter: `D: Dim`
#[derive(Debug, Clone)]
pub struct GenericParam<P: Phase = Raw> {
    pub name: Spanned<GenericParamName>,
    pub constraint: GenericConstraint,
    /// Optional default type, e.g. `F: Type = Unframed`.
    pub default: Option<TypeExpr<P>>,
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
