//! One value per local DAG body of an unchecked registry, in the registry's
//! own shape, so the facts a check computes pair with their bodies
//! structurally instead of by keyed lookup, and the checking view over them.

use std::collections::BTreeMap;

use crate::dag_id::DagId;

use super::model::{DagTIR, TirCore};
use super::program::{DagRegistry, TirRead, UncheckedTir};

/// One value for each local body of one [`DagRegistry`]: the root's, then
/// every other local body's, keyed and ordered like the registry.
///
/// Only [`DagRegistry::map_local`] creates a table, and its other methods
/// keep its keys, so a table always has exactly one entry per local body of
/// the registry it was mapped from. The registry's local bodies are fixed
/// once checking starts, so pairing a table with its registry
/// ([`DagRegistry::with_local_facts`], [`DagRegistry::into_local_facts`])
/// needs no lookup.
#[derive(Debug, Clone)]
pub struct LocalDagFacts<T> {
    root_id: DagId,
    root: T,
    others: BTreeMap<DagId, T>,
}

impl DagRegistry {
    /// Compute one value for each local body, in local order: the root, then
    /// every other local body in identity order.
    ///
    /// # Errors
    ///
    /// Returns the first error `fact` returns, in local order.
    pub(crate) fn map_local<'r, T, E>(
        &'r self,
        mut fact: impl FnMut(&'r DagTIR) -> Result<T, E>,
    ) -> Result<LocalDagFacts<T>, E> {
        let (root, others) = self.local_parts();
        Ok(LocalDagFacts {
            root_id: root.dag_id().clone(),
            root: fact(root)?,
            others: others
                .iter()
                .map(|(dag_id, dag)| fact(dag).map(|value| (dag_id.clone(), value)))
                .collect::<Result<_, E>>()?,
        })
    }

    /// Every local body with its fact from `facts`, in local order.
    pub(crate) fn with_local_facts<'a, T>(
        &'a self,
        facts: &'a LocalDagFacts<T>,
    ) -> impl Iterator<Item = (&'a DagTIR, &'a T)> {
        let (root, others) = self.local_parts();
        debug_assert!(facts.mirrors(root.dag_id(), others.keys()));
        std::iter::once((root, &facts.root)).chain(others.values().zip(facts.others.values()))
    }

    /// Consume the local bodies, each paired with its fact from `facts`,
    /// with the imported handles.
    pub(crate) fn into_local_facts<T>(self, facts: LocalDagFacts<T>) -> PairedLocalDags<T> {
        let (root, others, shared) = self.into_parts();
        debug_assert!(facts.mirrors(root.dag_id(), others.keys()));
        PairedLocalDags {
            root: (root, facts.root),
            others: others
                .into_values()
                .zip(facts.others.into_values())
                .collect(),
            shared,
        }
    }
}

/// The local bodies of a consumed registry, each paired with its fact, and
/// its imported handles.
pub struct PairedLocalDags<T> {
    /// The root body and its fact.
    pub root: (DagTIR, T),
    /// Every other local body with its fact, in identity order.
    pub others: Vec<(DagTIR, T)>,
    /// The imported checked handles, in identity order.
    pub shared: BTreeMap<DagId, std::sync::Arc<super::checked_dag::CheckedDag>>,
}

impl<T> LocalDagFacts<T> {
    /// Whether this table has exactly the entries of a registry with these
    /// local identities.
    fn mirrors<'a>(&self, root_id: &DagId, others: impl Iterator<Item = &'a DagId>) -> bool {
        self.root_id == *root_id && others.eq(self.others.keys())
    }

    /// The fact of one local body.
    pub(crate) fn get(&self, dag_id: &DagId) -> Option<&T> {
        if *dag_id == self.root_id {
            Some(&self.root)
        } else {
            self.others.get(dag_id)
        }
    }

    /// Every local body's identity with its fact, in local order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&DagId, &T)> {
        std::iter::once((&self.root_id, &self.root)).chain(self.others.iter())
    }

    /// Replace each fact, keeping the keys.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns, in local order.
    pub(crate) fn try_map<U, E>(
        self,
        mut map: impl FnMut(&DagId, T) -> Result<U, E>,
    ) -> Result<LocalDagFacts<U>, E> {
        let root = map(&self.root_id, self.root)?;
        let others = self
            .others
            .into_iter()
            .map(|(dag_id, value)| map(&dag_id, value).map(|value| (dag_id, value)))
            .collect::<Result<_, E>>()?;
        Ok(LocalDagFacts {
            root_id: self.root_id,
            root,
            others,
        })
    }

    /// Derive one fact from each fact, keeping the keys.
    ///
    /// # Errors
    ///
    /// Returns the first error `map` returns, in local order.
    pub(crate) fn try_map_ref<U, E>(
        &self,
        mut map: impl FnMut(&DagId, &T) -> Result<U, E>,
    ) -> Result<LocalDagFacts<U>, E> {
        Ok(LocalDagFacts {
            root_id: self.root_id.clone(),
            root: map(&self.root_id, &self.root)?,
            others: self
                .others
                .iter()
                .map(|(dag_id, value)| map(dag_id, value).map(|value| (dag_id.clone(), value)))
                .collect::<Result<_, E>>()?,
        })
    }

    /// Replace each fact, keeping the keys.
    pub(crate) fn map<U>(self, mut map: impl FnMut(T) -> U) -> LocalDagFacts<U> {
        LocalDagFacts {
            root_id: self.root_id,
            root: map(self.root),
            others: self
                .others
                .into_iter()
                .map(|(dag_id, value)| (dag_id, map(value)))
                .collect(),
        }
    }

    /// Pair each fact with the other table's fact of the same body.
    pub(crate) fn zip<U>(self, other: LocalDagFacts<U>) -> LocalDagFacts<(T, U)> {
        debug_assert!(other.mirrors(&self.root_id, self.others.keys()));
        LocalDagFacts {
            root_id: self.root_id,
            root: (self.root, other.root),
            others: self
                .others
                .into_iter()
                .zip(other.others.into_values())
                .map(|((dag_id, left), right)| (dag_id, (left, right)))
                .collect(),
        }
    }
}

impl<T, U> LocalDagFacts<(T, U)> {
    /// Split a table of pairs into a table of each half.
    pub(crate) fn unzip(self) -> (LocalDagFacts<T>, LocalDagFacts<U>) {
        let (others_left, others_right) = self
            .others
            .into_iter()
            .map(|(dag_id, (left, right))| ((dag_id.clone(), left), (dag_id, right)))
            .unzip();
        (
            LocalDagFacts {
                root_id: self.root_id.clone(),
                root: self.root.0,
                others: others_left,
            },
            LocalDagFacts {
                root_id: self.root_id,
                root: self.root.1,
                others: others_right,
            },
        )
    }
}

/// A project TIR in the middle of its check: unchecked local bodies with the
/// checked trees published for them so far.
#[derive(Clone, Copy)]
pub struct CheckingTir<'a, B = crate::tir::texpr::CheckedBodies> {
    pub tir: &'a UncheckedTir,
    /// What checking has published for each local body so far.
    pub bodies: &'a LocalDagFacts<B>,
}

/// What checking has published for one local body so far: its checked trees
/// once they are published.
pub trait PublishedBodies {
    /// The checked trees of the body, once published.
    fn published(&self) -> Option<&crate::tir::texpr::CheckedBodies>;
}

impl PublishedBodies for crate::tir::texpr::CheckedBodies {
    fn published(&self) -> Option<&crate::tir::texpr::CheckedBodies> {
        Some(self)
    }
}

impl<B: PublishedBodies> TirRead for CheckingTir<'_, B> {
    fn core(&self) -> &TirCore {
        &self.tir.core
    }

    fn root(&self) -> &DagTIR {
        self.tir.root()
    }

    fn dag(&self, dag_id: &crate::dag_id::DagId) -> Option<&DagTIR> {
        self.tir.dag(dag_id)
    }

    fn dag_bodies(&self) -> Box<dyn Iterator<Item = &DagTIR> + '_> {
        self.tir.dag_bodies()
    }

    fn checked_bodies(
        &self,
        dag_id: &crate::dag_id::DagId,
    ) -> Option<&crate::tir::texpr::CheckedBodies> {
        self.bodies
            .get(dag_id)
            .and_then(PublishedBodies::published)
            .or_else(|| self.tir.checked_bodies(dag_id))
    }
}
