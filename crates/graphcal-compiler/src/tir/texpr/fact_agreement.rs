//! Agreement between typed trees and the retained expression facts.
//!
//! While both records exist, publication requires that they say the same
//! thing about every checked expression. This module is deleted together with
//! the expression facts once the evaluator consumes typed trees.

use thiserror::Error;

use crate::expression_id::ExprId;
use crate::registry::checked_type::Symbolic;
use crate::tir::expression_facts::{
    CheckedExpressionFacts, CheckedExpressionRecord, ExpressionFact,
};

use super::model::{
    StaticPosition, TConstRef, TExpr, TExprKind, TIndexArg, TMatchPattern, TNodeRef, visit_tnodes,
};
use super::typed_bodies::TypedBodies;

/// A way the typed trees and the expression facts disagree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FactDisagreement {
    #[error("typed node {0:?} has no expression fact")]
    MissingFact(ExprId),
    #[error("{typed} typed nodes but {facts} expression facts")]
    Coverage { typed: usize, facts: usize },
    #[error("typed node {0:?} and its fact disagree on the contextual literal")]
    Contextual(ExprId),
    #[error("typed node {0:?} and its fact disagree on the checked type")]
    Type(ExprId),
    #[error("typed node {0:?} and its fact disagree on the constructor application")]
    Constructor(ExprId),
    #[error("typed node {0:?} and its fact disagree on the match targets")]
    MatchTargets(ExprId),
    #[error("typed node {0:?} and its fact disagree on the static positions")]
    StaticPositions(ExprId),
}

/// Require every typed node to agree with the fact retained for it, and every
/// fact to belong to a typed node.
///
/// # Errors
///
/// Returns the first disagreement found.
pub fn check(
    bodies: &TypedBodies<Symbolic>,
    facts: &CheckedExpressionFacts,
) -> Result<(), FactDisagreement> {
    let mut typed = 0_usize;
    let mut result = Ok(());
    for (_, body) in bodies.roots() {
        visit_tnodes(body.as_node(), &mut |node| {
            if result.is_err() {
                return;
            }
            typed = typed.saturating_add(1);
            result = facts
                .get(node.id())
                .map_err(|_| FactDisagreement::MissingFact(node.id().clone()))
                .and_then(|record| agrees(node, record));
        });
    }
    result?;
    let fact_count = facts.records().count();
    if typed == fact_count {
        Ok(())
    } else {
        Err(FactDisagreement::Coverage {
            typed,
            facts: fact_count,
        })
    }
}

fn agrees(
    node: TNodeRef<'_, Symbolic>,
    record: &CheckedExpressionRecord,
) -> Result<(), FactDisagreement> {
    let expr = match node {
        TNodeRef::Contextual(literal) => {
            return match record.fact {
                ExpressionFact::Contextual(operand) if operand == literal.literal().operand() => {
                    Ok(())
                }
                _ => Err(FactDisagreement::Contextual(literal.id().clone())),
            };
        }
        TNodeRef::Value(expr) => expr,
    };
    let id = || expr.id().clone();
    let value = record
        .fact
        .symbolic_value()
        .ok_or_else(|| FactDisagreement::Type(id()))?;
    if &value.checked_type != expr.ty() {
        return Err(FactDisagreement::Type(id()));
    }
    let application = match expr.kind() {
        TExprKind::Construct { application, .. }
        | TExprKind::Const(TConstRef::Constructor(application)) => Some(application),
        _ => None,
    };
    if application != value.constructor.as_deref() {
        return Err(FactDisagreement::Constructor(id()));
    }
    if !match_targets_agree(expr, record) {
        return Err(FactDisagreement::MatchTargets(id()));
    }
    if static_positions(expr)
        .into_iter()
        .map(|(operand, position)| (operand, &position.axis, position.position))
        .ne(record.static_indexes.iter().map(|requirement| {
            (
                &requirement.operand,
                &requirement.axis,
                requirement.position,
            )
        }))
    {
        return Err(FactDisagreement::StaticPositions(id()));
    }
    Ok(())
}

fn match_targets_agree(expr: &TExpr<Symbolic>, record: &CheckedExpressionRecord) -> bool {
    let targets: Vec<_> = match expr.kind() {
        TExprKind::Match { arms, .. } => arms
            .iter()
            .filter_map(|arm| match &arm.pattern {
                TMatchPattern::Constructor { target, .. } => Some(target),
                TMatchPattern::IndexLabel(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    targets.iter().all(|target| {
        record
            .constructor_matches
            .values()
            .any(|fact| fact == *target)
    }) && record
        .constructor_matches
        .values()
        .all(|fact| targets.contains(&fact))
}

fn static_positions(expr: &TExpr<Symbolic>) -> Vec<(&ExprId, &StaticPosition<Symbolic>)> {
    match expr.kind() {
        TExprKind::Index { args, .. } => args
            .iter()
            .filter_map(|arg| match arg {
                TIndexArg::Expr {
                    operand,
                    static_position: Some(position),
                } => Some((operand.id(), position)),
                TIndexArg::Expr { .. } | TIndexArg::Variant(_) | TIndexArg::Var(_) => None,
            })
            .collect(),
        TExprKind::Key {
            arg,
            static_position: Some(position),
            ..
        } => vec![(arg.id(), position)],
        _ => Vec::new(),
    }
}
