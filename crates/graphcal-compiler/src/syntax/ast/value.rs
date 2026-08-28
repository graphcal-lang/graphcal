use std::marker::PhantomData;

use crate::dimension::Rational;
use crate::exact_rational::ExactRational;
use crate::syntax::ast::common::{Ident, ModulePath};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::UnitRef;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::{AtLeastTwo, NonEmpty};
use crate::syntax::phase::{Phase, Raw};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::token::ContextualKeyword;
use crate::syntax::type_name::{ConstructorName, FieldName};

/// Expression-level sugar — only legal in [`Raw`].
///
/// Each variant corresponds to a surface expression form that is rewritten
/// into ordinary `ExprKind` variants by [`crate::desugar::convert`]. In
/// `Desugared`, the `Sugar` slot is `Infallible` and these variants vanish.
#[derive(Debug, Clone)]
pub enum RawExprSugar {
    /// Table literal: `table[Phase, Fin(3)] { ... }`.
    ///
    /// Desugars to [`ExprKind::MapLiteral`] — the `indexes` metadata is
    /// dropped (entries already carry full `Index#Variant` keys), and the
    /// `table` keyword is purely surface syntax preserved by the formatter
    /// via the raw AST.
    TableLiteral {
        indexes: Vec<TableIndexSpec>,
        entries: Vec<MapEntry<Raw>>,
    },
}

// ---------------------------------------------------------------------------
// Unresolved-ref variants (the only reference form in the syntax AST)
// ---------------------------------------------------------------------------

/// An unresolved expression reference whose namespace is selected by syntax.
///
/// A plain path is a Term reference. `Index#Label` is represented separately,
/// so HIR lowering never probes Static and Term to decide what a dot meant.
#[derive(Debug, Clone)]
pub enum UnresolvedRef {
    /// A bare Term name or a Term member selected after `::`.
    Path(IdentPath),
    /// An owner-qualified index label selected by `#`.
    IndexLabel {
        index: IdentPath,
        label: Spanned<IndexVariantName>,
        span: Span,
    },
}

/// A span-aware local/member name in expression position.
///
/// Dotted namespace-owner segments and the member after `::` are separate
/// fields. Consequently this type cannot encode the old ambiguous `a.b`
/// convention.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentPath {
    owner: Option<NonEmpty<Ident>>,
    member: Ident,
}

impl IdentPath {
    /// Construct from a segment sequence at typed construction boundaries. One
    /// segment is local; two or more treat the final segment as the member.
    #[must_use]
    pub fn new(segments: NonEmpty<Ident>) -> Self {
        let mut segments = segments.into_vec();
        let member = match segments.pop() {
            Some(member) => member,
            None => unreachable!("NonEmpty always contains a member identifier"),
        };
        match NonEmpty::try_from_vec(segments) {
            Ok(owner) => Self::member(owner, member),
            Err(_) => Self::bare(member),
        }
    }

    /// Construct a one-segment local name.
    #[must_use]
    pub const fn bare(member: Ident) -> Self {
        Self {
            owner: None,
            member,
        }
    }

    /// Construct a member selected after a dotted namespace owner and `::`.
    #[must_use]
    pub const fn member(owner: NonEmpty<Ident>, member: Ident) -> Self {
        Self {
            owner: Some(owner),
            member,
        }
    }

    /// Dotted namespace-owner segments before `::`.
    #[must_use]
    pub fn owner_segments(&self) -> Option<&[Ident]> {
        self.owner.as_ref().map(NonEmpty::as_slice)
    }

    /// Consume and return all identifier segments, dropping punctuation only
    /// at this explicit compatibility boundary.
    #[must_use]
    pub fn into_segments(self) -> NonEmpty<Ident> {
        match self.owner {
            Some(owner) => {
                let mut segments = owner.into_vec();
                segments.push(self.member);
                match NonEmpty::try_from_vec(segments) {
                    Ok(segments) => segments,
                    Err(_) => unreachable!("an IdentPath always contains its member"),
                }
            }
            None => NonEmpty::singleton(self.member),
        }
    }

    /// Consume and return all identifier segments.
    #[must_use]
    pub fn into_vec(self) -> Vec<Ident> {
        self.into_segments().into_vec()
    }

    /// Number of source name segments. Always at least one.
    #[must_use]
    pub const fn len(&self) -> usize {
        match &self.owner {
            Some(owner) => owner.len() + 1,
            None => 1,
        }
    }

    /// Returns `false`; provided for sequence-like APIs.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Returns whether this is a local name with no member boundary.
    #[must_use]
    const fn is_bare(&self) -> bool {
        self.owner.is_none()
    }

    /// Source span covering the namespace owner, `::`, and selected member.
    #[must_use]
    pub(crate) fn span(&self) -> Span {
        self.owner.as_ref().map_or(self.member.span, |owner| {
            owner.first().span.merge(self.member.span)
        })
    }

    /// Drop per-segment spans while preserving the `::` boundary.
    #[must_use]
    pub(crate) fn to_name_path(&self) -> crate::syntax::names::NamePath {
        self.owner.as_ref().map_or_else(
            || NamePath::local(self.member.name.clone()),
            |owner| {
                NamePath::member(
                    crate::syntax::names::NamespacePath::new(owner.clone().map(|ident| ident.name)),
                    self.member.name.clone(),
                )
            },
        )
    }

    /// Selected local/member identifier.
    #[must_use]
    pub const fn leaf(&self) -> &Ident {
        &self.member
    }

    /// Split owner segments and selected member at a compatibility boundary.
    #[must_use]
    pub(crate) fn split_last(&self) -> (&[Ident], &Ident) {
        (
            self.owner
                .as_ref()
                .map_or(&[] as &[Ident], NonEmpty::as_slice),
            &self.member,
        )
    }

    /// Owner segments before `::`. Empty for local names.
    #[must_use]
    pub fn qualifier_segments(&self) -> &[Ident] {
        self.split_last().0
    }

    /// Owner segments and selected member only for a member path.
    #[must_use]
    pub fn qualifier_and_leaf(&self) -> Option<(&[Ident], &Ident)> {
        self.owner
            .as_ref()
            .map(|owner| (owner.as_slice(), &self.member))
    }

    /// Selected identifier only when this is a local path.
    #[must_use]
    pub const fn as_bare(&self) -> Option<&Ident> {
        match &self.owner {
            None => Some(&self.member),
            Some(_) => None,
        }
    }

    /// Mutably return the selected identifier only for a local path.
    pub(crate) fn as_bare_mut(&mut self) -> Option<&mut Ident> {
        match &self.owner {
            None => Some(&mut self.member),
            Some(_) => None,
        }
    }

    /// Consume this path and return its identifier when local.
    pub(crate) fn into_bare(self) -> Result<Ident, Self> {
        if self.is_bare() {
            Ok(self.member)
        } else {
            Err(self)
        }
    }

    /// Convert this spanned syntax path into a span-less [`NamePath`].
    #[must_use]
    fn into_name_path(self) -> NamePath {
        let member = self.member.name;
        match self.owner {
            Some(owner) => NamePath::member(
                crate::syntax::names::NamespacePath::new(owner.map(|ident| ident.name)),
                member,
            ),
            None => NamePath::local(member),
        }
    }

    /// Pair the span-less path with its full source span.
    #[must_use]
    pub(crate) fn into_spanned_name_path(self) -> Spanned<NamePath> {
        let span = self.span();
        Spanned::new(self.into_name_path(), span)
    }

    /// Human-readable source spelling for diagnostics and formatting.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.owner.as_ref().map_or_else(
            || self.member.name.to_string(),
            |owner| {
                format!(
                    "{}::{}",
                    owner
                        .iter()
                        .map(|segment| segment.name.as_str())
                        .collect::<Vec<_>>()
                        .join("."),
                    self.member.name
                )
            },
        )
    }
}

impl From<Ident> for IdentPath {
    fn from(ident: Ident) -> Self {
        Self::bare(ident)
    }
}

impl std::fmt::Display for IdentPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display_path())
    }
}

impl UnresolvedRef {
    /// Source span of the complete syntax-directed reference.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Path(path) => path.span(),
            Self::IndexLabel { span, .. } => *span,
        }
    }
}
/// Source category of one DAG input binding.
///
/// Unmarked means exactly a Term parameter. There is intentionally no `param`
/// marker variant; Static inputs require their explicit marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputBindingCategory {
    Unmarked,
    Type,
    Dimension,
    Index,
}

/// One categorized input binding in an `include` or direct DAG invocation.
#[derive(Debug, Clone)]
pub struct ParamBinding<P: Phase = Raw> {
    pub category: InputBindingCategory,
    /// Target name in the invoked DAG's selected namespace.
    pub name: Ident,
    /// Authored binding value. The category fixes how this syntax is validated;
    /// resolution never retries a different target namespace.
    pub value: Expr<P>,
    /// Span covering the marker (when present), name, and value.
    pub(crate) span: Span,
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
    pub(crate) kind_span: Span,
    /// The bound value expression.
    pub value: Expr<P>,
    pub(crate) span: Span,
}

/// An expression in index position of an indexed type.
///
/// The closed set of key introduction forms.
///
/// Shared by the AST and HIR so downstream phases dispatch on the typed kind
/// rather than a source spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFormKind {
    /// `key(Axis, spelling)` — static, compile-time membership-checked.
    Static,
    /// `fin_key(Fin(N), e)` — runtime range-checked position key.
    Fin,
    /// `floor_key(C, q)` — last coordinate at or before `q` (fallible).
    Floor,
    /// `ceil_key(C, q)` — first coordinate at or after `q` (fallible).
    Ceil,
    /// `nearest_key(C, q)` — closest coordinate, ties toward the axis start.
    Nearest,
}

impl KeyFormKind {
    /// Canonical source spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "key",
            Self::Fin => "fin_key",
            Self::Floor => "floor_key",
            Self::Ceil => "ceil_key",
            Self::Nearest => "nearest_key",
        }
    }
}

/// In `Velocity[Maneuver]` or `Velocity[module.Maneuver]`, the index path is
/// an [`IndexExpr::Name`]. Structural indexes use the explicit
/// [`IndexExpr::Finite`] constructor, as in `Dimensionless[Fin(N)]`.
#[derive(Debug, Clone)]
pub enum IndexExpr {
    /// A named index or generic Index parameter path: `Maneuver`, `I`, `module.Maneuver`.
    Name(Spanned<NamePath>),
    /// The built-in finite structural-index constructor `Fin(N)`.
    Finite { cardinality: NatExpr, span: Span },
    /// A bare Nat written in an Index slot. Retained only so semantic lowering
    /// can issue the targeted `Fin(...)` migration diagnostic; it is never
    /// accepted as an index.
    BareNat(NatExpr),
}

impl IndexExpr {
    /// Get the source span of this index expression.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Name(name) => name.span,
            Self::Finite { span, .. } => *span,
            Self::BareNat(nat_expr) => nat_expr.span(),
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
    pub(crate) span: Span,
}

impl<P: Phase> TypeExpr<P> {
    /// The domain bounds attached to this type expression.
    ///
    /// Bounds on an indexed type (`Velocity[Maneuver](min: 0.0 m/s)`) are
    /// parsed onto the base type expression, so this looks through one
    /// `Indexed` wrapper when the outer expression carries none.
    #[must_use]
    pub(crate) fn domain_bounds(&self) -> &[DomainBound<P>] {
        if !self.constraints.is_empty() {
            return &self.constraints;
        }
        match &self.kind {
            TypeExprKind::Indexed { base, .. } => &base.constraints,
            _ => &[],
        }
    }
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
    DatetimeApplication { type_args: NonEmpty<TypeExpr<P>> },
    /// `Complex<D>` — built-in complex quantity type parameterized by a dimension.
    ///
    /// Kept separate from [`Self::TypeApplication`] so downstream phases carry
    /// the built-in semantic identity instead of matching the source name.
    ComplexApplication { generic_args: Vec<GenericArg<P>> },
    /// `Key<I>` — built-in index-key type parameterized by an index axis.
    ///
    /// Kept separate from [`Self::TypeApplication`] so downstream phases carry
    /// the built-in semantic identity instead of matching the source name.
    KeyApplication { generic_args: Vec<GenericArg<P>> },
    /// An index label written in a type slot, such as `Phase#Launch`.
    ///
    /// The parser preserves this structured reference so HIR can issue a
    /// namespace/category diagnostic rather than treating `#` as punctuation
    /// that terminates an otherwise valid type expression.
    IndexLabel {
        index: IdentPath,
        label: Spanned<IndexVariantName>,
    },
    /// A dimension expression like `Length`, `Length^2`, `Mass * Length / Time^2`
    DimExpr(DimExpr),
    /// An indexed type such as `Velocity[Maneuver]`, `Dimensionless[Fin(3)]`, or `D[I]`
    Indexed {
        base: Box<TypeExpr<P>>,
        indexes: NonEmpty<IndexExpr>,
    },
    /// A user-defined generic type application like `Vec3<Length, ECI>` or
    /// `FixedVec<3>`.
    /// Built-in parameterized types have their own variants instead — see
    /// [`Self::DatetimeApplication`] and [`Self::ComplexApplication`]. Generic
    /// arguments remain syntax-level and unsorted until the referenced type's
    /// parameter constraints are known.
    TypeApplication {
        name: Spanned<NamePath>,
        generic_args: NonEmpty<GenericArg<P>>,
    },
}

/// A dimension expression: product/quotient of dimension terms.
/// E.g., `Length^3 / Time^2`
#[derive(Debug, Clone)]
pub struct DimExpr {
    pub terms: Vec<DimExprItem>,
    pub(crate) span: Span,
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
    /// Source-written exponent; `None` preserves omission for formatting.
    pub power: Option<Rational>,
    pub span: Span,
}

impl DimTerm {
    /// Return the semantic exponent, normalizing an omitted suffix to one.
    #[must_use]
    pub fn effective_power(&self) -> Rational {
        effective_power(self.power)
    }
}

fn effective_power(source_power: Option<Rational>) -> Rational {
    source_power.unwrap_or(Rational::ONE)
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
    pub name: Spanned<UnitRef>,
    /// Source-written exponent; `None` preserves omission for formatting.
    pub power: Option<Rational>,
}

impl UnitExprItem {
    /// Return the semantic exponent, normalizing an omitted suffix to one.
    #[must_use]
    pub fn effective_power(&self) -> Rational {
        effective_power(self.power)
    }
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
#[derive(Debug)]
pub struct Expr<P: Phase = Raw> {
    pub kind: ExprKind<P>,
    pub span: Span,
    // Marker forcing a concrete (non-recursive) use of `P` so the compiler
    // can determine variance for `Expr<P>` and, transitively, every type
    // that contains `Expr<P>`. Private so callers must use `Expr::new` —
    // that keeps the phase marker out of their sight entirely.
    _phase: PhantomData<fn() -> P>,
}

// Manual impl instead of `#[derive(Clone)]`: derived clone glue recurses
// once per tree level without any stack-growth guard, so cloning a long
// left-nested operator chain overflows the stack. Routing each level
// through `with_stack_growth` lets the stack grow on demand (the derived
// `ExprKind` clone calls back into this impl through `Box<Expr<P>>`).
impl<P: Phase> Clone for Expr<P>
where
    ExprKind<P>: Clone,
{
    fn clone(&self) -> Self {
        crate::stack::with_stack_growth(|| Self {
            kind: self.kind.clone(),
            span: self.span,
            _phase: PhantomData,
        })
    }
}

impl<P: Phase> Drop for Expr<P> {
    fn drop(&mut self) {
        // Move `kind` out under a non-recursive placeholder so Rust drops only
        // the placeholder after this impl returns. Recursive child drops then
        // run under the stack-growth guard.
        let kind = self.take_kind();
        crate::stack::with_stack_growth(|| drop(kind));
    }
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

    /// Consume this expression into its kind and span without cloning the
    /// recursive expression tree.
    ///
    /// The recursive kind is replaced with a non-recursive placeholder before
    /// this expression's custom [`Drop`] implementation runs.
    #[must_use]
    pub fn into_parts(mut self) -> (ExprKind<P>, Span) {
        let kind = self.take_kind();
        (kind, self.span)
    }

    const fn take_kind(&mut self) -> ExprKind<P> {
        std::mem::replace(&mut self.kind, ExprKind::Number(0.0))
    }

    /// Reclassify an include-binding RHS that has index-argument syntax.
    ///
    /// Include bindings are parsed uniformly as expressions because the target
    /// namespace is known only after the dependency is loaded. This adapter is
    /// the syntax boundary where a bare path or contextual `Fin(nat_expr)` call
    /// regains its typed [`IndexExpr`] shape before semantic binding
    /// classification.
    #[must_use]
    pub fn index_binding_arg(&self) -> Option<IndexExpr> {
        match &self.kind {
            ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => {
                Some(IndexExpr::Name(path.clone().into_spanned_name_path()))
            }
            ExprKind::FnCall {
                callee,
                generic_args,
                args,
            } => {
                let constructor = callee.as_bare()?;
                if !ContextualKeyword::Fin.matches(constructor.name.as_str())
                    || !generic_args.is_empty()
                {
                    return None;
                }
                let [cardinality] = args.as_slice() else {
                    return None;
                };
                Some(IndexExpr::Finite {
                    cardinality: nat_expr_from_binding_expr(cardinality)?,
                    span: self.span,
                })
            }
            ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } if generic_args.is_empty() && fields.is_empty() => {
                let constructor = callee.as_bare()?;
                (!ContextualKeyword::Fin.matches(constructor.name.as_str()))
                    .then(|| IndexExpr::Name(callee.clone().into_spanned_name_path()))
            }
            _ => None,
        }
    }
}

fn nat_expr_from_binding_expr<P: Phase>(expr: &Expr<P>) -> Option<NatExpr> {
    match &expr.kind {
        ExprKind::Integer(value) => u64::try_from(*value)
            .ok()
            .map(|value| NatExpr::Literal(value, expr.span)),
        ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => {
            path.as_bare().cloned().map(NatExpr::Var)
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let lhs = nat_expr_from_binding_expr(lhs)?;
            let rhs = nat_expr_from_binding_expr(rhs)?;
            match op {
                BinOp::Add => Some(NatExpr::add(lhs, rhs, expr.span)),
                BinOp::Mul => Some(NatExpr::mul(lhs, rhs, expr.span)),
                BinOp::Sub
                | BinOp::Div
                | BinOp::Mod
                | BinOp::Pow(_)
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => None,
            }
        }
        ExprKind::UnresolvedRef(UnresolvedRef::IndexLabel { .. })
        | ExprKind::Number(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::GraphRef(_)
        | ExprKind::UnaryOp { .. }
        | ExprKind::FnCall { .. }
        | ExprKind::If { .. }
        | ExprKind::QuantityLiteral { .. }
        | ExprKind::Convert { .. }
        | ExprKind::DisplayTimezone { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::ConstructorCall { .. }
        | ExprKind::MapLiteral { .. }
        | ExprKind::ForComp { .. }
        | ExprKind::IndexAccess { .. }
        | ExprKind::Scan { .. }
        | ExprKind::Unfold { .. }
        | ExprKind::KeyForm { .. }
        | ExprKind::Match { .. }
        | ExprKind::InlineDagRef { .. }
        | ExprKind::Sugar(_) => None,
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
    /// String literal: `"hello"` (classified contextually during HIR lowering).
    StringLiteral(String),
    /// Graph reference: `@name` or `@alias.member`. The payload encodes
    /// qualification structurally — `Local` for bare `@name`, `Qualified`
    /// for `@alias.member` (after the namespace-alias rewrite). Producers
    /// never invent or interpret a flat-string separator.
    GraphRef(Spanned<ScopedName>),
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
        generic_args: Vec<GenericArg<P>>,
        args: Vec<Expr<P>>,
    },
    /// Conditional: `if cond { then_expr } else { else_expr }`
    If {
        condition: Box<Expr<P>>,
        then_branch: Box<Expr<P>>,
        else_branch: Box<Expr<P>>,
    },
    /// Quantity literal: `400 km`, `9.80665 m/s^2`
    QuantityLiteral { value: f64, unit: UnitExpr },
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
    /// Map literal: `{ Maneuver#Departure: 2.46 km/s, Maneuver#Correction: 0.05 km/s }`
    MapLiteral { entries: Vec<MapEntry<P>> },
    /// For comprehension: `for m: Maneuver { @delta_v[m] + 1.0 }`
    ForComp {
        bindings: Vec<ForBinding>,
        body: Box<Expr<P>>,
    },
    /// Index access: `@delta_v[m]`, `@delta_v[Maneuver#Departure]`, `@P[a, b]`
    IndexAccess {
        expr: Box<Expr<P>>,
        args: NonEmpty<IndexArg<P>>,
    },
    /// Scan: `scan(source, init, |acc, item| body)`
    Scan {
        source: Box<Expr<P>>,
        init: Box<Expr<P>>,
        acc_name: Spanned<LocalName>,
        val_name: Spanned<LocalName>,
        body: Box<Expr<P>>,
    },
    /// Unfold: `unfold(axis, init, |prev_state, prev_i, i| body)`
    ///
    /// Generates an indexed value from a seed over an explicit coordinate
    /// index. The closure receives the previous state and the previous/current
    /// coordinate labels.
    Unfold {
        axis: Spanned<NamePath>,
        init: Box<Expr<P>>,
        prev_state_name: Spanned<LocalName>,
        prev_index_name: Spanned<LocalName>,
        index_name: Spanned<LocalName>,
        body: Box<Expr<P>>,
    },
    /// A key introduction form: `key(Axis, spelling)`, `fin_key(Fin(N), e)`,
    /// `floor_key(C, q)`, `ceil_key(C, q)`, or `nearest_key(C, q)`.
    ///
    /// The axis argument is an index reference (a named path or `Fin(N)`),
    /// not an expression, so these are special forms following the `unfold`
    /// precedent rather than ordinary function calls.
    KeyForm {
        kind: KeyFormKind,
        axis: IndexExpr,
        arg: Box<Expr<P>>,
    },
    /// Match expression: `match @status { Nominal => ..., Warning(message: code) => ... }`
    Match {
        scrutinee: Box<Expr<P>>,
        arms: Vec<MatchArm<P>>,
    },
    /// Inline DAG invocation: `@dag(args)::out` or `@module.dag(args)::out`.
    ///
    /// Each syntactic occurrence denotes a fresh DAG instantiation that is
    /// desugared during TIR lowering to the equivalent
    /// `include <path>(args) as <synthetic>; @<synthetic>.out`. Preserved as
    /// a distinct AST variant so source spans survive for diagnostics.
    ///
    /// The post-`@` expression as a whole must denote a graph value.
    /// `@dag(args)::out` may project an explicitly exported node or the
    /// effective bound/default value of a param input port. The qualified
    /// `@module.dag(args)::out` form has the same rule. Bare `@dag(args)` is
    /// rejected because an unprojected DAG instance is not a graph value.
    InlineDagRef {
        /// Path to the DAG module being invoked. The first segment may name a
        /// local DAG or an imported module alias. An imported alias is itself
        /// callable (`@module(args)::out`) whether it targets a file root or an
        /// inline DAG; additional segments descend through child DAG modules
        /// (`@module.child(args)::out`).
        path: ModulePath,
        /// Param/index bindings, same shape as `include` bindings.
        args: Vec<ParamBinding<P>>,
        /// Projected output value name (after the closing paren `.`).
        output: Spanned<DeclName>,
    },
    /// Unresolved reference produced by the parser.
    ///
    /// Carries an unresolved identifier path. HIR expression lowering is the
    /// single stage that classifies and resolves these paths.
    UnresolvedRef(UnresolvedRef),
    /// Phase-specific expression sugar.
    ///
    /// In [`Raw`], this is [`crate::syntax::ast::RawExprSugar`] and carries
    /// surface forms like `TableLiteral` that are eliminated by the desugar
    /// pass. In `Desugared`, the payload is [`core::convert::Infallible`] —
    /// the variant is statically unreachable.
    Sugar(P::ExprSugar),
}

/// An index specification in a table literal's bracket list:
/// `table[Phase, Fin(3)]`.
#[derive(Debug, Clone)]
pub enum TableIndexSpec {
    /// A named index: `Phase`, `Maneuver`, or `module.Maneuver`.
    Named(Spanned<NamePath>),
    /// An explicit finite structural index: `Fin(3)`.
    Finite { cardinality: u64, span: Span },
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
    const fn new(slice_axes: Vec<TableIndexSpec>, row_axis: TableIndexSpec) -> Self {
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
    pub(crate) fn try_from_vec(
        mut axes: Vec<TableIndexSpec>,
    ) -> Result<Self, crate::syntax::non_empty::EmptyVecError> {
        let row_axis = axes.pop().ok_or(crate::syntax::non_empty::EmptyVecError)?;
        Ok(Self::new(axes, row_axis))
    }

    /// Slice axes preceding the row axis.
    #[must_use]
    pub(crate) fn slice_axes(&self) -> &[TableIndexSpec] {
        &self.slice_axes
    }

    /// The row axis.
    #[must_use]
    pub const fn row_axis(&self) -> &TableIndexSpec {
        &self.row_axis
    }

    /// Number of shared axes. Always at least 1.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn len(&self) -> usize {
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
/// Plain map literals use named indexes. Tables over `Fin(N)` axes desugar to
/// map entries with an explicitly typed structural key, so downstream passes
/// never recover index structure from a fabricated name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapEntryIndex {
    /// A declared named index.
    Named(NamePath),
    /// A finite structural index, `Fin(N)`.
    Finite(u64),
}

impl From<IndexName> for MapEntryIndex {
    fn from(value: IndexName) -> Self {
        Self::Named(NamePath::local(value.into_atom()))
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
            Self::Finite(size) => write!(f, "Fin({size})"),
        }
    }
}

impl TableIndexSpec {
    /// Get the source span of this table index specification.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Named(spanned) => spanned.span,
            Self::Finite { span, .. } => *span,
        }
    }

    /// Returns `true` if this is a structural `Fin(N)` index.
    #[must_use]
    pub const fn is_finite_index(&self) -> bool {
        matches!(self, Self::Finite { .. })
    }
}

/// A single key in a map literal entry: `Index#Variant`.
///
/// Table sugar can mention the same semantic index more than once for one
/// desugared key: once in `table[...]` and again in a qualified slice or
/// heterogeneous header label. `additional_index_spans` preserves those
/// source references for editor features without duplicating the key value.
#[derive(Debug, Clone)]
pub struct MapEntryKey {
    pub index: Spanned<MapEntryIndex>,
    pub additional_index_spans: Vec<Span>,
    pub variant: Spanned<IndexEntryKey>,
}

/// An entry in a map literal.
///
/// Single-axis: `Maneuver#Departure: 2.46 km/s` (keys has 1 element)
/// Multi-axis:  `(Phase#Launch, Maneuver#Departure): 2.46 km/s` (keys has 2+ elements)
#[derive(Debug, Clone)]
pub struct MapEntry<P: Phase = Raw> {
    pub keys: NonEmpty<MapEntryKey>,
    pub value: Expr<P>,
}

/// A binding in a `for` comprehension: `m: Maneuver` or `i: Fin(3)`
#[derive(Debug, Clone)]
pub struct ForBinding {
    pub var: Spanned<LocalName>,
    pub index: ForBindingIndex,
}

/// The index in a for binding: either a named index or `Fin(N)`.
#[derive(Debug, Clone)]
pub enum ForBindingIndex {
    /// A named index: `for m: Maneuver { ... }` or `for m: module.Maneuver { ... }`.
    Named(Spanned<NamePath>),
    /// An explicit structural index: `for i: Fin(N) { ... }`.
    Finite { cardinality: NatExpr, span: Span },
}

/// A Nat expression (type-level natural number).
///
/// Supports literals, variables, addition (Level 1), and multiplication (Level 2).
/// Operator chains use flat, two-or-more operand lists so ordinary long source
/// chains do not create recursively cloned, formatted, compared, or dropped trees.
#[derive(Debug, Clone)]
pub enum NatExpr {
    /// An integer literal, e.g., `3`
    Literal(u64, Span),
    /// A variable (generic Nat parameter), e.g., `N`
    Var(Ident),
    /// Addition of two or more Nat expressions, e.g., `N + 1`, `M + N + 1`.
    Add(AtLeastTwo<Self>, Span),
    /// Multiplication of two or more Nat expressions, e.g., `N * 3`, `M * N * 2`.
    Mul(AtLeastTwo<Self>, Span),
}

impl NatExpr {
    /// Construct an addition, extending an existing left-hand addition instead
    /// of creating a left-deep tree.
    #[must_use]
    pub fn add(lhs: Self, rhs: Self, span: Span) -> Self {
        match lhs {
            Self::Add(mut operands, _) => {
                operands.push(rhs);
                Self::Add(operands, span)
            }
            lhs => Self::Add(AtLeastTwo::new(lhs, rhs), span),
        }
    }

    /// Construct a multiplication, extending an existing left-hand product
    /// instead of creating a left-deep tree.
    #[must_use]
    pub fn mul(lhs: Self, rhs: Self, span: Span) -> Self {
        match lhs {
            Self::Mul(mut operands, _) => {
                operands.push(rhs);
                Self::Mul(operands, span)
            }
            lhs => Self::Mul(AtLeastTwo::new(lhs, rhs), span),
        }
    }

    /// Get the source span.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Literal(_, span) | Self::Add(_, span) | Self::Mul(_, span) => *span,
            Self::Var(ident) => ident.span,
        }
    }

    const fn precedence(&self) -> u8 {
        match self {
            Self::Add(..) => 1,
            Self::Mul(..) => 2,
            Self::Literal(..) | Self::Var(_) => 3,
        }
    }

    fn fmt_operands(
        operands: &AtLeastTwo<Self>,
        separator: &str,
        precedence: u8,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        for (index, operand) in operands.iter().enumerate() {
            if index > 0 {
                f.write_str(separator)?;
            }
            operand.fmt_with_min_precedence(f, precedence)?;
        }
        Ok(())
    }

    fn fmt_with_min_precedence(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        min_precedence: u8,
    ) -> std::fmt::Result {
        let precedence = self.precedence();
        let needs_parens = precedence < min_precedence;
        if needs_parens {
            f.write_str("(")?;
        }
        match self {
            Self::Literal(n, _) => write!(f, "{n}")?,
            Self::Var(ident) => f.write_str(&ident.name)?,
            Self::Add(operands, _) => {
                Self::fmt_operands(operands, " + ", precedence, f)?;
            }
            Self::Mul(operands, _) => {
                Self::fmt_operands(operands, " * ", precedence, f)?;
            }
        }
        if needs_parens {
            f.write_str(")")?;
        }
        Ok(())
    }
}

impl std::fmt::Display for NatExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_with_min_precedence(f, 0)
    }
}

#[cfg(test)]
mod nat_expr_display_tests {
    use super::*;

    #[test]
    fn display_parenthesizes_addition_under_multiplication() {
        let span = Span::new(0, 0);
        let expr = NatExpr::mul(
            NatExpr::add(NatExpr::Literal(1, span), NatExpr::Literal(2, span), span),
            NatExpr::Literal(3, span),
            span,
        );
        assert_eq!(expr.to_string(), "(1 + 2) * 3");
    }
}

/// A generic argument whose source shape is valid as either a dimension/type
/// expression or a Nat expression.
///
/// Bare names (`N`) and products of bare names (`M * N`) cannot be sorted until
/// the referenced declaration's [`GenericConstraint`](super::GenericConstraint)
/// is known. Keeping that ambiguity explicit prevents casing or spelling from
/// leaking into semantic dispatch.
#[derive(Debug, Clone)]
pub enum AmbiguousGenericArg {
    /// A bare type-level name.
    Name(Ident),
    /// A product of two or more ambiguous arguments.
    Mul(AtLeastTwo<Self>, Span),
}

impl AmbiguousGenericArg {
    /// Construct a product, extending an existing left-hand product instead of
    /// creating a left-deep tree.
    #[must_use]
    pub fn mul(lhs: Self, rhs: Self, span: Span) -> Self {
        match lhs {
            Self::Mul(mut operands, _) => {
                operands.push(rhs);
                Self::Mul(operands, span)
            }
            lhs @ Self::Name(_) => Self::Mul(AtLeastTwo::new(lhs, rhs), span),
        }
    }

    /// Get the source span.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Name(ident) => ident.span,
            Self::Mul(_, span) => *span,
        }
    }

    fn fmt_with_min_precedence(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        min_precedence: u8,
    ) -> std::fmt::Result {
        let precedence = match self {
            Self::Name(_) => 2,
            Self::Mul(..) => 1,
        };
        let needs_parens = precedence < min_precedence;
        if needs_parens {
            f.write_str("(")?;
        }
        match self {
            Self::Name(ident) => f.write_str(&ident.name)?,
            Self::Mul(operands, _) => {
                for (index, operand) in operands.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" * ")?;
                    }
                    operand.fmt_with_min_precedence(f, precedence)?;
                }
            }
        }
        if needs_parens {
            f.write_str(")")?;
        }
        Ok(())
    }
}

impl std::fmt::Display for AmbiguousGenericArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fmt_with_min_precedence(f, 0)
    }
}

/// An unresolved generic argument shared by type and constructor applications.
///
/// Arguments that are syntactically unambiguous retain their source category.
/// Bare names and name-only products use [`Self::Ambiguous`] until HIR lowering
/// can resolve them against the declaration's generic-parameter sort.
#[derive(Debug, Clone)]
pub enum GenericArg<P: Phase = Raw> {
    /// An unambiguously type-shaped expression, such as `D[I]` or `D / Time`.
    Type(TypeExpr<P>),
    /// An explicit index-shaped expression, currently `Fin(N)`.
    Index(IndexExpr),
    /// An unambiguously Nat-shaped expression, such as `3` or `N + 1`.
    Nat(NatExpr),
    /// A bare name or name-only product that may denote either sort.
    Ambiguous(AmbiguousGenericArg),
}

impl<P: Phase> GenericArg<P> {
    /// Get the source span of this generic argument.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Type(te) => te.span,
            Self::Index(index) => index.span(),
            Self::Nat(ne) => ne.span(),
            Self::Ambiguous(arg) => arg.span(),
        }
    }
}

/// An argument in an index access: a qualified variant, a loop variable, or an expression.
#[derive(Debug, Clone)]
pub enum IndexArg<P: Phase = Raw> {
    /// Qualified variant: `Maneuver#Departure` or `module::Maneuver#Departure`
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

/// A match pattern: `Impulsive(delta_v: dv)`, `Nominal`, `Maneuver#Departure`.
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
    /// Named-index label pattern: `Maneuver#Departure`.
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
    pub(crate) const fn span(&self) -> Span {
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

/// Source classification of a power exponent.
///
/// The parser records this before numeric literals lose their source spelling.
/// HIR carries the classification unchanged so dimensional analysis and
/// evaluation never reconstruct exact rationals from binary64 or source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerExponent {
    /// Integer or parenthesized rational syntax such as `2`, `-2`, or `(3/2)`.
    Exact(ExactRational),
    /// Decimal/scientific float syntax. `exact` is present only when the
    /// decimal spelling can be represented exactly by [`ExactRational`].
    FloatSyntax { exact: Option<ExactRational> },
    /// Any other exponent expression, whose value is available only at runtime.
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow(PowerExponent),
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
