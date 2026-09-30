//! The typed expression tree data model.

use crate::builtin::BuiltinConst;
use crate::dag_id::DagId;
use crate::datetime_literal::{CivilDateTimeLiteral, OffsetDateTimeLiteral, ZonedDateTimeLiteral};
use crate::expression_id::ExprId;
use crate::hir::expr::{
    ForBinding, ForBindingIndex, FunctionRef, IndexVariantRef, LocalDef, LocalId, MapEntryKey,
    PatternBinding, ResolvedUnitExpr, UnfoldRecurrence,
};
use crate::registry::checked_type::{CheckedType, Concrete, Concreteness, IndexTypeRef};
use crate::registry::time_zone::IanaTimeZoneId;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::FieldName;
use crate::tir::static_index::StaticIndexUse;

use super::nominal::{ConstructorApplication, ConstructorMatch};

/// One checked value expression.
#[derive(Debug)]
pub struct TExpr<V: Concreteness = Concrete> {
    id: ExprId,
    span: Span,
    ty: CheckedType<V>,
    kind: TExprKind<V>,
}

// Manual impls instead of derives: derived clone and drop glue recurse once
// per tree level without a stack-growth guard, so a long left-nested operator
// chain would overflow the stack. Each level runs under `with_stack_growth`.
impl<V: Concreteness> Clone for TExpr<V> {
    fn clone(&self) -> Self {
        crate::stack::with_stack_growth(|| Self {
            id: self.id.clone(),
            span: self.span,
            ty: self.ty.clone(),
            kind: self.kind.clone(),
        })
    }
}

impl<V: Concreteness> Drop for TExpr<V> {
    fn drop(&mut self) {
        // Move the recursive kind out under a leaf placeholder so only the
        // placeholder is dropped after this impl returns.
        let kind = std::mem::replace(&mut self.kind, TExprKind::Bool(false));
        crate::stack::with_stack_growth(|| drop(kind));
    }
}

impl<V: Concreteness> TExpr<V> {
    pub(super) const fn new(
        id: ExprId,
        span: Span,
        ty: CheckedType<V>,
        kind: TExprKind<V>,
    ) -> Self {
        Self { id, span, ty, kind }
    }

    /// The HIR occurrence this node checks.
    #[must_use]
    pub const fn id(&self) -> &ExprId {
        &self.id
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// The checked type of the value this node evaluates to.
    #[must_use]
    pub const fn ty(&self) -> &CheckedType<V> {
        &self.ty
    }

    #[must_use]
    pub(crate) const fn kind(&self) -> &TExprKind<V> {
        &self.kind
    }

    /// The constructor application this node makes, if it applies one.
    #[must_use]
    pub const fn application(&self) -> Option<&ConstructorApplication<V>> {
        match &self.kind {
            TExprKind::Construct { application, .. }
            | TExprKind::Const(Spanned {
                value: TConstRef::Constructor(application),
                ..
            }) => Some(application),
            _ => None,
        }
    }

    /// Visit this node's typed children in structural order.
    pub fn visit_children<'a>(&'a self, visitor: &mut dyn FnMut(TNodeRef<'a, V>)) {
        let values: Vec<&'a Self> = match &self.kind {
            TExprKind::Number(_)
            | TExprKind::Integer(_)
            | TExprKind::Bool(_)
            | TExprKind::Quantity { .. }
            | TExprKind::GraphRef(_)
            | TExprKind::Const(_)
            | TExprKind::Local(_)
            | TExprKind::Variant(_) => Vec::new(),
            TExprKind::Binary { lhs, rhs, .. } => vec![&**lhs, &**rhs],
            TExprKind::Unary { operand, .. }
            | TExprKind::Convert { expr: operand, .. }
            | TExprKind::DisplayTimezone { expr: operand, .. }
            | TExprKind::Field { expr: operand, .. }
            | TExprKind::For { body: operand, .. }
            | TExprKind::Key { arg: operand, .. } => vec![&**operand],
            TExprKind::Call { args, .. } => {
                for arg in args {
                    visitor(match arg {
                        TArg::Value(arg) => TNodeRef::Value(arg),
                        TArg::Contextual(arg) => TNodeRef::Contextual(arg),
                    });
                }
                return;
            }
            TExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => vec![&**condition, &**then_branch, &**else_branch],
            TExprKind::Construct { fields, .. } => {
                fields.iter().map(|field| &field.value).collect()
            }
            TExprKind::Map { entries } => entries.iter().map(|entry| &entry.value).collect(),
            TExprKind::Index { expr, args } => std::iter::once(&**expr)
                .chain(args.iter().filter_map(|arg| match arg {
                    TIndexArg::Expr { operand, .. } => Some(&**operand),
                    TIndexArg::Variant(_) | TIndexArg::Var(_) => None,
                }))
                .collect(),
            TExprKind::Scan {
                source, init, body, ..
            } => vec![&**source, &**init, &**body],
            TExprKind::Unfold { init, body, .. } => vec![&**init, &**body],
            TExprKind::Match { scrutinee, arms } => std::iter::once(&**scrutinee)
                .chain(arms.iter().map(|arm| &arm.body))
                .collect(),
            TExprKind::DagCall { args, .. } => args.iter().map(|arg| &arg.value).collect(),
        };
        for value in values {
            visitor(TNodeRef::Value(value));
        }
    }
}
/// A borrowed typed node: a value, or a contextual literal where accepted.
#[derive(Debug, Clone, Copy)]
pub enum TNodeRef<'a, V: Concreteness = Concrete> {
    Value(&'a TExpr<V>),
    Contextual(&'a TContextual),
}

impl<V: Concreteness> TNodeRef<'_, V> {
    #[must_use]
    pub const fn id(&self) -> &ExprId {
        match self {
            Self::Value(expr) => expr.id(),
            Self::Contextual(literal) => literal.id(),
        }
    }
}

/// Visit every node of a typed tree in pre-order.
pub fn visit_tnodes<'a, V: Concreteness>(
    node: TNodeRef<'a, V>,
    visitor: &mut dyn FnMut(TNodeRef<'a, V>),
) {
    crate::stack::with_stack_growth(|| {
        let expr = match node {
            TNodeRef::Value(expr) => Some(expr),
            TNodeRef::Contextual(_) => None,
        };
        visitor(node);
        if let Some(expr) = expr {
            expr.visit_children(&mut |child| visit_tnodes(child, visitor));
        }
    });
}

/// The typed form of each HIR expression kind a checked value can have.
#[derive(Debug, Clone)]
pub enum TExprKind<V: Concreteness = Concrete> {
    Number(f64),
    Integer(i64),
    Bool(bool),
    Quantity {
        value: f64,
        unit: ResolvedUnitExpr,
    },
    GraphRef(Spanned<crate::hir::expr::LocalDecl>),
    Const(Spanned<TConstRef<V>>),
    Local(Spanned<LocalId>),
    Binary {
        op: crate::syntax::ast::BinOp,
        lhs: Box<TExpr<V>>,
        rhs: Box<TExpr<V>>,
    },
    Unary {
        op: crate::syntax::ast::UnaryOp,
        operand: Box<TExpr<V>>,
    },
    Call {
        callee: Spanned<FunctionRef>,
        args: Vec<TArg<V>>,
    },
    If {
        condition: Box<TExpr<V>>,
        then_branch: Box<TExpr<V>>,
        else_branch: Box<TExpr<V>>,
    },
    Convert {
        expr: Box<TExpr<V>>,
        target: ResolvedUnitExpr,
    },
    DisplayTimezone {
        expr: Box<TExpr<V>>,
        timezone: IanaTimeZoneId,
    },
    Field {
        expr: Box<TExpr<V>>,
        field: Spanned<FieldName>,
    },
    /// A constructor call with its checked nominal application.
    Construct {
        application: ConstructorApplication<V>,
        fields: Vec<TFieldInit<V>>,
    },
    Map {
        entries: Vec<TMapEntry<V>>,
    },
    For {
        bindings: Vec<ForBinding>,
        body: Box<TExpr<V>>,
    },
    Index {
        expr: Box<TExpr<V>>,
        args: NonEmpty<TIndexArg<V>>,
    },
    Scan {
        source: Box<TExpr<V>>,
        init: Box<TExpr<V>>,
        acc: LocalDef,
        val: LocalDef,
        body: Box<TExpr<V>>,
    },
    Unfold {
        recurrence: Box<UnfoldRecurrence>,
        init: Box<TExpr<V>>,
        body: Box<TExpr<V>>,
    },
    /// A key introduction; a `static` key carries its checked position.
    Key {
        kind: crate::syntax::ast::KeyFormKind,
        axis: ForBindingIndex,
        arg: Box<TExpr<V>>,
        static_position: Option<StaticPosition<V>>,
    },
    Match {
        scrutinee: Box<TExpr<V>>,
        arms: Vec<TMatchArm<V>>,
    },
    Variant(IndexVariantRef),
    DagCall {
        target: Spanned<DagId>,
        args: Vec<TParamBinding<V>>,
        static_bindings: crate::ir::static_substitution::StaticSubstitution,
        output: Spanned<ResolvedDeclName>,
    },
}

/// A checked constant-like reference.
#[derive(Debug, Clone)]
pub enum TConstRef<V: Concreteness = Concrete> {
    Decl(crate::hir::expr::LocalDecl),
    Builtin(BuiltinConst),
    /// A field-less constructor used as a value, with its checked application.
    Constructor(ConstructorApplication<V>),
}

/// A function argument: a value, or a contextual literal the callee accepts.
#[derive(Debug, Clone)]
pub enum TArg<V: Concreteness = Concrete> {
    Value(Box<TExpr<V>>),
    Contextual(TContextual),
}

/// A checked body root: a value, or a contextual plot string.
#[derive(Debug, Clone)]
pub enum TBody<V: Concreteness = Concrete> {
    Value(Box<TExpr<V>>),
    Contextual(TContextual),
}

impl<V: Concreteness> TBody<V> {
    #[must_use]
    pub const fn as_node(&self) -> TNodeRef<'_, V> {
        match self {
            Self::Value(expr) => TNodeRef::Value(expr),
            Self::Contextual(literal) => TNodeRef::Contextual(literal),
        }
    }
}

/// A contextual literal accepted by the construct that consumes it.
#[derive(Debug, Clone)]
pub struct TContextual {
    id: ExprId,
    span: Span,
    literal: ContextualLiteral,
}

impl TContextual {
    pub(super) const fn new(id: ExprId, span: Span, literal: ContextualLiteral) -> Self {
        Self { id, span, literal }
    }

    #[must_use]
    pub const fn id(&self) -> &ExprId {
        &self.id
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    #[must_use]
    pub const fn literal(&self) -> &ContextualLiteral {
        &self.literal
    }
}

/// The parsed payload of a contextual literal.
#[derive(Debug, Clone)]
pub enum ContextualLiteral {
    String(String),
    OffsetDateTime(OffsetDateTimeLiteral),
    CivilDateTime(CivilDateTimeLiteral),
    ZonedDateTime(ZonedDateTimeLiteral),
    TimeZone(IanaTimeZoneId),
}

/// A checked constructor field initializer, in written order.
#[derive(Debug, Clone)]
pub struct TFieldInit<V: Concreteness = Concrete> {
    pub name: FieldName,
    pub value: TExpr<V>,
}

/// A checked map-literal entry.
#[derive(Debug, Clone)]
pub struct TMapEntry<V: Concreteness = Concrete> {
    pub keys: NonEmpty<MapEntryKey>,
    pub value: TExpr<V>,
}

/// A checked index-access argument.
#[derive(Debug, Clone)]
pub enum TIndexArg<V: Concreteness = Concrete> {
    Variant(IndexVariantRef),
    Var(Spanned<LocalId>),
    /// A computed selector; an `Int` position carries its checked proof.
    Expr {
        operand: Box<TExpr<V>>,
        static_position: Option<StaticPosition<V>>,
    },
}

/// A position proved in range of an axis, or awaiting that axis's size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPosition<V: Concreteness = Concrete> {
    pub axis: IndexTypeRef<V>,
    pub position: u64,
    /// Whether the position introduces a key or selects an entry.
    pub usage: StaticIndexUse,
}

/// One checked match arm.
#[derive(Debug, Clone)]
pub struct TMatchArm<V: Concreteness = Concrete> {
    pub pattern: TMatchPattern,
    pub body: TExpr<V>,
    pub span: Span,
}

/// A checked match pattern; a constructor pattern carries its resolved target.
#[derive(Debug, Clone)]
pub enum TMatchPattern {
    Constructor {
        target: ConstructorMatch,
        bindings: crate::desugar::desugared_ast::PatternBindings<PatternBinding>,
    },
    IndexLabel(IndexVariantRef),
}

/// A checked inline-DAG parameter binding.
#[derive(Debug, Clone)]
pub struct TParamBinding<V: Concreteness = Concrete> {
    pub target: ResolvedDeclName,
    pub value: TExpr<V>,
}
