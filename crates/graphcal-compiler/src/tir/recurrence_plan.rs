//! The evaluation plan of an accepted recurrence group
//! (`internals/recurrence-groups.md`): its members in plan order, each with
//! its classified reads, and the element dependencies evaluation follows.

use crate::resolved_name::ResolvedDeclName;

/// The two definition shapes of a recurrence group member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceShape {
    /// `for t: A, … { body }`: the body runs at every step.
    For,
    /// `unfold(A, init, |prev_state, prev_t, t| body)`: the init is the
    /// first step, and the closure runs at every later step.
    Unfold,
}

impl RecurrenceShape {
    /// Whether a member of this shape evaluates its body, and with it its
    /// reads of other members, at `step`.
    #[must_use]
    pub const fn runs_body_at(self, step: usize) -> bool {
        match self {
            Self::For => true,
            Self::Unfold => step != 0,
        }
    }
}

/// The step offset of a classified read between members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAccess {
    /// The element at the reader's current step (`@y[t, …]`).
    Same,
    /// The element at the reader's previous step (`@y[prev_t, …]`).
    Previous,
}

/// One classified read of a member by a member of the same group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupRead {
    target: usize,
    access: StepAccess,
}

impl GroupRead {
    /// A read of the member at plan position `target`, for the checker.
    pub(crate) const fn new(target: usize, access: StepAccess) -> Self {
        Self { target, access }
    }

    /// The member read, as a position in the plan order.
    #[must_use]
    pub const fn target(&self) -> usize {
        self.target
    }

    /// The step the read selects, relative to the reader's step.
    #[must_use]
    pub const fn access(&self) -> StepAccess {
        self.access
    }
}

/// One member of an accepted group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceMember {
    declaration: ResolvedDeclName,
    shape: RecurrenceShape,
    reads: Vec<GroupRead>,
}

impl RecurrenceMember {
    /// A member and its distinct reads, for the checker.
    pub(crate) const fn new(
        declaration: ResolvedDeclName,
        shape: RecurrenceShape,
        reads: Vec<GroupRead>,
    ) -> Self {
        Self {
            declaration,
            shape,
            reads,
        }
    }

    /// The member's runtime identity.
    #[must_use]
    pub const fn declaration(&self) -> &ResolvedDeclName {
        &self.declaration
    }

    /// Whether the member is a `for` or an `unfold`.
    #[must_use]
    pub const fn shape(&self) -> RecurrenceShape {
        self.shape
    }

    /// The member's distinct reads of other members of the group.
    #[must_use]
    pub fn reads(&self) -> &[GroupRead] {
        &self.reads
    }
}

/// The evaluation plan of an accepted recurrence group.
///
/// Only the checker ([`crate::tir::recurrence_group`]) constructs one, so
/// holding a plan proves that its group satisfies R1–R7 and that its member
/// order evaluates every member after the members it reads at the same step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceGroupPlan {
    members: Vec<RecurrenceMember>,
}

impl RecurrenceGroupPlan {
    /// The plan of members already in plan order, for the checker.
    pub(crate) const fn new(members: Vec<RecurrenceMember>) -> Self {
        Self { members }
    }

    /// The members in plan order: the order they are evaluated in within
    /// every step.
    #[must_use]
    pub fn members(&self) -> &[RecurrenceMember] {
        &self.members
    }

    /// The elements `(member, step)` that evaluating element `step` of
    /// `member` reads (`Group.ElementDep`): its same-step reads when its body
    /// runs at `step`, its previous-step reads and, for an `unfold`, its own
    /// previous state from the second step on. Members are positions in the
    /// plan order.
    #[must_use]
    pub fn element_dependencies(&self, member: usize, step: usize) -> Vec<(usize, usize)> {
        let Some(reader) = self.members.get(member) else {
            return Vec::new();
        };
        let mut dependencies = Vec::new();
        for read in &reader.reads {
            let element = match read.access {
                StepAccess::Same => reader
                    .shape
                    .runs_body_at(step)
                    .then_some((read.target, step)),
                StepAccess::Previous => step.checked_sub(1).map(|previous| (read.target, previous)),
            };
            if let Some(element) = element
                && !dependencies.contains(&element)
            {
                dependencies.push(element);
            }
        }
        if reader.shape == RecurrenceShape::Unfold
            && let Some(previous) = step.checked_sub(1)
            && !dependencies.contains(&(member, previous))
        {
            dependencies.push((member, previous));
        }
        dependencies
    }
}
