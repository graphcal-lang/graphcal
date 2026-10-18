//! Recurrence groups: dependency cycles among `for` and `unfold`
//! declarations that are acyclic element by element along one axis.
//!
//! The rule is specified in `internals/recurrence-groups.md` and
//! machine-checked in `formal/Graphcal/Graph/RecurrenceGroup/`. This module
//! is its checker. `group_facts` reads one cyclic component of a callable's
//! runtime dependency graph out of HIR as `GroupFacts`; `check_group`, a pure
//! function of those facts, applies rules R1–R7 in order and returns either
//! the [`RecurrenceGroupPlan`] evaluation follows or the first violation.

use std::collections::{BTreeSet, HashMap};

use crate::dag_id::DagId;
use crate::hir::expr::{Expr, ExprKind, ForBindingIndex, IndexArg, LocalId};
use crate::nat::NatPolyForm;
use crate::resolved_name::{ResolvedDeclName, ResolvedIndexName, ResolvedUnitName};
use crate::semantic_error::graph::RecurrenceRejection;
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::Span;
use crate::tir::recurrence_plan::{
    GroupRead, RecurrenceGroupPlan, RecurrenceMember, RecurrenceShape, StepAccess,
};
use crate::tir::schedule::{ComponentRejection, ScheduledDecl};
use crate::tir::typed::model::DagTIR;

/// The axis a recurrence-shaped definition steps along, compared by resolved
/// identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupAxis {
    Named(ResolvedIndexName),
    Finite(NatPolyForm),
}

/// A member's definition, after removing a top-level display conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Definition {
    ForOver(GroupAxis),
    UnfoldOver(GroupAxis),
    OtherFormula,
}

impl Definition {
    const fn shape(&self) -> Option<RecurrenceShape> {
        match self {
            Self::ForOver(_) => Some(RecurrenceShape::For),
            Self::UnfoldOver(_) => Some(RecurrenceShape::Unfold),
            Self::OtherFormula => None,
        }
    }

    const fn axis(&self) -> Option<&GroupAxis> {
        match self {
            Self::ForOver(axis) | Self::UnfoldOver(axis) => Some(axis),
            Self::OtherFormula => None,
        }
    }
}

/// How the leading key of an element access is written, by binder identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeadingKey {
    /// The reader's own current-step binder.
    StepBinder,
    /// The reader's own previous-step binder (`unfold` only).
    PreviousBinder,
    /// Any other local variable.
    OtherLocal,
    /// Any other key expression.
    Computed,
}

/// How a member's evaluation observes another member's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadForm {
    /// `@y[k, …]`: only the leading key is classified.
    Element(LeadingKey),
    /// `@y` without an element access.
    Whole,
    /// A dependency without an `@y` occurrence, such as a dynamic unit's
    /// scale.
    Implicit,
}

/// One read between members, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadFact {
    pub(crate) source: usize,
    pub(crate) target: usize,
    pub(crate) form: ReadForm,
    /// The `@y` occurrence, the quantity literal of an implicit read, or the
    /// reader's declaration.
    pub(crate) span: Span,
}

/// One member of a candidate group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberFact {
    pub(crate) identity: ResolvedDeclName,
    pub(crate) name: DeclName,
    pub(crate) span: Span,
    pub(crate) definition: Definition,
}

impl MemberFact {
    const fn owner(&self) -> &DagId {
        self.identity.owner()
    }
}

/// One candidate group: its members and reads, both in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupFacts {
    pub(crate) members: Vec<MemberFact>,
    pub(crate) reads: Vec<ReadFact>,
}

/// Classify a read inside a member of the given shape (`Shape.access`).
const fn access(shape: RecurrenceShape, form: ReadForm) -> Option<StepAccess> {
    match (shape, form) {
        (_, ReadForm::Element(LeadingKey::StepBinder)) => Some(StepAccess::Same),
        (RecurrenceShape::Unfold, ReadForm::Element(LeadingKey::PreviousBinder)) => {
            Some(StepAccess::Previous)
        }
        _ => None,
    }
}

/// One violated rule, at a member (by position) and a span in its source.
struct Violation {
    member: usize,
    span: Span,
    reason: RecurrenceRejection,
}

/// Apply R1–R7 to the candidate group `facts` of the callable `callable`
/// (`Group.check`).
///
/// # Errors
///
/// Returns the first violated rule in rule order, members and reads in
/// source order, or [`ComponentRejection::PlainCycle`] when the group has no
/// `unfold` member or its violation cannot be reported in `callable`'s
/// source.
pub(crate) fn check_group(
    facts: &GroupFacts,
    callable: &DagId,
) -> Result<RecurrenceGroupPlan, ComponentRejection> {
    if !facts
        .members
        .iter()
        .any(|member| member.definition.shape() == Some(RecurrenceShape::Unfold))
    {
        return Err(ComponentRejection::PlainCycle);
    }
    let report = |violation: Violation| {
        let member = &facts.members[violation.member];
        if member.owner() == callable {
            ComponentRejection::Recurrence {
                name: member.name.clone(),
                span: violation.span,
                reason: violation.reason,
            }
        } else {
            ComponentRejection::PlainCycle
        }
    };
    check_members(facts, callable).map_err(report)?;
    let accesses = classify_reads(facts).map_err(report)?;
    let order = order_members(facts, &accesses).map_err(report)?;
    Ok(plan(facts, &accesses, &order))
}

/// R1, R3, R4: one owner, a top-level `for` or `unfold`, one axis. (R2, which
/// rejected unfinished members, is retired: a hole's interface lists its
/// keyed reads, so an unfinished member is classified like a finished one.)
fn check_members(facts: &GroupFacts, callable: &DagId) -> Result<(), Violation> {
    let members = &facts.members;
    let Some(first) = members.first() else {
        return Ok(());
    };
    if members.iter().any(|member| member.owner() != first.owner()) {
        // Reported at the first read that leaves the callable's own members.
        let read = facts.reads.iter().find(|read| {
            members[read.source].owner() == callable && members[read.target].owner() != callable
        });
        return Err(read.map_or_else(
            || Violation {
                member: 0,
                span: first.span,
                reason: RecurrenceRejection::MixedOwners {
                    other: first.name.clone(),
                },
            },
            |read| Violation {
                member: read.source,
                span: members[read.source].span,
                reason: RecurrenceRejection::MixedOwners {
                    other: members[read.target].name.clone(),
                },
            },
        ));
    }
    let at_member = |member: usize, reason: RecurrenceRejection| Violation {
        member,
        span: members[member].span,
        reason,
    };
    if let Some(member) = members
        .iter()
        .position(|member| member.definition.shape().is_none())
    {
        return Err(at_member(member, RecurrenceRejection::NotRecurrenceShape));
    }
    if let Some(member) = members
        .iter()
        .position(|member| member.definition.axis() != first.definition.axis())
    {
        return Err(at_member(
            member,
            RecurrenceRejection::AxisMismatch {
                first: first.name.clone(),
            },
        ));
    }
    Ok(())
}

/// R5 and R6: no self-read, and every read classified as a same-step or a
/// previous-step read. Returns the access of every read, in read order.
fn classify_reads(facts: &GroupFacts) -> Result<Vec<StepAccess>, Violation> {
    let members = &facts.members;
    if let Some(read) = facts.reads.iter().find(|read| read.source == read.target) {
        return Err(Violation {
            member: read.source,
            span: read.span,
            reason: RecurrenceRejection::SelfRead,
        });
    }
    facts
        .reads
        .iter()
        .map(|read| {
            members[read.source]
                .definition
                .shape()
                .and_then(|shape| access(shape, read.form))
                .ok_or_else(|| {
                    let target = members[read.target].name.clone();
                    Violation {
                        member: read.source,
                        span: read.span,
                        reason: match read.form {
                            ReadForm::Whole => RecurrenceRejection::WholeValueRead { target },
                            ReadForm::Implicit => RecurrenceRejection::ImplicitRead { target },
                            ReadForm::Element(_) => RecurrenceRejection::NotStepKeyed { target },
                        },
                    }
                })
        })
        .collect()
}

/// R7: no same-step cycle. Returns the plan order of the members.
fn order_members(facts: &GroupFacts, accesses: &[StepAccess]) -> Result<Vec<usize>, Violation> {
    let same_targets = |member: usize| {
        facts
            .reads
            .iter()
            .zip(accesses)
            .filter(move |(read, access)| read.source == member && **access == StepAccess::Same)
            .map(|(read, _)| read.target)
    };
    greedy_order(facts.members.len(), &same_targets).map_err(|stuck| {
        // Placement stops only with a member left, and the walk starts there.
        let cycle = extract_cycle(stuck[0], &stuck, &same_targets);
        let start = cycle[0];
        let span = facts
            .reads
            .iter()
            .zip(accesses)
            .find(|(read, access)| {
                read.source == start
                    && cycle.get(1) == Some(&read.target)
                    && **access == StepAccess::Same
            })
            .map_or(facts.members[start].span, |(read, _)| read.span);
        Violation {
            member: start,
            span,
            reason: RecurrenceRejection::SameStepCycle {
                cycle: cycle
                    .iter()
                    .map(|member| facts.members[*member].name.clone())
                    .collect(),
            },
        }
    })
}

/// The plan of an accepted group: its members in `order`, each with its
/// distinct classified reads.
fn plan(facts: &GroupFacts, accesses: &[StepAccess], order: &[usize]) -> RecurrenceGroupPlan {
    let position = order
        .iter()
        .enumerate()
        .map(|(position, member)| (*member, position))
        .collect::<HashMap<_, _>>();
    let members = order
        .iter()
        .map(|&member| {
            let mut reads = Vec::new();
            for (read, access) in facts
                .reads
                .iter()
                .zip(accesses)
                .filter(|(read, _)| read.source == member)
            {
                let read = GroupRead::new(position[&read.target], *access);
                if !reads.contains(&read) {
                    reads.push(read);
                }
            }
            RecurrenceMember::new(
                facts.members[member].identity.clone(),
                facts.members[member]
                    .definition
                    .shape()
                    .unwrap_or(RecurrenceShape::For),
                reads,
            )
        })
        .collect();
    RecurrenceGroupPlan::new(members)
}

/// Place members one at a time, each time the first member in source order
/// whose same-step targets are all placed (`Group.kahn`). Returns the order,
/// or the members that cannot be placed.
fn greedy_order<I: Iterator<Item = usize>>(
    count: usize,
    same_targets: &impl Fn(usize) -> I,
) -> Result<Vec<usize>, Vec<usize>> {
    let mut placed = vec![false; count];
    let mut order = Vec::with_capacity(count);
    while order.len() < count {
        let ready = (0..count)
            .find(|&member| !placed[member] && same_targets(member).all(|target| placed[target]));
        let Some(member) = ready else {
            return Err((0..count).filter(|member| !placed[*member]).collect());
        };
        placed[member] = true;
        order.push(member);
    }
    Ok(order)
}

/// A same-step cycle among `stuck` members, every one of which reads another
/// stuck member at the same step: walk from `start` to the first stuck member
/// each member reads until a member repeats, and return the repeated part
/// (`Group.extractCycle`). The result is never empty: it is the walk so far
/// should a member read no stuck member.
fn extract_cycle<I: Iterator<Item = usize>>(
    start: usize,
    stuck: &[usize],
    same_targets: &impl Fn(usize) -> I,
) -> Vec<usize> {
    let mut visited = Vec::new();
    let mut current = start;
    while !visited.contains(&current) {
        visited.push(current);
        let Some(next) = same_targets(current).find(|target| stuck.contains(target)) else {
            return visited;
        };
        current = next;
    }
    let repeated = visited
        .iter()
        .position(|member| *member == current)
        .unwrap_or(0);
    visited.split_off(repeated)
}

/// A member's own binders: the step binder and, for an `unfold`, the
/// previous-step binder.
#[derive(Debug, Clone, Copy)]
struct OwnBinders {
    step: LocalId,
    previous: Option<LocalId>,
}

/// The definition of a formula, after removing one top-level display
/// conversion, and its own binders when it has a recurrence shape.
fn definition_of(expr: &Expr) -> (Definition, Option<OwnBinders>) {
    let expr = match expr.kind() {
        ExprKind::Convert { expr, .. } | ExprKind::DisplayTimezone { expr, .. } => expr,
        _ => expr,
    };
    match expr.kind() {
        ExprKind::ForComp { bindings, .. } => {
            bindings
                .first()
                .map_or((Definition::OtherFormula, None), |first| {
                    (
                        Definition::ForOver(match &first.index {
                            ForBindingIndex::Named(name) => GroupAxis::Named(name.value.clone()),
                            ForBindingIndex::Finite { cardinality, .. } => {
                                GroupAxis::Finite(cardinality.value.clone())
                            }
                        }),
                        Some(OwnBinders {
                            step: first.local.id,
                            previous: None,
                        }),
                    )
                })
        }
        ExprKind::Unfold { recurrence, .. } => (
            Definition::UnfoldOver(GroupAxis::Named(recurrence.axis.value.clone())),
            Some(OwnBinders {
                step: recurrence.current_index.id,
                previous: Some(recurrence.previous_index.id),
            }),
        ),
        _ => (Definition::OtherFormula, None),
    }
}

/// Check one cyclic component of the runtime schedule of the callable
/// `callable`: each scheduled declaration with the DAG that declares it, in
/// schedule insertion order.
///
/// # Errors
///
/// Returns why the component is not an accepted recurrence group.
pub(crate) fn check_component(
    component: &[(&ScheduledDecl, &DagTIR)],
    callable: &DagId,
    unit_scale_reads: &HashMap<ResolvedUnitName, BTreeSet<ResolvedDeclName>>,
) -> Result<RecurrenceGroupPlan, ComponentRejection> {
    let candidates = component
        .iter()
        .map(|(declaration, dag)| CandidateMember {
            identity: declaration.identity(),
            name: declaration.name(),
            span: declaration.span(),
            dag,
        })
        .collect::<Vec<_>>();
    check_group(&group_facts(&candidates, unit_scale_reads), callable)
}

/// One candidate member as the runtime schedule knows it.
struct CandidateMember<'t> {
    identity: &'t ResolvedDeclName,
    name: &'t DeclName,
    span: Span,
    /// The DAG that declares the member.
    dag: &'t DagTIR,
}

/// Read the facts of one candidate group out of HIR.
///
/// Every read a member's evaluation can make of a member is listed: each
/// `@y` occurrence, each quantity literal whose dynamic unit's scale reads a
/// member (`unit_scale_reads`), and, so that no dependency escapes the
/// classification, every remaining runtime dependency on a member as an
/// implicit read at the reader's declaration.
fn group_facts(
    candidates: &[CandidateMember<'_>],
    unit_scale_reads: &HashMap<ResolvedUnitName, BTreeSet<ResolvedDeclName>>,
) -> GroupFacts {
    let positions = candidates
        .iter()
        .enumerate()
        .map(|(position, candidate)| (candidate.identity, position))
        .collect::<HashMap<_, _>>();
    let mut members = Vec::with_capacity(candidates.len());
    let mut reads = Vec::new();
    for (source, candidate) in candidates.iter().enumerate() {
        let dag = candidate.dag;
        let (definition, binders) = dag
            .runtime_expr(candidate.identity)
            .map_or((Definition::OtherFormula, None), definition_of);
        let first_read = reads.len();
        if let Some(expr) = dag.runtime_expr(candidate.identity) {
            let mut collector = ReadCollector {
                dag,
                positions: &positions,
                unit_scale_reads,
                binders,
                source,
                reads: &mut reads,
            };
            collector.walk(expr);
        }
        let explained = reads[first_read..]
            .iter()
            .map(|read| read.target)
            .collect::<BTreeSet<_>>();
        let unexplained = dag
            .semantic()
            .dependencies
            .runtime_deps
            .get(candidate.identity)
            .into_iter()
            .flatten()
            .filter_map(|dependency| positions.get(dependency).copied())
            .filter(|target| !explained.contains(target))
            .collect::<BTreeSet<_>>();
        reads.extend(unexplained.into_iter().map(|target| ReadFact {
            source,
            target,
            form: ReadForm::Implicit,
            span: candidate.span,
        }));
        members.push(MemberFact {
            identity: candidate.identity.clone(),
            name: candidate.name.clone(),
            span: candidate.span,
            definition,
        });
    }
    GroupFacts { members, reads }
}

/// Collects the reads of one member's formula.
struct ReadCollector<'c, 't> {
    dag: &'t DagTIR,
    positions: &'c HashMap<&'t ResolvedDeclName, usize>,
    unit_scale_reads: &'c HashMap<ResolvedUnitName, BTreeSet<ResolvedDeclName>>,
    binders: Option<OwnBinders>,
    source: usize,
    reads: &'c mut Vec<ReadFact>,
}

impl ReadCollector<'_, '_> {
    fn member(&self, reference: &crate::hir::expr::LocalDecl) -> Option<usize> {
        self.positions
            .get(&self.dag.frame().resolve(reference))
            .copied()
    }

    fn push(&mut self, target: usize, form: ReadForm, span: Span) {
        self.reads.push(ReadFact {
            source: self.source,
            target,
            form,
            span,
        });
    }

    fn leading_key(&self, arg: &IndexArg) -> LeadingKey {
        match (arg, self.binders) {
            (IndexArg::Var(local), Some(binders)) if local.value == binders.step => {
                LeadingKey::StepBinder
            }
            (IndexArg::Var(local), Some(binders)) if Some(local.value) == binders.previous => {
                LeadingKey::PreviousBinder
            }
            (IndexArg::Var(_), _) => LeadingKey::OtherLocal,
            (IndexArg::Variant(_) | IndexArg::Expr(_), _) => LeadingKey::Computed,
        }
    }

    fn walk(&mut self, expr: &Expr) {
        crate::stack::with_stack_growth(|| match expr.kind() {
            ExprKind::IndexAccess { expr: base, args } => {
                if let ExprKind::GraphRef(target) = base.kind() {
                    if let Some(member) = self.member(&target.value) {
                        let key = self.leading_key(args.first());
                        self.push(member, ReadForm::Element(key), target.span);
                    }
                } else {
                    self.walk(base);
                }
                for arg in args {
                    if let IndexArg::Expr(arg) = arg {
                        self.walk(arg);
                    }
                }
            }
            ExprKind::GraphRef(target) => {
                if let Some(member) = self.member(&target.value) {
                    self.push(member, ReadForm::Whole, target.span);
                }
            }
            ExprKind::QuantityLiteral { unit, .. } => {
                let targets = unit
                    .terms
                    .iter()
                    .filter_map(|term| {
                        self.unit_scale_reads
                            .get(&self.dag.frame().resolve_unit(&term.name.value))
                    })
                    .flatten()
                    .filter_map(|dependency| self.positions.get(dependency).copied())
                    .collect::<BTreeSet<_>>();
                for target in targets {
                    self.push(target, ReadForm::Implicit, expr.span);
                }
            }
            _ => crate::hir::expr::visit_expr_children(expr, &mut |child| self.walk(child)),
        });
    }
}

#[cfg(test)]
mod tests;
