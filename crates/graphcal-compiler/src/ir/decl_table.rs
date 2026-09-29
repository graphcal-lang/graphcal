//! The declaration table of one DAG body, keyed by canonical identity.
//!
//! [`ResolvedDeclName`] is the only semantic key. Source order is a list of
//! identities, and the source spelling is an index from the written local name
//! to its identity, built once when the table is constructed. Construction
//! rejects duplicate identities, duplicate spellings, and declarations owned by
//! another DAG, so consumers never re-join declarations by name.

use std::collections::HashMap;

use thiserror::Error;

use crate::dag_id::DagId;
use crate::ir::entry::{
    AssertEntry, BodyPhase, ConstEntry, Decl, FigureEntry, LayerEntry, NodeEntry, ParamEntry,
    PlotEntry,
};
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;

/// A declaration table could not be built from its entries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeclTableError {
    #[error("declaration `{name}` is declared more than once")]
    Duplicate { name: DeclName },
    #[error("declaration `{name}` is owned by `{declaration_owner}`, not by DAG `{owner}`")]
    ForeignOwner {
        name: DeclName,
        declaration_owner: DagId,
        owner: DagId,
    },
}

/// The value, assertion, and visualization declarations of one DAG body.
#[derive(Debug, Clone)]
pub struct DeclTable<P: BodyPhase> {
    order: Vec<ResolvedDeclName>,
    decls: HashMap<ResolvedDeclName, Decl<P>>,
    spelling: HashMap<DeclName, ResolvedDeclName>,
}

/// The empty table of a DAG without value, assertion, or visualization
/// declarations.
impl<P: BodyPhase> Default for DeclTable<P> {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            decls: HashMap::new(),
            spelling: HashMap::new(),
        }
    }
}

impl<P: BodyPhase> DeclTable<P> {
    /// Build the table for DAG `owner` from declarations in source order.
    ///
    /// # Errors
    ///
    /// Returns a [`DeclTableError`] when two declarations share an identity,
    /// or when a declaration belongs to a different DAG.
    pub fn new(
        owner: &DagId,
        decls: impl IntoIterator<Item = Decl<P>>,
    ) -> Result<Self, DeclTableError> {
        let mut table = Self {
            order: Vec::new(),
            decls: HashMap::new(),
            spelling: HashMap::new(),
        };
        for decl in decls {
            if decl.declaration_owner() != owner {
                return Err(DeclTableError::ForeignOwner {
                    name: decl.name().clone(),
                    declaration_owner: decl.declaration_owner().clone(),
                    owner: owner.clone(),
                });
            }
            let identity = decl.identity();
            let name = decl.name().clone();
            if table.decls.insert(identity.clone(), decl).is_some() {
                return Err(DeclTableError::Duplicate { name });
            }
            // Equal spellings share an identity, so the check above also
            // makes every spelling unique.
            table.spelling.insert(name, identity.clone());
            table.order.push(identity);
        }
        Ok(table)
    }

    /// Transform every declaration body, preserving identities, order, and
    /// spelling.
    ///
    /// Declarations are transformed in ascending `rank`, in source order
    /// within one rank, so a caller can keep a phase's established diagnostic
    /// order independent of the table's source order.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `transform`.
    pub fn try_map<Q: BodyPhase, E, K: Ord>(
        self,
        rank: impl Fn(&Decl<P>) -> K,
        mut transform: impl FnMut(Decl<P>) -> Result<Decl<Q>, E>,
    ) -> Result<DeclTable<Q>, E> {
        let Self {
            order,
            mut decls,
            spelling,
        } = self;
        let mut ranked = order
            .iter()
            .filter_map(|identity| decls.remove_entry(identity))
            .collect::<Vec<_>>();
        ranked.sort_by_key(|(_, decl)| rank(decl));
        let decls = ranked
            .into_iter()
            .map(|(identity, decl)| {
                let transformed = transform(decl)?;
                debug_assert_eq!(
                    transformed.identity(),
                    identity,
                    "a body transformation preserves the declaration identity"
                );
                Ok((identity, transformed))
            })
            .collect::<Result<HashMap<_, _>, E>>()?;
        Ok(DeclTable {
            order,
            decls,
            spelling,
        })
    }

    /// Update declaration bodies in place, in source order.
    ///
    /// `update` must preserve each declaration's name and owner; use
    /// [`Self::rebase`] to move declarations to another DAG.
    pub(crate) fn update(&mut self, mut update: impl FnMut(&mut Decl<P>)) {
        for identity in &self.order {
            if let Some(decl) = self.decls.get_mut(identity) {
                update(decl);
                debug_assert_eq!(
                    &decl.identity(),
                    identity,
                    "an in-place update preserves the declaration identity"
                );
            }
        }
    }

    /// Move every declaration to DAG `owner` under its unchanged name.
    ///
    /// `transform` sees each declaration in source order under its original
    /// identity, before the move.
    ///
    /// # Errors
    ///
    /// Returns a [`DeclTableError`] when the transformed declarations do not
    /// form a valid table.
    pub(crate) fn rebase(
        self,
        owner: &DagId,
        mut transform: impl FnMut(Decl<P>) -> Decl<P>,
    ) -> Result<Self, DeclTableError> {
        let (decls, _) = self.into_parts();
        Self::new(
            owner,
            decls.into_iter().map(|decl| {
                let mut decl = transform(decl);
                decl.set_declaration_owner(owner.clone());
                decl
            }),
        )
    }

    /// Canonical identities in source order.
    #[must_use]
    pub fn order(&self) -> &[ResolvedDeclName] {
        &self.order
    }

    /// Declarations in source order.
    pub fn iter(&self) -> impl Iterator<Item = &Decl<P>> {
        self.order
            .iter()
            .filter_map(|identity| self.decls.get(identity))
    }

    /// Consume the table into its declarations in source order and its
    /// spelling index.
    #[must_use]
    pub fn into_parts(self) -> (Vec<Decl<P>>, HashMap<DeclName, ResolvedDeclName>) {
        let Self {
            order,
            mut decls,
            spelling,
        } = self;
        let decls = order
            .into_iter()
            .filter_map(|identity| decls.remove(&identity))
            .collect();
        (decls, spelling)
    }

    /// The declaration with canonical identity `identity`.
    #[must_use]
    pub fn get(&self, identity: &ResolvedDeclName) -> Option<&Decl<P>> {
        self.decls.get(identity)
    }

    /// The identity written as `name` in this DAG body.
    #[must_use]
    pub fn lookup(&self, name: &DeclName) -> Option<&ResolvedDeclName> {
        self.spelling.get(name)
    }

    /// Every source spelling with its canonical identity.
    #[must_use]
    pub const fn spelling(&self) -> &HashMap<DeclName, ResolvedDeclName> {
        &self.spelling
    }

    /// Const declarations in source order.
    pub fn consts(&self) -> impl Iterator<Item = &ConstEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Const(entry) => Some(entry),
            _ => None,
        })
    }

    /// Param declarations in source order.
    pub fn params(&self) -> impl Iterator<Item = &ParamEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Param(entry) => Some(entry),
            _ => None,
        })
    }

    /// Node declarations in source order.
    pub fn nodes(&self) -> impl Iterator<Item = &NodeEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Node(entry) => Some(entry),
            _ => None,
        })
    }

    /// Assert declarations in source order.
    pub fn asserts(&self) -> impl Iterator<Item = &AssertEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Assert(entry) => Some(entry),
            _ => None,
        })
    }

    /// Plot declarations in source order.
    pub fn plots(&self) -> impl Iterator<Item = &PlotEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Plot(entry) => Some(entry),
            _ => None,
        })
    }

    /// Figure declarations in source order.
    pub fn figures(&self) -> impl Iterator<Item = &FigureEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Figure(entry) => Some(entry),
            _ => None,
        })
    }

    /// Layer declarations in source order.
    pub fn layers(&self) -> impl Iterator<Item = &LayerEntry<P>> {
        self.iter().filter_map(|decl| match decl {
            Decl::Layer(entry) => Some(entry),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests;
