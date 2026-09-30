//! Structure-preserving maps over typed trees that rewrite only their types.
//!
//! Discharging a symbolic tree to a concrete one, specializing a template's
//! tree into an instance, and binding a generic bound's `Nat` parameters all
//! keep a tree's shape and rewrite the checked types, constructor
//! applications, match targets, and static positions it carries. The one
//! traversal lives here; each use supplies a [`TypeMap`].

use std::borrow::Cow;

use super::nominal::{ConstructorApplication, ConstructorMatch};
use crate::semantic::checked_type::{CheckedType, Concrete, Concreteness, IndexTypeRef, Symbolic};
use crate::syntax::span::{Span, Spanned};

use super::model::{
    StaticPosition, TBody, TConstRef, TConstructorArm, TExpr, TExprKind, TExternArg, TFieldInit,
    TIndexArg, TKeyForm, TLabelArm, TMapEntry, TMatchArms, TParamBinding,
};

/// How a structure-preserving map rewrites the types a tree carries.
///
/// Within one node the map is asked, in order, for the node's type, its
/// constructor application, the static positions it proves, and its match
/// targets; the node's children follow in structural order, so failures are
/// reported for the first offending node in pre-order.
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

    /// A constructor match arm's resolved target.
    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch;
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

/// A symbolic tree some of whose types still mention a `Nat` variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotConcrete;

/// Close every type of a symbolic tree that mentions no `Nat` variable.
pub struct ToConcrete;

impl TypeMap<Symbolic, Concrete> for ToConcrete {
    type Error = NotConcrete;

    fn node_type(
        &mut self,
        ty: &CheckedType<Symbolic>,
        _span: Span,
    ) -> Result<CheckedType<Concrete>, NotConcrete> {
        ty.to_concrete().ok_or(NotConcrete)
    }

    fn application(
        &mut self,
        application: &ConstructorApplication<Symbolic>,
        _ty: &CheckedType<Concrete>,
        _span: Span,
    ) -> Result<ConstructorApplication<Concrete>, NotConcrete> {
        let applied = &application.applied;
        let generic_args = applied
            .generic_args()
            .iter()
            .map(crate::semantic::checked_type::CheckedGenericArg::to_concrete)
            .collect::<Option<_>>()
            .ok_or(NotConcrete)?;
        Ok(ConstructorApplication {
            constructor: application.constructor.clone(),
            applied: std::sync::Arc::new(applied.try_map_types(
                applied.runtime_type().clone(),
                generic_args,
                |ty| ty.to_concrete().ok_or(NotConcrete),
            )?),
        })
    }

    fn static_position(
        &mut self,
        position: &StaticPosition<Symbolic>,
        _span: Span,
    ) -> Result<StaticPosition<Concrete>, NotConcrete> {
        Ok(StaticPosition {
            axis: position.axis.to_concrete().ok_or(NotConcrete)?,
            position: position.position,
            usage: position.usage,
        })
    }

    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch {
        target.clone()
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
            TExprKind::KeyShift { key, addend } => TExprKind::KeyShift {
                key: key.boxed(map)?,
                addend: addend.boxed(map)?,
            },
            TExprKind::GraphRef(target) => TExprKind::GraphRef(target.clone()),
            TExprKind::Const(target) => TExprKind::Const(Spanned::new(
                match &target.value {
                    TConstRef::Decl(declaration) => TConstRef::Decl(declaration.clone()),
                    TConstRef::Constructor(application) => {
                        TConstRef::Constructor(map.application(application, ty, span)?)
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
            TExprKind::Extern { function, args } => TExprKind::Extern {
                function: function.clone(),
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
            TExprKind::Construct {
                application,
                fields,
            } => TExprKind::Construct {
                application: map.application(application, ty, span)?,
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(TFieldInit {
                            name: field.name.clone(),
                            value: field.value.map_types(map)?,
                        })
                    })
                    .collect::<Result<_, M::Error>>()?,
            },
            TExprKind::Map { entries } => TExprKind::Map {
                entries: entries
                    .iter()
                    .map(|entry| {
                        Ok(TMapEntry {
                            keys: entry.keys.clone(),
                            value: entry.value.map_types(map)?,
                        })
                    })
                    .collect::<Result<_, M::Error>>()?,
            },
            TExprKind::For { bindings, body } => TExprKind::For {
                bindings: bindings.clone(),
                body: body.boxed(map)?,
            },
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
            } => TExprKind::Unfold {
                recurrence: recurrence.clone(),
                init: init.boxed(map)?,
                body: body.boxed(map)?,
            },
            TExprKind::Key { form, axis, arg } => {
                let form = match form {
                    TKeyForm::Static(position) => {
                        TKeyForm::Static(map.static_position(position, span)?)
                    }
                    TKeyForm::Fin => TKeyForm::Fin,
                    TKeyForm::Search(search) => TKeyForm::Search(*search),
                };
                TExprKind::Key {
                    form,
                    axis: axis.clone(),
                    arg: arg.boxed(map)?,
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
            TExprKind::Variant(variant) => TExprKind::Variant(variant.clone()),
            TExprKind::DagCall {
                target,
                args,
                static_bindings,
                output,
            } => TExprKind::DagCall {
                target: target.clone(),
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
