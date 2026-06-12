//! HIR type-level reference types.
//!
//! The syntax AST preserves source paths (`NamePath` / `IdentPath`) for
//! type-level references. These HIR types represent the corresponding resolved
//! boundary: every module-owned reference carries a canonical `ResolvedName`,
//! while lexical generic parameters carry a `GenericParamId` scoped to their
//! owning type/function signature.

use crate::registry::time_scale::TimeScale;
use crate::syntax::ast::{GenericConstraint, MulDivOp};
use crate::syntax::dimension::Rational;
use crate::syntax::names::{GenericParamName, ResolvedName, TimeScaleName, namespace};
use crate::syntax::span::{Span, Spanned};

/// Canonical identity for a generic parameter in a lexical generic scope.
///
/// Generic parameters are not module-level symbols, so they should not be
/// represented as `ResolvedName<GenericParam>`. Their identity is the owning
/// generic scope plus the parameter leaf name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericParamId {
    pub owner: GenericParamOwner,
    pub name: GenericParamName,
}

impl GenericParamId {
    /// Create a generic parameter identity from its owner and leaf name.
    #[must_use]
    pub const fn new(owner: GenericParamOwner, name: GenericParamName) -> Self {
        Self { owner, name }
    }
}

/// The lexical scope that owns a generic parameter list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericParamOwner {
    /// Generic parameter on a user-defined `type` declaration.
    Type(ResolvedName<namespace::StructType>),
    /// Generic parameter on a function signature.
    Function(ResolvedName<namespace::Fn>),
}

/// A resolved generic-parameter definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParamDef {
    pub id: Spanned<GenericParamId>,
    pub constraint: GenericConstraint,
    pub default: Option<TypeExpr>,
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
    Datetime(TimeScaleName),
}

impl BuiltinType {
    /// The default `Datetime` type is UTC.
    #[must_use]
    pub const fn datetime_utc() -> Self {
        Self::Datetime(TimeScaleName::new(TimeScale::UTC))
    }
}

/// A resolved type expression that still preserves source-level structure.
///
/// This is not TIR's semantic `ResolvedTypeExpr`: HIR keeps references to named
/// dimensions/types/indexes as canonical identities instead of immediately
/// collapsing them to registry values such as `Dimension`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

impl TypeExpr {
    /// Create a HIR type expression.
    #[must_use]
    pub const fn new(kind: TypeExprKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// The resolved shape of a type expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExprKind {
    /// A built-in type with closed meaning.
    Builtin(BuiltinType),
    /// A scalar dimension expression.
    DimExpr(DimExpr),
    /// A label type for a concrete index.
    Label(Spanned<ResolvedName<namespace::Index>>),
    /// A user-defined non-generic struct/tagged-union type.
    Struct(Spanned<ResolvedName<namespace::StructType>>),
    /// A generic type parameter (`F: Type`).
    GenericTypeParam(Spanned<GenericParamId>),
    /// A user-defined generic type application.
    TypeApplication {
        name: Spanned<ResolvedName<namespace::StructType>>,
        type_args: Vec<TypeExpr>,
    },
    /// An indexed type expression.
    Indexed {
        base: Box<TypeExpr>,
        indexes: Vec<IndexRef>,
    },
}

/// A resolved dimension expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimExpr {
    pub terms: Vec<DimExprItem>,
    pub span: Span,
}

/// One term of a dimension expression with its combining operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimExprItem {
    pub op: MulDivOp,
    pub term: DimTermRef,
}

/// A resolved dimension term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimTermRef {
    pub target: DimTermTarget,
    /// `None` means exponent 1. Rational exponents (`^(1/2)`) are kept exact.
    pub power: Option<Rational>,
    pub span: Span,
}

/// Target of a resolved dimension term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimTermTarget {
    /// A concrete module-owned dimension declaration.
    Dimension(Spanned<ResolvedName<namespace::Dim>>),
    /// A generic dimension parameter (`D: Dim`).
    GenericParam(Spanned<GenericParamId>),
}

/// A resolved index reference in an indexed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRef {
    /// A concrete module-owned index declaration.
    Concrete(Spanned<ResolvedName<namespace::Index>>),
    /// A generic index parameter (`I: Index`).
    GenericParam(Spanned<GenericParamId>),
    /// A type-level natural-number expression.
    NatExpr(NatExpr),
}

/// A resolved type-level natural-number expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatExpr {
    /// Integer literal.
    Literal(u64, Span),
    /// Generic natural-number parameter (`N: Nat`).
    Param(Spanned<GenericParamId>),
    /// Addition.
    Add(Box<Self>, Box<Self>, Span),
    /// Multiplication.
    Mul(Box<Self>, Box<Self>, Span),
}

impl NatExpr {
    /// Source span for the expression.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Literal(_, span) | Self::Add(_, _, span) | Self::Mul(_, _, span) => *span,
            Self::Param(param) => param.span,
        }
    }
}
