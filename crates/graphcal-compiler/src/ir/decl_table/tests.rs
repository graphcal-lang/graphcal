use super::*;
use crate::declaration_category::{DeclCategory, ValueDeclCategory};
use crate::plot_visibility::PlotVisibility;
use crate::syntax::ast::MarkType;
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::Span;

/// A body phase whose bodies carry no data, isolating table invariants.
#[derive(Debug, Clone, Copy)]
enum Bare {}

impl BodyPhase for Bare {
    type Expr = ();
    type TypeAnnotation = ();
    type NodeDefinition = ();
    type AssertBody = ();
    type PlotBody = ();
    type CompositionFields = ();
}

/// Same shape as [`Bare`], used to observe a phase change.
#[derive(Debug, Clone, Copy)]
enum Counted {}

impl BodyPhase for Counted {
    type Expr = usize;
    type TypeAnnotation = ();
    type NodeDefinition = usize;
    type AssertBody = usize;
    type PlotBody = ();
    type CompositionFields = ();
}

fn owner() -> DagId {
    DagId::root_in_package("test", "main")
}

fn name(spelling: &str) -> DeclName {
    DeclName::expect_valid(spelling)
}

fn node(spelling: &str, declaration_owner: DagId) -> Decl<Bare> {
    Decl::Node(NodeEntry {
        identity: ResolvedDeclName::for_test(declaration_owner, name(spelling)),
        type_ann: (),
        definition: (),
        span: Span::new(0, 0),
    })
}

fn param(spelling: &str) -> Decl<Bare> {
    Decl::Param(ParamEntry {
        identity: ResolvedDeclName::for_test(owner(), name(spelling)),
        type_ann: (),
        default: None,
        span: Span::new(0, 0),
        override_reconciliations: Vec::new(),
    })
}

fn assertion(spelling: &str) -> Decl<Bare> {
    Decl::Assert(AssertEntry {
        identity: ResolvedDeclName::for_test(owner(), name(spelling)),
        body: (),
        span: Span::new(0, 0),
    })
}

fn plot(spelling: &str) -> Decl<Bare> {
    Decl::Plot(PlotEntry {
        identity: ResolvedDeclName::for_test(owner(), name(spelling)),
        mark_type: MarkType::Line,
        body: (),
        visibility: PlotVisibility::Standalone,
    })
}

fn leaves<P: BodyPhase>(table: &DeclTable<P>) -> Vec<String> {
    table
        .order()
        .iter()
        .map(|identity| identity.to_unowned_def_name().to_string())
        .collect()
}

#[test]
fn table_keeps_source_order_and_indexes_spellings() {
    let table = DeclTable::new(
        &owner(),
        [param("b"), node("a", owner()), assertion("ok"), plot("p")],
    )
    .unwrap();

    assert_eq!(leaves(&table), ["b", "a", "ok", "p"]);
    let identity = table.lookup(&name("a")).unwrap();
    assert_eq!(identity.owner(), &owner());
    assert_eq!(
        table.get(identity).map(Decl::category),
        Some(DeclCategory::Value(ValueDeclCategory::Node))
    );
    assert!(table.lookup(&name("missing")).is_none());
    assert_eq!(table.spelling().len(), 4);
    assert_eq!(table.params().count(), 1);
    assert_eq!(table.nodes().count(), 1);
    assert_eq!(table.asserts().count(), 1);
    assert_eq!(table.plots().count(), 1);
    assert_eq!(table.consts().count(), 0);
    assert_eq!(table.figures().count(), 0);
    assert_eq!(table.layers().count(), 0);
}

#[test]
fn table_rejects_duplicate_identities() {
    let error = DeclTable::new(&owner(), [param("x"), node("x", owner())]).unwrap_err();
    assert_eq!(error, DeclTableError::Duplicate { name: name("x") });
}

#[test]
fn table_rejects_declarations_of_another_dag() {
    let other = DagId::root_in_package("test", "other");
    let error = DeclTable::new(&owner(), [node("x", other.clone())]).unwrap_err();
    assert_eq!(
        error,
        DeclTableError::ForeignOwner {
            name: name("x"),
            declaration_owner: other,
            owner: owner(),
        }
    );
}

#[test]
fn try_map_transforms_by_rank_and_preserves_order() {
    let table = DeclTable::new(&owner(), [node("n", owner()), param("p"), assertion("a")]).unwrap();
    let mut visited = Vec::new();
    let mapped = table
        .try_map(
            |decl| match decl {
                Decl::Param(_) => 0,
                Decl::Node(_) => 1,
                _ => 2,
            },
            |decl| -> Result<Decl<Counted>, ()> {
                visited.push(decl.name().to_string());
                let position = visited.len();
                Ok(match decl {
                    Decl::Param(entry) => Decl::Param(ParamEntry {
                        identity: entry.identity,
                        type_ann: (),
                        default: Some(position),
                        span: entry.span,
                        override_reconciliations: entry.override_reconciliations,
                    }),
                    Decl::Node(entry) => Decl::Node(NodeEntry {
                        identity: entry.identity,
                        type_ann: (),
                        definition: position,
                        span: entry.span,
                    }),
                    Decl::Assert(entry) => Decl::Assert(AssertEntry {
                        identity: entry.identity,
                        body: position,
                        span: entry.span,
                    }),
                    other => panic!("the table holds no {} declarations", other.category()),
                })
            },
        )
        .unwrap();

    assert_eq!(visited, ["p", "n", "a"]);
    assert_eq!(leaves(&mapped), ["n", "p", "a"]);
    assert_eq!(
        mapped
            .nodes()
            .map(|entry| entry.definition)
            .collect::<Vec<_>>(),
        [2]
    );
    assert!(mapped.lookup(&name("p")).is_some());

    let (decls, spelling) = mapped.into_parts();
    assert_eq!(
        decls
            .iter()
            .map(|decl| decl.name().to_string())
            .collect::<Vec<_>>(),
        ["n", "p", "a"]
    );
    assert_eq!(spelling.len(), 3);
}

#[test]
fn try_map_stops_at_the_first_error() {
    let table = DeclTable::new(&owner(), [param("p"), node("n", owner())]).unwrap();
    let mut calls = 0;
    let error = table
        .try_map(
            |_| 0,
            |decl| -> Result<Decl<Bare>, String> {
                calls += 1;
                Err(decl.name().to_string())
            },
        )
        .unwrap_err();
    assert_eq!(error, "p");
    assert_eq!(calls, 1);
}

#[test]
fn rebase_moves_every_declaration_to_the_new_owner() {
    let table = DeclTable::new(&owner(), [param("b"), node("a", owner()), plot("p")]).unwrap();
    let instance = DagId::root_in_package("test", "instance");
    let mut seen = Vec::new();
    let rebased = table
        .rebase(&instance, |decl| {
            seen.push(decl.identity());
            decl
        })
        .unwrap();

    // The transform observes the original identities in source order.
    assert!(seen.iter().all(|identity| identity.owner() == &owner()));
    assert_eq!(seen.len(), 3);
    assert_eq!(leaves(&rebased), ["b", "a", "p"]);
    assert!(
        rebased
            .iter()
            .all(|decl| decl.declaration_owner() == &instance)
    );
    let identity = rebased.lookup(&name("a")).unwrap();
    assert_eq!(identity.owner(), &instance);
    assert!(rebased.get(identity).is_some());
}

#[test]
fn rebase_revalidates_the_transformed_declarations() {
    let table = DeclTable::new(&owner(), [param("b"), node("a", owner())]).unwrap();
    let error = table
        .rebase(&owner(), |decl| match decl {
            Decl::Node(mut entry) => {
                entry.identity = entry.identity.with_leaf(name("b"));
                Decl::Node(entry)
            }
            other => other,
        })
        .unwrap_err();
    assert!(matches!(error, DeclTableError::Duplicate { .. }));
}

#[test]
fn update_edits_bodies_in_source_order() {
    let mut table =
        DeclTable::new(&owner(), [param("b"), node("a", owner()), assertion("ok")]).unwrap();
    let mut visited = Vec::new();
    table.update(|decl| {
        visited.push(decl.name().to_string());
        if let Decl::Param(entry) = decl {
            entry.default = Some(());
        }
    });
    assert_eq!(visited, ["b", "a", "ok"]);
    assert!(table.params().all(|entry| entry.default.is_some()));
}
