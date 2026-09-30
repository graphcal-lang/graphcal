//! HIR expression tree: its completeness parameter, nodes, references, and
//! binder payloads.

use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedIndexVariant, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::dimension::UnitRef as SyntaxUnitRef;

use crate::builtin::{BuiltinConst, BuiltinFn, ScaleFreeBuiltin};
use crate::dag_id::DagId;
use crate::datetime_literal::{CivilDateTimeLiteral, OffsetDateTimeLiteral, ZonedDateTimeLiteral};
use crate::desugar::desugared_ast as ast;
use crate::semantic::time_scale::TimeScale;
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::FieldName;

use crate::hir::types::GenericArg;
use crate::nat::NatPolyForm;

use core::fmt;
use core::hash::Hash;

use super::local_decl::LocalDecl;
use super::local_unit::LocalUnit;
use crate::expression_id::ExprId;

pub mod sealed {
    pub trait Sealed {}
}

/// Marker trait for HIR expression completeness.
///
/// Sealed: only the compiler's completeness markers implement it.
///
/// Completeness parameter of HIR expression trees.
///
/// HIR lowering is diagnostic-accumulating: an IDE must keep a tree for code
/// that does not resolve. The batch pipeline must never observe such a tree,
/// and it keys checked facts by per-node occurrence identities.
/// [`Expr<C>`](Expr) makes both distinctions types, mirroring the AST
/// [`Phase`](crate::syntax::phase::Phase) technique:
///
/// | Completeness | error node               | node identity | declaration reference | unit reference        |
/// |--------------|--------------------------|---------------|-----------------------|-----------------------|
/// | `Tolerant`   | diagnostic + children    | none          | source definition     | source definition     |
/// | [`Draft`]    | [`NoErrorNode`]          | none          | [`LocalDecl`]         | [`LocalUnit`]         |
/// | [`Strict`]   | [`NoErrorNode`]          | `ExprId`      | [`LocalDecl`]         | [`LocalUnit`]         |
///
/// `Tolerant` is defined beside the lowerer, because its error node carries
/// a lowering diagnostic. Strict lowering refines `Tolerant` into [`Draft`];
/// finishing a body ([`CheckedExpr`](super::CheckedExpr)) numbers a [`Draft`] into
/// [`Strict`]. Consumers of checked HIR discharge the error-node arm with
/// [`NoErrorNode::absurd`] instead of a runtime fallback, and read identities
/// without a fallible lookup.
pub trait Completeness: 'static + fmt::Debug + Clone + Copy + sealed::Sealed + Sized {
    /// Occurrence identity stored on every node.
    type Id: fmt::Debug + Clone;

    /// Payload of [`ExprKind::Error`](ExprKind::Error).
    type Error: fmt::Debug + Clone;

    /// How a declaration reference names its target: the source definition
    /// in an IDE tree, a frame-relative [`LocalDecl`] in a complete tree.
    type DeclRef: fmt::Debug + Clone + PartialEq + Eq + Hash + Ord;

    /// How a unit reference names its unit: the source definition in an IDE
    /// tree, a frame-relative [`LocalUnit`] in a complete tree.
    type UnitRef: fmt::Debug + Clone + PartialEq + Eq;

    /// Expression children retained under an error node, in source order.
    fn error_children(error: &Self::Error) -> &[Expr<Self>];

    /// Mutable access to the children retained under an error node.
    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>];
}

/// The error-node payload of a complete tree: no value exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoErrorNode {}

impl NoErrorNode {
    /// Discharge the impossible error-node arm of a complete tree.
    #[must_use]
    pub const fn absurd<T>(self) -> T {
        match self {}
    }
}

/// Complete but unnumbered HIR: every reference resolved, no identities yet.
///
/// Produced by strict lowering and by programmatic construction of
/// already-resolved nodes; finishing a body numbers it into [`Strict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draft {}

impl sealed::Sealed for Draft {}

impl Completeness for Draft {
    type Id = ();
    type Error = NoErrorNode;
    type DeclRef = LocalDecl;
    type UnitRef = LocalUnit;

    fn error_children(error: &Self::Error) -> &[Expr<Self>] {
        error.absurd()
    }

    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>] {
        error.absurd()
    }
}

/// Finished HIR: complete, and every node carries an occurrence identity.
///
/// Produced only by finishing a [`Draft`] body, so identities are always
/// fresh and unique within their revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strict {}

impl sealed::Sealed for Strict {}

impl Completeness for Strict {
    type Id = ExprId;
    type Error = NoErrorNode;
    type DeclRef = LocalDecl;
    type UnitRef = LocalUnit;

    fn error_children(error: &Self::Error) -> &[Expr<Self>] {
        error.absurd()
    }

    fn error_children_mut(error: &mut Self::Error) -> &mut [Expr<Self>] {
        error.absurd()
    }
}

/// Stable lexical identity for a local expression binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub(in crate::hir) u32);

impl LocalId {
    /// Numeric index unique within one lowered expression tree.
    #[must_use]
    pub(crate) const fn index(self) -> u32 {
        self.0
    }
}

/// A lexical local binding introduced by an expression form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDef {
    pub id: LocalId,
    pub name: LocalName,
    pub span: Span,
}

/// HIR expression node. Identity-bearing clones cannot change their semantics.
///
/// ```compile_fail,E0616
/// use graphcal_compiler::hir::expr::{Draft, Expr, ExprKind};
/// use graphcal_compiler::syntax::span::Span;
/// let mut expr = Expr::<Draft>::new(ExprKind::Bool(true), Span::new(0, 4));
/// expr.kind = ExprKind::Bool(false);
/// ```
///
/// A [`Strict`] node carries an occurrence identity, so it cannot be built
/// directly; only finishing a [`Draft`] body assigns one:
///
/// ```compile_fail,E0599
/// use graphcal_compiler::hir::expr::{Expr, ExprKind, Strict};
/// use graphcal_compiler::syntax::span::Span;
/// let expr = Expr::<Strict>::new(ExprKind::Bool(true), Span::new(0, 4));
/// ```
#[derive(Debug)]
pub struct Expr<C: Completeness = Strict> {
    // Keep the identity/span handle small: the heterogeneous operation payload
    // must not be copied through every lowering and checking return value.
    pub(super) kind: Box<ExprKind<C>>,
    pub span: Span,
    pub(super) id: C::Id,
}

// Manual impl instead of `#[derive(Clone)]`: derived clone glue recurses
// once per tree level without any stack-growth guard, so cloning a long
// left-nested operator chain overflows the stack. Routing each level
// through `with_stack_growth` lets the stack grow on demand (the derived
// `ExprKind` clone calls back into this impl through `Box<Expr<C>>`).
impl<C: Completeness> Clone for Expr<C> {
    fn clone(&self) -> Self {
        crate::stack::with_stack_growth(|| Self {
            kind: self.kind.clone(),
            span: self.span,
            id: self.id.clone(),
        })
    }
}

impl<C: Completeness> Expr<C> {
    /// Inspect semantics without allowing an identity-bearing clone to be rewritten.
    #[must_use]
    pub const fn kind(&self) -> &ExprKind<C> {
        &self.kind
    }

    /// Consume the node for reconstruction. The identity, if any, is dropped.
    #[must_use]
    pub fn into_kind(self) -> ExprKind<C> {
        *self.kind
    }
}

impl<C: Completeness<Id = ()>> Expr<C> {
    /// Build an unnumbered node.
    #[must_use]
    pub fn new(kind: ExprKind<C>, span: Span) -> Self {
        Self {
            kind: Box::new(kind),
            span,
            id: (),
        }
    }
}

impl Expr<Strict> {
    /// Occurrence identity assigned when the body was finished, never derived
    /// from a span.
    #[must_use]
    pub const fn id(&self) -> &crate::expression_id::ExprId {
        &self.id
    }
}

/// A resolved index-variant reference with its source spans kept separate.
///
/// The index path and variant segment are not necessarily adjacent: table
/// desugaring can associate a bare row label with the axis in `table[...]`.
/// Qualified slices and heterogeneous headers can also repeat that same index
/// path, so additional index occurrences are retained for editor features.
/// Diagnostics on a contiguous primary `Index#Variant` path can still use
/// [`IndexVariantRef::path_span`], while span-precise consumers address each
/// written occurrence independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVariantRef {
    /// The resolved index variant.
    pub variant: ResolvedIndexVariant,
    /// Span of the index path as written (`Maneuver` in `Maneuver#Departure`,
    /// or the axis token inside `table[...]` for desugared table rows).
    /// `None` when the variant is written without an index segment (a bare
    /// label in a match pattern whose index is inferred).
    pub index_span: Option<Span>,
    /// Other written occurrences of the same index path introduced by table
    /// syntax, such as the axis in `table[...]` beside a qualified slice.
    pub additional_index_spans: Vec<Span>,
    /// Span of just the variant segment (the final path segment / row label).
    pub variant_span: Span,
}

impl IndexVariantRef {
    /// Iterate over every written occurrence of the resolved index path.
    pub fn index_spans(&self) -> impl Iterator<Item = Span> + '_ {
        self.index_span
            .into_iter()
            .chain(self.additional_index_spans.iter().copied())
    }

    /// Whole-reference span for diagnostics on a contiguous primary
    /// `Index#Variant` path. For desugared table rows the primary index span
    /// may live elsewhere, so prefer [`Self::variant_span`] there.
    #[must_use]
    pub fn path_span(&self) -> Span {
        self.index_span
            .map_or(self.variant_span, |index| index.merge(self.variant_span))
    }
}

/// A unit reference in an IDE tree after module-aware resolution.
///
/// `spelling` preserves the source alias for diagnostics and display labels;
/// `resolved` is the canonical definition identity, for navigation. IDE trees
/// are never run; a complete tree names units by
/// [`LocalUnit`](super::LocalUnit) handles instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitRef {
    spelling: SyntaxUnitRef,
    resolved: ResolvedUnitName,
}

impl ResolvedUnitRef {
    #[must_use]
    pub const fn new(spelling: SyntaxUnitRef, resolved: ResolvedUnitName) -> Self {
        Self { spelling, resolved }
    }

    /// Return the source spelling, including any module alias.
    #[must_use]
    pub const fn spelling(&self) -> &SyntaxUnitRef {
        &self.spelling
    }

    /// Return the canonical owner-qualified unit identity.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedUnitName {
        &self.resolved
    }
}

impl std::fmt::Display for ResolvedUnitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.spelling.fmt(f)
    }
}

/// One term in a module-resolved unit expression, naming its unit by `R`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitExprItem<R = super::LocalUnit> {
    pub op: ast::MulDivOp,
    pub name: Spanned<R>,
    /// Exact semantic exponent, normalized at the AST-to-HIR boundary.
    pub power: crate::dimension::Rational,
}

/// A unit expression whose terms retain canonical defining-module identities.
///
/// Each term names its unit by `R`: [`LocalUnit`](super::LocalUnit) handles
/// in a complete tree, [`ResolvedUnitRef`] definitions in an IDE tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitExpr<R = super::LocalUnit> {
    pub terms: Vec<ResolvedUnitExprItem<R>>,
    pub span: Span,
}

/// Resolved expression shape.
#[derive(Debug, Clone)]
pub enum ExprKind<C: Completeness = Strict> {
    /// A reference that failed to resolve.
    ///
    /// Representable only in tolerant trees, where the payload carries the
    /// diagnostic and every independently lowerable descendant expression for
    /// IDE references and additional diagnostics. In a [`Strict`] tree the
    /// payload is the uninhabited [`NoErrorNode`](super::NoErrorNode).
    Error(C::Error),
    Number(f64),
    Integer(i64),
    Bool(bool),
    StringLiteral(String),
    /// An offset-bearing instant parsed during HIR lowering.
    OffsetDateTimeLiteral(OffsetDateTimeLiteral),
    /// An offset/scale-free civil coordinate parsed during HIR lowering.
    CivilDateTimeLiteral(CivilDateTimeLiteral),
    /// A civil coordinate and timezone resolved to one unambiguous instant.
    ZonedDateTimeLiteral(ZonedDateTimeLiteral),
    /// An IANA timezone literal validated and canonicalized during HIR lowering.
    IanaTimeZoneLiteral(IanaTimeZoneId),
    TypeSystemRef(Spanned<TypeSystemRef>),
    GraphRef(Spanned<C::DeclRef>),
    ConstRef(Spanned<ConstRef<C::DeclRef>>),
    LocalRef(Spanned<LocalId>),
    BinOp {
        op: ast::BinOp,
        lhs: Box<Expr<C>>,
        rhs: Box<Expr<C>>,
    },
    UnaryOp {
        op: ast::UnaryOp,
        operand: Box<Expr<C>>,
    },
    FnCall {
        callee: Spanned<FunctionRef>,
        args: Vec<Expr<C>>,
    },
    If {
        condition: Box<Expr<C>>,
        then_branch: Box<Expr<C>>,
        else_branch: Box<Expr<C>>,
    },
    QuantityLiteral {
        value: f64,
        unit: ResolvedUnitExpr<C::UnitRef>,
    },
    Convert {
        expr: Box<Expr<C>>,
        target: ResolvedUnitExpr<C::UnitRef>,
    },
    DisplayTimezone {
        expr: Box<Expr<C>>,
        timezone: IanaTimeZoneId,
    },
    FieldAccess {
        expr: Box<Expr<C>>,
        field: Spanned<FieldName>,
    },
    ConstructorCall {
        callee: Spanned<ResolvedConstructorName>,
        generic_args: Vec<GenericArg>,
        fields: Vec<FieldInit<C>>,
    },
    MapLiteral {
        entries: Vec<MapEntry<C>>,
    },
    ForComp {
        bindings: Vec<ForBinding>,
        body: Box<Expr<C>>,
    },
    IndexAccess {
        expr: Box<Expr<C>>,
        args: NonEmpty<IndexArg<C>>,
    },
    Scan {
        source: Box<Expr<C>>,
        init: Box<Expr<C>>,
        acc: LocalDef,
        val: LocalDef,
        body: Box<Expr<C>>,
    },
    Unfold {
        recurrence: Box<UnfoldRecurrence>,
        init: Box<Expr<C>>,
        body: Box<Expr<C>>,
    },
    /// A key introduction form over a resolved axis:
    /// `key(Axis, spelling)`, `fin_key(Fin(N), e)`, or a coordinate search.
    KeyForm {
        kind: crate::syntax::ast::KeyFormKind,
        axis: ForBindingIndex,
        axis_span: Span,
        arg: Box<Expr<C>>,
    },
    Match {
        scrutinee: Box<Expr<C>>,
        arms: Vec<MatchArm<C>>,
    },
    VariantLiteral(IndexVariantRef),
    DagCall {
        target: Spanned<DagId>,
        args: Vec<ParamBinding<C>>,
        /// The call's Static bindings: the same canonical substitution an
        /// include applies to its template.
        static_bindings: crate::ir::static_substitution::StaticSubstitution,
        output: Spanned<ResolvedDeclName>,
    },
}

/// Resolved axis and lexical bindings for an `unfold` recurrence.
///
/// Boxed as one semantic unit inside [`ExprKind::Unfold`] to keep expression
/// variants compact while preserving the role of every binding.
#[derive(Debug, Clone)]
pub struct UnfoldRecurrence {
    pub axis: Spanned<ResolvedIndexName>,
    pub previous_state: LocalDef,
    pub previous_index: LocalDef,
    pub current_index: LocalDef,
}

/// Type-system identifier used as a value expression, usually in include bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSystemRef {
    Type(ResolvedStructTypeName),
    Dimension(ResolvedDimName),
    Index(ResolvedIndexName),
    IndexVariant(ResolvedIndexVariant),
}

impl TypeSystemRef {
    #[must_use]
    fn surface_description(&self) -> String {
        match self {
            Self::Type(name) => format!("type `{}`", name.as_str()),
            Self::Dimension(name) => format!("dimension `{}`", name.as_str()),
            Self::Index(name) => format!("index `{}`", name.as_str()),
            Self::IndexVariant(variant) => format!(
                "index label `{}#{}`",
                variant.index().as_str(),
                variant.variant()
            ),
        }
    }

    #[must_use]
    pub(crate) fn value_position_error(&self) -> String {
        format!("{} cannot be used as a value", self.surface_description())
    }
}

/// Resolved constant-like expression target.
///
/// `D` is how a declaration reference names its target, per
/// [`Completeness::DeclRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstRef<D = LocalDecl> {
    Decl(D),
    Constructor(ResolvedConstructorName),
    Builtin(BuiltinConst),
}

/// A resolved function callee before its generic arguments are applied.
///
/// This is the pre-application form: `epoch` is still bare here, and becomes
/// [`FunctionRef::Epoch`] only once its time-scale argument is lowered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnappliedFunctionRef {
    Builtin(BuiltinFn),
    /// An externally-provided function declared by an `import plugin` block.
    External(ExternFnRef),
}

impl std::fmt::Display for UnappliedFunctionRef {
    /// Render source-like function spelling at diagnostic boundaries.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin(name) => std::fmt::Display::fmt(name, f),
            Self::External(extern_ref) => std::fmt::Display::fmt(extern_ref, f),
        }
    }
}

/// Function call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionRef {
    Builtin(ScaleFreeBuiltin),
    /// `epoch<S>` with its required static time scale resolved at the HIR boundary.
    Epoch {
        scale: Spanned<TimeScale>,
    },
    /// An externally-provided function declared by an `import plugin` block.
    External(ExternFnRef),
}

impl FunctionRef {
    /// Return the built-in identity represented by this reference.
    #[must_use]
    pub const fn builtin(&self) -> Option<BuiltinFn> {
        match self {
            Self::Builtin(builtin) => Some(builtin.function()),
            Self::Epoch { .. } => Some(BuiltinFn::EPOCH),
            Self::External(_) => None,
        }
    }
}

impl std::fmt::Display for FunctionRef {
    /// Render source-like function spelling at diagnostic and display boundaries.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin(name) => std::fmt::Display::fmt(name, f),
            Self::Epoch { scale } => write!(f, "epoch<{}>", scale.value),
            Self::External(extern_ref) => std::fmt::Display::fmt(extern_ref, f),
        }
    }
}

/// A resolved reference to an extern (plugin) function.
///
/// Carries the canonical plugin identity plus the source alias the call was
/// qualified with. The alias is for diagnostics and IDE features; semantic
/// lookups key on [`ExternFnRef::key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternFnRef {
    /// Canonical plugin identity (the `import plugin "…"` path string).
    pub plugin: crate::plugin_identity::PluginIdentity,
    /// The module alias the call site was qualified with.
    pub alias: ModuleAliasName,
    /// The function leaf name.
    pub name: crate::syntax::function_name::FnName,
}

impl ExternFnRef {
    /// The canonical `(plugin, function)` lookup key.
    #[must_use]
    pub fn key(&self) -> crate::plugin_identity::ExternFnKey {
        crate::plugin_identity::ExternFnKey {
            plugin: self.plugin.clone(),
            name: self.name.clone(),
        }
    }
}

impl std::fmt::Display for ExternFnRef {
    /// Renders `alias::name` for diagnostics and display boundaries only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.alias, self.name)
    }
}

/// A lowered assertion body.
#[derive(Debug, Clone)]
pub enum AssertBody<C: Completeness = Strict> {
    Expr(Box<Expr<C>>),
    Tolerance {
        actual: Box<Expr<C>>,
        expected: Box<Expr<C>>,
        tolerance: Box<Expr<C>>,
    },
}

impl<C: Completeness> AssertBody<C> {
    /// Assertion operands have independent lexical scopes but one source revision.
    pub fn expressions(&self) -> impl Iterator<Item = &Expr<C>> {
        let operands: [Option<&Expr<C>>; 3] = match self {
            Self::Expr(expr) => [Some(expr), None, None],
            Self::Tolerance {
                actual,
                expected,
                tolerance,
            } => [Some(actual), Some(expected), Some(tolerance)],
        };
        operands.into_iter().flatten()
    }
}

/// Field initializer after expression lowering.
#[derive(Debug, Clone)]
pub struct FieldInit<C: Completeness = Strict> {
    pub name: Spanned<FieldName>,
    pub value: Expr<C>,
}

/// A named param binding in a DAG call expression.
#[derive(Debug, Clone)]
pub struct ParamBinding<C: Completeness = Strict> {
    pub target: Spanned<ResolvedDeclName>,
    pub value: Expr<C>,
}

/// A resolved map literal entry.
#[derive(Debug, Clone)]
pub struct MapEntry<C: Completeness = Strict> {
    pub keys: NonEmpty<MapEntryKey>,
    pub value: Expr<C>,
}

/// A single resolved map key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapEntryKey {
    IndexVariant(IndexVariantRef),
    FinitePosition { size: u64, position: Spanned<u64> },
}

/// A resolved for-comprehension binding.
#[derive(Debug, Clone)]
pub struct ForBinding {
    pub local: LocalDef,
    pub index: ForBindingIndex,
}

/// Index target in a for-comprehension binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForBindingIndex {
    Named(Spanned<ResolvedIndexName>),
    Finite {
        cardinality: Spanned<NatPolyForm>,
        span: Span,
    },
}

/// A resolved index-access argument.
#[derive(Debug, Clone)]
pub enum IndexArg<C: Completeness = Strict> {
    Variant(IndexVariantRef),
    Var(Spanned<LocalId>),
    Expr(Box<Expr<C>>),
}

/// One lowered match arm.
#[derive(Debug, Clone)]
pub struct MatchArm<C: Completeness = Strict> {
    pub pattern: MatchPattern,
    pub body: Expr<C>,
    pub span: Span,
}

/// Resolved match pattern.
#[derive(Debug, Clone)]
pub enum MatchPattern {
    Constructor {
        constructor: Spanned<ResolvedConstructorName>,
        bindings: ast::PatternBindings<PatternBinding>,
        span: Span,
    },
    IndexLabel {
        variant: IndexVariantRef,
        span: Span,
    },
}

impl MatchPattern {
    pub(in crate::hir) fn bound_locals(&self) -> Vec<LocalDef> {
        match self {
            Self::Constructor { bindings, .. } => bindings
                .as_slice()
                .iter()
                .filter_map(|binding| match binding {
                    PatternBinding::Bind { local, .. } => Some(local.clone()),
                    PatternBinding::Wildcard { .. } => None,
                })
                .collect(),
            Self::IndexLabel { .. } => Vec::new(),
        }
    }
}

/// Binding inside a constructor match pattern.
#[derive(Debug, Clone)]
pub enum PatternBinding {
    Bind {
        field: Spanned<FieldName>,
        local: LocalDef,
    },
    Wildcard {
        field: Spanned<FieldName>,
        span: Span,
    },
}
