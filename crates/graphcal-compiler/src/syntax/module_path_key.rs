//! Span-free identity of a module path.
//!
//! A [`ModulePathKey`] is the spelling of a module path without source spans:
//! one or more name atoms in source order. Two equal logical paths always
//! produce equal keys, without depending on a joined string format, so the key
//! is safe to use as a map key wherever module paths are compared (loader
//! resolution tables and the resolver's ambiguous-path check). A parsed path's
//! key comes from `ModulePath::key`.

use std::fmt;

use crate::syntax::names::NameAtom;
use crate::syntax::non_empty::NonEmpty;

/// Span-free spelling of a module path: one or more name atoms.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModulePathKey(NonEmpty<NameAtom>);

impl ModulePathKey {
    /// Key from already separated segments.
    #[must_use]
    pub const fn new(segments: NonEmpty<NameAtom>) -> Self {
        Self(segments)
    }

    /// The segments, in source order.
    #[must_use]
    pub const fn segments(&self) -> &NonEmpty<NameAtom> {
        &self.0
    }
}

impl fmt::Display for ModulePathKey {
    /// The source spelling: segments joined by `.`, as written in a module path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, rest) = self.0.split_first();
        f.write_str(first.as_str())?;
        for segment in rest {
            write!(f, ".{segment}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(name: &str) -> NameAtom {
        NameAtom::parse(name).unwrap()
    }

    #[test]
    fn keys_compare_by_segments() {
        let key = ModulePathKey::new(NonEmpty::new(atom("pkg"), vec![atom("lib")]));
        assert_eq!(
            key,
            ModulePathKey::new(NonEmpty::new(atom("pkg"), vec![atom("lib")]))
        );
        assert_ne!(key, ModulePathKey::new(NonEmpty::singleton(atom("pkg"))));
        assert_ne!(
            key,
            ModulePathKey::new(NonEmpty::new(atom("lib"), vec![atom("pkg")]))
        );
    }

    #[test]
    fn display_joins_segments_like_the_source_path() {
        assert_eq!(
            ModulePathKey::new(NonEmpty::new(atom("a"), vec![atom("b"), atom("c")])).to_string(),
            "a.b.c"
        );
        assert_eq!(
            ModulePathKey::new(NonEmpty::singleton(atom("a"))).to_string(),
            "a"
        );
    }
}
