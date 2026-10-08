//! The typed expression tree data model.

use crate::builtin::AggregationFn;
use crate::datetime_literal::{EpochLiteral, OffsetDateTimeLiteral, ZonedDateTimeLiteral};
use crate::expression_id::ExprId;
use crate::extern_struct_result::ExternStructResult;
use crate::function_signature::{
    FunctionParam, IndexBinder, ParamKind, ResultKind, ScalarValueKind,
};
use crate::hir::expr::{
    ExternFnRef, ForBinding, IndexVariantRef, LocalDef, LocalId, MapEntryKey, PatternBinding,
    ResolvedUnitExpr, UnfoldRecurrence,
};
use crate::resolved_name::ResolvedDeclName;
use crate::semantic::checked_type::{CheckedType, Concrete, Concreteness, IndexTypeRef};
use crate::semantic::index_axis::IndexAxis;
use crate::semantic::key_value::{FinKeyShift, KeyValue};
use crate::semantic::struct_value::{StructFieldsError, StructValue};
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::function_name::FnParamName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::FieldName;
use crate::tir::static_index::StaticIndexUse;

use super::nominal::{ConstructorApplication, ConstructorMatch};
use super::operators::{BExpr, CExpr, DExpr, IExpr, LinearAlgebraCall, QExpr};

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
        let kind = std::mem::replace(&mut self.kind, TExprKind::Bool(BExpr::Literal(false)));
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
            TExprKind::Construct(construct)
            | TExprKind::Const(Spanned {
                value: TConstRef::Constructor(construct),
                ..
            }) => Some(construct.application()),
            _ => None,
        }
    }

    /// Visit this node's typed children in structural order.
    #[expect(
        clippy::match_same_arms,
        reason = "each operation family has its own operand type"
    )]
    pub fn visit_children<'a>(&'a self, visitor: &mut dyn FnMut(TNodeRef<'a, V>)) {
        let unbox =
            |operands: Vec<&'a Box<Self>>| operands.into_iter().map(|operand| &**operand).collect();
        let values: Vec<&'a Self> = match &self.kind {
            TExprKind::QuantityLiteral { .. }
            | TExprKind::GraphRef(_)
            | TExprKind::Const(_)
            | TExprKind::Local(_)
            | TExprKind::Variant { .. } => Vec::new(),
            TExprKind::Quantity(operation) => unbox(operation.operands()),
            TExprKind::Int(operation) => unbox(operation.operands()),
            TExprKind::Bool(operation) => unbox(operation.operands()),
            TExprKind::Complex(operation) => unbox(operation.operands()),
            TExprKind::Datetime(operation) => unbox(operation.operands()),
            TExprKind::KeyShift { key, addend, .. } => vec![&**key, &**addend],
            TExprKind::Convert { expr: operand, .. }
            | TExprKind::DisplayTimezone { expr: operand, .. }
            | TExprKind::Field { expr: operand, .. }
            | TExprKind::For { body: operand, .. }
            | TExprKind::Key { arg: operand, .. } => vec![&**operand],
            TExprKind::DatetimeLiteral(_) => Vec::new(),
            TExprKind::Aggregate { arg, .. } => vec![&**arg],
            TExprKind::LinearAlgebra(call) => unbox(call.operands()),
            TExprKind::Extern { args, .. } => args.iter().map(|arg| &arg.value).collect(),
            TExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => vec![&**condition, &**then_branch, &**else_branch],
            TExprKind::Construct(construct) => {
                construct.fields().map(|field| &field.value).collect()
            }
            TExprKind::Map { entries, .. } => entries.iter().map(|entry| &entry.value).collect(),
            TExprKind::Index { expr, args } => std::iter::once(&**expr)
                .chain(args.iter().filter_map(|arg| match arg {
                    TIndexArg::Key(operand) | TIndexArg::Position { operand, .. } => {
                        Some(&**operand)
                    }
                    TIndexArg::Variant(_) | TIndexArg::Var(_) => None,
                }))
                .collect(),
            TExprKind::Scan {
                source, init, body, ..
            } => vec![&**source, &**init, &**body],
            TExprKind::Unfold { init, body, .. } => vec![&**init, &**body],
            TExprKind::Match { scrutinee, arms } => {
                std::iter::once(&**scrutinee).chain(arms.bodies()).collect()
            }
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
///
/// Operators are recorded as the operation their operand types select
/// ([`QExpr`], [`IExpr`], [`BExpr`], [`CExpr`], [`DExpr`], and the Fin-key
/// shift), grouped by the checked result type.
///
/// A node that introduces keys or entries of an axis carries, once the tree
/// is concrete, that axis's [`IndexAxis`] (and a constant key its
/// [`KeyValue`]), established when the tree was discharged; a symbolic tree
/// carries nothing in their place (see [`Concreteness::Discharged`]).
#[derive(Debug, Clone)]
pub enum TExprKind<V: Concreteness = Concrete> {
    /// A quantity literal with its unit.
    QuantityLiteral {
        value: f64,
        unit: ResolvedUnitExpr,
    },
    Quantity(QExpr<Box<TExpr<V>>>),
    Int(IExpr<Box<TExpr<V>>>),
    Bool(BExpr<Box<TExpr<V>>>),
    Complex(CExpr<Box<TExpr<V>>>),
    Datetime(DExpr<Box<TExpr<V>>>),
    /// `k + c` on a `Fin` key: the key `c` positions later, on the wider
    /// axis the node's type names. The static addend stays as the shift's
    /// source; the concrete shift is derived from the key's and the node's
    /// axes.
    KeyShift {
        key: Box<TExpr<V>>,
        addend: Box<TExpr<V>>,
        shift: V::Discharged<FinKeyShift>,
    },
    GraphRef(Spanned<crate::hir::expr::LocalDecl>),
    Const(Spanned<TConstRef<V>>),
    Local(Spanned<LocalId>),
    /// A datetime built from the literal arguments checking parsed.
    DatetimeLiteral(DatetimeLiteral),
    /// A reduction of a rank-one indexed value.
    Aggregate {
        function: AggregationFn,
        arg: Box<TExpr<V>>,
    },
    /// A shape-aware operation on indexed quantities.
    LinearAlgebra(LinearAlgebraCall<Box<TExpr<V>>>),
    /// A plugin function call, each argument paired with the declared
    /// parameter checking matched it against, and the declared result kind
    /// the host's result is validated against.
    Extern {
        function: ExternFnRef,
        args: Vec<TExternArg<V>>,
        result: TExternResult<V>,
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
    Construct(TConstruct<V>),
    /// A map literal, with the placement of its entries on the axes their
    /// keys select on.
    Map {
        entries: Vec<TMapEntry<V>>,
        layout: V::Discharged<super::map_layout::MapLayout>,
    },
    For {
        bindings: NonEmpty<TForBinding<V>>,
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
        axis: V::Discharged<IndexAxis>,
    },
    /// A key introduction of the axis `axis` from `arg`.
    Key {
        form: TKeyForm<V>,
        arg: Box<TExpr<V>>,
        axis: V::Discharged<IndexAxis>,
    },
    Match {
        scrutinee: Box<TExpr<V>>,
        arms: TMatchArms<V>,
    },
    /// A qualified label (`Maneuver#Departure`), with the key it denotes.
    Variant {
        variant: IndexVariantRef,
        key: V::Discharged<KeyValue>,
    },
    /// An inline call of the DAG `slot` names in the call targets of the
    /// body that holds this node.
    DagCall {
        slot: super::call_targets::CallSlot,
        args: Vec<TParamBinding<V>>,
        static_bindings: crate::ir::static_substitution::StaticSubstitution,
        output: Spanned<ResolvedDeclName>,
    },
}

/// A datetime literal a constructor call builds, as checking parsed it.
#[derive(Debug, Clone)]
pub enum DatetimeLiteral {
    /// `datetime("…Z")` or `datetime("…+hh:mm")`.
    Offset(OffsetDateTimeLiteral),
    /// `datetime("…", "Area/City")`, resolved in its timezone.
    Zoned(ZonedDateTimeLiteral),
    /// `epoch<S>("…")`.
    Epoch(EpochLiteral),
}

/// One binding of a comprehension, with the axis it ranges over.
#[derive(Debug, Clone)]
pub struct TForBinding<V: Concreteness = Concrete> {
    pub binding: ForBinding,
    pub axis: V::Discharged<IndexAxis>,
}

/// How a key introduction selects its key.
#[derive(Debug, Clone)]
pub enum TKeyForm<V: Concreteness = Concrete> {
    /// `key(Axis, position)`: the position, proved in range of the axis, and
    /// the key at it.
    Static {
        position: StaticPosition<V>,
        key: V::Discharged<KeyValue>,
    },
    /// `fin_key(Fin(N), position)`: a runtime position, range-checked.
    Fin,
    /// A search of a coordinate axis for a quantity.
    Search(CoordinateSearch),
}

/// A search of a coordinate axis for a quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateSearch {
    /// `floor_key`: the last coordinate at or before the target.
    Floor,
    /// `ceil_key`: the first coordinate at or after the target.
    Ceil,
    /// `nearest_key`: the closest coordinate, ties toward the axis start.
    Nearest,
}

impl CoordinateSearch {
    /// The key form that spells this search.
    #[must_use]
    pub const fn kind(self) -> crate::syntax::ast::KeyFormKind {
        match self {
            Self::Floor => crate::syntax::ast::KeyFormKind::Floor,
            Self::Ceil => crate::syntax::ast::KeyFormKind::Ceil,
            Self::Nearest => crate::syntax::ast::KeyFormKind::Nearest,
        }
    }
}

/// A checked constant-like reference.
#[derive(Debug, Clone)]
pub enum TConstRef<V: Concreteness = Concrete> {
    Decl(crate::hir::expr::LocalDecl),
    /// A field-less constructor used as a value: a call with no fields.
    Constructor(TConstruct<V>),
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
    Epoch(EpochLiteral),
    ZonedDateTime(ZonedDateTimeLiteral),
    TimeZone(IanaTimeZoneId),
}

/// The resolved signature of a plugin function.
pub type ExternSignature = crate::function_signature::FunctionSignature<ExternStructResult>;

/// One axis of the indexed result of a plugin call: the index variable the
/// signature names it by, and, once the tree is concrete, the axis of the
/// call node's checked type at that place.
#[derive(Debug, Clone)]
pub struct TResultAxis<V: Concreteness = Concrete> {
    pub binder: IndexBinder,
    pub axis: V::Discharged<IndexAxis>,
}

/// The declared result kind of a plugin call, each axis of an indexed result
/// with the axis the call's checked type ranges over there.
pub type TExternResult<V = Concrete> =
    ResultKind<ExternStructResult, crate::function_signature::DimBinder, TResultAxis<V>>;

/// The declared parameter of one plugin-call argument, by the ABI kind the
/// argument crosses as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternArgKind {
    /// A single quantity.
    Quantity { param: FnParamName },
    /// A single Boolean.
    Bool { param: FnParamName },
    /// A single integer.
    Int { param: FnParamName },
    /// A dense array of `element`s whose axes, in order, bind `indexes`.
    Indexed {
        param: FnParamName,
        element: ScalarValueKind,
        indexes: NonEmpty<IndexBinder>,
    },
}

impl ExternArgKind {
    /// The ABI kind of an argument for `param`.
    #[must_use]
    pub fn for_param(param: &FunctionParam) -> Self {
        let name = param.name.clone();
        match &param.kind {
            ParamKind::Scalar(ScalarValueKind::Quantity(_)) => Self::Quantity { param: name },
            ParamKind::Scalar(ScalarValueKind::Bool) => Self::Bool { param: name },
            ParamKind::Scalar(ScalarValueKind::Int) => Self::Int { param: name },
            ParamKind::Indexed { element, indexes } => Self::Indexed {
                param: name,
                element: element.clone(),
                indexes: indexes.clone(),
            },
        }
    }

    /// The declared parameter.
    #[must_use]
    pub const fn param(&self) -> &FnParamName {
        match self {
            Self::Quantity { param }
            | Self::Bool { param }
            | Self::Int { param }
            | Self::Indexed { param, .. } => param,
        }
    }
}

/// One argument of a plugin call with the declared parameter checking matched
/// it against; its checked type agrees with that parameter's kind.
#[derive(Debug, Clone)]
pub struct TExternArg<V: Concreteness = Concrete> {
    pub kind: ExternArgKind,
    pub value: TExpr<V>,
}

/// A checked constructor field initializer, in written order.
#[derive(Debug, Clone)]
pub struct TFieldInit<V: Concreteness = Concrete> {
    pub name: FieldName,
    pub value: TExpr<V>,
}

/// A constructor call: its checked application and its field initializers
/// in written order, each placed at the declared field it initializes.
///
/// Built only by `TConstruct::try_new`, which admits exactly the
/// application's declared fields; a type map rewrites the application's
/// field types but never its field names, so applying a call to its
/// evaluated fields always yields a [`StructValue`].
#[derive(Debug, Clone)]
pub struct TConstruct<V: Concreteness = Concrete> {
    pub(super) application: ConstructorApplication<V>,
    /// Each initializer, in written order, with the declaration position of
    /// the field it initializes.
    pub(super) fields: Vec<(usize, TFieldInit<V>)>,
}

impl<V: Concreteness> TConstruct<V> {
    /// A call of `application` with `fields`, in written order.
    ///
    /// # Errors
    ///
    /// Returns a [`StructFieldsError`] when `fields` is not exactly the
    /// application's declared field set.
    pub(crate) fn try_new(
        application: ConstructorApplication<V>,
        fields: Vec<TFieldInit<V>>,
    ) -> Result<Self, StructFieldsError> {
        let declared = application.applied.fields();
        let constructor = || application.applied.constructor().clone();
        let mut placed = vec![false; declared.len()];
        let fields = fields
            .into_iter()
            .map(|init| {
                let slot = declared
                    .iter()
                    .position(|field| *field.name() == init.name)
                    .ok_or_else(|| StructFieldsError::Unexpected {
                        constructor: constructor(),
                        field: init.name.clone(),
                    })?;
                if std::mem::replace(&mut placed[slot], true) {
                    return Err(StructFieldsError::Duplicate {
                        constructor: constructor(),
                        field: init.name,
                    });
                }
                Ok((slot, init))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(missing) = declared
            .iter()
            .zip(&placed)
            .find_map(|(field, placed)| (!placed).then_some(field))
        {
            return Err(StructFieldsError::Missing {
                constructor: constructor(),
                field: missing.name().clone(),
            });
        }
        Ok(Self {
            application,
            fields,
        })
    }

    /// The checked application.
    #[must_use]
    pub const fn application(&self) -> &ConstructorApplication<V> {
        &self.application
    }

    /// The field initializers, in written order.
    pub fn fields(&self) -> impl ExactSizeIterator<Item = &TFieldInit<V>> {
        self.fields.iter().map(|(_, init)| init)
    }
}

impl TConstruct {
    /// The value of this call: each field's value as `value` produces it,
    /// evaluated in written order, at the field's declared place.
    ///
    /// # Errors
    ///
    /// Returns the first error `value` returns, in written order.
    pub fn apply<'s, T, E>(
        &'s self,
        mut value: impl FnMut(&'s TFieldInit) -> Result<T, E>,
    ) -> Result<StructValue<T>, E> {
        let mut values = self
            .fields
            .iter()
            .map(|(slot, init)| Ok((*slot, value(init)?)))
            .collect::<Result<Vec<_>, E>>()?;
        values.sort_by_key(|(slot, _)| *slot);
        Ok(StructValue::from_declared(
            std::sync::Arc::clone(&self.application.applied),
            values.into_iter().map(|(_, value)| value).collect(),
        ))
    }
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
    /// A computed key of the indexed axis.
    Key(Box<TExpr<V>>),
    /// A static `Int` position on a `Fin` axis (`@m[0, 1]`), proved in range;
    /// the operand stays as the position's source.
    Position {
        operand: Box<TExpr<V>>,
        position: StaticPosition<V>,
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

/// The checked arms of a match, by what its scrutinee's type selects on.
#[derive(Debug, Clone)]
pub enum TMatchArms<V: Concreteness = Concrete> {
    /// A key scrutinee, matched by the label of its entry, with the arm
    /// each entry of its axis takes.
    Labels {
        arms: Vec<TLabelArm<V>>,
        dispatch: V::Discharged<super::label_dispatch::LabelDispatch>,
    },
    /// A union scrutinee, matched by its constructor.
    Constructors(Vec<TConstructorArm<V>>),
}

impl<V: Concreteness> TMatchArms<V> {
    /// Every arm's body, in written order.
    pub fn bodies(&self) -> Box<dyn Iterator<Item = &TExpr<V>> + '_> {
        match self {
            Self::Labels { arms, .. } => Box::new(arms.iter().map(|arm| &arm.body)),
            Self::Constructors(arms) => Box::new(arms.iter().map(|arm| &arm.body)),
        }
    }
}

/// One checked arm matching an index label.
#[derive(Debug, Clone)]
pub struct TLabelArm<V: Concreteness = Concrete> {
    pub label: IndexVariantRef,
    pub body: TExpr<V>,
    pub span: Span,
}

/// One checked arm matching a constructor, with its resolved target.
#[derive(Debug, Clone)]
pub struct TConstructorArm<V: Concreteness = Concrete> {
    pub target: ConstructorMatch,
    pub bindings: crate::desugar::desugared_ast::PatternBindings<PatternBinding>,
    pub body: TExpr<V>,
    pub span: Span,
}

/// A checked inline-DAG parameter binding.
#[derive(Debug, Clone)]
pub struct TParamBinding<V: Concreteness = Concrete> {
    pub target: ResolvedDeclName,
    pub value: TExpr<V>,
}
