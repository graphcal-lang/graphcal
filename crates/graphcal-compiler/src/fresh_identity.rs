//! Allocation-backed identities that are equal only to their own clones.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Debug)]
struct Marker;

/// An identity minted by [`FreshIdentity::fresh`].
///
/// Two identities are equal exactly when one is a clone of the other. Each
/// clone keeps the shared allocation alive, so a live identity's address can
/// never be reused by a later [`FreshIdentity::fresh`].
#[derive(Debug, Clone)]
pub struct FreshIdentity(Arc<Marker>);

impl FreshIdentity {
    /// Mint an identity distinct from every other live identity.
    pub fn fresh() -> Self {
        Self(Arc::new(Marker))
    }
}

impl PartialEq for FreshIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for FreshIdentity {}

impl Hash for FreshIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn identities_equal_only_their_clones() {
        let first = FreshIdentity::fresh();
        let second = FreshIdentity::fresh();
        assert_eq!(first, first.clone());
        assert_ne!(first, second);
        assert_eq!(
            HashSet::from([first.clone(), first, second.clone(), second]).len(),
            2
        );
    }
}
