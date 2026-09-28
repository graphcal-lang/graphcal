//! Module aliases and module-scoped declaration names.

use crate::dag_id::{DagId, DagSegment, IncludeInstanceId};
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::{NameAtomError, NameDef, NameNamespace, NamePath, Qualified};
use crate::syntax::non_empty::NonEmpty;

/// Module alias namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleAliasNameNamespace {}

impl NameNamespace for ModuleAliasNameNamespace {
    const DISPLAY_NAME: &'static str = "ModuleAliasName";
}

/// Name of a module alias introduced by an import/include declaration (e.g.,
/// `"constants"`, `"std"`).
pub type ModuleAliasName = NameDef<ModuleAliasNameNamespace>;

/// One qualifier segment of a [`ScopedName`], and the namespace assigned to
/// one included DAG instance.
///
/// Source-visible qualifiers (import aliases, module-form include aliases, and
/// inline DAG names) are [`Self::Named`]. A selective include introduces no
/// module alias, so its private namespace is the opaque
/// [`Self::IncludeInstance`] identity; it can never be spelled in source.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ScopeSegment {
    /// A source-visible module alias.
    Named(ModuleAliasName),
    /// The private namespace of an anonymous selective include instance.
    IncludeInstance(IncludeInstanceId),
}

impl ScopeSegment {
    /// The source-visible alias, or `None` for an anonymous include instance.
    #[must_use]
    pub const fn alias(&self) -> Option<&ModuleAliasName> {
        match self {
            Self::Named(alias) => Some(alias),
            Self::IncludeInstance(_) => None,
        }
    }

    /// The concrete include instance this namespace denotes under `owner`.
    #[must_use]
    pub fn instance_of(&self, owner: &DagId) -> DagId {
        match self {
            Self::Named(alias) => owner.named_instance_child(alias.as_str()),
            Self::IncludeInstance(id) => owner.include_instance_child(*id),
        }
    }

    /// Qualifier segment that names a [`DagSegment`] below some owner.
    ///
    /// # Errors
    ///
    /// Returns [`NameAtomError`] when a spelled segment is not a valid name
    /// atom (only file-path components can be).
    pub fn try_from_dag_segment(segment: &DagSegment) -> Result<Self, NameAtomError> {
        match segment {
            DagSegment::SourceModule(name) | DagSegment::NamedInstance(name) => {
                ModuleAliasName::try_new(name.as_ref()).map(Self::Named)
            }
            DagSegment::IncludeInstance(id) => Ok(Self::IncludeInstance(*id)),
        }
    }

    /// Qualifier segment for a DAG segment below a file root, whose spelled
    /// segments are inline DAG names or include aliases and therefore valid
    /// name atoms.
    #[must_use]
    pub fn from_nested_dag_segment(segment: &DagSegment) -> Self {
        match segment {
            DagSegment::SourceModule(name) | DagSegment::NamedInstance(name) => {
                Self::Named(ModuleAliasName::expect_valid(name.as_ref()))
            }
            DagSegment::IncludeInstance(id) => Self::IncludeInstance(*id),
        }
    }

    /// Rendered text, used only to keep [`ScopedName`]'s established ordering.
    fn rendered(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Named(alias) => std::borrow::Cow::Borrowed(alias.as_str()),
            Self::IncludeInstance(id) => std::borrow::Cow::Owned(id.to_string()),
        }
    }
}

impl From<ModuleAliasName> for ScopeSegment {
    fn from(alias: ModuleAliasName) -> Self {
        Self::Named(alias)
    }
}

impl std::fmt::Display for ScopeSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(alias) => std::fmt::Display::fmt(alias, f),
            Self::IncludeInstance(id) => std::fmt::Display::fmt(id, f),
        }
    }
}

impl std::fmt::Debug for ScopeSegment {
    /// Debug views show every qualifier as one quoted spelling.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(alias) => std::fmt::Debug::fmt(alias, f),
            Self::IncludeInstance(id) => std::fmt::Debug::fmt(&id.to_string(), f),
        }
    }
}

impl Ord for ScopeSegment {
    /// Order by rendered text, then named before anonymous.
    ///
    /// This is the order qualifiers had while anonymous include namespaces
    /// were rendered alias spellings, so sorted outputs stay unchanged.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rendered()
            .cmp(&other.rendered())
            .then_with(|| match (self, other) {
                (Self::Named(_), Self::IncludeInstance(_)) => std::cmp::Ordering::Less,
                (Self::IncludeInstance(_), Self::Named(_)) => std::cmp::Ordering::Greater,
                (Self::Named(left), Self::Named(right)) => left.cmp(right),
                (Self::IncludeInstance(left), Self::IncludeInstance(right)) => left.cmp(right),
            })
    }
}

impl PartialOrd for ScopeSegment {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A declaration name that may optionally be qualified by a module path.
///
/// This classifies both parts of a source [`Qualified`] path: owner segments
/// are module scopes (source aliases or anonymous include instances) and the
/// leaf is the declaration. Dots can only be path separators, so two unequal
/// `ScopedName` values cannot share a canonical [`std::fmt::Display`]
/// rendering (`helpers.math::G0`). That rendering is for boundary use only
/// (diagnostics, debug output, third-party APIs); the compiler core uses
/// [`Qualified::qualifier`] and [`Qualified::leaf`].
pub type ScopedName = Qualified<ScopeSegment, DeclName>;

impl ScopedName {
    /// Qualify a declaration by one module scope.
    #[must_use]
    pub fn in_scope(scope: impl Into<ScopeSegment>, member: DeclName) -> Self {
        Self::qualified(NonEmpty::singleton(scope.into()), member)
    }

    /// Classify a namespace-neutral path in a declaration-reference position:
    /// owner segments name module aliases and the leaf a declaration.
    #[must_use]
    pub fn classify_path(path: &NamePath) -> Self {
        path.map_ref(
            |segment| ScopeSegment::Named(ModuleAliasName::classify(segment.clone())),
            |leaf| DeclName::classify(leaf.clone()),
        )
    }

    /// Convert this semantic name to a validated syntactic path.
    ///
    /// Returns `None` when a qualifier segment is an anonymous include
    /// instance: such a namespace has no source spelling, so no source path
    /// (and no module-resolver lookup) can denote it.
    #[must_use]
    pub fn to_name_path(&self) -> Option<NamePath> {
        let owner = match self.owner() {
            None => None,
            Some(owner) => Some(
                owner
                    .try_map_ref(|segment| {
                        segment.alias().map(|alias| alias.atom().clone()).ok_or(())
                    })
                    .ok()?,
            ),
        };
        Some(NamePath::from_parts(owner, self.leaf().atom().clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn module(name: &str) -> ModuleAliasName {
        ModuleAliasName::try_new(name).unwrap()
    }

    fn member(name: &str) -> DeclName {
        DeclName::try_new(name).unwrap()
    }

    #[test]
    fn scoped_name_qualified_display_uses_member_boundary() {
        let name = ScopedName::in_scope(module("module"), member("x"));
        assert_eq!(name.to_string(), "module::x");
        assert_eq!(name.leaf().as_str(), "x");
        assert_eq!(
            name.qualifier()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["module"]
        );
    }

    #[test]
    fn scoped_name_supports_nested_typed_qualifier_path() {
        let name = ScopedName::qualified(
            NonEmpty::new(module("helpers").into(), vec![module("math").into()]),
            member("G0"),
        );
        assert_eq!(name.to_string(), "helpers.math::G0");
        assert_eq!(name.leaf().as_str(), "G0");
        let path = name.to_name_path().unwrap();
        assert_eq!(path.to_string(), "helpers.math::G0");
        assert_eq!(ScopedName::classify_path(&path), name);
    }

    #[test]
    fn scoped_name_typed_components_reject_empty_or_dotted_text() {
        assert_eq!(ModuleAliasName::try_new(""), Err(NameAtomError::Empty));
        assert_eq!(DeclName::try_new(""), Err(NameAtomError::Empty));
        assert_eq!(
            ModuleAliasName::try_new("helpers.math"),
            Err(NameAtomError::ContainsDot)
        );
        assert_eq!(
            DeclName::try_new("math.G0"),
            Err(NameAtomError::ContainsDot)
        );
    }

    #[test]
    fn anonymous_include_scopes_are_typed_qualifier_segments() {
        let first = ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(10));
        let second = ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(20));
        let spelled = ScopeSegment::Named(module("<include@10>"));

        assert_ne!(first, second);
        assert_eq!(first.alias(), None);
        assert_eq!(first.to_string(), "<include@10>");
        assert_eq!(format!("{first:?}"), "\"<include@10>\"");
        // A named alias with the same rendering is a different namespace.
        assert_ne!(first, spelled);
        assert_eq!(spelled.cmp(&first), std::cmp::Ordering::Less);

        let name = ScopedName::in_scope(first, member("x"));
        assert_eq!(name.to_string(), "<include@10>::x");
        assert_eq!(name.to_name_path(), None);
        assert_ne!(name, ScopedName::in_scope(spelled, member("x")));
    }

    #[test]
    fn scope_segments_map_to_instance_dag_ids() {
        let owner = DagId::root_in_package("test", "main");
        let id = IncludeInstanceId::at_source_offset(7);

        assert_eq!(
            ScopeSegment::Named(module("inst")).instance_of(&owner),
            owner.named_instance_child("inst")
        );
        assert_eq!(
            ScopeSegment::IncludeInstance(id).instance_of(&owner),
            owner.include_instance_child(id)
        );
        for segment in owner
            .named_instance_child("inst")
            .include_instance_child(id)
            .child("inner")
            .segments()
            .iter()
            .skip(1)
        {
            let scope = ScopeSegment::from_nested_dag_segment(segment);
            assert_eq!(scope.to_string(), segment.to_string());
            assert_eq!(ScopeSegment::try_from_dag_segment(segment), Ok(scope));
        }
        assert!(
            ScopeSegment::try_from_dag_segment(&DagSegment::SourceModule("a.b".into())).is_err()
        );
    }

    #[test]
    fn scope_segments_order_by_rendered_text() {
        let anonymous = ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(63));
        let later = ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(100));
        let named = ScopeSegment::Named(module("alpha"));

        assert!(later < anonymous);
        assert!(anonymous < named);
        assert!(ScopeSegment::Named(module("a")) < named);
    }

    #[test]
    fn distinct_scoped_names_have_distinct_canonical_renderings() {
        let names = [
            ScopedName::local(member("x")),
            ScopedName::in_scope(module("helpers"), member("x")),
            ScopedName::qualified(
                NonEmpty::new(module("helpers").into(), vec![module("math").into()]),
                member("x"),
            ),
            ScopedName::in_scope(module("math"), member("x")),
        ];
        let renderings = names
            .iter()
            .map(ToString::to_string)
            .collect::<HashSet<_>>();
        assert_eq!(renderings.len(), names.len());
    }
}
