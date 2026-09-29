use std::collections::HashMap;

use super::*;
use crate::dimension::Dimension;
use crate::hir::expr::{CheckedExpr, Draft, Expr, ExprKind};
use crate::registry::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::syntax::span::Span;
use crate::tir::expression_facts::{ConstructorMatch, ContextualOperand, StaticIndexRequirement};

fn integer(value: i64, offset: usize) -> Expr<Draft> {
    Expr::new(ExprKind::Integer(value), Span::new(offset, 1))
}

fn finished(expr: Expr<Draft>) -> CheckedExpr {
    CheckedExpr::from_draft_for_test(expr)
}

fn children(expr: &Expr) -> Vec<&Expr> {
    let mut children = Vec::new();
    crate::hir::expr::visit_expr_children(expr, &mut |child| children.push(child));
    children
}

fn dimensionless() -> CheckedType<Symbolic> {
    CheckedType::Quantity(Dimension::dimensionless())
}

fn no_facts(
    matches: &HashMap<crate::resolved_name::ResolvedConstructorName, ConstructorMatch>,
) -> NodeFacts<'_> {
    NodeFacts {
        constructor: None,
        constructor_matches: matches,
        static_indexes: &[],
    }
}

fn binary() -> CheckedExpr {
    finished(Expr::new(
        ExprKind::BinOp {
            op: crate::syntax::ast::BinOp::Add,
            lhs: Box::new(integer(1, 0)),
            rhs: Box::new(integer(2, 4)),
        },
        Span::new(0, 5),
    ))
}

#[test]
fn a_parent_assembles_from_its_recorded_children() {
    let expr = binary();
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    for child in children(&expr) {
        pending
            .record_value(child, dimensionless(), &no_facts(&matches))
            .unwrap();
    }
    pending
        .record_value(&expr, dimensionless(), &no_facts(&matches))
        .unwrap();
    let bodies = TypedBodies::publish(&[&expr], pending).unwrap();
    let Some(TBody::Value(root)) = bodies.get(expr.id()) else {
        panic!("expected a typed value root");
    };
    let TExprKind::Binary { lhs, rhs, .. } = root.kind() else {
        panic!("expected a binary node, got {:?}", root.kind());
    };
    assert!(matches!(lhs.kind(), TExprKind::Integer(1)));
    assert!(matches!(rhs.kind(), TExprKind::Integer(2)));
    let mut ids = Vec::new();
    visit_tnodes(TNodeRef::Value(root), &mut |node| {
        ids.push(node.id().clone());
    });
    let mut expected = Vec::new();
    crate::hir::expr::visit_expr(&expr, &mut |node| expected.push(node.id().clone()));
    assert_eq!(ids, expected);
}

#[test]
fn a_parent_without_its_checked_children_is_rejected() {
    let expr = binary();
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    assert!(matches!(
        pending.record_value(&expr, dimensionless(), &no_facts(&matches)),
        Err(AssemblyError::UncheckedChild { .. })
    ));
}

#[test]
fn a_node_recorded_twice_is_rejected() {
    let expr = finished(integer(1, 0));
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    pending
        .record_value(&expr, dimensionless(), &no_facts(&matches))
        .unwrap();
    assert_eq!(
        pending.record_value(&expr, dimensionless(), &no_facts(&matches)),
        Err(AssemblyError::CheckedTwice(expr.id().clone()))
    );
}

#[test]
fn contextual_literals_are_never_values() {
    let literal = finished(Expr::new(
        ExprKind::StringLiteral("red".to_string()),
        Span::new(0, 5),
    ));
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    assert_eq!(
        pending.record_value(&literal, dimensionless(), &no_facts(&matches)),
        Err(AssemblyError::ContextualValue(literal.id().clone()))
    );
    assert_eq!(
        pending.record_contextual(&literal),
        Ok(ContextualOperand::String)
    );

    let negated = finished(Expr::new(
        ExprKind::UnaryOp {
            op: crate::syntax::ast::UnaryOp::Neg,
            operand: Box::new(Expr::new(
                ExprKind::StringLiteral("red".to_string()),
                Span::new(1, 5),
            )),
        },
        Span::new(0, 6),
    ));
    let mut pending = PendingNodes::default();
    pending.record_contextual(children(&negated)[0]).unwrap();
    assert!(matches!(
        pending.record_value(&negated, dimensionless(), &no_facts(&matches)),
        Err(AssemblyError::ContextualValue(_))
    ));
}

#[test]
fn only_contextual_literals_are_recorded_as_contextual() {
    let expr = finished(integer(1, 0));
    let mut pending = PendingNodes::default();
    assert_eq!(
        pending.record_contextual(&expr),
        Err(AssemblyError::NotContextual(expr.id().clone()))
    );
}

#[test]
fn a_static_position_must_belong_to_a_selector_of_its_node() {
    let expr = binary();
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    for child in children(&expr) {
        pending
            .record_value(child, dimensionless(), &no_facts(&matches))
            .unwrap();
    }
    let requirement = StaticIndexRequirement {
        operand: children(&expr)[0].id().clone(),
        axis: IndexTypeRef::from_finite_index(crate::registry::index::FiniteIndex::new(
            crate::registry::index::IndexCardinality::try_from_u64(2).unwrap(),
        ))
        .to_symbolic(),
        position: 1,
        usage: crate::tir::expression_facts::StaticIndexUse::Selection,
    };
    assert_eq!(
        pending.record_value(
            &expr,
            dimensionless(),
            &NodeFacts {
                constructor: None,
                constructor_matches: &matches,
                static_indexes: std::slice::from_ref(&requirement),
            },
        ),
        Err(AssemblyError::UnplacedStaticPosition(expr.id().clone()))
    );
}

#[test]
fn publication_requires_every_root_and_claims_every_node() {
    let expr = binary();
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    for child in children(&expr) {
        pending
            .record_value(child, dimensionless(), &no_facts(&matches))
            .unwrap();
    }
    assert!(matches!(
        TypedBodies::publish(&[&expr], pending),
        Err(TypedBodiesError::MissingRoot(_))
    ));

    let mut pending = PendingNodes::default();
    for child in children(&expr) {
        pending
            .record_value(child, dimensionless(), &no_facts(&matches))
            .unwrap();
    }
    assert!(matches!(
        TypedBodies::publish(&[children(&expr)[0]], pending),
        Err(TypedBodiesError::Unclaimed(_))
    ));
}

#[test]
fn deep_typed_trees_clone_and_drop_without_overflow() {
    let depth = 100_000;
    let mut ids = crate::expression_id::ExprIds::default();
    let mut node = TExpr::new(
        ids.allocate().unwrap(),
        Span::new(0, 1),
        dimensionless(),
        TExprKind::Number(1.0),
    );
    for _ in 0..depth {
        node = TExpr::new(
            ids.allocate().unwrap(),
            Span::new(0, 1),
            dimensionless(),
            TExprKind::Unary {
                op: crate::syntax::ast::UnaryOp::Neg,
                operand: Box::new(node),
            },
        );
    }
    let copy = node.clone();
    let mut count = 0_usize;
    visit_tnodes(TNodeRef::Value(&copy), &mut |_| count += 1);
    assert_eq!(count, depth + 1);
    drop(copy);
    drop(node);
}

use crate::body_revision::BodyRevision;
use crate::dag_id::DagId;
use crate::tir::expression_facts::{
    CheckedExpressionFacts, CheckedExpressionRecord, CheckingEnvironment, ExpressionFact, ValueFact,
};
use crate::tir::texpr::fact_agreement::{FactDisagreement, check};

fn owner() -> DagId {
    DagId::from_virtual_relative_path(std::path::Path::new("agreement.gcl")).unwrap()
}

fn negated_integer() -> CheckedExpr {
    finished(Expr::new(
        ExprKind::UnaryOp {
            op: crate::syntax::ast::UnaryOp::Neg,
            operand: Box::new(Expr::new(ExprKind::Integer(3), Span::new(1, 1))),
        },
        Span::new(0, 2),
    ))
}

fn facts(root: &Expr) -> CheckedExpressionFacts {
    let revision = BodyRevision::fresh();
    let environment = CheckingEnvironment::new(owner(), revision.clone());
    let mut records = HashMap::new();
    crate::hir::expr::visit_expr(root, &mut |expr| {
        records.insert(
            expr.id().clone(),
            CheckedExpressionRecord::new(
                expr,
                ExpressionFact::Symbolic(ValueFact {
                    checked_type: CheckedType::Int,
                    constructor: None,
                }),
                std::sync::Arc::clone(&environment),
            ),
        );
    });
    CheckedExpressionFacts::publish(owner(), revision, &[root], records, &|_| Ok(None)).unwrap()
}

fn typed(roots: &[&Expr], nodes: &[&Expr], ty: &CheckedType<Symbolic>) -> TypedBodies {
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    for node in nodes {
        pending
            .record_value(node, ty.clone(), &no_facts(&matches))
            .unwrap();
    }
    TypedBodies::publish(roots, pending).unwrap()
}

#[test]
fn typed_trees_agree_with_the_facts_of_the_same_pass() {
    let expr = negated_integer();
    let operand = children(&expr)[0];
    let facts = facts(&expr);
    assert_eq!(
        check(
            &typed(&[&expr], &[operand, &expr], &CheckedType::Int),
            &facts
        ),
        Ok(())
    );
    assert_eq!(
        check(
            &typed(&[&expr], &[operand, &expr], &dimensionless()),
            &facts
        ),
        Err(FactDisagreement::Type(expr.id().clone()))
    );
    assert_eq!(
        check(&typed(&[operand], &[operand], &CheckedType::Int), &facts),
        Err(FactDisagreement::Coverage { typed: 1, facts: 2 })
    );
    let other = negated_integer();
    let other_operand = children(&other)[0];
    assert_eq!(
        check(
            &typed(&[&other], &[other_operand, &other], &CheckedType::Int),
            &facts
        ),
        Err(FactDisagreement::MissingFact(other.id().clone()))
    );
}
