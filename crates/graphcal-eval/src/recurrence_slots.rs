//! The element slots of one recurrence group while a frame evaluates it.
//!
//! A recurrence group (`internals/recurrence-groups.md`) is evaluated element
//! by element: step by step along its axis and, within a step, member by
//! member in plan order. Each element is held in a slot until the group
//! finishes. A read of a member inside the group goes through
//! [`RecurrenceSlots::computed`], which refuses an element that has not been
//! computed: the checker's plan guarantees such a read never happens, so the
//! refusal is reported as an internal error rather than a value.

use graphcal_compiler::resolved_name::ResolvedDeclName;

use crate::runtime_presentation::EvaluatedRuntimeValue;

/// The status of one element of a group member.
#[derive(Debug, Clone)]
pub(crate) enum ElementSlot {
    /// Not evaluated yet.
    Pending,
    Computed(EvaluatedRuntimeValue),
    /// Its own evaluation failed.
    Failed,
    /// Not evaluated because an input is not available.
    Unavailable,
}

impl ElementSlot {
    pub(crate) const fn is_computed(&self) -> bool {
        matches!(self, Self::Computed(_))
    }
}

/// Why a member element cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadableElement {
    /// The declaration is not a member of the running group.
    NotAMember,
    /// The element has not been computed: it is pending, failed, or
    /// unavailable.
    NotComputed,
}

/// The slots of every member of one running group, in plan order.
#[derive(Debug)]
pub struct RecurrenceSlots {
    members: Vec<(ResolvedDeclName, Vec<ElementSlot>)>,
}

impl RecurrenceSlots {
    /// Pending slots for `members`, each with `steps` elements.
    pub(crate) fn new<'a>(
        members: impl IntoIterator<Item = &'a ResolvedDeclName>,
        steps: usize,
    ) -> Self {
        Self {
            members: members
                .into_iter()
                .map(|member| (member.clone(), vec![ElementSlot::Pending; steps]))
                .collect(),
        }
    }

    /// Whether `declaration` is a member of the running group.
    #[must_use]
    pub fn contains(&self, declaration: &ResolvedDeclName) -> bool {
        self.members.iter().any(|(member, _)| member == declaration)
    }

    /// The computed element `step` of `declaration`.
    ///
    /// # Errors
    ///
    /// Returns why the element cannot be read.
    pub fn computed(
        &self,
        declaration: &ResolvedDeclName,
        step: usize,
    ) -> Result<&EvaluatedRuntimeValue, UnreadableElement> {
        let (_, slots) = self
            .members
            .iter()
            .find(|(member, _)| member == declaration)
            .ok_or(UnreadableElement::NotAMember)?;
        match slots.get(step) {
            Some(ElementSlot::Computed(value)) => Ok(value),
            _ => Err(UnreadableElement::NotComputed),
        }
    }

    /// The slot of element `step` of the member at plan position `member`.
    pub(crate) fn slot(&self, member: usize, step: usize) -> Option<&ElementSlot> {
        self.members
            .get(member)
            .and_then(|(_, slots)| slots.get(step))
    }

    /// Replace the slot of element `step` of the member at `member`.
    pub(crate) fn set(&mut self, member: usize, step: usize, slot: ElementSlot) {
        if let Some(target) = self
            .members
            .get_mut(member)
            .and_then(|(_, slots)| slots.get_mut(step))
        {
            *target = slot;
        }
    }

    /// Take every element of the member at `member`, when all of them are
    /// computed.
    pub(crate) fn take_computed(&mut self, member: usize) -> Option<Vec<EvaluatedRuntimeValue>> {
        let (_, slots) = self.members.get_mut(member)?;
        if !slots.iter().all(ElementSlot::is_computed) {
            return None;
        }
        Some(
            std::mem::take(slots)
                .into_iter()
                .filter_map(|slot| match slot {
                    ElementSlot::Computed(value) => Some(value),
                    ElementSlot::Pending | ElementSlot::Failed | ElementSlot::Unavailable => None,
                })
                .collect(),
        )
    }
}
