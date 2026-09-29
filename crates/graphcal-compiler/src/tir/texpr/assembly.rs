//! Bottom-up assembly of typed trees while one checking pass records nodes.
//!
//! Inference records every expression after its children, so each checked
//! node is assembled from the typed children already recorded for it. The
//! mapping from a HIR node to its typed form is the only place a [`TExpr`] is
//! built from inference results, and it rejects any shape the checker did not
//! prove: a missing or contextual child in a value position, a constructor
//! without its application, a match arm without its target, or a static
//! position that belongs to no selector of the node.

use std::collections::HashMap;

use thiserror::Error;

use crate::expression_id::ExprId;
use crate::hir::expr::{ConstRef, Expr, ExprKind, IndexArg, MatchPattern};
use crate::registry::checked_type::{CheckedType, Symbolic};
use crate::resolved_name::ResolvedConstructorName;
use crate::syntax::span::Spanned;
use crate::tir::expression_facts::{
    ConstructorApplication, ConstructorMatch, ContextualOperand, StaticIndexRequirement,
};

use super::model::{
    ContextualLiteral, StaticPosition, TArg, TConstRef, TContextual, TExpr, TExprKind, TFieldInit,
    TIndexArg, TMapEntry, TMatchArm, TMatchPattern, TParamBinding,
};

/// Why a checked node could not be assembled into a typed tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssemblyError {
    #[error("expression checked more than once in one checking pass: {0:?}")]
    CheckedTwice(ExprId),
    #[error("checked expression {parent:?} has an unchecked child {child:?}")]
    UncheckedChild { parent: ExprId, child: ExprId },
    #[error("contextual literal {0:?} is used as a value")]
    ContextualValue(ExprId),
    #[error("expression {0:?} is not a contextual literal")]
    NotContextual(ExprId),
    #[error("constructor {0:?} has no checked application")]
    MissingApplication(ExprId),
    #[error("expression {0:?} carries a constructor application it does not apply")]
    UnappliedApplication(ExprId),
    #[error("a constructor arm of match {0:?} has no checked target")]
    MissingMatchTarget(ExprId),
    #[error("a static position of {0:?} belongs to none of its selectors")]
    UnplacedStaticPosition(ExprId),
}

/// The node-specific facts checking established for one value expression.
pub struct NodeFacts<'a> {
    pub constructor: Option<&'a ConstructorApplication<Symbolic>>,
    pub constructor_matches: &'a HashMap<ResolvedConstructorName, ConstructorMatch>,
    pub static_indexes: &'a [StaticIndexRequirement],
}

/// Typed nodes recorded so far whose parent has not been recorded yet.
///
/// After a successful pass only the checked roots remain.
#[derive(Debug, Default)]
pub struct PendingNodes {
    nodes: HashMap<ExprId, TArg<Symbolic>>,
}

impl PendingNodes {
    /// Record a checked value expression, consuming its recorded children.
    pub fn record_value(
        &mut self,
        expr: &Expr,
        ty: CheckedType<Symbolic>,
        facts: &NodeFacts<'_>,
    ) -> Result<(), AssemblyError> {
        let kind = self.assemble(expr, facts)?;
        self.insert(
            expr.id(),
            TArg::Value(Box::new(TExpr::new(expr.id().clone(), expr.span, ty, kind))),
        )
    }

    /// Record a contextual literal accepted by its consumer.
    pub fn record_contextual(&mut self, expr: &Expr) -> Result<ContextualOperand, AssemblyError> {
        let literal = match expr.kind() {
            ExprKind::StringLiteral(value) => ContextualLiteral::String(value.clone()),
            ExprKind::OffsetDateTimeLiteral(value) => ContextualLiteral::OffsetDateTime(*value),
            ExprKind::CivilDateTimeLiteral(value) => ContextualLiteral::CivilDateTime(*value),
            ExprKind::ZonedDateTimeLiteral(value) => {
                ContextualLiteral::ZonedDateTime(value.clone())
            }
            ExprKind::IanaTimeZoneLiteral(value) => ContextualLiteral::TimeZone(value.clone()),
            _ => return Err(AssemblyError::NotContextual(expr.id().clone())),
        };
        let operand = literal.operand();
        self.insert(
            expr.id(),
            TArg::Contextual(TContextual::new(expr.id().clone(), expr.span, literal)),
        )?;
        Ok(operand)
    }

    /// Take the typed node of a checked root out of the pending set.
    pub fn take_root(&mut self, root: &ExprId) -> Option<TArg<Symbolic>> {
        self.nodes.remove(root)
    }

    /// Any recorded node that no parent and no root has claimed.
    pub fn unclaimed(&self) -> Option<&ExprId> {
        self.nodes.keys().next()
    }

    /// Every node no parent has claimed: the trees of the roots checked.
    pub fn into_roots(self) -> HashMap<ExprId, super::model::TBody<Symbolic>> {
        self.nodes
            .into_iter()
            .map(|(id, node)| {
                let body = match node {
                    TArg::Value(value) => super::model::TBody::Value(value),
                    TArg::Contextual(literal) => super::model::TBody::Contextual(literal),
                };
                (id, body)
            })
            .collect()
    }

    fn insert(&mut self, id: &ExprId, node: TArg<Symbolic>) -> Result<(), AssemblyError> {
        match self.nodes.entry(id.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(node);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(AssemblyError::CheckedTwice(id.clone()))
            }
        }
    }

    fn take_arg(&mut self, parent: &Expr, child: &Expr) -> Result<TArg<Symbolic>, AssemblyError> {
        self.nodes
            .remove(child.id())
            .ok_or_else(|| AssemblyError::UncheckedChild {
                parent: parent.id().clone(),
                child: child.id().clone(),
            })
    }

    fn take_boxed(
        &mut self,
        parent: &Expr,
        child: &Expr,
    ) -> Result<Box<TExpr<Symbolic>>, AssemblyError> {
        match self.take_arg(parent, child)? {
            TArg::Value(value) => Ok(value),
            TArg::Contextual(literal) => Err(AssemblyError::ContextualValue(literal.id().clone())),
        }
    }

    fn take_value(
        &mut self,
        parent: &Expr,
        child: &Expr,
    ) -> Result<TExpr<Symbolic>, AssemblyError> {
        self.take_boxed(parent, child).map(|value| *value)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive mapping from each HIR expression kind to its typed form"
    )]
    fn assemble(
        &mut self,
        expr: &Expr,
        facts: &NodeFacts<'_>,
    ) -> Result<TExprKind<Symbolic>, AssemblyError> {
        let id = || expr.id().clone();
        let mut positions = StaticPositions::new(facts.static_indexes);
        let application = || {
            facts
                .constructor
                .cloned()
                .ok_or_else(|| AssemblyError::MissingApplication(id()))
        };
        let applies_constructor = matches!(
            expr.kind(),
            ExprKind::ConstructorCall { .. }
                | ExprKind::ConstRef(Spanned {
                    value: ConstRef::Constructor(_),
                    ..
                })
        );
        if facts.constructor.is_some() && !applies_constructor {
            return Err(AssemblyError::UnappliedApplication(id()));
        }
        let kind = match expr.kind() {
            ExprKind::Error(no_error) => no_error.absurd(),
            ExprKind::Number(value) => TExprKind::Number(*value),
            ExprKind::Integer(value) => TExprKind::Integer(*value),
            ExprKind::Bool(value) => TExprKind::Bool(*value),
            ExprKind::StringLiteral(_)
            | ExprKind::OffsetDateTimeLiteral(_)
            | ExprKind::CivilDateTimeLiteral(_)
            | ExprKind::ZonedDateTimeLiteral(_)
            | ExprKind::IanaTimeZoneLiteral(_)
            | ExprKind::TypeSystemRef(_) => return Err(AssemblyError::ContextualValue(id())),
            ExprKind::QuantityLiteral { value, unit } => TExprKind::Quantity {
                value: *value,
                unit: unit.clone(),
            },
            ExprKind::VariantLiteral(variant) => TExprKind::Variant(variant.clone()),
            ExprKind::GraphRef(target) => TExprKind::GraphRef(target.clone()),
            ExprKind::ConstRef(target) => TExprKind::Const(Spanned::new(
                match &target.value {
                    ConstRef::Decl(declaration) => TConstRef::Decl(declaration.clone()),
                    ConstRef::Builtin(constant) => TConstRef::Builtin(*constant),
                    ConstRef::Constructor(_) => TConstRef::Constructor(application()?),
                },
                target.span,
            )),
            ExprKind::LocalRef(local) => TExprKind::Local(local.clone()),
            ExprKind::BinOp { op, lhs, rhs } => TExprKind::Binary {
                op: *op,
                lhs: self.take_boxed(expr, lhs)?,
                rhs: self.take_boxed(expr, rhs)?,
            },
            ExprKind::UnaryOp { op, operand } => TExprKind::Unary {
                op: *op,
                operand: self.take_boxed(expr, operand)?,
            },
            ExprKind::FnCall { callee, args } => TExprKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|arg| self.take_arg(expr, arg))
                    .collect::<Result<_, _>>()?,
            },
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => TExprKind::If {
                condition: self.take_boxed(expr, condition)?,
                then_branch: self.take_boxed(expr, then_branch)?,
                else_branch: self.take_boxed(expr, else_branch)?,
            },
            ExprKind::Convert {
                expr: inner,
                target,
            } => TExprKind::Convert {
                expr: self.take_boxed(expr, inner)?,
                target: target.clone(),
            },
            ExprKind::DisplayTimezone {
                expr: inner,
                timezone,
            } => TExprKind::DisplayTimezone {
                expr: self.take_boxed(expr, inner)?,
                timezone: timezone.clone(),
            },
            ExprKind::FieldAccess { expr: inner, field } => TExprKind::Field {
                expr: self.take_boxed(expr, inner)?,
                field: field.clone(),
            },
            ExprKind::ConstructorCall { fields, .. } => TExprKind::Construct {
                application: application()?,
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(TFieldInit {
                            name: field.name.value.clone(),
                            value: self.take_value(expr, &field.value)?,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
            },
            ExprKind::MapLiteral { entries } => TExprKind::Map {
                entries: entries
                    .iter()
                    .map(|entry| {
                        Ok(TMapEntry {
                            keys: entry.keys.clone(),
                            value: self.take_value(expr, &entry.value)?,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
            },
            ExprKind::ForComp { bindings, body } => TExprKind::For {
                bindings: bindings.clone(),
                body: self.take_boxed(expr, body)?,
            },
            ExprKind::IndexAccess { expr: inner, args } => TExprKind::Index {
                expr: self.take_boxed(expr, inner)?,
                args: args.try_map_ref(|arg| {
                    Ok::<_, AssemblyError>(match arg {
                        IndexArg::Variant(variant) => TIndexArg::Variant(variant.clone()),
                        IndexArg::Var(local) => TIndexArg::Var(local.clone()),
                        IndexArg::Expr(operand) => TIndexArg::Expr {
                            static_position: positions.take(operand.id()),
                            operand: self.take_boxed(expr, operand)?,
                        },
                    })
                })?,
            },
            ExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => TExprKind::Scan {
                source: self.take_boxed(expr, source)?,
                init: self.take_boxed(expr, init)?,
                acc: acc.clone(),
                val: val.clone(),
                body: self.take_boxed(expr, body)?,
            },
            ExprKind::Unfold {
                recurrence,
                init,
                body,
            } => TExprKind::Unfold {
                recurrence: recurrence.clone(),
                init: self.take_boxed(expr, init)?,
                body: self.take_boxed(expr, body)?,
            },
            ExprKind::KeyForm {
                kind, axis, arg, ..
            } => TExprKind::Key {
                kind: *kind,
                axis: axis.clone(),
                static_position: positions.take(arg.id()),
                arg: self.take_boxed(expr, arg)?,
            },
            ExprKind::Match { scrutinee, arms } => TExprKind::Match {
                scrutinee: self.take_boxed(expr, scrutinee)?,
                arms: arms
                    .iter()
                    .map(|arm| {
                        let pattern = match &arm.pattern {
                            MatchPattern::Constructor {
                                constructor,
                                bindings,
                                ..
                            } => TMatchPattern::Constructor {
                                target: facts
                                    .constructor_matches
                                    .get(&constructor.value)
                                    .cloned()
                                    .ok_or_else(|| AssemblyError::MissingMatchTarget(id()))?,
                                bindings: bindings.clone(),
                            },
                            MatchPattern::IndexLabel { variant, .. } => {
                                TMatchPattern::IndexLabel(variant.clone())
                            }
                        };
                        Ok(TMatchArm {
                            pattern,
                            body: self.take_value(expr, &arm.body)?,
                            span: arm.span,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
            },
            ExprKind::DagCall {
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
                            target: binding.target.value.clone(),
                            value: self.take_value(expr, &binding.value)?,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
                static_bindings: static_bindings.clone(),
                output: output.clone(),
            },
        };
        if positions.all_placed() {
            Ok(kind)
        } else {
            Err(AssemblyError::UnplacedStaticPosition(id()))
        }
    }
}

/// The static index proofs of one node, placed on the selectors they prove.
struct StaticPositions<'a> {
    requirements: &'a [StaticIndexRequirement],
    placed: usize,
}

impl<'a> StaticPositions<'a> {
    const fn new(requirements: &'a [StaticIndexRequirement]) -> Self {
        Self {
            requirements,
            placed: 0,
        }
    }

    /// The proof for `operand`: requirements are retained in selector order.
    fn take(&mut self, operand: &ExprId) -> Option<StaticPosition<Symbolic>> {
        let requirement = self.requirements.get(self.placed)?;
        (requirement.operand == *operand).then(|| {
            self.placed = self.placed.saturating_add(1);
            StaticPosition {
                axis: requirement.axis.clone(),
                position: requirement.position,
                usage: requirement.usage,
            }
        })
    }

    const fn all_placed(&self) -> bool {
        self.placed == self.requirements.len()
    }
}
