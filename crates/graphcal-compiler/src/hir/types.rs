//! HIR type-level reference types.
//!
//! The syntax AST preserves source paths (`NamePath` / `IdentPath`) for
//! type-level references. These HIR types represent the corresponding resolved
//! boundary: every module-owned reference carries a canonical `ResolvedName`,
//! while lexical generic parameters carry a `GenericParamId` scoped to their
//! owning type signature.

use crate::dimension::Rational;
use crate::registry::time_scale::TimeScale;
use crate::syntax::ast::MulDivOp;
use crate::syntax::dimension::ResolvedDimName;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::non_empty::{AtLeastTwo, NonEmpty};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{GenericParamName, ResolvedStructTypeName};

/// Canonical identity for a generic parameter in a lexical generic scope.
///
/// Generic parameters are not module-level symbols, so they should not be
/// represented as `ResolvedName<GenericParam>`. Their identity is the owning
/// generic scope plus the parameter leaf name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericParamId {
    owner: GenericParamOwner,
    pub name: GenericParamName,
}

impl GenericParamId {
    /// Create a generic parameter identity from its owner and leaf name.
    #[must_use]
    pub(crate) const fn new(owner: GenericParamOwner, name: GenericParamName) -> Self {
        Self { owner, name }
    }
}

/// The lexical scope that owns a generic parameter list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericParamOwner {
    /// Generic parameter on a user-defined `type` declaration.
    Type(ResolvedStructTypeName),
}

/// Built-in type forms with closed semantic meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinType {
    /// `Dimensionless`.
    Dimensionless,
    /// `Bool`.
    Bool,
    /// `Int`.
    Int,
    /// `Datetime` or `Datetime<Scale>`.
    Datetime(TimeScale),
}

impl BuiltinType {
    /// The default `Datetime` type is UTC.
    #[must_use]
    pub(crate) const fn datetime_utc() -> Self {
        Self::Datetime(TimeScale::UTC)
    }
}

/// A canonically resolved declaration type and its HIR domain bounds.
#[derive(Debug, Clone)]
pub struct TypeAnnotation {
    pub type_expr: TypeExpr,
    pub domain_bounds: Vec<DomainBound>,
    pub span: Span,
}

/// One declaration domain bound lowered to HIR at the same boundary as its type.
#[derive(Debug, Clone)]
pub struct DomainBound {
    pub kind: crate::syntax::ast::DomainBoundKind,
    pub value: crate::hir::Expr,
    pub span: Span,
}

/// A resolved type expression that still preserves source-level structure.
///
/// This is not TIR's semantic `ResolvedTypeExpr`: HIR keeps references to named
/// dimensions/types/indexes as canonical identities instead of immediately
/// collapsing them to registry values such as `Dimension`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub(crate) span: Span,
}

impl TypeExpr {
    /// Create a HIR type expression.
    #[must_use]
    pub(crate) const fn new(kind: TypeExprKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// The resolved shape of a type expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExprKind {
    /// A built-in type with closed meaning.
    Builtin(BuiltinType),
    /// A dimension expression denoting a quantity type.
    DimExpr(DimExpr),
    /// An index name in a type-expression syntactic slot.
    ///
    /// This is not a value type. It is accepted only where the surrounding
    /// generic parameter expects an `Index` argument; declaration annotations
    /// reject it before TIR construction.
    Index(IndexRef),
    /// A user-defined non-generic struct/tagged-union type.
    Struct(Spanned<ResolvedStructTypeName>),
    /// A generic type parameter (`F: Type`).
    GenericTypeParam(Spanned<GenericParamId>),
    /// Built-in dimension-aware complex quantity type `Complex<D>`.
    Complex(DimArg),
    /// Built-in index-key type `Key<I>`: the value type of element keys of
    /// axis `I`.
    Key(IndexRef),
    /// A user-defined generic type application.
    TypeApplication {
        name: Spanned<ResolvedStructTypeName>,
        generic_args: Vec<GenericArg>,
    },
    /// An indexed type expression.
    Indexed {
        base: Box<TypeExpr>,
        indexes: NonEmpty<IndexRef>,
    },
}

/// A dimension-sorted generic argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimArg {
    /// The closed built-in `Dimensionless` dimension.
    Dimensionless(Span),
    /// A concrete or generic dimension expression.
    Expr(DimExpr),
}

impl DimArg {
    /// Source span for diagnostics.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Dimensionless(span) => *span,
            Self::Expr(expr) => expr.span,
        }
    }
}

/// A generic argument classified against its declaration signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericArg {
    Dim(DimArg),
    Index(IndexRef),
    Nat(NatExpr),
    Type(TypeExpr),
}

impl GenericArg {
    /// Source span for diagnostics.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Dim(arg) => arg.span(),
            Self::Index(index) => match index {
                IndexRef::Concrete(name) => name.span,
                IndexRef::GenericParam(param) => param.span,
                IndexRef::Finite(cardinality) => cardinality.span(),
            },
            Self::Nat(nat) => nat.span(),
            Self::Type(type_expr) => type_expr.span,
        }
    }
}

/// A resolved dimension expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimExpr {
    pub terms: Vec<DimExprItem>,
    pub(crate) span: Span,
}

/// One term of a dimension expression with its combining operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimExprItem {
    pub(crate) op: MulDivOp,
    pub term: DimTermRef,
}

/// A resolved dimension term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimTermRef {
    pub target: DimTermTarget,
    /// `None` means exponent 1. Rational exponents (`^(1/2)`) are kept exact.
    pub(crate) power: Option<Rational>,
    pub(crate) span: Span,
}

/// Target of a resolved dimension term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimTermTarget {
    /// A concrete module-owned dimension declaration.
    Dimension(Spanned<ResolvedDimName>),
    /// A generic dimension parameter (`D: Dim`).
    GenericParam(Spanned<GenericParamId>),
}

/// A resolved index reference in an indexed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRef {
    /// A concrete module-owned index declaration.
    Concrete(Spanned<ResolvedIndexName>),
    /// A generic index parameter (`I: Index`).
    GenericParam(Spanned<GenericParamId>),
    /// A structural finite index `Fin(N)`.
    Finite(NatExpr),
}

/// A resolved type-level natural-number expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatExpr {
    /// Integer literal.
    Literal(u64, Span),
    /// Generic natural-number parameter (`N: Nat`).
    Param(Spanned<GenericParamId>),
    /// Addition of two or more operands.
    Add(AtLeastTwo<Self>, Span),
    /// Multiplication of two or more operands.
    Mul(AtLeastTwo<Self>, Span),
}

impl NatExpr {
    /// Source span for the expression.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Literal(_, span) | Self::Add(_, span) | Self::Mul(_, span) => *span,
            Self::Param(param) => param.span,
        }
    }
}
