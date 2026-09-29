//! HIR type-level reference types.
//!
//! The syntax AST preserves source paths (`NamePath` / `IdentPath`) for
//! type-level references. These HIR types represent the corresponding resolved
//! boundary: every module-owned reference carries a canonical `ResolvedName`,
//! while lexical generic parameters carry a `GenericParamId` scoped to their
//! owning type signature.

use crate::dimension::Rational;
use crate::registry::time_scale::TimeScale;
use crate::resolved_name::{ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName};
use crate::syntax::ast::MulDivOp;
use crate::syntax::non_empty::{AtLeastTwo, NonEmpty};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;

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

    /// The lexical scope that owns this parameter.
    #[must_use]
    pub(crate) const fn owner(&self) -> &GenericParamOwner {
        &self.owner
    }
}

/// The lexical scope that owns a generic parameter list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericParamOwner {
    /// Generic parameter on a user-defined `type` declaration.
    Type(ResolvedStructTypeName),
    /// Dimension or index binder of an extern plugin function signature.
    ExternFn(crate::plugin_identity::ExternFnKey),
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
    pub decl_type: DeclType,
    pub domain_bounds: Vec<DomainBound>,
    pub span: Span,
}

/// One declaration domain bound lowered to HIR at the same boundary as its type.
#[derive(Debug, Clone)]
pub struct DomainBound {
    pub kind: crate::syntax::ast::DomainBoundKind,
    pub value: crate::hir::CheckedExpr,
    pub span: Span,
}

/// The type of a declaration or struct field: a value type, optionally
/// indexed by one or more axes (`T[I, J]`).
///
/// Indexing is a declaration-level shape, not a value type: an indexed type
/// cannot appear as a `Type`-sorted generic argument, and the element of an
/// indexed type is always a [`ValueType`] (never another indexed type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclType {
    /// A scalar (non-indexed) value type.
    Value(ValueType),
    /// An indexed type expression.
    Indexed {
        element: ValueType,
        indexes: NonEmpty<IndexRef>,
        span: Span,
    },
}

/// A resolved value type that still preserves source-level structure.
///
/// This is not TIR's semantic `ResolvedTypeExpr`: HIR keeps references to named
/// dimensions/types/indexes as canonical identities instead of immediately
/// collapsing them to registry values such as `Dimension`. An index name is
/// never a value type; it appears only as an [`IndexRef`] (in `Key<I>`, an
/// indexed declaration type, or an `Index`-sorted [`GenericArg`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueType {
    pub kind: ValueTypeKind,
    pub(crate) span: Span,
}

impl ValueType {
    /// Create a HIR value type.
    #[must_use]
    pub(crate) const fn new(kind: ValueTypeKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// The resolved shape of a value type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueTypeKind {
    /// A built-in type with closed meaning.
    Builtin(BuiltinType),
    /// A dimension expression denoting a quantity type.
    DimExpr(DimExpr),
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
    /// A `Type`-sorted argument: always a (non-indexed) value type.
    Type(ValueType),
}

impl GenericArg {
    /// Source span for diagnostics.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Dim(arg) => arg.span(),
            Self::Index(index) => index.span(),
            Self::Nat(nat) => nat.span(),
            Self::Type(value_type) => value_type.span,
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
    /// Exact semantic exponent, normalized at the AST-to-HIR boundary.
    pub(crate) power: Rational,
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

impl std::fmt::Display for IndexRef {
    /// Leaf-only diagnostic spelling (`Phase`, `I`, `Fin(N + 1)`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Concrete(name) => f.write_str(name.value.as_str()),
            Self::GenericParam(param) => write!(f, "{}", param.value.name),
            Self::Finite(cardinality) => write!(f, "Fin({cardinality})"),
        }
    }
}

impl IndexRef {
    /// Source span for diagnostics.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Concrete(name) => name.span,
            Self::GenericParam(param) => param.span,
            Self::Finite(cardinality) => cardinality.span(),
        }
    }
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

impl std::fmt::Display for NatExpr {
    /// Diagnostic spelling (`N + 1`, `2 * N`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn join(
            f: &mut std::fmt::Formatter<'_>,
            operands: &AtLeastTwo<NatExpr>,
            separator: &str,
        ) -> std::fmt::Result {
            for (position, operand) in operands.iter().enumerate() {
                if position > 0 {
                    f.write_str(separator)?;
                }
                write!(f, "{operand}")?;
            }
            Ok(())
        }
        match self {
            Self::Literal(value, _) => write!(f, "{value}"),
            Self::Param(param) => write!(f, "{}", param.value.name),
            Self::Add(operands, _) => join(f, operands, " + "),
            Self::Mul(operands, _) => join(f, operands, " * "),
        }
    }
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
