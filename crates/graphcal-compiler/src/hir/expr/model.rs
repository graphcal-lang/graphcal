//! HIR expression tree: nodes, references, and binder payloads.

use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedIndexVariant, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::dimension::UnitRef as SyntaxUnitRef;
use std::collections::HashMap;

use crate::builtin::{BuiltinConst, BuiltinFn, ScaleFreeBuiltin};
use crate::dag_id::DagId;
use crate::datetime_literal::{CivilDateTimeLiteral, OffsetDateTimeLiteral, ZonedDateTimeLiteral};
use crate::desugar::desugared_ast as ast;
use crate::registry::time_scale::TimeScale;
use crate::registry::time_zone::IanaTimeZoneId;
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::FieldName;

use crate::hir::types::{GenericArg, NatExpr};

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
/// use graphcal_compiler::hir::expr::{Expr, ExprKind};
/// use graphcal_compiler::syntax::span::Span;
/// let mut expr = Expr::new(ExprKind::Bool(true), Span::new(0, 4));
/// expr.kind = ExprKind::Bool(false);
/// ```
#[derive(Debug)]
pub struct Expr {
    // Keep the identity/span handle small: the heterogeneous operation payload
    // must not be copied through every lowering and checking return value.
    pub(super) kind: Box<ExprKind>,
    pub span: Span,
    pub(super) id: Option<crate::expression_id::ExprId>,
}

// Manual impl instead of `#[derive(Clone)]`: derived clone glue recurses
// once per tree level without any stack-growth guard, so cloning a long
// left-nested operator chain overflows the stack. Routing each level
// through `with_stack_growth` lets the stack grow on demand (the derived
// `ExprKind` clone calls back into this impl through `Box<Expr>`).
impl Clone for Expr {
    fn clone(&self) -> Self {
        crate::stack::with_stack_growth(|| Self {
            kind: self.kind.clone(),
            span: self.span,
            id: self.id.clone(),
        })
    }
}

impl Expr {
    #[must_use]
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self {
            kind: Box::new(kind),
            span,
            id: None,
        }
    }

    /// Inspect semantics without allowing an identity-bearing clone to be rewritten.
    #[must_use]
    pub const fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Consume the node for reconstruction. `Expr::new` starts without an identity.
    #[must_use]
    pub fn into_kind(self) -> ExprKind {
        *self.kind
    }

    /// Identity is available after strict body lowering, never derived from a span.
    pub fn id(
        &self,
    ) -> Result<&crate::expression_id::ExprId, crate::expression_id::UnassignedExprId> {
        self.id
            .as_ref()
            .ok_or(crate::expression_id::UnassignedExprId)
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

/// A unit reference after module-aware resolution.
///
/// `spelling` preserves the source alias for diagnostics and display labels;
/// `resolved` is the canonical definition identity used by the compiler core.
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

/// One term in a module-resolved unit expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitExprItem {
    pub op: ast::MulDivOp,
    pub name: Spanned<ResolvedUnitRef>,
    /// Exact semantic exponent, normalized at the AST-to-HIR boundary.
    pub power: crate::dimension::Rational,
}

/// A unit expression whose terms retain canonical defining-module identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitExpr {
    pub terms: Vec<ResolvedUnitExprItem>,
    pub span: Span,
}

/// Resolved expression shape.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// A reference that failed to resolve.
    ///
    /// Produced only by tolerant lowering; the diagnostic for the failure is
    /// reported alongside the lowered tree. Independently lowerable descendant
    /// expressions are retained for IDE references and additional diagnostics.
    /// The strict entry points reject trees containing this node, so the batch
    /// pipeline never observes it.
    Error {
        children: Vec<Expr>,
    },
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
    GraphRef(Spanned<ResolvedDeclName>),
    ConstRef(Spanned<ConstRef>),
    LocalRef(Spanned<LocalId>),
    BinOp {
        op: ast::BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: ast::UnaryOp,
        operand: Box<Expr>,
    },
    FnCall {
        callee: Spanned<FunctionRef>,
        args: Vec<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    QuantityLiteral {
        value: f64,
        unit: ResolvedUnitExpr,
    },
    Convert {
        expr: Box<Expr>,
        target: ResolvedUnitExpr,
    },
    DisplayTimezone {
        expr: Box<Expr>,
        timezone: IanaTimeZoneId,
    },
    FieldAccess {
        expr: Box<Expr>,
        field: Spanned<FieldName>,
    },
    ConstructorCall {
        callee: Spanned<ResolvedConstructorName>,
        generic_args: Vec<GenericArg>,
        fields: Vec<FieldInit>,
    },
    MapLiteral {
        entries: Vec<MapEntry>,
    },
    ForComp {
        bindings: Vec<ForBinding>,
        body: Box<Expr>,
    },
    IndexAccess {
        expr: Box<Expr>,
        args: NonEmpty<IndexArg>,
    },
    Scan {
        source: Box<Expr>,
        init: Box<Expr>,
        acc: LocalDef,
        val: LocalDef,
        body: Box<Expr>,
    },
    Unfold {
        recurrence: Box<UnfoldRecurrence>,
        init: Box<Expr>,
        body: Box<Expr>,
    },
    /// A key introduction form over a resolved axis:
    /// `key(Axis, spelling)`, `fin_key(Fin(N), e)`, or a coordinate search.
    KeyForm {
        kind: crate::syntax::ast::KeyFormKind,
        axis: ForBindingIndex,
        axis_span: Span,
        arg: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    VariantLiteral(IndexVariantRef),
    DagCall {
        target: Spanned<DagId>,
        args: Vec<ParamBinding>,
        static_bindings: DagCallStaticBindings,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstRef {
    Decl(ResolvedDeclName),
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
pub enum AssertBody {
    Expr(Box<Expr>),
    Tolerance {
        actual: Box<Expr>,
        expected: Box<Expr>,
        tolerance: Box<Expr>,
    },
}

impl AssertBody {
    /// Assertion operands have independent lexical scopes but one source revision.
    pub fn expressions(&self) -> impl Iterator<Item = &Expr> {
        let operands: [Option<&Expr>; 3] = match self {
            Self::Expr(expr) => [Some(expr), None, None],
            Self::Tolerance {
                actual,
                expected,
                tolerance,
            } => [Some(actual), Some(expected), Some(tolerance)],
        };
        operands.into_iter().flatten()
    }

    pub(super) fn expressions_mut(&mut self) -> impl Iterator<Item = &mut Expr> {
        let operands: [Option<&mut Expr>; 3] = match self {
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
pub struct FieldInit {
    pub name: Spanned<FieldName>,
    pub value: Expr,
}

/// A named param binding in a DAG call expression.
#[derive(Debug, Clone)]
pub struct ParamBinding {
    pub target: Spanned<ResolvedDeclName>,
    pub value: Expr,
}

/// Canonical Static substitutions supplied by one direct DAG call.
#[derive(Debug, Clone, Default)]
pub struct DagCallStaticBindings {
    pub types: HashMap<ResolvedStructTypeName, ResolvedStructTypeName>,
    pub dimensions: HashMap<ResolvedDimName, ResolvedDimName>,
    pub indexes: HashMap<ResolvedIndexName, DagCallIndexBinding>,
}

/// Concrete index target supplied to a direct DAG call.
#[derive(Debug, Clone)]
pub enum DagCallIndexBinding {
    Declared(ResolvedIndexName),
    Finite(crate::registry::types::FiniteIndex),
}

/// A resolved map literal entry.
#[derive(Debug, Clone)]
pub struct MapEntry {
    pub keys: NonEmpty<MapEntryKey>,
    pub value: Expr,
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
    Finite { cardinality: NatExpr, span: Span },
}

/// A resolved index-access argument.
#[derive(Debug, Clone)]
pub enum IndexArg {
    Variant(IndexVariantRef),
    Var(Spanned<LocalId>),
    Expr(Box<Expr>),
}

/// One lowered match arm.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Expr,
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
