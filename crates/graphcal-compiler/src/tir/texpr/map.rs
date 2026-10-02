//! Structure-preserving maps over typed trees that rewrite only their types.
//!
//! Discharging a symbolic tree to a concrete one, specializing a template's
//! tree into an instance, and binding a generic bound's `Nat` parameters all
//! keep a tree's shape and rewrite the checked types, constructor
//! applications, match targets, and static positions it carries; moving a
//! tree into a body with other call targets renumbers its call slots the same
//! way. The one traversal lives here; each use supplies a [`TypeMap`].

use std::borrow::Cow;

use super::call_targets::{CallSlot, CallTargets};
use super::map_layout::MapLayout;
use super::nominal::{ConstructorApplication, ConstructorMatch};
use crate::hir::expr::MapEntryKey;
use crate::semantic::checked_type::{CheckedType, Concrete, Concreteness, IndexTypeRef, Symbolic};
use crate::semantic::index_axis::IndexAxis;
use crate::semantic::key_value::KeyValue;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};

use super::model::{
    StaticPosition, TBody, TConstRef, TConstruct, TConstructorArm, TExpr, TExprKind, TExternArg,
    TFieldInit, TForBinding, TIndexArg, TKeyForm, TLabelArm, TMapEntry, TMatchArms, TParamBinding,
};

/// The entry of its axis a constant key a node carries names.
#[derive(Debug, Clone, Copy)]
pub enum KeyEntry<'a> {
    /// A static `key(Axis, position)` position.
    Position(u64),
    /// A qualified label.
    Label(&'a IndexVariantName),
}

/// How a structure-preserving map rewrites the types a tree carries.
///
/// Within one node the map is asked, in order, for the node's type, its
/// constructor application, the static positions it proves, its match
/// targets, and its call slot; the node's children follow in structural
/// order, so failures are reported for the first offending node in pre-order.
pub trait TypeMap<V: Concreteness, W: Concreteness> {
    type Error;

    /// A node's checked type; `span` is the node's.
    fn node_type(&mut self, ty: &CheckedType<V>, span: Span)
    -> Result<CheckedType<W>, Self::Error>;

    /// A constructor application, given the node's already mapped type.
    fn application(
        &mut self,
        application: &ConstructorApplication<V>,
        ty: &CheckedType<W>,
        span: Span,
    ) -> Result<ConstructorApplication<W>, Self::Error>;

    /// A static position the node proves in range of its axis.
    fn static_position(
        &mut self,
        position: &StaticPosition<V>,
        span: Span,
    ) -> Result<StaticPosition<W>, Self::Error>;

    /// The axis a node ranges over or introduces keys of, given the axis it
    /// carried and the index its mapped type names there (`None` when that
    /// type has no axis at that place).
    fn axis(
        &mut self,
        carried: &V::Discharged<IndexAxis>,
        index: Option<&IndexTypeRef<W>>,
        span: Span,
    ) -> Result<W::Discharged<IndexAxis>, Self::Error>;

    /// The placement of a map literal's entries, whose keys are `entries`, on
    /// the axes `indexes` its mapped type names, outermost first (`None` where
    /// that type has no axis), given the placement it carried.
    fn map_layout(
        &mut self,
        carried: &V::Discharged<MapLayout>,
        indexes: &[Option<&IndexTypeRef<W>>],
        entries: &[&NonEmpty<MapEntryKey>],
        span: Span,
    ) -> Result<W::Discharged<MapLayout>, Self::Error>;

    /// The constant key naming `entry` of the axis `index` a node carries,
    /// given the key it carried.
    fn key(
        &mut self,
        carried: &V::Discharged<KeyValue>,
        index: Option<&IndexTypeRef<W>>,
        entry: KeyEntry<'_>,
        span: Span,
    ) -> Result<W::Discharged<KeyValue>, Self::Error>;

    /// A constructor match arm's resolved target.
    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch;

    /// A call node's slot in the call targets of the mapped tree's body.
    fn call_slot(&mut self, slot: CallSlot) -> CallSlot;
}

/// Renumber the call nodes of a tree from the call targets of the body it
/// was checked in to those of the body that publishes it, adding each
/// target the publishing body does not have yet. Types are kept as they are.
struct Rehome<'a> {
    from: &'a CallTargets,
    into: &'a mut CallTargets,
}

impl<V: Concreteness> TypeMap<V, V> for Rehome<'_> {
    type Error = std::convert::Infallible;

    fn node_type(
        &mut self,
        ty: &CheckedType<V>,
        _span: Span,
    ) -> Result<CheckedType<V>, Self::Error> {
        Ok(ty.clone())
    }

    fn application(
        &mut self,
        application: &ConstructorApplication<V>,
        _ty: &CheckedType<V>,
        _span: Span,
    ) -> Result<ConstructorApplication<V>, Self::Error> {
        Ok(application.clone())
    }

    fn static_position(
        &mut self,
        position: &StaticPosition<V>,
        _span: Span,
    ) -> Result<StaticPosition<V>, Self::Error> {
        Ok(position.clone())
    }

    fn axis(
        &mut self,
        carried: &V::Discharged<IndexAxis>,
        _index: Option<&IndexTypeRef<V>>,
        _span: Span,
    ) -> Result<V::Discharged<IndexAxis>, Self::Error> {
        Ok(carried.clone())
    }

    fn map_layout(
        &mut self,
        carried: &V::Discharged<MapLayout>,
        _indexes: &[Option<&IndexTypeRef<V>>],
        _entries: &[&NonEmpty<MapEntryKey>],
        _span: Span,
    ) -> Result<V::Discharged<MapLayout>, Self::Error> {
        Ok(carried.clone())
    }

    fn key(
        &mut self,
        carried: &V::Discharged<KeyValue>,
        _index: Option<&IndexTypeRef<V>>,
        _entry: KeyEntry<'_>,
        _span: Span,
    ) -> Result<V::Discharged<KeyValue>, Self::Error> {
        Ok(carried.clone())
    }

    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch {
        target.clone()
    }

    fn call_slot(&mut self, slot: CallSlot) -> CallSlot {
        self.into.intern(self.from.target(slot).clone())
    }
}

impl<V: Concreteness> TBody<V> {
    /// This tree with its call nodes numbered by `into` instead of `from`,
    /// the call targets it was checked with; `into` gains every target it
    /// lacks.
    pub(crate) fn rehome_calls(&self, from: &CallTargets, into: &mut CallTargets) -> Self {
        match self.map_types(&mut Rehome { from, into }) {
            Ok(body) => body,
            Err(never) => match never {},
        }
    }
}

/// The symbolic view of a tree's types, whatever its [`Concreteness`].
pub trait SymbolicView: Concreteness {
    fn symbolic_type(ty: &CheckedType<Self>) -> Cow<'_, CheckedType<Symbolic>>;
    fn symbolic_index(index: &IndexTypeRef<Self>) -> Cow<'_, IndexTypeRef<Symbolic>>;
}

impl SymbolicView for Symbolic {
    fn symbolic_type(ty: &CheckedType<Self>) -> Cow<'_, CheckedType<Symbolic>> {
        Cow::Borrowed(ty)
    }

    fn symbolic_index(index: &IndexTypeRef<Self>) -> Cow<'_, IndexTypeRef<Symbolic>> {
        Cow::Borrowed(index)
    }
}

impl SymbolicView for Concrete {
    fn symbolic_type(ty: &CheckedType<Self>) -> Cow<'_, CheckedType<Symbolic>> {
        Cow::Owned(ty.to_symbolic())
    }

    fn symbolic_index(index: &IndexTypeRef<Self>) -> Cow<'_, IndexTypeRef<Symbolic>> {
        Cow::Owned(index.to_symbolic())
    }
}

/// An index argument whose static position is already mapped, awaiting its
/// operand.
enum PreparedIndexArg<'a, V: Concreteness, W: Concreteness> {
    Variant(&'a crate::hir::expr::IndexVariantRef),
    Var(&'a Spanned<crate::hir::expr::LocalId>),
    Key(&'a TExpr<V>),
    Position(&'a TExpr<V>, StaticPosition<W>),
}

impl<'a, V: Concreteness, W: Concreteness> PreparedIndexArg<'a, V, W> {
    fn new<M: TypeMap<V, W>>(
        arg: &'a TIndexArg<V>,
        map: &mut M,
        span: Span,
    ) -> Result<Self, M::Error> {
        Ok(match arg {
            TIndexArg::Position { operand, position } => {
                Self::Position(operand, map.static_position(position, span)?)
            }
            TIndexArg::Key(operand) => Self::Key(operand),
            TIndexArg::Variant(variant) => Self::Variant(variant),
            TIndexArg::Var(local) => Self::Var(local),
        })
    }
}

/// The index of the `depth`-th axis of `ty`, outermost first, when `ty` has
/// that many indexed levels.
fn nested_index<W: Concreteness>(ty: &CheckedType<W>, depth: usize) -> Option<&IndexTypeRef<W>> {
    let mut current = ty;
    for _ in 0..depth {
        let CheckedType::Indexed { element, .. } = current else {
            return None;
        };
        current = element;
    }
    match current {
        CheckedType::Indexed { index, .. } => Some(index),
        _ => None,
    }
}

/// The index of a key type.
const fn key_index<W: Concreteness>(ty: &CheckedType<W>) -> Option<&IndexTypeRef<W>> {
    match ty {
        CheckedType::Key(index) => Some(index),
        _ => None,
    }
}

impl<V: Concreteness> TConstruct<V> {
    /// This call with its application and field values rewritten by `map`.
    ///
    /// A type map keeps the application's field names, so each initializer
    /// keeps its declared place.
    fn map_types<W: Concreteness, M: TypeMap<V, W>>(
        &self,
        ty: &CheckedType<W>,
        span: Span,
        map: &mut M,
    ) -> Result<TConstruct<W>, M::Error> {
        Ok(TConstruct {
            application: map.application(&self.application, ty, span)?,
            fields: self
                .fields
                .iter()
                .map(|(slot, field)| {
                    Ok((
                        *slot,
                        TFieldInit {
                            name: field.name.clone(),
                            value: field.value.map_types(map)?,
                        },
                    ))
                })
                .collect::<Result<_, M::Error>>()?,
        })
    }
}

impl<V: Concreteness> TBody<V> {
    /// Rewrite this body's types with `map`.
    pub(crate) fn map_types<W: Concreteness, M: TypeMap<V, W>>(
        &self,
        map: &mut M,
    ) -> Result<TBody<W>, M::Error> {
        Ok(match self {
            Self::Value(expr) => TBody::Value(Box::new(expr.map_types(map)?)),
            Self::Contextual(literal) => TBody::Contextual(literal.clone()),
        })
    }
}

impl<V: Concreteness> TExpr<V> {
    /// Rewrite this tree's types with `map`, keeping its structure.
    pub(crate) fn map_types<W: Concreteness, M: TypeMap<V, W>>(
        &self,
        map: &mut M,
    ) -> Result<TExpr<W>, M::Error> {
        crate::stack::with_stack_growth(|| {
            let span = self.span();
            let ty = map.node_type(self.ty(), span)?;
            let kind = self.map_kind(&ty, map)?;
            Ok(TExpr::new(self.id().clone(), span, ty, kind))
        })
    }

    fn boxed<W: Concreteness, M: TypeMap<V, W>>(
        &self,
        map: &mut M,
    ) -> Result<Box<TExpr<W>>, M::Error> {
        self.map_types(map).map(Box::new)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive structure-preserving mapping of every typed expression kind"
    )]
    fn map_kind<W: Concreteness, M: TypeMap<V, W>>(
        &self,
        ty: &CheckedType<W>,
        map: &mut M,
    ) -> Result<TExprKind<W>, M::Error> {
        let span = self.span();
        Ok(match self.kind() {
            TExprKind::QuantityLiteral { value, unit } => TExprKind::QuantityLiteral {
                value: *value,
                unit: unit.clone(),
            },
            TExprKind::Quantity(operation) => {
                TExprKind::Quantity(operation.try_map(|operand| operand.boxed(map))?)
            }
            TExprKind::Int(operation) => {
                TExprKind::Int(operation.try_map(|operand| operand.boxed(map))?)
            }
            TExprKind::Bool(operation) => {
                TExprKind::Bool(operation.try_map(|operand| operand.boxed(map))?)
            }
            TExprKind::Complex(operation) => {
                TExprKind::Complex(operation.try_map(|operand| operand.boxed(map))?)
            }
            TExprKind::Datetime(operation) => {
                TExprKind::Datetime(operation.try_map(|operand| operand.boxed(map))?)
            }
            TExprKind::KeyShift { key, addend, axis } => {
                let axis = map.axis(axis, key_index(ty), span)?;
                TExprKind::KeyShift {
                    key: key.boxed(map)?,
                    addend: addend.boxed(map)?,
                    axis,
                }
            }
            TExprKind::GraphRef(target) => TExprKind::GraphRef(target.clone()),
            TExprKind::Const(target) => TExprKind::Const(Spanned::new(
                match &target.value {
                    TConstRef::Decl(declaration) => TConstRef::Decl(declaration.clone()),
                    TConstRef::Constructor(construct) => {
                        TConstRef::Constructor(construct.map_types(ty, span, map)?)
                    }
                },
                target.span,
            )),
            TExprKind::Local(local) => TExprKind::Local(local.clone()),
            TExprKind::DatetimeLiteral(literal) => TExprKind::DatetimeLiteral(literal.clone()),
            TExprKind::Aggregate { function, arg } => TExprKind::Aggregate {
                function: *function,
                arg: arg.boxed(map)?,
            },
            TExprKind::LinearAlgebra(call) => {
                TExprKind::LinearAlgebra(call.try_map(|operand| operand.boxed(map))?)
            }
            TExprKind::Extern {
                function,
                args,
                result,
            } => TExprKind::Extern {
                function: function.clone(),
                result: result.clone(),
                args: args
                    .iter()
                    .map(|arg| {
                        Ok(TExternArg {
                            kind: arg.kind.clone(),
                            value: arg.value.map_types(map)?,
                        })
                    })
                    .collect::<Result<_, M::Error>>()?,
            },
            TExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => TExprKind::If {
                condition: condition.boxed(map)?,
                then_branch: then_branch.boxed(map)?,
                else_branch: else_branch.boxed(map)?,
            },
            TExprKind::Convert { expr, target } => TExprKind::Convert {
                expr: expr.boxed(map)?,
                target: target.clone(),
            },
            TExprKind::DisplayTimezone { expr, timezone } => TExprKind::DisplayTimezone {
                expr: expr.boxed(map)?,
                timezone: timezone.clone(),
            },
            TExprKind::Field { expr, field } => TExprKind::Field {
                expr: expr.boxed(map)?,
                field: field.clone(),
            },
            TExprKind::Construct(construct) => {
                TExprKind::Construct(construct.map_types(ty, span, map)?)
            }
            TExprKind::Map { entries, layout } => {
                let keys = entries.iter().map(|entry| &entry.keys).collect::<Vec<_>>();
                let arity = keys.first().map_or(0, |keys| keys.len());
                let indexes = (0..arity)
                    .map(|depth| nested_index(ty, depth))
                    .collect::<Vec<_>>();
                let layout = map.map_layout(layout, &indexes, &keys, span)?;
                TExprKind::Map {
                    entries: entries
                        .iter()
                        .map(|entry| {
                            Ok(TMapEntry {
                                keys: entry.keys.clone(),
                                value: entry.value.map_types(map)?,
                            })
                        })
                        .collect::<Result<_, M::Error>>()?,
                    layout,
                }
            }
            TExprKind::For { bindings, body } => {
                let mut depth = 0;
                let bindings = bindings.try_map_ref(|binding| {
                    let axis = map.axis(&binding.axis, nested_index(ty, depth), span)?;
                    depth += 1;
                    Ok::<_, M::Error>(TForBinding {
                        binding: binding.binding.clone(),
                        axis,
                    })
                })?;
                TExprKind::For {
                    bindings,
                    body: body.boxed(map)?,
                }
            }
            TExprKind::Index { expr, args } => {
                // The node's own proofs precede its children.
                let (first, rest) = args.split_first();
                let first = PreparedIndexArg::new(first, map, span)?;
                let rest = rest
                    .iter()
                    .map(|arg| PreparedIndexArg::new(arg, map, span))
                    .collect::<Result<Vec<_>, M::Error>>()?;
                let prepared = crate::syntax::non_empty::NonEmpty::new(first, rest);
                let expr = expr.boxed(map)?;
                let args = prepared.try_map(|arg| {
                    Ok(match arg {
                        PreparedIndexArg::Variant(variant) => TIndexArg::Variant(variant.clone()),
                        PreparedIndexArg::Var(local) => TIndexArg::Var(local.clone()),
                        PreparedIndexArg::Key(operand) => TIndexArg::Key(operand.boxed(map)?),
                        PreparedIndexArg::Position(operand, position) => TIndexArg::Position {
                            operand: operand.boxed(map)?,
                            position,
                        },
                    })
                })?;
                TExprKind::Index { expr, args }
            }
            TExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => TExprKind::Scan {
                source: source.boxed(map)?,
                init: init.boxed(map)?,
                acc: acc.clone(),
                val: val.clone(),
                body: body.boxed(map)?,
            },
            TExprKind::Unfold {
                recurrence,
                init,
                body,
                axis,
            } => {
                let axis = map.axis(axis, nested_index(ty, 0), span)?;
                TExprKind::Unfold {
                    recurrence: recurrence.clone(),
                    init: init.boxed(map)?,
                    body: body.boxed(map)?,
                    axis,
                }
            }
            TExprKind::Key { form, arg, axis } => {
                let form = match form {
                    TKeyForm::Static { position, key } => {
                        let position = map.static_position(position, span)?;
                        let key = map.key(
                            key,
                            key_index(ty),
                            KeyEntry::Position(position.position),
                            span,
                        )?;
                        TKeyForm::Static { position, key }
                    }
                    TKeyForm::Fin => TKeyForm::Fin,
                    TKeyForm::Search(search) => TKeyForm::Search(*search),
                };
                let axis = map.axis(axis, key_index(ty), span)?;
                TExprKind::Key {
                    form,
                    arg: arg.boxed(map)?,
                    axis,
                }
            }
            TExprKind::Match { scrutinee, arms } => {
                // The node's own match targets precede its children.
                let targets = match arms {
                    TMatchArms::Labels(_) => Vec::new(),
                    TMatchArms::Constructors(arms) => arms
                        .iter()
                        .map(|arm| map.match_target(&arm.target))
                        .collect(),
                };
                let scrutinee = scrutinee.boxed(map)?;
                let arms = match arms {
                    TMatchArms::Labels(arms) => TMatchArms::Labels(
                        arms.iter()
                            .map(|arm| {
                                Ok(TLabelArm {
                                    label: arm.label.clone(),
                                    body: arm.body.map_types(map)?,
                                    span: arm.span,
                                })
                            })
                            .collect::<Result<_, M::Error>>()?,
                    ),
                    TMatchArms::Constructors(arms) => TMatchArms::Constructors(
                        arms.iter()
                            .zip(targets)
                            .map(|(arm, target)| {
                                Ok(TConstructorArm {
                                    target,
                                    bindings: arm.bindings.clone(),
                                    body: arm.body.map_types(map)?,
                                    span: arm.span,
                                })
                            })
                            .collect::<Result<_, M::Error>>()?,
                    ),
                };
                TExprKind::Match { scrutinee, arms }
            }
            TExprKind::Variant { variant, key } => {
                let index = IndexTypeRef::from_resolved(variant.variant.index().clone());
                TExprKind::Variant {
                    variant: variant.clone(),
                    key: map.key(
                        key,
                        Some(&index),
                        KeyEntry::Label(variant.variant.variant()),
                        span,
                    )?,
                }
            }
            TExprKind::DagCall {
                slot,
                args,
                static_bindings,
                output,
            } => TExprKind::DagCall {
                slot: map.call_slot(*slot),
                args: args
                    .iter()
                    .map(|binding| {
                        Ok(TParamBinding {
                            target: binding.target.clone(),
                            value: binding.value.map_types(map)?,
                        })
                    })
                    .collect::<Result<_, M::Error>>()?,
                static_bindings: static_bindings.clone(),
                output: output.clone(),
            },
        })
    }
}
