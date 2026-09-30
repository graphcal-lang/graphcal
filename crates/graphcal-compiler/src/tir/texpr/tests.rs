use std::collections::HashMap;

use super::*;
use crate::dimension::Dimension;
use crate::hir::expr::{CheckedExpr, Draft, Expr, ExprKind};
use crate::semantic::checked_type::{CheckedType, IndexTypeRef, Symbolic};
use crate::syntax::span::Span;
use crate::tir::static_index::StaticIndexRequirement;

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
        extern_params: None,
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
    let bodies = claim_roots(&[&expr], pending).unwrap();
    let [(id, TBody::Value(root))] = bodies.as_slice() else {
        panic!("expected one typed value root");
    };
    assert_eq!(id, expr.id());
    let TExprKind::Quantity(operators::QExpr::Arith {
        op: operators::ArithOp::Add,
        lhs,
        rhs,
    }) = root.kind()
    else {
        panic!("expected a real sum, got {:?}", root.kind());
    };
    assert!(matches!(
        lhs.kind(),
        TExprKind::Int(operators::IExpr::Literal(1))
    ));
    assert!(matches!(
        rhs.kind(),
        TExprKind::Int(operators::IExpr::Literal(2))
    ));
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
    assert_eq!(pending.record_contextual(&literal), Ok(()));

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
        axis: IndexTypeRef::from_finite_index(crate::semantic::index_def::FiniteIndex::new(
            crate::semantic::index_def::IndexCardinality::try_from_u64(2).unwrap(),
        ))
        .to_symbolic(),
        position: 1,
        usage: crate::tir::static_index::StaticIndexUse::Selection,
    };
    assert_eq!(
        pending.record_value(
            &expr,
            dimensionless(),
            &NodeFacts {
                constructor: None,
                extern_params: None,
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
        claim_roots(&[&expr], pending),
        Err(TypedBodiesError::MissingRoot(_))
    ));

    let mut pending = PendingNodes::default();
    for child in children(&expr) {
        pending
            .record_value(child, dimensionless(), &no_facts(&matches))
            .unwrap();
    }
    assert!(matches!(
        claim_roots(&[children(&expr)[0]], pending),
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
        TExprKind::Quantity(operators::QExpr::Number(1.0)),
    );
    for _ in 0..depth {
        node = TExpr::new(
            ids.allocate().unwrap(),
            Span::new(0, 1),
            dimensionless(),
            TExprKind::Quantity(operators::QExpr::Neg(Box::new(node))),
        );
    }
    let copy = node.clone();
    let mut count = 0_usize;
    visit_tnodes(TNodeRef::Value(&copy), &mut |_| count += 1);
    assert_eq!(count, depth + 1);
    drop(copy);
    drop(node);
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

fn typed(
    roots: &[&Expr],
    nodes: &[&Expr],
    nominal_uses: HashMap<crate::expression_id::ExprId, std::sync::Arc<[NominalObservation]>>,
) -> CheckedBodies {
    let matches = HashMap::new();
    let mut pending = PendingNodes::default();
    for node in nodes {
        pending
            .record_value(node, CheckedType::Int, &no_facts(&matches))
            .unwrap();
    }
    CheckedBodies::discharge(claim_roots(roots, pending).unwrap(), nominal_uses, &|_| {
        Ok(None)
    })
    .unwrap()
}

#[test]
fn published_trees_cover_their_roots_and_keep_only_their_nominal_uses() {
    let expr = negated_integer();
    let operand = children(&expr)[0];
    let other = negated_integer();
    let observation =
        NominalObservation::TypeArgument(crate::resolved_name::ResolvedStructTypeName::for_test(
            crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("uses.gcl"))
                .unwrap(),
            crate::syntax::type_name::StructTypeName::expect_valid("Used"),
        ));
    let uses: std::sync::Arc<[NominalObservation]> = vec![observation.clone()].into();
    let bodies = typed(
        &[&expr],
        &[operand, &expr],
        HashMap::from([
            (expr.id().clone(), std::sync::Arc::clone(&uses)),
            (other.id().clone(), uses),
        ]),
    );
    assert!(bodies.cover([&*expr]));
    assert!(bodies.cover([&*expr, &*expr]));
    assert!(!bodies.cover([&*other]));
    assert!(!bodies.cover([&*expr, &*other]));
    assert!(!bodies.cover(std::iter::empty()));
    assert_eq!(bodies.nominal_uses(expr.id()), [observation]);
    assert!(bodies.nominal_uses(other.id()).is_empty());
    assert!(bodies.shared_nominal_uses(other.id()).is_none());
    assert!(bodies.executable_value(expr.id()).is_ok());
}

fn key_node(
    ids: &mut crate::expression_id::ExprIds,
    axis: IndexTypeRef<Symbolic>,
    position: u64,
) -> TExpr<Symbolic> {
    let arg = TExpr::new(
        ids.allocate().unwrap(),
        Span::new(4, 1),
        CheckedType::Int,
        TExprKind::Int(operators::IExpr::Literal(i64::try_from(position).unwrap())),
    );
    TExpr::new(
        ids.allocate().unwrap(),
        Span::new(0, 6),
        CheckedType::Key(axis.clone()),
        TExprKind::Key {
            form: TKeyForm::Static(StaticPosition {
                axis,
                position,
                usage: crate::tir::static_index::StaticIndexUse::Key,
            }),
            axis: crate::hir::expr::ForBindingIndex::Finite {
                cardinality: crate::syntax::span::Spanned::new(
                    crate::nat::NatPolyForm::from_constant(3),
                    Span::new(0, 1),
                ),
                span: Span::new(0, 1),
            },
            arg: Box::new(arg),
        },
    )
}

fn fin(size: u64) -> IndexTypeRef<Symbolic> {
    IndexTypeRef::from_finite_index(
        crate::semantic::index_def::FiniteIndex::try_from_u64(size).unwrap(),
    )
}

fn known(
    size: u64,
) -> impl Fn(
    &IndexTypeRef<Symbolic>,
) -> Result<
    Option<crate::semantic::index_def::IndexCardinality>,
    crate::tir::static_index::UnavailableIndex,
> {
    move |_| {
        Ok(Some(
            crate::semantic::index_def::IndexCardinality::try_from_u64(size).unwrap(),
        ))
    }
}

#[test]
fn discharge_publishes_only_trees_whose_every_obligation_is_met() {
    let mut ids = crate::expression_id::ExprIds::default();
    let ready = CheckedBody::discharge(
        TBody::Value(Box::new(key_node(&mut ids, fin(3), 1))),
        &known(3),
    )
    .unwrap();
    let CheckedBody::Executable(TBody::Value(tree)) = ready else {
        panic!("a discharged tree is executable: {ready:?}");
    };
    assert!(matches!(tree.ty(), CheckedType::Key(_)));

    // An axis still awaiting its binding keeps the tree deferred.
    let waiting = CheckedBody::discharge(
        TBody::Value(Box::new(key_node(&mut ids, fin(3), 1))),
        &|_| Ok(None),
    )
    .unwrap();
    assert!(matches!(waiting, CheckedBody::Deferred(_)));

    // A `Nat` variable keeps the tree deferred even when its size is known.
    let n = IndexTypeRef::from_finite_index_form(crate::nat::NatPolyForm::from_var(
        crate::generic_param::test_support::type_param("N"),
    ))
    .unwrap();
    let symbolic =
        CheckedBody::discharge(TBody::Value(Box::new(key_node(&mut ids, n, 1))), &known(3))
            .unwrap();
    assert!(matches!(symbolic, CheckedBody::Deferred(_)));

    // A position outside the now-known axis is an error, not a deferral.
    assert!(matches!(
        CheckedBody::discharge(
            TBody::Value(Box::new(key_node(&mut ids, fin(3), 5))),
            &known(3)
        ),
        Err(DischargeError::StaticIndex(_))
    ));
}

#[test]
fn executable_lookup_distinguishes_missing_deferred_and_contextual_roots() {
    let mut ids = crate::expression_id::ExprIds::default();
    let ready = key_node(&mut ids, fin(3), 1);
    let waiting = key_node(&mut ids, fin(3), 2);
    let literal = TContextual::new(
        ids.allocate().unwrap(),
        Span::new(0, 3),
        ContextualLiteral::String("red".to_owned()),
    );
    let (ready_id, waiting_id, literal_id) = (
        ready.id().clone(),
        waiting.id().clone(),
        literal.id().clone(),
    );
    let bodies = CheckedBodies::discharge(
        vec![
            (ready_id.clone(), TBody::Value(Box::new(ready))),
            (literal_id.clone(), TBody::Contextual(literal)),
        ],
        HashMap::new(),
        &known(3),
    )
    .unwrap();
    assert!(bodies.executable_value(&ready_id).is_ok());
    assert_eq!(
        bodies.executable_value(&literal_id).unwrap_err(),
        ExecutableBodyError::Contextual(literal_id.clone())
    );
    assert!(bodies.contextual(&literal_id).is_some());
    assert!(bodies.contextual(&ready_id).is_none());
    assert_eq!(
        bodies.executable_value(&waiting_id).unwrap_err(),
        ExecutableBodyError::Missing(waiting_id.clone())
    );
    let deferred = CheckedBodies::discharge(
        vec![(waiting_id.clone(), TBody::Value(Box::new(waiting)))],
        HashMap::new(),
        &|_| Ok(None),
    )
    .unwrap();
    assert_eq!(
        deferred.executable_value(&waiting_id).unwrap_err(),
        ExecutableBodyError::Deferred(waiting_id)
    );
}

#[test]
fn type_maps_keep_structure_and_rewrite_every_carried_type() {
    let mut ids = crate::expression_id::ExprIds::default();
    let tree = key_node(&mut ids, fin(3), 1);
    let concrete = tree.map_types(&mut map::ToConcrete).unwrap();
    assert_eq!(concrete.ty().to_symbolic(), *tree.ty());
    assert_eq!(concrete.id(), tree.id());
    let (
        TExprKind::Key {
            form: TKeyForm::Static(before),
            arg: before_arg,
            ..
        },
        TExprKind::Key {
            form: TKeyForm::Static(after),
            arg: after_arg,
            ..
        },
    ) = (tree.kind(), concrete.kind())
    else {
        panic!("a map keeps the key form and its proof");
    };
    assert_eq!(after.axis.to_symbolic(), before.axis);
    assert_eq!(
        (after.position, after.usage),
        (before.position, before.usage)
    );
    assert_eq!(before_arg.id(), after_arg.id());
}
