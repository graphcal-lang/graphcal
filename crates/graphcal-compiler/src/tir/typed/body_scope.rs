//! The scope of the DAG that runs a body, and parts of a body bound to it.
//!
//! A [`Scoped`] part is created only inside [`crate::tir::typed`]; see
//! [`super::evaluation_unit`] for how a scope is selected.

use crate::hir::expr::{LocalDecl, LocalUnit};
use crate::resolved_name::{ResolvedDeclName, ResolvedUnitName};

use super::checked_dag::CheckedDag;
use super::dag_position::DagPosition;

/// The scope a body runs in: the frame of the checked DAG that owns it.
///
/// Created only by [`super::evaluation_unit`], from the owner of a typed identity (or, for
/// an external value, the root module), and handed out only inside a
/// [`Scoped`] part or a [`ScopedTree`] of that DAG.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BodyScope<'t> {
    dag: &'t CheckedDag,
    position: DagPosition,
}

impl<'t> BodyScope<'t> {
    /// The scope of `dag`, at `position` in its program's registry, for the
    /// compiler's own selections.
    pub(crate) const fn of(position: DagPosition, dag: &'t CheckedDag) -> Self {
        Self { dag, position }
    }

    /// The position of this scope's DAG in its program's registry.
    pub(crate) const fn position(self) -> DagPosition {
        self.position
    }

    /// The DAG whose frame this is, for the compiler's own specialization.
    pub(crate) const fn dag(self) -> &'t CheckedDag {
        self.dag
    }

    /// Identity of the DAG that runs bodies in this scope.
    #[must_use]
    pub(crate) const fn dag_id(self) -> &'t crate::dag_id::DagId {
        self.dag.dag_id()
    }

    /// The declaration `handle` denotes when this scope's DAG runs the body
    /// holding it.
    #[must_use]
    pub(crate) fn resolve(self, handle: &LocalDecl) -> ResolvedDeclName {
        self.dag.body().frame().resolve(handle)
    }

    /// The unit whose scale `unit` has when this scope's DAG runs the body
    /// holding it.
    #[must_use]
    pub(crate) fn resolve_unit(self, unit: &LocalUnit) -> ResolvedUnitName {
        self.dag.body().frame().resolve_unit(unit)
    }
}

/// A part of one evaluation unit's source, together with the scope the unit
/// runs in.
///
/// Parts are narrowed only by projections whose result is borrowed from the
/// part itself, so every part of a unit keeps the scope of the DAG that owns
/// the unit.
#[derive(Debug)]
pub struct Scoped<'t, T: ?Sized> {
    scope: BodyScope<'t>,
    part: &'t T,
}

impl<T: ?Sized> Clone for Scoped<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Scoped<'_, T> {}

impl<'t, T: ?Sized> Scoped<'t, T> {
    pub(super) const fn new(scope: BodyScope<'t>, part: &'t T) -> Self {
        Self { scope, part }
    }

    /// The part itself, for reading its source structure.
    #[must_use]
    pub const fn get(self) -> &'t T {
        self.part
    }

    /// The scope the part runs in, for the compiler's own specialization.
    /// Evaluation reaches it only through a [`ScopedTree`] of the part.
    #[must_use]
    pub(crate) const fn scope(self) -> BodyScope<'t> {
        self.scope
    }

    /// Identity of the DAG that runs this part.
    #[must_use]
    pub const fn dag_id(self) -> &'t crate::dag_id::DagId {
        self.scope.dag_id()
    }

    /// A sub-part of this part, in the same scope.
    ///
    /// `project` must return a reference derived from its argument, so a
    /// part of another unit cannot be attached to this unit's scope.
    #[must_use]
    pub fn map<U: ?Sized>(self, project: impl for<'p> FnOnce(&'p T) -> &'p U) -> Scoped<'t, U> {
        Scoped::new(self.scope, project(self.part))
    }

    /// A sub-part of this part that may be absent, in the same scope.
    #[must_use]
    pub fn filter_map<U: ?Sized>(
        self,
        project: impl for<'p> FnOnce(&'p T) -> Option<&'p U>,
    ) -> Option<Scoped<'t, U>> {
        project(self.part).map(|part| Scoped::new(self.scope, part))
    }
}

impl<'t, T> Scoped<'t, [T]> {
    /// Every element of this part, each in the same scope.
    #[must_use]
    pub fn iter(self) -> impl ExactSizeIterator<Item = Scoped<'t, T>> + use<'t, T> {
        let scope = self.scope;
        self.part.iter().map(move |part| Scoped::new(scope, part))
    }

    /// The number of elements of this part.
    #[must_use]
    pub const fn len(self) -> usize {
        self.part.len()
    }

    /// Whether this part has no elements.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.part.is_empty()
    }

    /// The element at `index`, in the same scope.
    #[must_use]
    pub fn nth(self, index: usize) -> Option<Scoped<'t, T>> {
        self.part
            .get(index)
            .map(|part| Scoped::new(self.scope, part))
    }
}
