//! Module aliases and module-scoped declaration names.

use std::sync::Arc;

use crate::syntax::decl_name::DeclName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace};

/// Module alias namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleAliasNameNamespace {}

impl NameNamespace for ModuleAliasNameNamespace {
    const DISPLAY_NAME: &'static str = "ModuleAliasName";
}

/// Name of a module alias introduced by an import/include declaration (e.g.,
/// `"constants"`, `"std"`).
pub type ModuleAliasName = NameDef<ModuleAliasNameNamespace>;

/// A declaration name that may optionally be qualified by a module path.
///
/// The qualifier is stored as structured path segments, not as a flat
/// dot-separated string. This allows arbitrary-depth qualification such as
/// `helpers.math.G0` while keeping the declaration member (`G0`) directly
/// accessible and distinct from the qualifier.
///
/// The [`std::fmt::Display`] impl renders `qualifier: ["helpers", "math"],
/// member: "G0"` as `helpers.math.G0`. That serialized form is for boundary
/// use only (diagnostics, debug output, third-party APIs); the compiler core
/// should use the typed accessors instead of splitting strings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopedName {
    /// Module/path segments that qualify `member`. Empty for a local name.
    qualifier: Arc<[Arc<str>]>,
    /// The declaration/member name inside the qualifier scope.
    member: Arc<str>,
}

impl ScopedName {
    /// Create a scoped name from display-boundary text.
    ///
    /// Dotted input is treated as a qualified path so `ScopedName::local("a.b")`
    /// cannot display indistinguishably from `ScopedName::qualified("a", "b")`.
    #[must_use]
    pub fn local(member: impl Into<Arc<str>>) -> Self {
        let member = member.into();
        let parts = member.split('.').collect::<Vec<_>>();
        if parts.len() > 1 && parts.iter().all(|part| !part.is_empty()) {
            let qualifier = parts[..parts.len() - 1]
                .iter()
                .map(|part| Arc::<str>::from(*part))
                .collect();
            let leaf = Arc::<str>::from(parts[parts.len() - 1]);
            Self {
                qualifier,
                member: leaf,
            }
        } else {
            Self {
                qualifier: Arc::from([] as [Arc<str>; 0]),
                member,
            }
        }
    }

    /// Create a name qualified by a single module segment.
    #[must_use]
    pub fn qualified(module: impl Into<Arc<str>>, member: impl Into<Arc<str>>) -> Self {
        Self::qualified_path([module], member)
    }

    /// Create a name qualified by an arbitrary-depth module path.
    #[must_use]
    pub fn qualified_path(
        qualifier: impl IntoIterator<Item = impl Into<Arc<str>>>,
        member: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            qualifier: qualifier.into_iter().map(Into::into).collect(),
            member: member.into(),
        }
    }

    /// Returns the member (leaf declaration) part of the name.
    ///
    /// For `x` this returns `"x"`; for `helpers.math.x` this also returns
    /// `"x"`.
    #[must_use]
    pub fn member(&self) -> &str {
        &self.member
    }

    /// Returns the qualifier path segments. Empty means this name is local.
    #[must_use]
    pub fn qualifier(&self) -> &[Arc<str>] {
        &self.qualifier
    }

    /// Returns whether this is a qualified name.
    #[must_use]
    pub fn is_qualified(&self) -> bool {
        !self.qualifier.is_empty()
    }

    /// Qualify a name with a single-segment prefix, replacing any existing
    /// qualifier while preserving the member.
    ///
    /// `x.with_prefix("p")` → `p.x`.
    /// `m.x.with_prefix("p")` → `p.x`.
    #[must_use]
    pub(crate) fn with_prefix(&self, prefix: &str) -> Self {
        Self::qualified(prefix, Arc::clone(&self.member))
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for segment in self.qualifier.iter() {
            f.write_str(segment)?;
            f.write_str(".")?;
        }
        f.write_str(&self.member)
    }
}

impl From<NameAtom> for ScopedName {
    /// Wrap a bare atom as a local `ScopedName`. This is what
    /// `crate::syntax::ast::Ident::into_spanned` uses to lift parser
    /// identifiers into the typed name; qualified forms are constructed
    /// explicitly via [`ScopedName::qualified`] or [`ScopedName::qualified_path`].
    fn from(atom: NameAtom) -> Self {
        Self::local(atom.into_inner())
    }
}

impl From<String> for ScopedName {
    /// Wrap display-boundary text as a `ScopedName`.
    fn from(s: String) -> Self {
        Self::local(s)
    }
}

impl From<DeclName> for ScopedName {
    /// Wrap a `DeclName` as a local `ScopedName`. Use this at the resolver →
    /// IR boundary where resolver keys (local `DeclName`s) become IR keys
    /// (`ScopedName`s).
    fn from(name: DeclName) -> Self {
        Self::local(name.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_name_qualified_display_uses_dot() {
        let name = ScopedName::qualified("module", "x");
        assert_eq!(format!("{name}"), "module.x");
        assert_eq!(name.member(), "x");
        assert_eq!(
            name.qualifier().iter().map(|s| &**s).collect::<Vec<_>>(),
            ["module"]
        );
    }

    #[test]
    fn scoped_name_local_splits_dotted_boundary_text() {
        let name = ScopedName::local("helpers.math.G0");
        assert_eq!(format!("{name}"), "helpers.math.G0");
        assert_eq!(name.member(), "G0");
        assert_eq!(
            name.qualifier().iter().map(|s| &**s).collect::<Vec<_>>(),
            ["helpers", "math"]
        );
    }

    #[test]
    fn scoped_name_supports_nested_qualifier_path() {
        let name = ScopedName::qualified_path(["helpers", "math"], "G0");
        assert_eq!(format!("{name}"), "helpers.math.G0");
        assert_eq!(name.member(), "G0");
        assert_eq!(
            name.qualifier().iter().map(|s| &**s).collect::<Vec<_>>(),
            ["helpers", "math"]
        );
    }
}
