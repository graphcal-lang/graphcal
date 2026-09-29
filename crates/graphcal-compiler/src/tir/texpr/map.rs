//! Structure-preserving maps over typed trees that rewrite only their types.
//!
//! Discharging a symbolic tree to a concrete one, specializing a template's
//! tree into an instance, and binding a generic bound's `Nat` parameters all
//! keep a tree's shape and rewrite the checked types, constructor
//! applications, match targets, and static positions it carries. The one
//! traversal lives here; each use supplies a [`TypeMap`].

use std::borrow::Cow;

use crate::registry::checked_type::{CheckedType, Concrete, Concreteness, IndexTypeRef, Symbolic};
use crate::syntax::span::{Span, Spanned};
use crate::tir::expression_facts::{ConstructorApplication, ConstructorMatch};

use super::model::{
    StaticPosition, TArg, TBody, TConstRef, TExpr, TExprKind, TFieldInit, TIndexArg, TMapEntry,
    TMatchArm, TMatchPattern, TParamBinding,
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
        Ok(ConstructorApplication {
            constructor: application.constructor.clone(),
            runtime_type: application.runtime_type.clone(),
            generic_args: application
                .generic_args
                .iter()
                .map(crate::registry::checked_type::CheckedGenericArg::to_concrete)
                .collect::<Option<_>>()
                .ok_or(NotConcrete)?,
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

/// View a concrete tree at the symbolic level.
pub struct ToSymbolic;

impl TypeMap<Concrete, Symbolic> for ToSymbolic {
    type Error = std::convert::Infallible;

    fn node_type(
        &mut self,
        ty: &CheckedType<Concrete>,
        _span: Span,
    ) -> Result<CheckedType<Symbolic>, Self::Error> {
        Ok(ty.to_symbolic())
    }

    fn application(
        &mut self,
        application: &ConstructorApplication<Concrete>,
        _ty: &CheckedType<Symbolic>,
        _span: Span,
    ) -> Result<ConstructorApplication<Symbolic>, Self::Error> {
        Ok(ConstructorApplication {
            constructor: application.constructor.clone(),
            runtime_type: application.runtime_type.clone(),
            generic_args: application
                .generic_args
                .iter()
                .map(crate::registry::checked_type::CheckedGenericArg::to_symbolic)
                .collect(),
        })
    }

    fn static_position(
        &mut self,
        position: &StaticPosition<Concrete>,
        _span: Span,
    ) -> Result<StaticPosition<Symbolic>, Self::Error> {
        Ok(StaticPosition {
            axis: position.axis.to_symbolic(),
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
            TExprKind::Number(value) => TExprKind::Number(*value),
            TExprKind::Integer(value) => TExprKind::Integer(*value),
            TExprKind::Bool(value) => TExprKind::Bool(*value),
            TExprKind::Quantity { value, unit } => TExprKind::Quantity {
                value: *value,
                unit: unit.clone(),
            },
            TExprKind::GraphRef(target) => TExprKind::GraphRef(target.clone()),
            TExprKind::Const(target) => TExprKind::Const(Spanned::new(
                match &target.value {
                    TConstRef::Decl(declaration) => TConstRef::Decl(declaration.clone()),
                    TConstRef::Builtin(constant) => TConstRef::Builtin(*constant),
                    TConstRef::Constructor(application) => {
                        TConstRef::Constructor(map.application(application, ty, span)?)
                    }
                },
                target.span,
            )),
            TExprKind::Local(local) => TExprKind::Local(local.clone()),
            TExprKind::Binary { op, lhs, rhs } => TExprKind::Binary {
                op: *op,
                lhs: lhs.boxed(map)?,
                rhs: rhs.boxed(map)?,
            },
            TExprKind::Unary { op, operand } => TExprKind::Unary {
                op: *op,
                operand: operand.boxed(map)?,
            },
            TExprKind::Call { callee, args } => TExprKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|arg| {
                        Ok(match arg {
                            TArg::Value(value) => TArg::Value(value.boxed(map)?),
                            TArg::Contextual(literal) => TArg::Contextual(literal.clone()),
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
                let mut positions = args
                    .iter()
                    .map(|arg| match arg {
                        TIndexArg::Expr {
                            static_position: Some(position),
                            ..
                        } => map.static_position(position, span).map(Some),
                        TIndexArg::Expr { .. } | TIndexArg::Variant(_) | TIndexArg::Var(_) => {
                            Ok(None)
                        }
                    })
                    .collect::<Result<Vec<_>, M::Error>>()?
                    .into_iter();
                let expr = expr.boxed(map)?;
                let args = args.try_map_ref(|arg| {
                    let static_position = positions.next().flatten();
                    Ok(match arg {
                        TIndexArg::Variant(variant) => TIndexArg::Variant(variant.clone()),
                        TIndexArg::Var(local) => TIndexArg::Var(local.clone()),
                        TIndexArg::Expr { operand, .. } => TIndexArg::Expr {
                            operand: operand.boxed(map)?,
                            static_position,
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
            TExprKind::Key {
                kind,
                axis,
                arg,
                static_position,
            } => {
                let static_position = static_position
                    .as_ref()
                    .map(|position| map.static_position(position, span))
                    .transpose()?;
                TExprKind::Key {
                    kind: *kind,
                    axis: axis.clone(),
                    arg: arg.boxed(map)?,
                    static_position,
                }
            }
            TExprKind::Match { scrutinee, arms } => {
                let patterns = arms
                    .iter()
                    .map(|arm| match &arm.pattern {
                        TMatchPattern::Constructor { target, bindings } => {
                            TMatchPattern::Constructor {
                                target: map.match_target(target),
                                bindings: bindings.clone(),
                            }
                        }
                        TMatchPattern::IndexLabel(variant) => {
                            TMatchPattern::IndexLabel(variant.clone())
                        }
                    })
                    .collect::<Vec<_>>();
                let scrutinee = scrutinee.boxed(map)?;
                let arms = arms
                    .iter()
                    .zip(patterns)
                    .map(|(arm, pattern)| {
                        Ok(TMatchArm {
                            pattern,
                            body: arm.body.map_types(map)?,
                            span: arm.span,
                        })
                    })
                    .collect::<Result<_, M::Error>>()?;
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
