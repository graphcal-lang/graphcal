//! Evaluation-time lexical environment keyed by HIR [`LocalId`]s.

use super::model::LocalId;

/// A layered lexical environment for HIR locals.
///
/// Each binder (for-comp, scan, unfold, match arm) layers a child frame
/// holding its few bindings over the enclosing environment instead of cloning
/// the full local map; lookup walks the parent chain. [`LocalId`]s are unique
/// within one lowered body, so frames never shadow one another — the chain is
/// purely an ownership layering, and nested binders cost O(own bindings)
/// instead of O(visible locals).
#[derive(Debug)]
pub struct LocalEnv<'a, V> {
    parent: Option<&'a Self>,
    bindings: Vec<(LocalId, V)>,
}

impl<'a, V> LocalEnv<'a, V> {
    /// Create an empty root environment.
    #[must_use]
    pub const fn root() -> Self {
        Self {
            parent: None,
            bindings: Vec::new(),
        }
    }

    /// Create a root environment holding the given bindings.
    #[must_use]
    pub const fn from_bindings(bindings: Vec<(LocalId, V)>) -> Self {
        Self {
            parent: None,
            bindings,
        }
    }

    /// Layer a child frame holding `bindings` over this environment.
    #[must_use]
    pub const fn child<'b>(&'b self, bindings: Vec<(LocalId, V)>) -> LocalEnv<'b, V>
    where
        'a: 'b,
    {
        LocalEnv {
            parent: Some(self),
            bindings,
        }
    }

    /// Look up a local by its lexical identity, innermost frame first.
    #[must_use]
    pub fn get(&self, id: LocalId) -> Option<&V> {
        self.bindings
            .iter()
            .rev()
            .find(|(bound, _)| *bound == id)
            .map(|(_, value)| value)
            .or_else(|| self.parent.and_then(|parent| parent.get(id)))
    }

    /// Bind or update a local in this frame.
    ///
    /// Iterating binders (for-comp elements, scan/unfold steps) rebind the
    /// same `LocalId` once per iteration without growing the frame.
    pub fn bind(&mut self, id: LocalId, value: V) {
        match self.bindings.iter_mut().find(|(bound, _)| *bound == id) {
            Some((_, slot)) => *slot = value,
            None => self.bindings.push((id, value)),
        }
    }
}

impl<V> Default for LocalEnv<'_, V> {
    fn default() -> Self {
        Self::root()
    }
}
