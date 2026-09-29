use super::*;
use crate::hir::closed_expr::ClosedExpr;
use crate::hir::expr::{CheckedExpr, visit_expr};
use crate::registry::index::IndexCardinality;
use crate::syntax::ast::UnaryOp;

fn owner() -> DagId {
    DagId::from_virtual_relative_path(std::path::Path::new("facts.gcl")).unwrap()
}

fn publish<'a>(
    owner: DagId,
    revision: BodyRevision,
    roots: impl IntoIterator<Item = &'a Expr>,
    records: HashMap<ExprId, Box<CheckedExpressionRecord>>,
) -> Result<CheckedExpressionFacts, ExpressionFactsError> {
    CheckedExpressionFacts::publish(
        owner,
        revision,
        &roots.into_iter().collect::<Vec<_>>(),
        records,
        &|index| {
            Ok(index
                .finite_index()
                .map(crate::registry::index::FiniteIndex::cardinality))
        },
    )
}

fn body() -> ClosedExpr {
    ClosedExpr::try_new(Expr::new(
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand: Box::new(Expr::new(ExprKind::Integer(3), Span::new(0, 1))),
        },
        Span::new(0, 1),
    ))
    .unwrap()
}

fn rows(root: &Expr, revision: &BodyRevision) -> HashMap<ExprId, Box<CheckedExpressionRecord>> {
    let environment = CheckingEnvironment::new(owner(), revision.clone());
    let mut records = HashMap::new();
    visit_expr(root, &mut |expr| {
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
    records
}

#[test]
fn constructor_target_coverage_is_exact_even_with_repeated_references() {
    use crate::hir::expr::{MatchArm, MatchPattern};
    use crate::resolved_name::ResolvedConstructorName;
    use crate::syntax::ast::PatternBindings;
    use crate::syntax::span::Spanned;
    use crate::syntax::type_name::StructTypeName;

    let constructor = |name: &str| {
        ResolvedConstructorName::from_def(owner(), ConstructorName::expect_valid(name))
    };
    let identity = ResolvedStructTypeName::from_def(owner(), StructTypeName::expect_valid("Token"));
    let target = |name: &str| ConstructorMatch {
        definition: identity.clone(),
        runtime_type: identity.clone(),
        constructor: ConstructorName::expect_valid(name),
    };
    for names in [["First", "Second"], ["First", "First"]] {
        let span = Span::new(0, 1);
        // Isolate target-set coverage from full publication and source typing.
        let seed = body();
        let mut record = rows(&seed, &BodyRevision::fresh())
            .remove(seed.id())
            .unwrap();
        let body = CheckedExpr::from_draft_for_test(Expr::new(
            ExprKind::Match {
                scrutinee: Box::new(Expr::new(ExprKind::Integer(0), span)),
                arms: names
                    .iter()
                    .map(|name| MatchArm {
                        pattern: MatchPattern::Constructor {
                            constructor: Spanned::new(constructor(name), span),
                            bindings: PatternBindings::Bare,
                            span,
                        },
                        body: Expr::new(ExprKind::Integer(1), span),
                        span,
                    })
                    .collect(),
            },
            span,
        ));
        record.constructor_matches = names
            .iter()
            .map(|name| (constructor(name), target(name)))
            .collect();
        assert!(matches_constructor_targets(&body, &record));
        let first = record
            .constructor_matches
            .remove(&constructor("First"))
            .unwrap();
        assert!(!matches_constructor_targets(&body, &record));
        record
            .constructor_matches
            .insert(constructor("Third"), target("Third"));
        assert!(!matches_constructor_targets(&body, &record));
        record
            .constructor_matches
            .insert(constructor("First"), first);
        assert!(!matches_constructor_targets(&body, &record));
    }
}

#[test]
fn every_descendant_requires_a_row_and_extra_rows_are_rejected() {
    let body = body();
    let revision = BodyRevision::fresh();
    let records = rows(&body, &revision);
    assert_eq!(records.len(), 2);
    assert!(publish(owner(), revision.clone(), [&*body], records.clone()).is_ok());
    for id in records.keys() {
        let mut missing = records.clone();
        missing.remove(id);
        assert!(matches!(
            publish(owner(), revision.clone(), [&*body], missing),
            Err(ExpressionFactsError::Missing(_))
        ));
    }
    let other = ClosedExpr::try_new(Expr::new(ExprKind::Integer(4), Span::new(0, 1))).unwrap();
    let mut extra = records;
    extra.extend(rows(&other, &revision));
    assert!(matches!(
        publish(owner(), revision, [&*body], extra),
        Err(ExpressionFactsError::Extra(_))
    ));
}

#[test]
fn publication_rejects_old_semantic_revision_and_rebuilt_source_at_equal_coordinates() {
    let original = body();
    let rebuilt = body();
    let revision = BodyRevision::fresh();
    assert!(matches!(
        publish(
            owner(),
            BodyRevision::fresh(),
            [&*original],
            rows(&original, &revision)
        ),
        Err(ExpressionFactsError::WrongEnvironment)
    ));
    assert!(matches!(
        publish(
            owner(),
            revision.clone(),
            [&*rebuilt],
            rows(&original, &revision)
        ),
        Err(ExpressionFactsError::Missing(_))
    ));
    let facts = publish(
        owner(),
        revision.clone(),
        [&*original],
        rows(&original, &revision),
    )
    .unwrap();
    assert!(
        facts
            .validate_environment(&owner(), &BodyRevision::fresh())
            .is_err()
    );
}

#[test]
fn structural_and_contextual_corruption_is_rejected_without_inference() {
    let body = body();
    let revision = BodyRevision::fresh();
    let id = body.id();
    let original = rows(&body, &revision);
    let mut wrong_children = original.clone();
    wrong_children.get_mut(id).unwrap().children = None;
    assert!(matches!(
        publish(owner(), revision.clone(), [&*body], wrong_children),
        Err(ExpressionFactsError::Incompatible(_))
    ));
    let mut unchecked_scalar = original;
    unchecked_scalar.get_mut(id).unwrap().fact =
        ExpressionFact::Contextual(ContextualOperand::String);
    assert!(matches!(
        publish(owner(), revision, [&*body], unchecked_scalar),
        Err(ExpressionFactsError::Incompatible(_))
    ));
}

/// Publish `body` with its root typed `ty` under a fixed axis cardinality.
fn classify_root(
    ty: CheckedType<Symbolic>,
    cardinality: Option<IndexCardinality>,
) -> (ExpressionFact, CheckedExpressionFacts, ClosedExpr) {
    let body = body();
    let revision = BodyRevision::fresh();
    let mut records = rows(&body, &revision);
    records.get_mut(body.id()).unwrap().fact = ExpressionFact::Symbolic(ValueFact {
        checked_type: ty,
        constructor: None,
    });
    let facts = CheckedExpressionFacts::publish(owner(), revision, &[&*body], records, &|_| {
        Ok(cardinality)
    })
    .unwrap();
    (facts.get(body.id()).unwrap().fact.clone(), facts, body)
}

#[test]
fn publication_classifies_values_by_what_evaluation_may_rely_on() {
    let index = IndexTypeRef::<Symbolic>::with_owner(
        owner(),
        crate::syntax::index_name::IndexName::expect_valid("Axis"),
    );
    let indexed = CheckedType::Indexed {
        element: Box::new(CheckedType::Int),
        index,
    };
    let three = IndexCardinality::try_from_u64(3).unwrap();

    let (fact, facts, body) = classify_root(indexed.clone(), Some(three));
    assert!(matches!(&fact, ExpressionFact::Executable(value)
        if value.checked_type.to_symbolic() == indexed));
    facts.executable_value(body.id()).unwrap();

    // A rigid axis without a known cardinality keeps its concrete type but
    // waits for a binding.
    let (fact, facts, body) = classify_root(indexed, None);
    assert!(matches!(&fact, ExpressionFact::Pending(_)));
    assert!(matches!(
        facts.executable_value(body.id()),
        Err(ExpressionFactsError::Deferred(_))
    ));

    // A symbolic `Fin(N)` axis has no concrete type at all.
    let symbolic = CheckedType::Key(
        IndexTypeRef::from_finite_index_form(crate::nat::NatPolyForm::from_var(
            crate::generic_param::test_support::type_param("N"),
        ))
        .unwrap(),
    );
    let (fact, facts, body) = classify_root(symbolic, Some(three));
    assert!(matches!(&fact, ExpressionFact::Symbolic(_)));
    assert!(matches!(
        facts.executable_value(body.id()),
        Err(ExpressionFactsError::Deferred(_))
    ));
}

#[test]
fn a_waiting_operand_keeps_its_parent_waiting() {
    let body = body();
    let revision = BodyRevision::fresh();
    let mut records = rows(&body, &revision);
    let operand = records[body.id()].children()[0].clone();
    records.get_mut(&operand).unwrap().fact = ExpressionFact::Symbolic(ValueFact {
        checked_type: CheckedType::Key(IndexTypeRef::with_owner(
            owner(),
            crate::syntax::index_name::IndexName::expect_valid("Axis"),
        )),
        constructor: None,
    });
    let facts =
        CheckedExpressionFacts::publish(owner(), revision, &[&*body], records, &|_| Ok(None))
            .unwrap();
    assert!(matches!(
        facts.get(&operand).unwrap().fact,
        ExpressionFact::Pending(_)
    ));
    assert!(matches!(
        facts.get(body.id()).unwrap().fact,
        ExpressionFact::Pending(_)
    ));
    assert!(matches!(
        facts.executable_value(body.id()),
        Err(ExpressionFactsError::Deferred(_))
    ));
}

#[test]
fn contextual_subtype_and_conflicting_origins_are_rejected() {
    let zone = crate::registry::time_zone::TimeZoneRegistry::bundled()
        .parse_iana_id("Asia/Tokyo")
        .unwrap();
    let root = ClosedExpr::try_new(Expr::new(
        ExprKind::IanaTimeZoneLiteral(zone),
        Span::new(0, 1),
    ))
    .unwrap();
    let revision = BodyRevision::fresh();
    for kind in [ContextualOperand::String, ContextualOperand::TimeZone] {
        let row = CheckedExpressionRecord::new(
            &root,
            ExpressionFact::Contextual(kind),
            CheckingEnvironment::new(owner(), revision.clone()),
        );
        let result = publish(
            owner(),
            revision.clone(),
            [&*root],
            HashMap::from([(root.id().clone(), row)]),
        );
        assert_eq!(result.is_ok(), kind == ContextualOperand::TimeZone);
    }
    let root = body();
    let records = rows(&root, &revision);
    assert!(publish(owner(), revision.clone(), [&*root, &*root], records.clone()).is_ok());
    let mut relocated = (*root).clone();
    relocated.span = Span::new(100, 101);
    assert!(matches!(
        publish(owner(), revision, [&*root, &relocated], records),
        Err(ExpressionFactsError::ConflictingOrigin(_))
    ));
}

#[test]
fn static_membership_proof_cannot_be_deleted_misowned_or_invalid() {
    let span = Span::new(0, 1);
    let root = ClosedExpr::try_new(Expr::new(
        ExprKind::KeyForm {
            kind: crate::syntax::ast::KeyFormKind::Static,
            axis: crate::hir::expr::ForBindingIndex::Finite {
                cardinality: crate::syntax::span::Spanned::new(
                    crate::nat::NatPolyForm::from_constant(2),
                    span,
                ),
                span,
            },
            axis_span: span,
            arg: Box::new(Expr::new(ExprKind::Integer(1), span)),
        },
        span,
    ))
    .unwrap();
    let revision = BodyRevision::fresh();
    let axis = IndexTypeRef::<Symbolic>::from_finite_index(
        crate::registry::index::FiniteIndex::try_from_u64(2).unwrap(),
    );
    let mut records = rows(&root, &revision);
    let id = root.id();
    let record = records.get_mut(id).unwrap();
    record.fact = ExpressionFact::Symbolic(ValueFact {
        checked_type: CheckedType::Key(axis.clone()),
        constructor: None,
    });
    record.static_indexes.push(StaticIndexRequirement {
        operand: record.children()[0].clone(),
        axis,
        position: 1,
        usage: StaticIndexUse::Key,
    });
    publish(owner(), revision.clone(), [&*root], records.clone())
        .unwrap()
        .executable_value(id)
        .unwrap();
    let mut missing = records.clone();
    missing.get_mut(id).unwrap().static_indexes.clear();
    assert!(matches!(
        publish(owner(), revision.clone(), [&*root], missing),
        Err(ExpressionFactsError::Incompatible(_))
    ));
    let mut foreign = records.clone();
    foreign.get_mut(id).unwrap().static_indexes[0].operand = body().id().clone();
    assert!(matches!(
        publish(owner(), revision.clone(), [&*root], foreign),
        Err(ExpressionFactsError::Incompatible(_))
    ));
    records.get_mut(id).unwrap().static_indexes[0].position = 2;
    assert!(matches!(
        publish(owner(), revision, [&*root], records),
        Err(ExpressionFactsError::StaticIndex(_))
    ));
}

#[test]
fn immutable_source_and_fact_publications_are_shared_by_clones() {
    let original = body();
    let cloned = original.clone();
    assert!(std::ptr::eq(&raw const *original, &raw const *cloned));
    assert!(std::ptr::eq(original.source_map(), cloned.source_map()));
    let revision = BodyRevision::fresh();
    let facts = publish(
        owner(),
        revision.clone(),
        [&*original],
        rows(&original, &revision),
    )
    .unwrap();
    let row = facts.get(original.id()).unwrap();
    let independent = row.clone();
    assert!(std::sync::Arc::ptr_eq(
        &row.operation,
        &independent.operation
    ));
    assert!(std::ptr::eq(row.children(), independent.children()));
    let clone = facts.clone();
    assert!(std::ptr::eq(
        facts.get(original.id()).unwrap(),
        clone.get(original.id()).unwrap()
    ));
}

#[test]
fn diagnostic_relocation_preserves_fact_identity() {
    let original = body();
    let revision = BodyRevision::fresh();
    let records = rows(&original, &revision);
    let mut relocated = (*original).clone();
    relocated.span = Span::new(100, 101);
    let facts = publish(owner(), revision, [&relocated], records).unwrap();
    assert_eq!(facts.span(original.id()).unwrap(), Span::new(100, 101));
    assert_eq!(facts.get(original.id()).unwrap().children().len(), 1);
}
