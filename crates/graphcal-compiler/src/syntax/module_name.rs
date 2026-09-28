//! Module aliases and module-scoped declaration names.

use std::sync::Arc;

use crate::dag_id::{DagId, DagSegment, IncludeInstanceId};
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::{NameAtom, NameAtomError, NameDef, NameNamespace, NamePath};

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

/// Error returned when parsing a canonical [`ScopedName`] rendering.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopedNameParseError {
    /// A dot-delimited path component was not a valid name atom.
    #[error("invalid scoped-name segment {position} (`{segment}`): {source}")]
    InvalidSegment {
        /// One-based position of the invalid segment.
        position: usize,
        /// Segment spelling at the display boundary.
        segment: String,
        /// Violated name-atom invariant.
        #[source]
        source: NameAtomError,
    },
}

/// A declaration name that may optionally be qualified by a module path.
///
/// Qualifier segments and the declaration member are separate validated name
/// types. Consequently, dots can only be path separators: two unequal
/// `ScopedName` values cannot share a canonical [`std::fmt::Display`]
/// rendering.
///
/// The [`std::fmt::Display`] impl renders `qualifier: ["helpers", "math"],
/// member: "G0"` as `helpers.math::G0`. That serialized form is for boundary
/// use only (diagnostics, debug output, third-party APIs); the compiler core
/// should use [`Self::qualifier`] and [`Self::member`] instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopedName {
    /// Module/path segments that qualify `member`. Empty for a local name.
    qualifier: Arc<[ScopeSegment]>,
    /// The declaration/member name inside the qualifier scope.
    member: Arc<DeclName>,
}

impl ScopedName {
    /// Create a local name from an already-validated declaration name.
    #[must_use]
    pub fn local(member: DeclName) -> Self {
        Self {
            qualifier: Arc::from([] as [ScopeSegment; 0]),
            member: Arc::new(member),
        }
    }

    /// Create a name qualified by one already-validated module segment.
    #[must_use]
    pub fn qualified(module: impl Into<ScopeSegment>, member: DeclName) -> Self {
        Self::qualified_path([module], member)
    }

    /// Create a name qualified by an arbitrary-depth validated module path.
    #[must_use]
    pub fn qualified_path(
        qualifier: impl IntoIterator<Item = impl Into<ScopeSegment>>,
        member: DeclName,
    ) -> Self {
        Self {
            qualifier: qualifier.into_iter().map(Into::into).collect(),
            member: Arc::new(member),
        }
    }

    /// Parse canonical source-like display text at a serialization boundary.
    /// Local names have no separator; qualified names use dotted DAG owners
    /// followed by exactly one `::` member boundary.
    ///
    /// # Errors
    ///
    /// Returns [`ScopedNameParseError`] when any path component is empty. A
    /// literal dot cannot occur inside a component because dots delimit the
    /// serialized path.
    pub fn parse(display: impl AsRef<str>) -> Result<Self, ScopedNameParseError> {
        fn parse_segment(segment: &str, position: usize) -> Result<NameAtom, ScopedNameParseError> {
            NameAtom::parse(segment).map_err(|source| ScopedNameParseError::InvalidSegment {
                position,
                segment: segment.to_string(),
                source,
            })
        }

        let display = display.as_ref();
        let Some((owner, member)) = display.split_once("::") else {
            return parse_segment(display, 1).map(|atom| Self::local(DeclName::classify(atom)));
        };
        if member.contains("::") {
            return Err(ScopedNameParseError::InvalidSegment {
                position: 2,
                segment: member.to_string(),
                source: NameAtomError::ContainsDot,
            });
        }
        let owner = owner
            .split('.')
            .enumerate()
            .map(|(index, segment)| parse_segment(segment, index + 1))
            .collect::<Result<Vec<_>, _>>()?;
        let owner = crate::syntax::non_empty::NonEmpty::try_from_vec(owner).map_err(|_| {
            ScopedNameParseError::InvalidSegment {
                position: 1,
                segment: String::new(),
                source: NameAtomError::Empty,
            }
        })?;
        let member = parse_segment(member, owner.len() + 1)?;
        Ok(Self::qualified_path(
            owner.into_iter().map(ModuleAliasName::classify),
            DeclName::classify(member),
        ))
    }

    /// Returns the member (leaf declaration) part of the name.
    ///
    /// For `x` this returns the `DeclName` `x`; for `helpers.math.x` this also
    /// returns `x`.
    #[must_use]
    pub fn member(&self) -> &DeclName {
        &self.member
    }

    /// Returns the qualifier path segments. Empty means this name is local.
    #[must_use]
    pub fn qualifier(&self) -> &[ScopeSegment] {
        &self.qualifier
    }

    /// Convert this semantic name to a validated syntactic path.
    ///
    /// Returns `None` when a qualifier segment is an anonymous include
    /// instance: such a namespace has no source spelling, so no source path
    /// (and no module-resolver lookup) can denote it.
    #[must_use]
    pub fn to_name_path(&self) -> Option<NamePath> {
        let qualifier = self
            .qualifier
            .iter()
            .map(|segment| segment.alias().map(|alias| alias.atom().clone()))
            .collect::<Option<Vec<_>>>()?;
        Some(NamePath::from_parts(
            crate::syntax::non_empty::NonEmpty::try_from_vec(qualifier).ok(),
            self.member.atom().clone(),
        ))
    }

    /// Returns whether this is a qualified name.
    #[must_use]
    pub fn is_qualified(&self) -> bool {
        !self.qualifier.is_empty()
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.qualifier.is_empty() {
            return std::fmt::Display::fmt(&self.member, f);
        }
        for (index, segment) in self.qualifier.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            write!(f, "{segment}")?;
        }
        write!(f, "::{}", self.member)
    }
}

impl std::str::FromStr for ScopedName {
    type Err = ScopedNameParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for ScopedName {
    type Error = ScopedNameParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for ScopedName {
    type Error = ScopedNameParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<DeclName> for ScopedName {
    /// Wrap a `DeclName` as a local `ScopedName`. Use this at the resolver → IR
    /// boundary where local resolver keys become module-aware IR keys.
    fn from(name: DeclName) -> Self {
        Self::local(name)
    }
}

impl From<&DeclName> for ScopedName {
    fn from(name: &DeclName) -> Self {
        Self::local(name.clone())
    }
}

impl From<NamePath> for ScopedName {
    fn from(path: NamePath) -> Self {
        Self::from(&path)
    }
}

impl From<&NamePath> for ScopedName {
    fn from(path: &NamePath) -> Self {
        Self::qualified_path(
            path.qualifier()
                .iter()
                .cloned()
                .map(ModuleAliasName::classify),
            DeclName::classify(path.leaf().clone()),
        )
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
        let name = ScopedName::qualified(module("module"), member("x"));
        assert_eq!(name.to_string(), "module::x");
        assert_eq!(name.member().as_str(), "x");
        assert_eq!(
            name.qualifier()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["module"]
        );
    }

    #[test]
    fn scoped_name_parses_nested_boundary_text() {
        let name = ScopedName::parse("helpers.math::G0").unwrap();
        assert_eq!(name.to_string(), "helpers.math::G0");
        assert_eq!(name.member().as_str(), "G0");
        assert_eq!(
            name.qualifier()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["helpers", "math"]
        );
    }

    #[test]
    fn scoped_name_supports_nested_typed_qualifier_path() {
        let name = ScopedName::qualified_path([module("helpers"), module("math")], member("G0"));
        assert_eq!(name.to_string(), "helpers.math::G0");
        assert_eq!(name.member().as_str(), "G0");
    }

    #[test]
    fn scoped_name_rejects_empty_display_segments() {
        for invalid in ["", "::x", "x::", "helpers..math::x", "helpers::math::x"] {
            assert!(
                ScopedName::parse(invalid).is_err(),
                "`{invalid}` should be rejected"
            );
        }
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

        let name = ScopedName::qualified(first, member("x"));
        assert_eq!(name.to_string(), "<include@10>::x");
        assert_eq!(name.to_name_path(), None);
        assert_ne!(name, ScopedName::qualified(spelled, member("x")));
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
            ScopedName::qualified(module("helpers"), member("x")),
            ScopedName::qualified_path([module("helpers"), module("math")], member("x")),
            ScopedName::qualified(module("math"), member("x")),
        ];
        let renderings = names
            .iter()
            .map(ToString::to_string)
            .collect::<HashSet<_>>();
        assert_eq!(renderings.len(), names.len());
    }

    #[test]
    fn valid_scoped_names_round_trip_through_canonical_display() {
        let names = [
            ScopedName::local(member("x")),
            ScopedName::qualified(module("helpers"), member("G0")),
            ScopedName::qualified_path([module("helpers"), module("math")], member("G0")),
        ];

        for name in names {
            assert_eq!(name.to_string().parse::<ScopedName>().unwrap(), name);
        }
    }
}
