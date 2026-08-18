use crate::syntax::ast::common::{
    Attribute, BindableVisibility, ImportKind, ModulePath, Visibility,
};
use crate::syntax::ast::value::{
    DimExpr, Expr, GenericArg, MapEntryKey, MultiDeclSharedAxes, NatExpr, ParamBinding, TypeExpr,
    UnitExpr,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitName};
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use crate::syntax::module_name::{
    IncludeInstanceId, IncludeInstanceScope, ModuleAliasName, ScopedName,
};
use crate::syntax::names::NamePath;
use crate::syntax::phase::{Phase, Raw};
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
    /// The `///` doc block immediately preceding this declaration, when one
    /// exists. Attached by the parser (see `syntax::doc_attach`); purely
    /// documentary, so it never affects semantics or format equivalence.
    pub doc: Option<crate::syntax::comments::DocComment>,
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
    Sugar(P::DeclSugar),
}

impl<P: Phase> DeclKind<P> {
    /// Returns the declaration name as a string slice and its span, if the
    /// variant carries a name. `Import` and `Include` have no name and return
    /// `None`.
    #[must_use]
    pub(crate) fn name_and_span(&self) -> Option<(&str, Span)> {
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
            Self::Import(_) | Self::PluginImport(_) | Self::Include(_) | Self::Sugar(_) => None,
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
        /// The absolute tolerance expression (right of `+/-`).
        tolerance: Box<Expr<P>>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub(crate) mark_type_span: Span,
    pub properties: Vec<PlotField<P>>,
    pub(crate) span: Span,
}

/// An encoding channel mapping in a plot declaration.
///
/// Example: `x: for m: OpMode { @total_power[m] }`
#[derive(Debug, Clone)]
pub struct Encoding<P: Phase = Raw> {
    pub channel: EncodingChannel,
    pub(crate) channel_span: Span,
    pub value: Expr<P>,
    pub(crate) span: Span,
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
    pub(crate) span: Span,
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
/// `import nasa.rocket.{type Orbit, compute_thrust};` — brings only the listed names.
///
/// No param bindings — for DAG instantiation with param bindings, use `include`.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub path: ModulePath,
    pub kind: ImportKind,
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
/// qualified (`fluids.density(...)`), mirroring module-import explicitness.
/// The path string carries no filesystem semantics in Phase A; it identifies
/// the plugin in the embedder's host function registry.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
    pub span: Span,
}

/// One generic binder in an extern signature, categorized by its constraint.
///
/// Extern binders reuse the `name: constraint` grammar of `type` generics but
/// support only the `Dim` and `Index` constraints; the category is carried
/// typewise from the parser on.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
/// `include nasa.rocket.compute_thrust(args).{thrust};` — exposes selected
/// outputs as nodes in the including DAG.
#[derive(Debug, Clone)]
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
    pub fn instance_scope(&self) -> IncludeInstanceScope {
        match &self.kind {
            ImportKind::Module { alias } => {
                IncludeInstanceScope::Named(alias.as_ref().map_or_else(
                    || ModuleAliasName::from_atom(self.path.leaf().name.clone()),
                    |alias| alias.value.clone(),
                ))
            }
            ImportKind::Selective(_) => {
                IncludeInstanceScope::Anonymous(IncludeInstanceId::at(self.path.span()))
            }
        }
    }
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
//     param a: T[I], const node b: U[I, J] = table[I, (_, J)] { : _, J.X, …; … };
//
// — represented in the AST as `DeclKind::Multi(MultiDecl)`. A dedicated
// desugar pass (`syntax::desugar::desugar_multi_decls_in_file`) expands
// each `Multi` into N parallel ordinary declarations before lowering;
// consumers that want the surface form (formatter, surface-aware LSP
// features) read the AST variant directly.

/// Error returned when correlated multi-declaration collections do not have a
/// shape that can be desugared safely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MultiDeclShapeError {
    #[error("multi-declaration requires at least two slots, but received {slot_count}")]
    TooFewSlots { slot_count: usize },
    #[error("multi-declaration has {slot_count} slots but {slot_axis_count} slot-axis entries")]
    SlotAxisArity {
        slot_count: usize,
        slot_axis_count: usize,
    },
    #[error(
        "multi-declaration slice {slice_index} has {layout_count} column-layout entries but {slot_count} slots"
    )]
    SliceLayoutArity {
        slice_index: usize,
        slot_count: usize,
        layout_count: usize,
    },
    #[error(
        "multi-declaration row {row_index} has {value_count} values but its header has {header_count} columns"
    )]
    RowWidth {
        row_index: usize,
        header_count: usize,
        value_count: usize,
    },
    #[error(
        "multi-declaration column layout {layout_index} selects column {column_index}, but its header has {header_count} columns"
    )]
    ColumnOutOfBounds {
        layout_index: usize,
        column_index: usize,
        header_count: usize,
    },
    #[error(
        "multi-declaration column layout {layout_index} has invalid range {start}..{end} for a header with {header_count} columns"
    )]
    InvalidColumnRange {
        layout_index: usize,
        start: usize,
        end: usize,
        header_count: usize,
    },
}

/// The surface form of a multi-decl: parallel declaration slots sharing a
/// single `table[…] {…}` initializer.
///
/// Its correlated collections are private so a valid parser-produced value
/// cannot later be mutated into a shape that panics during desugaring. Use
/// [`MultiDecl::new`] for construction and the read-only accessors for
/// inspection.
#[derive(Debug, Clone)]
pub struct MultiDecl<P: Phase = Raw> {
    slots: Vec<MultiDeclSlot<P>>,
    shared_axes: MultiDeclSharedAxes,
    slot_axes: Vec<MultiSlotAxis>,
    slices: Vec<MultiDeclSlice<P>>,
    /// Full surface span: from the first slot's kind keyword through the
    /// closing `;`.
    pub(crate) span: Span,
    /// Span of the `table[…] {…}` sub-expression.
    pub(crate) table_expr_span: Span,
}

impl<P: Phase> MultiDecl<P> {
    /// Constructs a multi-declaration after validating all cross-collection
    /// arity invariants used by desugaring.
    pub fn new(
        slots: Vec<MultiDeclSlot<P>>,
        shared_axes: MultiDeclSharedAxes,
        slot_axes: Vec<MultiSlotAxis>,
        slices: Vec<MultiDeclSlice<P>>,
        span: Span,
        table_expr_span: Span,
    ) -> Result<Self, MultiDeclShapeError> {
        let slot_count = slots.len();
        if slot_count < 2 {
            return Err(MultiDeclShapeError::TooFewSlots { slot_count });
        }
        if slot_axes.len() != slot_count {
            return Err(MultiDeclShapeError::SlotAxisArity {
                slot_count,
                slot_axis_count: slot_axes.len(),
            });
        }
        if let Some((slice_index, slice)) = slices
            .iter()
            .enumerate()
            .find(|(_, slice)| slice.column_layout.len() != slot_count)
        {
            return Err(MultiDeclShapeError::SliceLayoutArity {
                slice_index,
                slot_count,
                layout_count: slice.column_layout.len(),
            });
        }
        Ok(Self {
            slots,
            shared_axes,
            slot_axes,
            slices,
            span,
            table_expr_span,
        })
    }

    #[must_use]
    pub fn slots(&self) -> &[MultiDeclSlot<P>] {
        &self.slots
    }

    #[must_use]
    pub const fn shared_axes(&self) -> &MultiDeclSharedAxes {
        &self.shared_axes
    }

    #[must_use]
    pub fn slot_axes(&self) -> &[MultiSlotAxis] {
        &self.slot_axes
    }

    #[must_use]
    pub fn slices(&self) -> &[MultiDeclSlice<P>] {
        &self.slices
    }
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
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    /// Span from kind keyword through end of the type annotation.
    pub(crate) header_span: Span,
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
    /// Named axis path — 2-D slot, typed `T[SharedAxis, ExtraAxis]`.
    Axis(Spanned<NamePath>),
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
        extra_axis: Spanned<NamePath>,
    },
}

/// One slice of a multi-decl body: optional slice-label prefix + header + rows.
///
/// Header cells, column layouts, and row values are validated together during
/// construction and exposed read-only.
#[derive(Debug, Clone)]
pub struct MultiDeclSlice<P: Phase = Raw> {
    prefix_keys: Vec<MapEntryKey>,
    header_cells: Vec<MultiHeaderCell>,
    column_layout: Vec<MultiSlotColumnSpan>,
    rows: Vec<MultiDataRow<P>>,
}

impl<P: Phase> MultiDeclSlice<P> {
    /// Constructs a slice whose row widths and layout ranges match its header.
    pub fn new(
        prefix_keys: Vec<MapEntryKey>,
        header_cells: Vec<MultiHeaderCell>,
        column_layout: Vec<MultiSlotColumnSpan>,
        rows: Vec<MultiDataRow<P>>,
    ) -> Result<Self, MultiDeclShapeError> {
        let header_count = header_cells.len();
        if let Some((row_index, row)) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.values.len() != header_count)
        {
            return Err(MultiDeclShapeError::RowWidth {
                row_index,
                header_count,
                value_count: row.values.len(),
            });
        }
        for (layout_index, layout) in column_layout.iter().enumerate() {
            match layout {
                MultiSlotColumnSpan::Single(column_index) if *column_index >= header_count => {
                    return Err(MultiDeclShapeError::ColumnOutOfBounds {
                        layout_index,
                        column_index: *column_index,
                        header_count,
                    });
                }
                MultiSlotColumnSpan::Range { start, end, .. }
                    if start >= end || *end > header_count =>
                {
                    return Err(MultiDeclShapeError::InvalidColumnRange {
                        layout_index,
                        start: *start,
                        end: *end,
                        header_count,
                    });
                }
                MultiSlotColumnSpan::Single(_) | MultiSlotColumnSpan::Range { .. } => {}
            }
        }
        Ok(Self {
            prefix_keys,
            header_cells,
            column_layout,
            rows,
        })
    }

    #[must_use]
    pub fn prefix_keys(&self) -> &[MapEntryKey] {
        &self.prefix_keys
    }

    #[must_use]
    pub fn header_cells(&self) -> &[MultiHeaderCell] {
        &self.header_cells
    }

    #[must_use]
    pub fn column_layout(&self) -> &[MultiSlotColumnSpan] {
        &self.column_layout
    }

    #[must_use]
    pub fn rows(&self) -> &[MultiDataRow<P>] {
        &self.rows
    }
}

/// One cell of a multi-decl header row.
#[derive(Debug, Clone)]
pub enum MultiHeaderCell {
    Underscore {
        span: Span,
    },
    Variant {
        /// Required axis path from the canonical `Axis.Variant` spelling.
        axis: Spanned<NamePath>,
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
///
/// The values are private because their width is validated by
/// [`MultiDeclSlice::new`].
#[derive(Debug, Clone)]
pub struct MultiDataRow<P: Phase = Raw> {
    label: Spanned<IndexEntryKey>,
    values: Vec<Expr<P>>,
}

impl<P: Phase> MultiDataRow<P> {
    #[must_use]
    pub const fn new(label: Spanned<IndexEntryKey>, values: Vec<Expr<P>>) -> Self {
        Self { label, values }
    }

    #[must_use]
    pub const fn label(&self) -> &Spanned<IndexEntryKey> {
        &self.label
    }

    #[must_use]
    pub fn values(&self) -> &[Expr<P>] {
        &self.values
    }
}

#[cfg(test)]
mod multi_decl_shape_tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn parsed_multi_decl() -> MultiDecl {
        let source = r"
param a: Int[I],
param b: Bool[I, J]
  = table[I, (_, J)] {
      : _, J.X;
      X: 1, true;
  };
";
        let file = Parser::new(source).parse_file().expect("valid multi-decl");
        let declaration = file
            .declarations
            .into_iter()
            .next()
            .expect("one declaration");
        match declaration.kind {
            DeclKind::Sugar(RawDeclSugar::Multi(multi)) => multi,
            other => panic!("expected multi-decl, got {other:?}"),
        }
    }

    #[test]
    fn multi_decl_rejects_too_few_slots() {
        let MultiDecl {
            mut slots,
            shared_axes,
            slot_axes,
            slices,
            span,
            table_expr_span,
        } = parsed_multi_decl();
        slots.pop();

        let error = MultiDecl::new(slots, shared_axes, slot_axes, slices, span, table_expr_span)
            .unwrap_err();

        assert_eq!(error, MultiDeclShapeError::TooFewSlots { slot_count: 1 });
    }

    #[test]
    fn multi_decl_rejects_slot_axis_arity_mismatch() {
        let MultiDecl {
            slots,
            shared_axes,
            mut slot_axes,
            slices,
            span,
            table_expr_span,
        } = parsed_multi_decl();
        slot_axes.pop();

        let error = MultiDecl::new(slots, shared_axes, slot_axes, slices, span, table_expr_span)
            .unwrap_err();

        assert_eq!(
            error,
            MultiDeclShapeError::SlotAxisArity {
                slot_count: 2,
                slot_axis_count: 1,
            }
        );
    }

    #[test]
    fn multi_decl_rejects_slice_layout_arity_mismatch() {
        let MultiDecl {
            slots,
            shared_axes,
            slot_axes,
            mut slices,
            span,
            table_expr_span,
        } = parsed_multi_decl();
        slices[0].column_layout.pop();

        let error = MultiDecl::new(slots, shared_axes, slot_axes, slices, span, table_expr_span)
            .unwrap_err();

        assert_eq!(
            error,
            MultiDeclShapeError::SliceLayoutArity {
                slice_index: 0,
                slot_count: 2,
                layout_count: 1,
            }
        );
    }

    #[test]
    fn multi_decl_slice_rejects_row_width_mismatch() {
        let mut slices = parsed_multi_decl().slices;
        let MultiDeclSlice {
            prefix_keys,
            header_cells,
            column_layout,
            mut rows,
        } = slices.pop().expect("one slice");
        rows[0].values.pop();

        let error =
            MultiDeclSlice::new(prefix_keys, header_cells, column_layout, rows).unwrap_err();

        assert_eq!(
            error,
            MultiDeclShapeError::RowWidth {
                row_index: 0,
                header_count: 2,
                value_count: 1,
            }
        );
    }

    #[test]
    fn multi_decl_slice_rejects_invalid_column_range() {
        let mut slices = parsed_multi_decl().slices;
        let MultiDeclSlice {
            prefix_keys,
            header_cells,
            mut column_layout,
            rows,
        } = slices.pop().expect("one slice");
        let MultiSlotColumnSpan::Range { extra_axis, .. } = &column_layout[1] else {
            panic!("expected ranged second slot")
        };
        column_layout[1] = MultiSlotColumnSpan::Range {
            start: 1,
            end: 3,
            extra_axis: extra_axis.clone(),
        };

        let error =
            MultiDeclSlice::new(prefix_keys, header_cells, column_layout, rows).unwrap_err();

        assert_eq!(
            error,
            MultiDeclShapeError::InvalidColumnRange {
                layout_index: 1,
                start: 1,
                end: 3,
                header_count: 2,
            }
        );
    }
}

/// Shared shape for value declarations with an expression body.
#[derive(Debug, Clone)]
pub struct ValueDecl<P: Phase = Raw> {
    pub visibility: Visibility,
    pub name: Spanned<DeclName>,
    pub type_ann: TypeExpr<P>,
    pub value: Expr<P>,
}

/// Runtime node declaration: `node name: Type = expr;`
pub type NodeDecl<P = Raw> = ValueDecl<P>;

/// Const node declaration: `const node name: Type = expr;`
pub type ConstNodeDecl<P = Raw> = ValueDecl<P>;

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

/// Whether a unit is allowed in compile-time (`const`) contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct UnitDef<P: Phase = Raw> {
    pub scale_expr: Expr<P>,
    pub unit_expr: UnitExpr,
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
/// - Record payload: `Impulsive(delta_v: Velocity)` — `payload` is
///   `Some(vec![…])`. Constructor payloads use parentheses exclusively.
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

/// The kind of an index declaration.
#[derive(Debug, Clone)]
pub enum IndexDeclKind<P: Phase = Raw> {
    /// Named variants: `{ Departure, Correction, Insertion }`
    Named {
        variants: Vec<Spanned<IndexVariantName>>,
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
    /// Optional sort-aware default, e.g. `F: Type = Unframed` or `N: Nat = 3`.
    /// The syntax remains unresolved until it is checked against `constraint`.
    pub default: Option<GenericArg<P>>,
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
    /// `F: Type` -- the generic stands for a value type.
    Type,
}

impl std::fmt::Display for GenericConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dim => "Dim",
            Self::Index => "Index",
            Self::Nat => "Nat",
            Self::Type => "Type",
        })
    }
}
