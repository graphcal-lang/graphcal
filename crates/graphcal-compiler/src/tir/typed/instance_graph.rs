//! The semantic instance edges of one instantiated program, by position.
//!
//! Instantiation is the only place an include edge is followed by name: it
//! finds the edge's template and either finds or materializes its instance.
//! [`InstanceGraph`] records both answers by [`DagPosition`], so checking
//! reads an edge's instance, an instance's template, and the template's
//! checked facts without a lookup that could miss.

use std::sync::Arc;

use crate::ir::static_substitution::StaticSpecializationId;

use super::checked_dag::CheckedDag;
use super::dag_position::DagPosition;
use super::dag_slots::{DagRegistryError, DagSlots, LocalDagFacts, SlotRef};
use super::model::DagTIR;

/// The registry of the DAG bodies of a TIR before it is checked.
type DagRegistry = DagSlots<DagTIR>;

/// The index of a canonical local body: a module or inline DAG that runs the
/// bodies it defines, as opposed to a materialized semantic instance.
///
/// Issued only by the [`InstanceGraph`] of the program, so it always indexes
/// that program's [`CanonicalFacts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalIndex(usize);

/// The canonical body a semantic instance is specialized from.
#[derive(Debug, Clone)]
pub enum TemplateBody {
    /// A local canonical body, checked with the program.
    Local(CanonicalIndex),
    /// An imported, already checked body.
    Shared(Arc<CheckedDag>),
}

/// Where one materialized semantic instance comes from.
#[derive(Debug, Clone)]
pub struct InstanceOrigin {
    template_position: DagPosition,
    template: TemplateBody,
    specialization: StaticSpecializationId,
}

impl InstanceOrigin {
    /// The position of the template.
    #[must_use]
    pub const fn template_position(&self) -> DagPosition {
        self.template_position
    }

    /// The template body.
    #[must_use]
    pub const fn template(&self) -> &TemplateBody {
        &self.template
    }

    /// The Static specialization the instance applies to its template.
    #[must_use]
    pub const fn specialization(&self) -> &StaticSpecializationId {
        &self.specialization
    }
}

/// The semantic instance edges of one program: for each DAG, the position
/// of the instance each of its edges materializes, and for each materialized
/// instance, its template.
///
/// Built only by instantiation, together with the registry it describes:
/// every local body present before instantiation is canonical, and every
/// local body instantiation adds is an instance, added through
/// `push_instance` so its origin is recorded with it.
#[derive(Debug)]
pub struct InstanceGraph {
    /// The local bodies at indices below this are canonical.
    canonical_locals: usize,
    /// For each position, the positions of the instances its edges
    /// materialize, aligned with its semantic instance edges.
    edges: Vec<Vec<DagPosition>>,
    /// The origin of each materialized instance, by local index minus
    /// `canonical_locals`.
    origins: Vec<InstanceOrigin>,
}

impl InstanceGraph {
    /// The graph of `dags` before instantiation: every local body is
    /// canonical and no edge is recorded yet.
    pub(super) fn new(dags: &DagRegistry) -> Self {
        Self {
            canonical_locals: dags.local_count(),
            edges: vec![Vec::new(); dags.len()],
            origins: Vec::new(),
        }
    }

    /// The template named `template`: its position and body; `None` when
    /// `dags` has no canonical body with this identity.
    pub(super) fn template(
        &self,
        dags: &DagRegistry,
        template: &crate::dag_id::DagId,
    ) -> Option<(DagPosition, TemplateBody)> {
        let position = dags.position(template)?;
        let body = match dags.slot_at(position) {
            SlotRef::Local(index) if index < self.canonical_locals => {
                TemplateBody::Local(CanonicalIndex(index))
            }
            SlotRef::Local(_) => return None,
            SlotRef::Shared(shared) => TemplateBody::Shared(Arc::clone(shared)),
        };
        Some((position, body))
    }

    /// Add the materialized instance `dag`, specialized from `template` at
    /// `template_position` with `specialization`, to `dags`.
    ///
    /// # Errors
    ///
    /// Returns [`DagRegistryError::DuplicateDag`] when its identity is
    /// already present.
    pub(super) fn push_instance(
        &mut self,
        dags: &mut DagRegistry,
        dag: DagTIR,
        (template_position, template): (DagPosition, TemplateBody),
        specialization: StaticSpecializationId,
    ) -> Result<DagPosition, DagRegistryError> {
        let position = dags.push_local(dag)?;
        debug_assert_eq!(
            dags.local_count(),
            self.canonical_locals + self.origins.len() + 1
        );
        self.edges.push(Vec::new());
        self.origins.push(InstanceOrigin {
            template_position,
            template,
            specialization,
        });
        Ok(position)
    }

    /// Record that the next edge of the DAG at `holder` materializes the
    /// instance at `instance`.
    pub(super) fn record_edge(&mut self, holder: DagPosition, instance: DagPosition) {
        self.edges[holder.index()].push(instance);
    }

    /// The positions of the instances the edges of the DAG at `position`
    /// materialize, in edge order.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another program with more
    /// DAGs.
    #[must_use]
    pub fn instances_of(&self, position: DagPosition) -> &[DagPosition] {
        &self.edges[position.index()]
    }

    /// One fact for each canonical local body, visiting them in visiting
    /// order (the root, then the others in identity order).
    ///
    /// # Errors
    ///
    /// Returns the first error `fact` returns, in visiting order.
    pub fn map_canonical<'r, T, E>(
        &self,
        dags: &'r DagRegistry,
        mut fact: impl FnMut(DagPosition, &'r DagTIR) -> Result<T, E>,
    ) -> Result<CanonicalFacts<T>, E> {
        let mut facts = dags
            .visiting_local_indices()
            .filter(|&index| index < self.canonical_locals)
            .map(|index| {
                let (position, dag) = dags.local_at_index(index);
                fact(position, dag).map(|value| (index, value))
            })
            .collect::<Result<Vec<_>, E>>()?;
        facts.sort_by_key(|(index, _)| *index);
        Ok(CanonicalFacts(
            facts.into_iter().map(|(_, value)| value).collect(),
        ))
    }

    /// One fact for each materialized instance, visiting them in visiting
    /// order (identity order).
    ///
    /// # Errors
    ///
    /// Returns the first error `fact` returns, in visiting order.
    pub fn map_instances<'r, T, E>(
        &'r self,
        dags: &'r DagRegistry,
        mut fact: impl FnMut(DagPosition, &'r DagTIR, &'r InstanceOrigin) -> Result<T, E>,
    ) -> Result<InstanceFacts<T>, E> {
        let mut facts = dags
            .visiting_local_indices()
            .filter_map(|index| index.checked_sub(self.canonical_locals))
            .map(|instance| {
                let (position, dag) = dags.local_at_index(self.canonical_locals + instance);
                fact(position, dag, &self.origins[instance]).map(|value| (instance, value))
            })
            .collect::<Result<Vec<_>, E>>()?;
        facts.sort_by_key(|(instance, _)| *instance);
        Ok(InstanceFacts(
            facts.into_iter().map(|(_, value)| value).collect(),
        ))
    }

    /// The table of every local body's fact: a canonical body's from
    /// `canonical`, an instance's from `instances`.
    ///
    /// Both tables must be mapped from this graph.
    #[must_use]
    pub fn join<T>(
        &self,
        canonical: CanonicalFacts<T>,
        instances: InstanceFacts<T>,
    ) -> LocalDagFacts<T> {
        debug_assert_eq!(canonical.0.len(), self.canonical_locals);
        debug_assert_eq!(instances.0.len(), self.origins.len());
        LocalDagFacts::from_aligned(canonical.0.into_iter().chain(instances.0).collect())
    }
}

/// One fact for each canonical local body of one program, by
/// [`CanonicalIndex`].
#[derive(Debug, Clone)]
pub struct CanonicalFacts<T>(Vec<T>);

/// One fact for each materialized instance of one program.
#[derive(Debug, Clone)]
pub struct InstanceFacts<T>(Vec<T>);

/// What a canonical fact table holds for a template: a local template's
/// fact, or the imported template itself.
pub enum TemplateFact<'a, T> {
    Local(&'a T),
    Shared(&'a CheckedDag),
}

impl<T> CanonicalFacts<T> {
    /// The fact of the canonical body at `index`.
    ///
    /// # Panics
    ///
    /// Panics when `index` was issued by another program's graph.
    #[must_use]
    pub fn get(&self, index: CanonicalIndex) -> &T {
        &self.0[index.0]
    }

    /// What this table holds for `template`.
    #[must_use]
    pub fn template<'a>(&'a self, template: &'a TemplateBody) -> TemplateFact<'a, T> {
        match template {
            TemplateBody::Local(index) => TemplateFact::Local(self.get(*index)),
            TemplateBody::Shared(shared) => TemplateFact::Shared(shared),
        }
    }

    /// Replace each fact, or stop at the first error, in canonical-index
    /// order.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns.
    pub fn try_map<U, E>(self, map: impl FnMut(T) -> Result<U, E>) -> Result<CanonicalFacts<U>, E> {
        Ok(CanonicalFacts(
            self.0.into_iter().map(map).collect::<Result<_, E>>()?,
        ))
    }

    /// Derive one fact from each fact, or stop at the first error, in
    /// canonical-index order.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns.
    pub fn try_map_ref<'a, U, E>(
        &'a self,
        map: impl FnMut(&'a T) -> Result<U, E>,
    ) -> Result<CanonicalFacts<U>, E> {
        Ok(CanonicalFacts(
            self.0.iter().map(map).collect::<Result<_, E>>()?,
        ))
    }

    /// Derive one fact from each fact.
    #[must_use]
    pub fn map_ref<'a, U>(&'a self, map: impl FnMut(&'a T) -> U) -> CanonicalFacts<U> {
        CanonicalFacts(self.0.iter().map(map).collect())
    }

    /// Every fact, in canonical-index order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T> InstanceFacts<T> {
    /// Derive one fact from each fact, or stop at the first error, in
    /// instance order.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns.
    pub fn try_map_ref<'a, U, E>(
        &'a self,
        map: impl FnMut(&'a T) -> Result<U, E>,
    ) -> Result<InstanceFacts<U>, E> {
        Ok(InstanceFacts(
            self.0.iter().map(map).collect::<Result<_, E>>()?,
        ))
    }

    /// Derive one fact from each fact.
    #[must_use]
    pub fn map_ref<'a, U>(&'a self, map: impl FnMut(&'a T) -> U) -> InstanceFacts<U> {
        InstanceFacts(self.0.iter().map(map).collect())
    }

    /// Pair each fact with the other table's fact of the same instance.
    #[must_use]
    pub fn zip_ref<'a, U>(&'a self, other: &'a InstanceFacts<U>) -> InstanceFacts<(&'a T, &'a U)> {
        debug_assert_eq!(self.0.len(), other.0.len());
        InstanceFacts(self.0.iter().zip(&other.0).collect())
    }
}
