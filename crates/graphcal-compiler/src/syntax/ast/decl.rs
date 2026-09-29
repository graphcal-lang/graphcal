use graphcal_ast_derive::PhaseLift;

use crate::syntax::ast::common::{
    Attribute, BindableVisibility, ImportItem, ImportKind, ModulePath, Visibility,
};
use crate::syntax::ast::multi_decl::MultiDecl;
use crate::syntax::ast::value::{
    DimExpr, Expr, GenericArg, IdentPath, NatExpr, ParamBinding, TypeExpr, UnitExpr,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitName};
use crate::syntax::format_equivalent::FormatEquivalent;
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::module_name::IncludeInstanceId;
use crate::syntax::module_name::{ModuleAliasName, ScopeSegment, ScopedName};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::{Desugared, Phase, Raw};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName, StructTypeName};

use super::plot_props::PlotPropertyName;

// ---------------------------------------------------------------------------
// Raw-only sugar variants
// ---------------------------------------------------------------------------

/// Declaration-level sugar — only legal in [`Raw`].
///
/// Each variant corresponds to a surface declaration form that is rewritten
/// into ordinary `DeclKind` variants by [`crate::desugar`]. After desugaring,
/// `DeclKind::Sugar(_)` carries [`core::convert::Infallible`] and these variants vanish from
/// the type system entirely.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum RawDeclSugar {
    /// Multi-declaration (issue #481): N parallel slots sharing one
    /// `table[…] {…}` initializer. Desugared into N separate
    /// `DeclKind::{Param, Node, ConstNode}` declarations.
    ///
    /// Multi-decl is by definition a raw-only construct — the desugar pass
    /// eliminates it — so [`MultiDecl`] carries no phase parameter.
    Multi(MultiDecl),
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
#[derive(Debug, Clone, FormatEquivalent)]
#[fe(phase = Raw)]
pub struct File<P: Phase = Raw> {
    pub declarations: Vec<Declaration<P>>,
}

impl<P: Phase> File<P> {
    /// Every `import plugin` block in this file, including those inside
    /// (arbitrarily nested) `dag` bodies, in source order.
    ///
    /// This is the single authority for file-wide plugin scans (plugin file
    /// loading, lockfile pinning, snapshot capture, call policy): a plugin
    /// imported by a nested DAG is as much a dependency of the file as a
    /// top-level one.
    #[must_use]
    pub fn plugin_imports(&self) -> Vec<&PluginImportDecl<P>> {
        fn collect<'a, P: Phase>(
            declarations: &'a [Declaration<P>],
            out: &mut Vec<&'a PluginImportDecl<P>>,
        ) {
            for declaration in declarations {
                match &declaration.kind {
                    DeclKind::PluginImport(plugin) => out.push(plugin),
                    DeclKind::Dag(dag) => collect(&dag.body, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        collect(&self.declarations, &mut out);
        out
    }

    /// Whether any declaration (including inside nested `dag` bodies) is a
    /// plugin import. Environments without a plugin host (the browser wasm
    /// engine) use this to reject plugin projects loudly and early.
    #[must_use]
    pub fn uses_plugins(&self) -> bool {
        !self.plugin_imports().is_empty()
    }
}
/// A top-level declaration.
#[derive(Debug, Clone, FormatEquivalent)]
#[fe(phase = Raw)]
pub struct Declaration<P: Phase = Raw> {
    pub attributes: Vec<Attribute>,
    pub kind: DeclKind<P>,
    #[fe(skip)]
    pub span: Span,
    /// The `///` doc block immediately preceding this declaration, when one
    /// exists. Attached by the parser (see `syntax::doc_attach`); purely
    /// documentary, so it never affects semantics or format equivalence: the
    /// doc is re-derived from comments on every parse, and the formatter
    /// preserves comments separately.
    #[fe(skip)]
    pub doc: Option<crate::syntax::comments::DocComment>,
}

/// The kind of a declaration.
///
/// The `Raw` → `Desugared` lift is a derived `TryFrom`: every ordinary
/// variant converts structurally, and declaration sugar comes back as the
/// error for the desugar pass to expand (one multi-decl becomes many
/// declarations, so it has no single-value lowering).
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
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
    PluginImport(PluginImportDecl<P>),
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
    /// `crate::syntax::phase::never`.
    #[phase_lift(residual = RawDeclSugar)]
    Sugar(P::DeclSugar),
}

/// Assert declaration: `assert name = <expr>;`
///
/// The body must evaluate to `Bool`. No type annotation (it's always Bool).
/// Assert declarations are leaf nodes — they are evaluated after the entire graph.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct AssertDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub body: AssertBody<P>,
}

/// The body of an assert declaration.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub enum AssertBody<P: Phase = Raw> {
    /// Plain boolean expression: `assert name = expr;`
    Expr(Expr<P>),
    /// Tolerance: `assert name = actual ~= expected +/- tolerance;`
    Tolerance {
        /// The actual value expression (left of `~=`).
        actual: Box<Expr<P>>,
        /// The expected value expression (right of `~=`).
        expected: Box<Expr<P>>,
        /// The absolute tolerance expression (right of `+/-`).
        tolerance: Box<Expr<P>>,
    },
}

/// The mark type in a plot declaration (Vega-Lite grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, FormatEquivalent)]
pub enum MarkType {
    Point,
    Line,
    Bar,
    Area,
    Rect,
    Tick,
}

impl MarkType {
    /// Every mark type, in the order the grammar lists them.
    pub const ALL: [Self; 6] = [
        Self::Point,
        Self::Line,
        Self::Bar,
        Self::Area,
        Self::Rect,
        Self::Tick,
    ];

    /// Source spelling of the mark type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Line => "line",
            Self::Bar => "bar",
            Self::Area => "area",
            Self::Rect => "rect",
            Self::Tick => "tick",
        }
    }

    /// The mark type spelled `spelling`, if any.
    #[must_use]
    pub fn parse(spelling: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mark| mark.as_str() == spelling)
    }
}

impl std::fmt::Display for MarkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An encoding channel in a plot declaration (Vega-Lite grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FormatEquivalent)]
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

impl EncodingChannel {
    /// Every encoding channel, in the order the grammar lists them.
    pub const ALL: [Self; 9] = [
        Self::X,
        Self::Y,
        Self::Color,
        Self::Size,
        Self::Shape,
        Self::Opacity,
        Self::Detail,
        Self::Text,
        Self::Tooltip,
    ];

    /// Source spelling of the channel.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Color => "color",
            Self::Size => "size",
            Self::Shape => "shape",
            Self::Opacity => "opacity",
            Self::Detail => "detail",
            Self::Text => "text",
            Self::Tooltip => "tooltip",
        }
    }

    /// The channel spelled `spelling`, if any.
    #[must_use]
    pub fn parse(spelling: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.as_str() == spelling)
    }
}

impl std::fmt::Display for EncodingChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The mark specification in a plot declaration: `mark: point` or `mark: line { stroke_width: 2.0 }`.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct MarkSpec<P: Phase = Raw> {
    pub mark_type: MarkType,
    #[fe(skip)]
    pub(crate) mark_type_span: Span,
    pub properties: Vec<PlotField<P>>,
    #[fe(skip)]
    pub(crate) span: Span,
}

/// An encoding channel mapping in a plot declaration.
///
/// Example: `x: for m: OpMode { @total_power[m] }`
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct Encoding<P: Phase = Raw> {
    pub channel: EncodingChannel,
    #[fe(skip)]
    pub(crate) channel_span: Span,
    pub value: Expr<P>,
    #[fe(skip)]
    pub(crate) span: Span,
}

/// A named field in a plot or figure declaration body.
///
/// Example: `title: "My Chart"`
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct PlotField<P: Phase = Raw> {
    /// The field name (e.g., "title", "width", "height").
    pub name: Spanned<PlotPropertyName>,
    /// The field value expression.
    pub value: Expr<P>,
    #[fe(skip)]
    pub(crate) span: Span,
}

/// Plot declaration: `plot name = { mark: point, encode: { x: ..., y: ... }, title: "..." };`
///
/// Plots are leaf declarations that depend on params/nodes via `@`-references.
/// They produce a plot specification, not a runtime `Value`.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
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
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
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
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
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
/// `import nasa.rocket::{type Orbit, compute_thrust};` — brings only the listed names.
///
/// No param bindings — for DAG instantiation with param bindings, use `include`.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum ImportDecl {
    /// Whole-DAG import, `[pub] import path [as alias];`. Only this form
    /// accepts a leading `pub`.
    Module {
        visibility: Visibility,
        path: ModulePath,
        alias: Option<Spanned<ModuleAliasName>>,
    },
    /// Selective import, `import path::{items};`. Re-exports are marked per
    /// item, so the declaration itself has no visibility.
    Selective {
        path: ModulePath,
        items: Vec<ImportItem>,
    },
}

impl ImportDecl {
    /// The imported module path.
    #[must_use]
    pub const fn path(&self) -> &ModulePath {
        match self {
            Self::Module { path, .. } | Self::Selective { path, .. } => path,
        }
    }
}

/// Extern plugin import (issue #943, Phase A of #25):
///
/// ```text
/// import plugin "plugins/coolprop.wasm" as fluids {
///     fn density(p: Pressure, t: Temperature) -> Density;
///     fn smooth<D: Dim>(x: D, window: Dimensionless) -> D;
/// }
/// ```
///
/// Declares externally-provided quantity functions with explicit graphcal
/// signatures. The alias is mandatory: extern functions are only callable
/// qualified (`fluids::density(...)`), mirroring module-import explicitness.
/// The path string carries no filesystem semantics in Phase A; it identifies
/// the plugin in the embedder's host function registry.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct PluginImportDecl<P: Phase = Raw> {
    /// The verbatim plugin path string.
    pub path: Spanned<crate::syntax::plugin::PluginPath>,
    /// The mandatory module alias extern calls are qualified with.
    pub alias: Spanned<crate::syntax::module_name::ModuleAliasName>,
    /// The declared extern function signatures, in source order.
    pub functions: Vec<ExternFnDecl<P>>,
}

/// One extern function signature inside an `import plugin` block:
/// `fn smooth<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I];`
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct ExternFnDecl<P: Phase = Raw> {
    /// The function name.
    pub name: Spanned<crate::syntax::function_name::FnName>,
    /// Explicit generic binders (`<D: Dim, I: Index>`) in source order,
    /// possibly empty.
    pub generics: Vec<ExternGenericBinder>,
    /// The named parameters, in order.
    pub params: Vec<ExternFnParam<P>>,
    /// The result type annotation.
    pub result: TypeExpr<P>,
    /// Span of the whole `fn ...;` declaration.
    #[fe(skip)]
    pub span: Span,
}

/// One generic binder in an extern signature, categorized by its constraint.
///
/// Extern binders reuse the `name: constraint` grammar of `type` generics but
/// support only the `Dim` and `Index` constraints; the category is carried
/// typewise from the parser on.
#[derive(Debug, Clone, FormatEquivalent)]
pub enum ExternGenericBinder {
    /// `D: Dim` — a dimension variable.
    Dim(Spanned<crate::syntax::dimension::DimVarName>),
    /// `I: Index` — an index variable.
    Index(Spanned<crate::syntax::index_name::IndexVarName>),
}

impl ExternGenericBinder {
    /// The binder's source span.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Dim(var) => var.span,
            Self::Index(var) => var.span,
        }
    }

    /// The binder's name as source text.
    #[must_use]
    pub fn name_str(&self) -> &str {
        match self {
            Self::Dim(var) => var.value.as_str(),
            Self::Index(var) => var.value.as_str(),
        }
    }
}

/// One named parameter in an extern function signature: `p: Pressure`.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct ExternFnParam<P: Phase = Raw> {
    /// The parameter name.
    pub name: Spanned<crate::syntax::function_name::FnParamName>,
    /// The parameter type annotation.
    pub type_ann: TypeExpr<P>,
}

/// Include declaration (DAG embedding / instantiation).
///
/// `include nasa.rocket.compute_thrust(args);` — bare form; instance alias is
/// the DAG's leaf name.
/// `include nasa.rocket.compute_thrust(args) as ct;` — explicit instance alias.
/// `include nasa.rocket.compute_thrust(args)::{thrust};` — exposes selected
/// outputs as nodes in the including DAG.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct IncludeDecl<P: Phase = Raw> {
    pub path: ModulePath,
    pub param_bindings: Vec<ParamBinding<P>>,
    pub kind: ImportKind,
}

impl<P: Phase> IncludeDecl<P> {
    /// Return the namespace of this configured DAG instance.
    ///
    /// Only module-form includes introduce a source-visible alias. Selective
    /// includes receive an opaque occurrence identity for internal lowering.
    #[must_use]
    pub fn instance_scope(&self) -> ScopeSegment {
        match &self.kind {
            ImportKind::Module { alias } => {
                ScopeSegment::Named(self.module_form_alias(alias.as_ref()))
            }
            ImportKind::Selective(_) => ScopeSegment::IncludeInstance(
                IncludeInstanceId::at_source_offset(self.path.span().offset()),
            ),
        }
    }

    /// The instance alias of a module-form include whose explicit `as` alias
    /// (if any) is `alias`: the explicit alias, else the DAG's leaf name.
    #[must_use]
    pub fn module_form_alias(&self, alias: Option<&Spanned<ModuleAliasName>>) -> ModuleAliasName {
        alias.map_or_else(
            || ModuleAliasName::classify(self.path.leaf().name.atom().clone()),
            |alias| alias.value.clone(),
        )
    }
}

/// Inline DAG declaration: `dag name { ... }`
///
/// The body contains declarations (same as file-level). Semantics are not yet
/// implemented — this phase only parses the syntax.
#[derive(Debug, Clone, FormatEquivalent)]
#[fe(phase = Raw)]
pub struct DagDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    /// The DAG name.
    pub name: Spanned<DeclName>,
    /// Declarations inside the DAG block.
    pub body: Vec<Declaration<P>>,
    /// Span covering the entire `dag name { ... }` block.
    #[fe(skip)]
    pub span: Span,
}
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct ParamDecl<P: Phase = Raw> {
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    /// The default value expression. `None` for required params (no default).
    pub value: Option<Expr<P>>,
}

/// Shared shape for value declarations with an expression body.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct ValueDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    pub value: Expr<P>,
}

/// Runtime node declaration, with either a formula or an unfinished body.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct NodeDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    #[phase_lift(map = map_formula)]
    pub definition: crate::node_definition::NodeDefinition<Expr<P>, IdentPath>,
}

/// Const node declaration: `const node name: Type = expr;`
pub type ConstNodeDecl<P = Raw> = ValueDecl<P>;

/// Base dimension declaration: `base dim Length;`
#[derive(Debug, Clone, FormatEquivalent)]
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
#[derive(Debug, Clone, FormatEquivalent)]
pub struct DimDecl {
    pub visibility: BindableVisibility,
    pub name: Spanned<DimName>,
    pub definition: Option<DimExpr>,
}

/// Whether a unit is allowed in compile-time (`const`) contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FormatEquivalent)]
pub enum UnitConstness {
    /// A compile-time unit: prelude units, `base unit`, or `const unit`.
    Const,
    /// A runtime unit declared with plain `unit`; its scale may depend on params or nodes.
    Dynamic,
}

impl UnitConstness {
    /// Returns `true` for units that may appear in `const node` bodies.
    #[must_use]
    pub const fn is_const(self) -> bool {
        matches!(self, Self::Const)
    }
}

/// Unit declaration: `const unit km: Length = 1000 m;`, `unit EUR: Money = (@rate) USD;`,
/// or `base unit bit: Information;`.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct UnitDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub constness: UnitConstness,
    pub name: Spanned<UnitName>,
    /// The dimension this unit measures.
    pub dim_type: DimExpr,
    /// Scale definition: `(scale_value, base_unit_expr)`.
    /// `None` iff this is a base unit (`base unit bit: Information;`).
    pub definition: Option<UnitDef<P>>,
}

/// The scale definition part of a unit declaration: `1000 m` or `1 kg * m / s^2`.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct UnitDef<P: Phase = Raw> {
    pub scale_expr: Expr<P>,
    pub unit_expr: UnitExpr,
    #[fe(skip)]
    pub(crate) span: Span,
}

/// Type declaration: required type stubs and tagged-union bodies.
///
/// Forms:
/// - Required type: `type T;` — the library requires a type bound from
///   outside; no body at declaration.
/// - Tagged union: `type Maneuver { Impulsive(delta_v: Velocity), Coast }`
/// - Record-shaped type: `type Position { Position(x: Length, y: Length) }`,
///   a single-variant union whose constructor name matches the type name.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct TypeDecl<P: Phase = Raw> {
    pub visibility: BindableVisibility,
    pub name: Spanned<StructTypeName>,
    pub generic_params: Vec<GenericParam<P>>,
    pub body: TypeDeclBody<P>,
}

/// Body of a `type` declaration.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
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
/// - Record payload: `Impulsive(delta_v: Velocity)` — `payload` is
///   `Some(vec![…])`. Constructor payloads use parentheses exclusively.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct UnionMember<P: Phase = Raw> {
    /// The constructor's name. Lives in the constructor namespace —
    /// distinct from the type namespace.
    pub name: Spanned<ConstructorName>,
    /// Inline payload fields, or `None` for unit constructors.
    pub payload: Option<Vec<FieldDecl<P>>>,
    #[fe(skip)]
    pub span: Span,
}

/// A field in a variant or struct type declaration.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct FieldDecl<P: Phase = Raw> {
    pub name: Spanned<FieldName>,
    pub type_ann: TypeExpr<P>,
}

/// The kind of an index declaration.
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub enum IndexDeclKind<P: Phase = Raw> {
    /// Named variants: `{ Departure, Correction, Insertion }`
    Named {
        variants: NonEmpty<Spanned<IndexVariantName>>,
    },
    /// Coordinate range with an exact increment and endpoint:
    /// `range(start, end, step: delta)`.
    Range {
        start: Box<Expr<P>>,
        end: Box<Expr<P>>,
        step: Box<Expr<P>>,
    },
    /// Linearly spaced coordinate index with an exact point count and endpoints:
    /// `linspace(start, end, points: count)`.
    Linspace {
        start: Box<Expr<P>>,
        end: Box<Expr<P>>,
        points: NatExpr,
    },
    /// Required named index (no variants): `index Foo;`
    ///
    /// Must be bound via parameterized include.
    RequiredNamed,
    /// Required coordinate index with a dimension constraint: `index Foo: Time;`
    ///
    /// Must be bound via parameterized include.
    RequiredCoordinate { dimension: DimExpr },
}

impl<P: Phase> IndexDeclKind<P> {
    /// Returns `true` for required index declarations that must be bound via include.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::RequiredNamed | Self::RequiredCoordinate { .. })
    }
}

/// Index declaration: `index Maneuver = { Departure, Correction, Insertion };`
/// or `index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);`
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct IndexDecl<P: Phase = Raw> {
    pub visibility: BindableVisibility,
    pub name: Spanned<IndexName>,
    pub kind: IndexDeclKind<P>,
}

/// A generic parameter: `D: Dim`
#[derive(Debug, Clone, PhaseLift, FormatEquivalent)]
#[phase_lift(from = Raw, to = Desugared)]
#[fe(phase = Raw)]
pub struct GenericParam<P: Phase = Raw> {
    pub name: Spanned<GenericParamName>,
    pub constraint: GenericConstraint,
    /// Optional sort-aware default, e.g. `F: Type = Unframed` or `N: Nat = 3`.
    /// The syntax remains unresolved until it is checked against `constraint`.
    pub default: Option<GenericArg<P>>,
}

/// Constraint on a generic parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FormatEquivalent)]
pub enum GenericConstraint {
    /// `D: Dim` -- the generic stands for a dimension.
    Dim,
    /// `I: Index` -- the generic stands for an index.
    Index,
    /// `N: Nat` -- the generic stands for a natural number (type-level).
    Nat,
    /// `F: Type` -- the generic stands for a value type.
    Type,
}

impl GenericConstraint {
    /// Every generic constraint, in the order the grammar lists them.
    pub const ALL: [Self; 4] = [Self::Dim, Self::Index, Self::Nat, Self::Type];

    /// Source spelling of the constraint.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dim => "Dim",
            Self::Index => "Index",
            Self::Nat => "Nat",
            Self::Type => "Type",
        }
    }

    /// The constraint spelled `spelling`, if any.
    #[must_use]
    pub fn parse(spelling: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|constraint| constraint.as_str() == spelling)
    }
}

impl std::fmt::Display for GenericConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
