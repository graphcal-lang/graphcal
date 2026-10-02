//! The DAGs of one program, each at the [`DagPosition`] it joined with.
//!
//! Owned local bodies and imported checked handles keep their positions from
//! the draft through the checked registry.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::dag_id::DagId;

use super::checked_dag::CheckedDag;
use super::dag_position::DagPosition;
use super::model::DagTIR;

/// What a registry holds for each local body: an unchecked [`DagTIR`] or a
/// [`CheckedDag`]. An imported body is a shared [`CheckedDag`] in both, seen
/// as the same kind of value as a local one.
pub trait SlotBody {
    /// The view of an imported checked handle as this kind of body.
    fn of_shared(shared: &CheckedDag) -> &Self;
    /// The body's canonical identity.
    fn slot_id(&self) -> &DagId;
}

impl SlotBody for DagTIR {
    fn of_shared(shared: &CheckedDag) -> &Self {
        shared.body()
    }

    fn slot_id(&self) -> &DagId {
        self.dag_id()
    }
}

impl SlotBody for CheckedDag {
    fn of_shared(shared: &CheckedDag) -> &Self {
        shared
    }

    fn slot_id(&self) -> &DagId {
        self.dag_id()
    }
}

/// Where the DAG at one position lives.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// The local body at this index.
    Local(usize),
    /// The imported handle at this index.
    Shared(usize),
}

/// Every DAG of one program at its position: local bodies (the root first)
/// and imported checked handles.
///
/// A DAG keeps the position it was given when it joined, so a position
/// handed out by a draft names the same DAG in the checked registry built
/// from it. The root is at [`DagPosition::ROOT`], so its presence is
/// structural. Each DAG is keyed from its own identity, so a caller cannot
/// pair a body with a different key.
///
/// Visiting order is independent of positions: the root, then the other
/// local bodies in identity order, then the imported handles in identity
/// order.
#[derive(Debug, Clone)]
pub struct DagSlots<L> {
    locals: Vec<L>,
    /// The position of each local body.
    local_positions: Vec<DagPosition>,
    shared: Vec<Arc<CheckedDag>>,
    slots: Vec<Slot>,
    positions: HashMap<DagId, DagPosition>,
    /// Local bodies other than the root, by identity, to their index.
    local_order: BTreeMap<DagId, usize>,
    /// Imported handles by identity, to their index.
    shared_order: BTreeMap<DagId, usize>,
}

/// Where one DAG of a registry lives (see [`DagSlots::slot_at`]).
pub(super) enum SlotRef<'a> {
    /// The local body at this index.
    Local(usize),
    /// The imported handle.
    Shared(&'a Arc<CheckedDag>),
}

/// Failure to add a DAG to a TIR registry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DagRegistryError {
    /// A canonical DAG identity can occur only once in one compiled registry.
    #[error("DAG `{dag_id}` is already present in the TIR registry")]
    DuplicateDag { dag_id: DagId },
}

impl<L: SlotBody> DagSlots<L> {
    /// A registry of only `root`, at [`DagPosition::ROOT`].
    pub(super) fn new(root: L) -> Self {
        let root_id = root.slot_id().clone();
        Self {
            locals: vec![root],
            local_positions: vec![DagPosition::ROOT],
            shared: Vec::new(),
            slots: vec![Slot::Local(0)],
            positions: HashMap::from([(root_id, DagPosition::ROOT)]),
            local_order: BTreeMap::new(),
            shared_order: BTreeMap::new(),
        }
    }

    fn next_position(&self, dag_id: &DagId) -> Result<DagPosition, DagRegistryError> {
        if self.positions.contains_key(dag_id) {
            return Err(DagRegistryError::DuplicateDag {
                dag_id: dag_id.clone(),
            });
        }
        Ok(DagPosition::new(self.slots.len()))
    }

    /// Add a local body at the next position.
    ///
    /// # Errors
    ///
    /// Returns [`DagRegistryError::DuplicateDag`] when its identity is already present.
    pub(super) fn push_local(&mut self, dag: L) -> Result<DagPosition, DagRegistryError> {
        let dag_id = dag.slot_id().clone();
        let position = self.next_position(&dag_id)?;
        let index = self.locals.len();
        self.locals.push(dag);
        self.local_positions.push(position);
        self.slots.push(Slot::Local(index));
        self.positions.insert(dag_id.clone(), position);
        self.local_order.insert(dag_id, index);
        Ok(position)
    }

    /// Add an imported handle at the next position.
    ///
    /// # Errors
    ///
    /// Returns [`DagRegistryError::DuplicateDag`] when its identity is already present.
    pub(super) fn push_shared(
        &mut self,
        dag: Arc<CheckedDag>,
    ) -> Result<DagPosition, DagRegistryError> {
        let dag_id = dag.dag_id().clone();
        let position = self.next_position(&dag_id)?;
        let index = self.shared.len();
        self.shared.push(dag);
        self.slots.push(Slot::Shared(index));
        self.positions.insert(dag_id.clone(), position);
        self.shared_order.insert(dag_id, index);
        Ok(position)
    }

    /// The root body.
    #[must_use]
    pub fn root(&self) -> &L {
        // `new` puts the root first and nothing removes it.
        &self.locals[0]
    }

    /// The root's identity.
    #[must_use]
    pub fn root_id(&self) -> &DagId {
        self.root().slot_id()
    }

    /// Whether a DAG with this identity is present.
    #[must_use]
    pub fn contains(&self, dag_id: &DagId) -> bool {
        self.positions.contains_key(dag_id)
    }

    /// The position of one DAG.
    #[must_use]
    pub fn position(&self, dag_id: &DagId) -> Option<DagPosition> {
        self.positions.get(dag_id).copied()
    }

    /// The DAG at `position`.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    #[must_use]
    pub fn at(&self, position: DagPosition) -> &L {
        match self.slots[position.index()] {
            Slot::Local(index) => &self.locals[index],
            Slot::Shared(index) => L::of_shared(&self.shared[index]),
        }
    }

    /// One DAG by identity.
    #[must_use]
    pub fn get(&self, dag_id: &DagId) -> Option<&L> {
        self.position(dag_id).map(|position| self.at(position))
    }

    /// The imported checked handle with this identity.
    #[must_use]
    pub fn shared(&self, dag_id: &DagId) -> Option<&CheckedDag> {
        self.shared_order
            .get(dag_id)
            .map(|&index| self.shared[index].as_ref())
    }

    /// The imported checked handle at `position`; `None` when the DAG there
    /// is local.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    #[must_use]
    pub fn shared_at(&self, position: DagPosition) -> Option<&CheckedDag> {
        match self.slots[position.index()] {
            Slot::Local(_) => None,
            Slot::Shared(index) => Some(&self.shared[index]),
        }
    }

    /// The imported checked handles, in identity order.
    pub fn shared_iter(&self) -> impl Iterator<Item = (&DagId, &Arc<CheckedDag>)> {
        self.shared_order
            .iter()
            .map(|(dag_id, &index)| (dag_id, &self.shared[index]))
    }

    /// The local bodies in visiting order: the root, then the others in
    /// identity order.
    pub fn local_iter(&self) -> impl Iterator<Item = (&DagId, &L)> {
        let root = self.root();
        std::iter::once((root.slot_id(), root)).chain(
            self.local_order
                .iter()
                .map(|(dag_id, &index)| (dag_id, &self.locals[index])),
        )
    }

    /// Every local body with its position, in no particular order.
    pub fn local_positioned(&self) -> impl Iterator<Item = (DagPosition, &L)> {
        self.local_positions.iter().copied().zip(&self.locals)
    }

    /// Every DAG in visiting order: the local bodies, then the imported
    /// handles in identity order.
    pub fn iter(&self) -> impl Iterator<Item = (&DagId, &L)> {
        self.local_iter().chain(
            self.shared_order
                .iter()
                .map(|(dag_id, &index)| (dag_id, L::of_shared(&self.shared[index]))),
        )
    }

    /// Every DAG with its position, in position order.
    pub fn positioned(&self) -> impl Iterator<Item = (DagPosition, &L)> {
        (0..self.slots.len()).map(|index| {
            let position = DagPosition::new(index);
            (position, self.at(position))
        })
    }

    /// Mutably borrow every local body, in position order.
    pub(super) fn locals_mut(&mut self) -> impl Iterator<Item = &mut L> {
        self.locals.iter_mut()
    }

    /// Mutably borrow the local body at `position`, first copying an
    /// imported handle into a local body at the same position.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    pub(super) fn localized_mut(&mut self, position: DagPosition) -> &mut L
    where
        L: Clone,
    {
        let index = match self.slots[position.index()] {
            Slot::Local(index) => index,
            Slot::Shared(index) => {
                let local = L::of_shared(&self.shared[index]).clone();
                let dag_id = local.slot_id().clone();
                self.shared_order.remove(&dag_id);
                let local_index = self.locals.len();
                self.locals.push(local);
                self.local_positions.push(position);
                self.local_order.insert(dag_id, local_index);
                self.slots[position.index()] = Slot::Local(local_index);
                local_index
            }
        };
        &mut self.locals[index]
    }

    /// Number of local bodies.
    pub(super) const fn local_count(&self) -> usize {
        self.locals.len()
    }

    /// The indices of the local bodies in visiting order: the root, then
    /// the others in identity order.
    pub(super) fn visiting_local_indices(&self) -> impl Iterator<Item = usize> + '_ {
        std::iter::once(0).chain(self.local_order.values().copied())
    }

    /// The local body at `index` with its position.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not the index of one of the local bodies.
    pub(super) fn local_at_index(&self, index: usize) -> (DagPosition, &L) {
        (self.local_positions[index], &self.locals[index])
    }

    /// Where the DAG at `position` lives: the index of a local body, or the
    /// imported handle.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    pub(super) fn slot_at(&self, position: DagPosition) -> SlotRef<'_> {
        match self.slots[position.index()] {
            Slot::Local(index) => SlotRef::Local(index),
            Slot::Shared(index) => SlotRef::Shared(&self.shared[index]),
        }
    }

    /// Every DAG with its position, in visiting order: the local bodies,
    /// then the imported handles in identity order.
    pub(super) fn iter_positions(&self) -> impl Iterator<Item = DagPosition> + '_ {
        self.visiting_local_indices()
            .map(|index| self.local_positions[index])
            .chain(
                self.shared_order
                    .keys()
                    .map(|dag_id| self.positions[dag_id]),
            )
    }

    /// Number of DAGs, local and imported.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.slots.len()
    }

    /// A registry always has its root.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Consume the registry into its local bodies, in position order, and
    /// its imported handles.
    pub(super) fn into_locals(self) -> (Vec<L>, Vec<Arc<CheckedDag>>) {
        (self.locals, self.shared)
    }
}

impl<L: SlotBody> std::ops::Index<&DagId> for DagSlots<L> {
    type Output = L;

    /// The DAG with this identity.
    ///
    /// # Panics
    ///
    /// Panics when no DAG of this registry has the identity.
    fn index(&self, dag_id: &DagId) -> &L {
        self.at(self.positions[dag_id])
    }
}

/// One fact for each local body of one [`DagSlots`] registry, aligned with
/// its local bodies.
///
/// Only [`DagSlots::map_local`] creates a table, and its methods keep the
/// alignment, so a table pairs with its registry's local bodies without a
/// lookup.
#[derive(Debug, Clone)]
pub struct LocalDagFacts<T> {
    facts: Vec<T>,
}

impl<L: SlotBody> DagSlots<L> {
    /// Compute one fact for each local body, visiting them in visiting order
    /// (the root, then the others in identity order).
    ///
    /// # Errors
    ///
    /// Returns the first error `fact` returns, in visiting order.
    pub fn map_local<'r, T, E>(
        &'r self,
        mut fact: impl FnMut(DagPosition, &'r L) -> Result<T, E>,
    ) -> Result<LocalDagFacts<T>, E> {
        let mut facts = std::iter::once(0)
            .chain(self.local_order.values().copied())
            .map(|index| {
                fact(self.local_positions[index], &self.locals[index]).map(|value| (index, value))
            })
            .collect::<Result<Vec<_>, E>>()?;
        // The visiting order is a permutation of the local indices.
        facts.sort_by_key(|(index, _)| *index);
        Ok(LocalDagFacts {
            facts: facts.into_iter().map(|(_, value)| value).collect(),
        })
    }

    /// Every local body with its fact from `facts`, in visiting order.
    ///
    /// # Panics
    ///
    /// Panics when `facts` was mapped from another registry with fewer
    /// local bodies.
    pub fn with_local_facts<'a, T>(
        &'a self,
        facts: &'a LocalDagFacts<T>,
    ) -> impl Iterator<Item = (&'a L, &'a T)> {
        std::iter::once(0)
            .chain(self.local_order.values().copied())
            .map(|index| (&self.locals[index], &facts.facts[index]))
    }

    /// Replace each fact of `facts` with the result of pairing it with its
    /// local body.
    pub fn map_local_facts<T, U>(
        &self,
        facts: LocalDagFacts<T>,
        mut map: impl FnMut(&L, T) -> U,
    ) -> LocalDagFacts<U> {
        debug_assert_eq!(self.locals.len(), facts.facts.len());
        LocalDagFacts {
            facts: self
                .locals
                .iter()
                .zip(facts.facts)
                .map(|(body, fact)| map(body, fact))
                .collect(),
        }
    }

    /// The local body with this identity and its fact, when the DAG is
    /// local; `None` for an imported or unknown DAG.
    #[must_use]
    pub fn local_with_fact<'a, T>(
        &'a self,
        facts: &'a LocalDagFacts<T>,
        dag_id: &DagId,
    ) -> Option<(&'a L, &'a T)> {
        match self.slots[self.position(dag_id)?.index()] {
            Slot::Local(index) => Some((&self.locals[index], &facts.facts[index])),
            Slot::Shared(_) => None,
        }
    }

    /// The fact of the local body at `position`; `None` when the DAG there
    /// is imported.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    #[must_use]
    pub fn local_fact_at<'a, T>(
        &self,
        facts: &'a LocalDagFacts<T>,
        position: DagPosition,
    ) -> Option<&'a T> {
        match self.slots[position.index()] {
            Slot::Local(index) => Some(&facts.facts[index]),
            Slot::Shared(_) => None,
        }
    }

    /// The fact of the local body with this identity.
    #[must_use]
    pub fn local_fact<'a, T>(&self, facts: &'a LocalDagFacts<T>, dag_id: &DagId) -> Option<&'a T> {
        match self.slots[self.position(dag_id)?.index()] {
            Slot::Local(index) => facts.facts.get(index),
            Slot::Shared(_) => None,
        }
    }

    /// Replace each local body with the result of pairing it with its fact,
    /// keeping every DAG's position.
    pub(super) fn zip_locals<T, M: SlotBody>(
        self,
        facts: LocalDagFacts<T>,
        mut pair: impl FnMut(L, T) -> M,
    ) -> DagSlots<M> {
        debug_assert_eq!(self.locals.len(), facts.facts.len());
        DagSlots {
            locals: self
                .locals
                .into_iter()
                .zip(facts.facts)
                .map(|(body, fact)| pair(body, fact))
                .collect(),
            local_positions: self.local_positions,
            shared: self.shared,
            slots: self.slots,
            positions: self.positions,
            local_order: self.local_order,
            shared_order: self.shared_order,
        }
    }
}

impl<T> LocalDagFacts<T> {
    /// The table of `facts`, one for each local body in local-index order.
    pub(super) const fn from_aligned(facts: Vec<T>) -> Self {
        Self { facts }
    }
}

impl<T> LocalDagFacts<T> {
    /// Replace each fact.
    pub fn map<U>(self, map: impl FnMut(T) -> U) -> LocalDagFacts<U> {
        LocalDagFacts {
            facts: self.facts.into_iter().map(map).collect(),
        }
    }

    /// Replace each fact, or stop at the first error, in alignment order.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns.
    pub fn try_map<U, E>(self, map: impl FnMut(T) -> Result<U, E>) -> Result<LocalDagFacts<U>, E> {
        Ok(LocalDagFacts {
            facts: self.facts.into_iter().map(map).collect::<Result<_, E>>()?,
        })
    }

    /// Derive one fact from each fact, or stop at the first error, in
    /// alignment order.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns.
    pub fn try_map_ref<U, E>(
        &self,
        map: impl FnMut(&T) -> Result<U, E>,
    ) -> Result<LocalDagFacts<U>, E> {
        Ok(LocalDagFacts {
            facts: self.facts.iter().map(map).collect::<Result<_, E>>()?,
        })
    }

    /// Pair each fact with the other table's fact of the same body.
    #[must_use]
    pub fn zip<U>(self, other: LocalDagFacts<U>) -> LocalDagFacts<(T, U)> {
        debug_assert_eq!(self.facts.len(), other.facts.len());
        LocalDagFacts {
            facts: self.facts.into_iter().zip(other.facts).collect(),
        }
    }
}

impl<T, U> LocalDagFacts<(T, U)> {
    /// Split a table of pairs into a table of each half.
    #[must_use]
    pub fn unzip(self) -> (LocalDagFacts<T>, LocalDagFacts<U>) {
        let (left, right) = self.facts.into_iter().unzip();
        (
            LocalDagFacts { facts: left },
            LocalDagFacts { facts: right },
        )
    }
}
