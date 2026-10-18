//! The examples of `formal/Graphcal/Graph/RecurrenceGroup/Examples.lean`,
//! checked against the pure core [`check_group`].

use super::*;
use crate::tir::recurrence_plan::RecurrenceGroupPlan;

fn dag(path: &str) -> DagId {
    DagId::from_virtual_relative_path(std::path::Path::new(path)).unwrap()
}

fn main_dag() -> DagId {
    dag("main.gcl")
}

fn step_axis() -> GroupAxis {
    GroupAxis::Named(ResolvedIndexName::for_test(
        main_dag(),
        crate::syntax::index_name::IndexName::expect_valid("Step".to_owned()),
    ))
}

fn phase_axis() -> GroupAxis {
    GroupAxis::Named(ResolvedIndexName::for_test(
        main_dag(),
        crate::syntax::index_name::IndexName::expect_valid("Phase".to_owned()),
    ))
}

fn member_in(owner: DagId, name: &str, definition: Definition) -> MemberFact {
    MemberFact {
        identity: ResolvedDeclName::for_test(owner, DeclName::expect_valid(name)),
        name: DeclName::expect_valid(name),
        span: Span::new(0, 0),
        definition,
    }
}

fn member(name: &str, definition: Definition) -> MemberFact {
    member_in(main_dag(), name, definition)
}

fn read(source: usize, target: usize, form: ReadForm) -> ReadFact {
    ReadFact {
        source,
        target,
        form,
        span: Span::new(source, target),
    }
}

const STEP_KEY: ReadForm = ReadForm::Element(LeadingKey::StepBinder);
const PREVIOUS_KEY: ReadForm = ReadForm::Element(LeadingKey::PreviousBinder);

/// `action` (0) is `for t: Step { -@k * @state[t] }`; `state` (1) is
/// `unfold(Step, @initial_state, |s, prev_t, t| s + @action[prev_t])`.
fn state_action(reads: Vec<ReadFact>) -> GroupFacts {
    GroupFacts {
        members: vec![
            member("action", Definition::ForOver(step_axis())),
            member("state", Definition::UnfoldOver(step_axis())),
        ],
        reads,
    }
}

fn names(plan: &RecurrenceGroupPlan) -> Vec<String> {
    plan.members()
        .iter()
        .map(|member| member.declaration().leaf().to_string())
        .collect()
}

fn reason(facts: &GroupFacts) -> RecurrenceRejection {
    match check_group(facts, &main_dag()) {
        Err(ComponentRejection::Recurrence { reason, .. }) => reason,
        other => panic!("expected a recurrence rejection, got {other:?}"),
    }
}

#[test]
fn state_action_is_accepted_with_state_first() {
    let facts = state_action(vec![read(0, 1, STEP_KEY), read(1, 0, PREVIOUS_KEY)]);
    let plan = check_group(&facts, &main_dag()).unwrap();
    assert_eq!(names(&plan), ["state", "action"]);
    // `action[t]` reads `state[t]`; `state[t]` reads `action[t-1]` and its
    // own previous state.
    assert_eq!(plan.element_dependencies(1, 0), [(0, 0)]);
    assert_eq!(plan.element_dependencies(1, 2), [(0, 2)]);
    assert_eq!(plan.element_dependencies(0, 0), [] as [(usize, usize); 0]);
    assert_eq!(plan.element_dependencies(0, 2), [(1, 1), (0, 1)]);
}

#[test]
fn a_same_step_read_next_to_a_previous_step_read_is_a_same_step_cycle() {
    let facts = state_action(vec![
        read(0, 1, STEP_KEY),
        read(1, 0, PREVIOUS_KEY),
        read(1, 0, STEP_KEY),
    ]);
    assert_eq!(
        check_group(&facts, &main_dag()),
        Err(ComponentRejection::Recurrence {
            name: DeclName::expect_valid("action"),
            span: Span::new(0, 1),
            reason: RecurrenceRejection::SameStepCycle {
                cycle: vec![
                    DeclName::expect_valid("action"),
                    DeclName::expect_valid("state")
                ],
            },
        })
    );
}

#[test]
fn a_delayed_copy_is_accepted_in_source_order() {
    let facts = GroupFacts {
        members: vec![
            member("prev_x", Definition::UnfoldOver(step_axis())),
            member("x", Definition::UnfoldOver(step_axis())),
        ],
        reads: vec![read(0, 1, PREVIOUS_KEY), read(1, 0, PREVIOUS_KEY)],
    };
    assert_eq!(
        names(&check_group(&facts, &main_dag()).unwrap()),
        ["prev_x", "x"]
    );
}

#[test]
fn a_self_read_is_rejected_even_at_the_previous_step() {
    let facts = GroupFacts {
        members: vec![member("x", Definition::UnfoldOver(step_axis()))],
        reads: vec![read(0, 0, PREVIOUS_KEY)],
    };
    assert_eq!(reason(&facts), RecurrenceRejection::SelfRead);
}

#[test]
fn reads_not_keyed_by_own_binders_are_rejected_by_form() {
    let target = || DeclName::expect_valid("action");
    for (form, expected) in [
        (
            ReadForm::Whole,
            RecurrenceRejection::WholeValueRead { target: target() },
        ),
        (
            ReadForm::Implicit,
            RecurrenceRejection::ImplicitRead { target: target() },
        ),
        (
            ReadForm::Element(LeadingKey::OtherLocal),
            RecurrenceRejection::NotStepKeyed { target: target() },
        ),
        (
            ReadForm::Element(LeadingKey::Computed),
            RecurrenceRejection::NotStepKeyed { target: target() },
        ),
    ] {
        let facts = state_action(vec![read(0, 1, STEP_KEY), read(1, 0, form)]);
        assert_eq!(reason(&facts), expected, "{form:?}");
    }
}

#[test]
fn a_for_member_has_no_previous_step() {
    let facts = state_action(vec![read(0, 1, PREVIOUS_KEY), read(1, 0, PREVIOUS_KEY)]);
    assert_eq!(
        reason(&facts),
        RecurrenceRejection::NotStepKeyed {
            target: DeclName::expect_valid("state")
        }
    );
}

#[test]
fn members_with_further_axes_are_accepted() {
    let facts = GroupFacts {
        members: vec![
            member("pop", Definition::UnfoldOver(step_axis())),
            member("births", Definition::ForOver(step_axis())),
        ],
        reads: vec![read(0, 1, PREVIOUS_KEY), read(1, 0, STEP_KEY)],
    };
    assert_eq!(
        names(&check_group(&facts, &main_dag()).unwrap()),
        ["pop", "births"]
    );
}

#[test]
fn member_rules_are_checked_in_order() {
    let scalar = GroupFacts {
        members: vec![
            member("state", Definition::UnfoldOver(step_axis())),
            member("gain", Definition::OtherFormula),
        ],
        reads: vec![read(0, 1, ReadForm::Whole), read(1, 0, ReadForm::Whole)],
    };
    assert_eq!(reason(&scalar), RecurrenceRejection::NotRecurrenceShape);
    let axes = GroupFacts {
        members: vec![
            member("state", Definition::UnfoldOver(step_axis())),
            member("action", Definition::ForOver(phase_axis())),
        ],
        reads: vec![read(0, 1, ReadForm::Whole), read(1, 0, ReadForm::Whole)],
    };
    assert_eq!(
        reason(&axes),
        RecurrenceRejection::AxisMismatch {
            first: DeclName::expect_valid("state")
        }
    );
}

#[test]
fn a_cycle_across_an_include_mixes_owners() {
    let facts = GroupFacts {
        members: vec![
            member("action", Definition::ForOver(step_axis())),
            member_in(dag("lib.gcl"), "state", Definition::UnfoldOver(step_axis())),
        ],
        reads: vec![read(0, 1, STEP_KEY), read(1, 0, PREVIOUS_KEY)],
    };
    assert_eq!(
        reason(&facts),
        RecurrenceRejection::MixedOwners {
            other: DeclName::expect_valid("state")
        }
    );
}

#[test]
fn a_group_of_an_included_instance_is_accepted() {
    let library = dag("lib.gcl");
    let facts = GroupFacts {
        members: vec![
            member_in(library.clone(), "action", Definition::ForOver(step_axis())),
            member_in(library, "state", Definition::UnfoldOver(step_axis())),
        ],
        reads: vec![read(0, 1, STEP_KEY), read(1, 0, PREVIOUS_KEY)],
    };
    assert_eq!(
        names(&check_group(&facts, &main_dag()).unwrap()),
        ["state", "action"]
    );
}

#[test]
fn cycles_without_an_unfold_keep_the_plain_diagnostic() {
    let scalars = GroupFacts {
        members: vec![
            member("a", Definition::OtherFormula),
            member("b", Definition::OtherFormula),
        ],
        reads: vec![read(0, 1, ReadForm::Whole), read(1, 0, ReadForm::Whole)],
    };
    assert_eq!(
        check_group(&scalars, &main_dag()),
        Err(ComponentRejection::PlainCycle)
    );
    let fors = GroupFacts {
        members: vec![
            member("a", Definition::ForOver(step_axis())),
            member("b", Definition::ForOver(step_axis())),
        ],
        reads: vec![read(0, 1, STEP_KEY), read(1, 0, STEP_KEY)],
    };
    assert_eq!(
        check_group(&fors, &main_dag()),
        Err(ComponentRejection::PlainCycle)
    );
}

#[test]
fn acceptance_does_not_depend_on_declaration_order() {
    let declared_first = state_action(vec![read(0, 1, STEP_KEY), read(1, 0, PREVIOUS_KEY)]);
    let swapped = GroupFacts {
        members: vec![
            member("state", Definition::UnfoldOver(step_axis())),
            member("action", Definition::ForOver(step_axis())),
        ],
        reads: vec![read(1, 0, STEP_KEY), read(0, 1, PREVIOUS_KEY)],
    };
    assert_eq!(
        check_group(&declared_first, &main_dag()).map(|plan| names(&plan)),
        check_group(&swapped, &main_dag()).map(|plan| names(&plan)),
    );
}
