use std::marker::PhantomData;

use crate::syntax::ast::common::{Ident, ModulePath};
use crate::syntax::names::{
    ConstructorName, DeclName, DimName, FieldName, IndexName, IndexVariantName, LocalName,
    NamePath, ScopedName, StructTypeName, UnitName,
};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::{Phase, Raw};
use crate::syntax::span::{Span, Spanned};

/// Expression-level sugar — only legal in [`Raw`].
///
/// Each variant corresponds to a surface expression form that is rewritten
/// into ordinary `ExprKind` variants by [`crate::desugar::convert`]. In
/// `Desugared`, the `Sugar` slot is `Infallible` and these variants vanish.
#[derive(Debug, Clone)]
pub enum RawExprSugar {
    /// Table literal: `table[Phase, 3] { ... }`.
    ///
    /// Desugars to [`ExprKind::MapLiteral`] — the `indexes` metadata is
    /// dropped (entries already carry full `Index.Variant` keys), and the
    /// `table` keyword is purely surface syntax preserved by the formatter
    /// via the raw AST.
    TableLiteral {
        indexes: Vec<TableIndexSpec>,
        entries: Vec<MapEntry<Raw>>,
    },
}

// ---------------------------------------------------------------------------
// Unresolved-ref variants (legal in `Raw` and `Desugared`, not in `Resolved`)
// ---------------------------------------------------------------------------

/// Unresolved reference, produced by the parser before name resolution.
///
/// Carried by `ExprKind::UnresolvedRef(P::RefSugar)`. The parser emits these
/// when the meaning of an identifier path cannot be determined from syntax
/// alone; the name-resolution pass rewrites them into concrete `ConstRef` /
/// `LocalRef` / `VariantLiteral` / `ConstructorCall` variants and produces a
/// [`crate::syntax::phase::Resolved`] AST in which `RefSugar = Infallible`.
///
/// This indirection is necessary because the same token shape can mean
/// different expression kinds depending on declarations and local scopes. For
/// example, the dotted expression `Foo.Bar` is parsed as the unresolved path
/// `Foo.Bar` in both of these programs:
///
/// ```graphcal
/// index Foo = { Bar };
/// node x: Dimensionless = Foo.Bar;
/// ```
///
/// and:
///
/// ```graphcal
/// node x: Dimensionless = Foo.Bar;
/// ```
///
/// Only after collecting names from the file can resolution know whether
/// `Foo` is an index. In the first program `Foo.Bar` becomes a
/// `VariantLiteral`; in the second it becomes a qualified `ConstRef` whose
/// validity is checked later.
///
/// Bare identifiers have the same issue. `PI` parses as the unresolved path
/// `PI` both when it denotes the built-in constant:
///
/// ```graphcal
/// node x: Dimensionless = PI;
/// ```
///
/// and when a local binding shadows that constant:
///
/// ```graphcal
/// index I = { A };
/// node x: Dimensionless[I] = for PI: I { PI };
/// ```
///
/// Name resolution turns the first `PI` into a `ConstRef`, but the loop body
/// `PI` in the second program into a `LocalRef`.
///
/// The payload is a path rather than separate "bare" and "qualified" variants
/// so the parser records the complete syntactic structure uniformly:
/// `Foo`, `Foo.Bar`, and `Foo.Bar.Baz` are all identifier paths. Segment-count
/// restrictions, such as index variants currently being two-segment paths, are
/// semantic rules enforced by name resolution rather than parser artifacts.
#[derive(Debug, Clone)]
pub enum UnresolvedRef {
    /// Unresolved identifier path: `Foo`, `Foo.Bar`, or `Foo.Bar.Baz`.
    Path(IdentPath),
}

/// A non-empty dot-separated identifier path in expression position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentPath {
    pub segments: NonEmpty<Ident>,
}

impl IdentPath {
    /// Construct a path from already-tokenized segments.
    #[must_use]
    pub const fn new(segments: NonEmpty<Ident>) -> Self {
        Self { segments }
    }

    /// Construct a one-segment path from an identifier.
    #[must_use]
    pub fn bare(ident: Ident) -> Self {
        Self::new(NonEmpty::singleton(ident))
    }

    /// Borrow all path segments in source order.
    #[must_use]
    pub fn segments(&self) -> &[Ident] {
        self.segments.as_slice()
    }

    /// Mutably borrow all path segments in source order.
    #[must_use]
    pub fn segments_mut(&mut self) -> &mut [Ident] {
        self.segments.as_mut_slice()
    }

    /// Consume and return the non-empty segment sequence.
    #[must_use]
    pub fn into_segments(self) -> NonEmpty<Ident> {
        self.segments
    }

    /// Consume and return the segment vector.
    #[must_use]
    pub fn into_vec(self) -> Vec<Ident> {
        self.segments.into_vec()
    }

    /// Number of path segments. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.segments.len()
    }

    /// Returns `false`; provided for API compatibility with sequence-like code.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Returns whether this is a one-segment identifier path.
    #[must_use]
    pub const fn is_bare(&self) -> bool {
        self.segments.len() == 1
    }

    /// Returns the source span covering the whole path.
    #[must_use]
    pub fn span(&self) -> Span {
        self.segments.first().span.merge(self.segments.last().span)
    }

    /// Returns the leaf segment of the path.
    #[must_use]
    pub fn leaf(&self) -> &Ident {
        self.segments.last()
    }

    /// Split the path into qualifier segments and the leaf segment.
    ///
    /// The qualifier slice is empty for one-segment paths.
    #[must_use]
    pub fn split_last(&self) -> (&[Ident], &Ident) {
        let (leaf, qualifier) = self.segments.split_last();
        (qualifier, leaf)
    }

    /// Returns the qualifier segments before the leaf. Empty for bare paths.
    #[must_use]
    pub fn qualifier_segments(&self) -> &[Ident] {
        self.split_last().0
    }

    /// Returns qualifier segments and leaf only when this path is qualified.
    #[must_use]
    pub fn qualifier_and_leaf(&self) -> Option<(&[Ident], &Ident)> {
        let (qualifier, leaf) = self.split_last();
        (!qualifier.is_empty()).then_some((qualifier, leaf))
    }

    /// Returns the only segment when this is a bare identifier path.
    #[must_use]
    pub fn as_bare(&self) -> Option<&Ident> {
        match self.segments.as_slice() {
            [ident] => Some(ident),
            _ => None,
        }
    }

    /// Mutably returns the only segment when this is a bare identifier path.
    pub fn as_bare_mut(&mut self) -> Option<&mut Ident> {
        match self.segments.as_mut_slice() {
            [ident] => Some(ident),
            _ => None,
        }
    }

    /// Consume this path and return its segment when it is bare.
    ///
    /// Returns the original path unchanged when it is qualified.
    pub fn into_bare(self) -> Result<Ident, Self> {
        if self.is_bare() {
            let mut segments = self.segments.into_vec();
            Ok(segments.remove(0))
        } else {
            Err(self)
        }
    }

    /// Convert this spanned syntax path into a span-less [`NamePath`].
    #[must_use]
    pub fn into_name_path(self) -> NamePath {
        NamePath::new(self.segments.map(|ident| ident.name))
    }

    /// Convert this syntax path into a [`NamePath`] paired with the path's full span.
    #[must_use]
    pub fn into_spanned_name_path(self) -> Spanned<NamePath> {
        let span = self.span();
        Spanned::new(self.into_name_path(), span)
    }

    /// Human-readable path string for diagnostics and formatting boundaries.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

impl From<Ident> for IdentPath {
    fn from(ident: Ident) -> Self {
        Self::bare(ident)
    }
}

impl std::fmt::Display for IdentPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (idx, segment) in self.segments.iter().enumerate() {
            if idx > 0 {
                f.write_str(".")?;
            }
            f.write_str(segment.name.as_str())?;
        }
        Ok(())
    }
}

impl UnresolvedRef {
    /// Returns the source span of the underlying identifier path.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Path(path) => path.span(),
        }
    }
}
/// A param binding in a module instantiation: `name: expr`.
///
/// Used in `include "path"(name: expr, ...) { ... };`
#[derive(Debug, Clone)]
pub struct ParamBinding<P: Phase = Raw> {
    /// The param name in the imported file.
    pub name: Ident,
    /// The value expression (evaluated in the importer's scope).
    pub value: Expr<P>,
    /// Span covering the entire `name: expr`.
    pub span: Span,
}
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
pub struct DomainBound<P: Phase = Raw> {
    /// The bound kind (`min` or `max`).
    pub kind: DomainBoundKind,
    /// The span of the keyword (`min` or `max`).
    pub kind_span: Span,
    /// The bound value expression.
    pub value: Expr<P>,
    pub span: Span,
}

/// A type expression (dimension annotation on declarations).
/// An expression in index position of an indexed type.
///
/// In `Velocity[Maneuver]` or `Velocity[module.Maneuver]`, the index path is
/// an `IndexExpr::Name`.
/// In `Dimensionless[3, 4]`, `3` and `4` are `IndexExpr::NatExpr(NatExpr::Literal(..))`.
/// In `D[N + 1]`, `N + 1` is an `IndexExpr::NatExpr`.
#[derive(Debug, Clone)]
pub enum IndexExpr {
    /// A named index or generic parameter path: `Maneuver`, `I`, `N`, `module.Maneuver`
    Name(Spanned<NamePath>),
    /// A type-level natural-number expression in index position: `3`, `N + 1`, `M + N`.
    NatExpr(NatExpr),
}

impl IndexExpr {
    /// Get the source span of this index expression.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Name(name) => name.span,
            Self::NatExpr(nat_expr) => nat_expr.span(),
        }
    }
}

/// E.g., `Length`, `Dimensionless`, `Length^3 / Time^2`
///
/// Optionally carries domain constraints: `Mass(min: 100 kg, max: 2000 kg)`.
#[derive(Debug, Clone)]
pub struct TypeExpr<P: Phase = Raw> {
    pub kind: TypeExprKind<P>,
    /// Optional domain constraints on the type.
    pub constraints: Vec<DomainBound<P>>,
    pub span: Span,
}

/// The kind of a type expression.
#[derive(Debug, Clone)]
pub enum TypeExprKind<P: Phase = Raw> {
    /// `Dimensionless`
    Dimensionless,
    /// `Bool`
    Bool,
    /// `Int`
    Int,
    /// `Datetime` (bare, without time scale parameter — defaults to UTC)
    Datetime,
    /// `Datetime<TimeScale>` — built-in datetime type parameterized by a time
    /// scale. Kept separate from [`Self::TypeApplication`] so downstream
    /// resolution dispatches on the variant rather than string-matching the
    /// built-in name.
    DatetimeApplication { type_args: Vec<TypeExpr<P>> },
    /// A dimension expression like `Length`, `Length^2`, `Mass * Length / Time^2`
    DimExpr(DimExpr),
    /// An indexed type like `Velocity[Maneuver]`, `Dimensionless[3, 4]`, or `D[M, N]`
    Indexed {
        base: Box<TypeExpr<P>>,
        indexes: Vec<IndexExpr>,
    },
    /// A user-defined generic type application like `Vec3<Length, ECI>`.
    /// Built-in parameterized types (currently only `Datetime<...>`) have their
    /// own variants instead — see [`Self::DatetimeApplication`].
    TypeApplication {
        name: Spanned<NamePath>,
        type_args: Vec<TypeExpr<P>>,
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

/// A single dimension term: `ident_path` or `ident_path ^ INTEGER`
#[derive(Debug, Clone)]
pub struct DimTerm {
    pub name: Spanned<NamePath>,
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
///
/// Construct via [`Expr::new`] — direct struct literal syntax is blocked
/// by the private phase marker.
#[derive(Debug, Clone)]
pub struct Expr<P: Phase = Raw> {
    pub kind: ExprKind<P>,
    pub span: Span,
    // Marker forcing a concrete (non-recursive) use of `P` so the compiler
    // can determine variance for `Expr<P>` and, transitively, every type
    // that contains `Expr<P>`. Private so callers must use `Expr::new` —
    // that keeps the phase marker out of their sight entirely.
    _phase: PhantomData<fn() -> P>,
}

impl<P: Phase> Expr<P> {
    /// Construct an expression with the given kind and span.
    #[must_use]
    pub const fn new(kind: ExprKind<P>, span: Span) -> Self {
        Self {
            kind,
            span,
            _phase: PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind<P: Phase = Raw> {
    /// Numeric literal: `1200.0`, `3.98e5`, `200_000.0`
    Number(f64),
    /// Integer literal: `42`, `1_000`
    Integer(i64),
    /// Boolean literal: `true`, `false`
    Bool(bool),
    /// String literal: `"hello"` (used as arguments to `datetime()`, `epoch()`, etc.)
    StringLiteral(String),
    /// A bare type-system name used where a value expression is required by
    /// surface syntax, such as an include binding RHS.
    TypeSystemRef(Spanned<TypeSystemRefKind>),
    /// Graph reference: `@name` or `@alias.member`. The payload encodes
    /// qualification structurally — `Local` for bare `@name`, `Qualified`
    /// for `@alias.member` (after the namespace-alias rewrite). Producers
    /// never invent or interpret a flat-string separator.
    GraphRef(Spanned<ScopedName>),
    /// Built-in constant reference (`PI`, `E`, `TAU`) or module-qualified
    /// constant (`module.CONST`). The payload encodes qualification
    /// structurally — see [`GraphRef`].
    ///
    /// [`GraphRef`]: ExprKind::GraphRef
    ConstRef(Spanned<ScopedName>),
    /// Binary operation: `a + b`, `a * b`, `a ^ b`, `a && b`, etc.
    BinOp {
        op: BinOp,
        lhs: Box<Expr<P>>,
        rhs: Box<Expr<P>>,
    },
    /// Unary operation: `-x`, `!x`
    UnaryOp { op: UnaryOp, operand: Box<Expr<P>> },
    /// Function call syntax: `sqrt(x)`, `atan2(y, x)`, `eye<3>()`, or `module.fn(x)`.
    ///
    /// The callee is a syntactic path. Bare calls and qualified calls have the
    /// same AST shape; semantic categorization/resolution happens later.
    FnCall {
        callee: IdentPath,
        type_args: Vec<GenericArg<P>>,
        args: Vec<Expr<P>>,
    },
    /// Conditional: `if cond { then_expr } else { else_expr }`
    If {
        condition: Box<Expr<P>>,
        then_branch: Box<Expr<P>>,
        else_branch: Box<Expr<P>>,
    },
    /// Unit-annotated literal: `400 km`, `9.80665 m/s^2`
    UnitLiteral { value: f64, unit: UnitExpr },
    /// Conversion: `expr -> unit_expr`
    Convert {
        expr: Box<Expr<P>>,
        target: UnitExpr,
    },
    /// Timezone display: `expr -> "America/New_York"` (datetime only)
    DisplayTimezone {
        expr: Box<Expr<P>>,
        timezone: String,
    },
    /// Local variable reference (loop variable, function parameter, match binding, etc.)
    LocalRef(Ident),
    /// Field access: `@transfer.dv1`, `@mission.transfer.dv1`
    FieldAccess {
        expr: Box<Expr<P>>,
        field: Spanned<FieldName>,
    },
    /// Constructor-call syntax for values of user-defined unified `type` declarations.
    ///
    /// Payload constructors use named arguments, e.g.
    /// `TransferResult(dv1: @dv1, dv2: @dv2)` or
    /// `module.TransferResult(dv1: @dv1)`. The callee is a syntactic path; name
    /// resolution decides whether it denotes a constructor.
    ConstructorCall {
        callee: IdentPath,
        generic_args: Vec<GenericArg<P>>,
        fields: Vec<FieldInit<P>>,
    },
    /// Map literal: `{ Maneuver.Departure: 2.46 km/s, Maneuver.Correction: 0.05 km/s }`
    MapLiteral { entries: Vec<MapEntry<P>> },
    /// For comprehension: `for m: Maneuver { @delta_v[m] + 1.0 }`
    ForComp {
        bindings: Vec<ForBinding>,
        body: Box<Expr<P>>,
    },
    /// Index access: `@delta_v[m]`, `@delta_v[Maneuver.Departure]`, `@P[a, b]`
    IndexAccess {
        expr: Box<Expr<P>>,
        args: Vec<IndexArg<P>>,
    },
    /// Scan: `scan(source, init, |acc, val| body)`
    Scan {
        source: Box<Expr<P>>,
        init: Box<Expr<P>>,
        acc_name: Spanned<LocalName>,
        val_name: Spanned<LocalName>,
        body: Box<Expr<P>>,
    },
    /// Unfold: `unfold(init, |prev_i, i| body)`
    ///
    /// Generates an indexed value from a seed by iterating over a range index.
    /// The closure receives `(prev_i, i)` bindings for the previous and current
    /// step indices, and the body can reference `@node_name[prev_i]`.
    Unfold {
        init: Box<Expr<P>>,
        prev_name: Spanned<LocalName>,
        curr_name: Spanned<LocalName>,
        body: Box<Expr<P>>,
    },
    /// Match expression: `match @status { Nominal => ..., Warning(message: code) => ... }`
    Match {
        scrutinee: Box<Expr<P>>,
        arms: Vec<MatchArm<P>>,
    },
    /// Standalone index variant reference after name resolution has proven a
    /// local `Index.Variant` reference.
    ///
    /// Qualified module variants currently stay as syntactic paths until a
    /// module-aware resolver can prove the target index.
    VariantLiteral {
        index: Spanned<NamePath>,
        variant: Spanned<IndexVariantName>,
    },
    /// Inline DAG invocation: `@dag(args).out` or `@module.dag(args).out`.
    ///
    /// Each syntactic occurrence denotes a fresh DAG instantiation that is
    /// desugared during TIR lowering to the equivalent
    /// `include <path>(args) as <synthetic>; @<synthetic>.out`. Preserved as
    /// a distinct AST variant so source spans survive for diagnostics.
    ///
    /// The post-`@` expression as a whole must denote a *node* — that is the
    /// invariant `@` enforces. `@dag(args).out` is well-formed because
    /// `dag(args).out` projects an output node from a fresh DAG instance, and
    /// likewise `@module.dag(args).out` projects an output node from a DAG
    /// brought into scope via `import module.{dag};` or `import path as
    /// module;`. Bare `@dag(args)` (no projection) is rejected — a DAG
    /// instance with no projection is not a node.
    InlineDagRef {
        /// Path to the DAG being invoked. Single-segment for same-file calls
        /// (`@dag(args).out`), multi-segment for cross-file qualified calls
        /// (`@module.dag(args).out`). The leaf segment names the DAG; any
        /// preceding segments resolve through module aliases brought into
        /// scope by `import`.
        path: ModulePath,
        /// Param/index bindings, same shape as `include` bindings.
        args: Vec<ParamBinding<P>>,
        /// Projected output node name (after the closing paren `.`).
        output: Spanned<DeclName>,
    },
    /// Unresolved reference produced by the parser.
    ///
    /// Carries [`crate::syntax::ast::UnresolvedRef`] in [`Raw`] and
    /// [`crate::syntax::phase::Desugared`], as an unresolved identifier path.
    /// In [`crate::syntax::phase::Resolved`] the payload is
    /// [`core::convert::Infallible`] — the variant is statically unreachable
    /// after the name-resolution pass has run.
    UnresolvedRef(P::RefSugar),
    /// Phase-specific expression sugar.
    ///
    /// In [`Raw`], this is [`crate::syntax::ast::RawExprSugar`] and carries
    /// surface forms like `TableLiteral` that are eliminated by the desugar
    /// pass. In `Desugared` and `Resolved`, the payload is
    /// [`core::convert::Infallible`] — the variant is statically unreachable.
    Sugar(P::ExprSugar),
}

/// An index specification in a table literal's bracket list: `table[Phase, 3]`
///
/// Named indexes reference declared index types, while Nat range literals
/// desugar to `range(N)` with synthetic variants `#0`, `#1`, etc.
#[derive(Debug, Clone)]
pub enum TableIndexSpec {
    /// A named index: `Phase`, `Maneuver`, or `module.Maneuver`
    Named(Spanned<NamePath>),
    /// A Nat range literal: `3` (desugars to `range(3)`)
    NatRange(u64, Span),
}

/// Shared axes in a multi-declaration table prefix.
///
/// The final axis has a distinct semantic role: it is the row axis. Any axes
/// before it are slice axes. This is intentionally not modeled as a generic
/// `NonEmpty<TableIndexSpec>` because the tail element is special.
#[derive(Debug, Clone)]
pub struct MultiDeclSharedAxes {
    slice_axes: Vec<TableIndexSpec>,
    row_axis: TableIndexSpec,
}

impl MultiDeclSharedAxes {
    /// Construct shared axes from zero or more slice axes and the always-present row axis.
    #[must_use]
    pub const fn new(slice_axes: Vec<TableIndexSpec>, row_axis: TableIndexSpec) -> Self {
        Self {
            slice_axes,
            row_axis,
        }
    }

    /// Convert a parser-order vector into semantic slice/row axes.
    ///
    /// # Errors
    ///
    /// Returns [`crate::syntax::non_empty::EmptyVecError`] when `axes` is empty.
    pub fn try_from_vec(
        mut axes: Vec<TableIndexSpec>,
    ) -> Result<Self, crate::syntax::non_empty::EmptyVecError> {
        let row_axis = axes.pop().ok_or(crate::syntax::non_empty::EmptyVecError)?;
        Ok(Self::new(axes, row_axis))
    }

    /// Slice axes preceding the row axis.
    #[must_use]
    pub fn slice_axes(&self) -> &[TableIndexSpec] {
        &self.slice_axes
    }

    /// The row axis.
    #[must_use]
    pub const fn row_axis(&self) -> &TableIndexSpec {
        &self.row_axis
    }

    /// Number of shared axes. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.slice_axes.len() + 1
    }

    /// Returns `false`; provided for sequence-like callers.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Iterate over axes in source order: slice axes first, then row axis.
    pub fn iter(&self) -> impl Iterator<Item = &TableIndexSpec> {
        self.slice_axes
            .iter()
            .chain(std::iter::once(&self.row_axis))
    }
}

impl std::ops::Index<usize> for MultiDeclSharedAxes {
    type Output = TableIndexSpec;

    #[expect(
        clippy::panic,
        reason = "Index implementations conventionally panic on out-of-bounds access"
    )]
    fn index(&self, index: usize) -> &Self::Output {
        match index.cmp(&self.slice_axes.len()) {
            std::cmp::Ordering::Less => &self.slice_axes[index],
            std::cmp::Ordering::Equal => &self.row_axis,
            std::cmp::Ordering::Greater => {
                panic!("multi-decl shared axis index out of bounds")
            }
        }
    }
}

/// An index key in a map literal entry.
///
/// Plain map literals use named indexes. Table literals over Nat axes desugar
/// to map entries with an explicitly typed Nat range key so downstream passes
/// do not have to recover `range(N)` semantics from a fabricated index name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapEntryIndex {
    /// A declared named index.
    Named(NamePath),
    /// A Nat range literal index, `range(N)`.
    NatRange(u64),
}

impl MapEntryIndex {
    /// Return the leaf registry name for declared named indexes only.
    ///
    /// Nat ranges are not declared registry indexes; callers must pattern-match
    /// and use a typed Nat-range identity instead of fabricating an `IndexName`.
    #[must_use]
    pub fn named_registry_name(&self) -> Option<IndexName> {
        match self {
            Self::Named(name) => Some(IndexName::from(name.leaf().clone())),
            Self::NatRange(_) => None,
        }
    }
}

impl From<IndexName> for MapEntryIndex {
    fn from(value: IndexName) -> Self {
        Self::Named(NamePath::from(value))
    }
}

impl From<NamePath> for MapEntryIndex {
    fn from(value: NamePath) -> Self {
        Self::Named(value)
    }
}

impl std::fmt::Display for MapEntryIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(name) => write!(f, "{name}"),
            Self::NatRange(size) => write!(f, "range({size})"),
        }
    }
}

/// A bare type-system identifier after name resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSystemRefKind {
    Type(StructTypeName),
    Dimension(DimName),
    Index(IndexName),
    BareVariant(IndexVariantName),
    Imported(StructTypeName),
}

impl TypeSystemRefKind {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Type(name) | Self::Imported(name) => name.as_str(),
            Self::Dimension(name) => name.as_str(),
            Self::Index(name) => name.as_str(),
            Self::BareVariant(name) => name.as_str(),
        }
    }
}

impl std::fmt::Display for TypeSystemRefKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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

/// A single key in a map literal entry: `Index.Variant`
#[derive(Debug, Clone)]
pub struct MapEntryKey {
    pub index: Spanned<MapEntryIndex>,
    pub variant: Spanned<IndexVariantName>,
}

/// An entry in a map literal.
///
/// Single-axis: `Maneuver.Departure: 2.46 km/s` (keys has 1 element)
/// Multi-axis:  `(Phase.Launch, Maneuver.Departure): 2.46 km/s` (keys has 2+ elements)
#[derive(Debug, Clone)]
pub struct MapEntry<P: Phase = Raw> {
    pub keys: NonEmpty<MapEntryKey>,
    pub value: Expr<P>,
}

/// A binding in a `for` comprehension: `m: Maneuver` or `i: range(3)`
#[derive(Debug, Clone)]
pub struct ForBinding {
    pub var: Spanned<LocalName>,
    pub index: ForBindingIndex,
}

/// The index in a for binding: either a named index or a `range(...)` expression.
#[derive(Debug, Clone)]
pub enum ForBindingIndex {
    /// A named index: `for m: Maneuver { ... }` or `for m: module.Maneuver { ... }`
    Named(Spanned<NamePath>),
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
pub enum GenericArg<P: Phase = Raw> {
    /// A type expression (for Dim or Index generic params): `Length`, `Maneuver`
    Type(TypeExpr<P>),
    /// A nat expression (for Nat generic params): `3`, `N + 1`
    Nat(NatExpr),
}

impl<P: Phase> GenericArg<P> {
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
pub enum IndexArg<P: Phase = Raw> {
    /// Qualified variant: `Maneuver.Departure` or `module.Maneuver.Departure`
    Variant {
        index: Spanned<NamePath>,
        variant: Spanned<IndexVariantName>,
    },
    /// Loop variable: `m`
    Var(Ident),
    /// Arbitrary expression: `i + 1`, `i - M`
    Expr(Box<Expr<P>>),
}

/// A field initializer in a constructor call.
#[derive(Debug, Clone)]
pub struct FieldInit<P: Phase = Raw> {
    pub name: Spanned<FieldName>,
    pub value: Expr<P>,
}

/// One arm of a `match` expression: `Impulsive(delta_v: dv) => expr`
#[derive(Debug, Clone)]
pub struct MatchArm<P: Phase = Raw> {
    pub pattern: MatchPattern,
    pub body: Expr<P>,
    pub span: Span,
}

/// A match pattern: `Impulsive(delta_v: dv)`, `Nominal`, `Maneuver.Departure`.
#[derive(Debug, Clone)]
pub enum MatchPattern {
    /// Syntactic path pattern before semantic categorization.
    ///
    /// The parser emits this for both constructor-looking and index-label-looking
    /// patterns. Name resolution may rewrite it to [`Self::Constructor`] or
    /// [`Self::IndexLabel`] only when it can prove that categorization from the
    /// local symbol context. Qualified paths that require module-aware lookup
    /// remain syntactic paths instead of being half-resolved.
    Path {
        path: IdentPath,
        bindings: Vec<PatternBinding>,
        span: Span,
    },
    /// Tagged-union constructor pattern: `Impulsive(delta_v: dv)` or `Nominal`.
    Constructor {
        name: Spanned<ConstructorName>,
        bindings: Vec<PatternBinding>,
        span: Span,
    },
    /// Named-index label pattern: `Maneuver.Departure`.
    ///
    /// Index labels are fieldless, so this variant deliberately has no
    /// binding payload. This variant is semantic: producers should construct it
    /// only after proving that the path denotes an index variant.
    IndexLabel {
        index: Spanned<NamePath>,
        variant: Spanned<IndexVariantName>,
        span: Span,
    },
}

impl MatchPattern {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Path { span, .. }
            | Self::Constructor { span, .. }
            | Self::IndexLabel { span, .. } => *span,
        }
    }

    #[must_use]
    pub fn bindings(&self) -> &[PatternBinding] {
        match self {
            Self::Path { bindings, .. } | Self::Constructor { bindings, .. } => bindings,
            Self::IndexLabel { .. } => &[],
        }
    }
}

/// A binding in a match pattern.
#[derive(Debug, Clone)]
pub enum PatternBinding {
    /// Bind a field to a variable: `message: msg`.
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
