//! Module aliases and module-scoped declaration names.

use std::sync::Arc;

use crate::syntax::decl_name::DeclName;
use crate::syntax::names::{NameAtom, NameAtomError, NameDef, NameNamespace, NamePath};
use crate::syntax::span::Span;

/// Module alias namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleAliasNameNamespace {}

impl NameNamespace for ModuleAliasNameNamespace {
    const DISPLAY_NAME: &'static str = "ModuleAliasName";
}

/// Name of a module alias introduced by an import/include declaration (e.g.,
/// `"constants"`, `"std"`).
pub type ModuleAliasName = NameDef<ModuleAliasNameNamespace>;

/// Opaque owner-local identity of a selective include instance.
///
/// A selective include introduces declaration aliases but no source-visible
/// module alias. Compiler lowering still needs a private namespace for the
/// included implementation, so its source occurrence is represented directly
/// instead of overloading the target module's leaf name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IncludeInstanceId {
    source_offset: usize,
}

impl IncludeInstanceId {
    /// Identify the include occurrence at `path_span` within its owning DAG.
    #[must_use]
    pub const fn at(path_span: Span) -> Self {
        Self {
            source_offset: path_span.offset(),
        }
    }

    /// Render this opaque identity into the flat IR's qualifier namespace.
    ///
    /// `NameAtom` intentionally permits generated resolver names beyond the
    /// source identifier grammar. The angle-bracket spelling cannot collide
    /// with a source module alias and is never parsed to recover the offset;
    /// [`IncludeInstanceId`] remains the authoritative representation.
    #[must_use]
    pub fn synthetic_scope_name(self) -> ModuleAliasName {
        ModuleAliasName::expect_valid(format!("<include@{}>", self.source_offset))
    }
}

/// Namespace assigned to one included DAG instance.
///
/// Module-form includes use a source-visible alias. Selective includes use an
/// opaque private identity because their brace form introduces no module alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IncludeInstanceScope {
    Named(ModuleAliasName),
    Anonymous(IncludeInstanceId),
}

impl IncludeInstanceScope {
    /// Name used at the boundary to the compiler's flat merged IR.
    #[must_use]
    pub fn merge_scope_name(&self) -> ModuleAliasName {
        match self {
            Self::Named(alias) => alias.clone(),
            Self::Anonymous(id) => id.synthetic_scope_name(),
        }
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
/// member: "G0"` as `helpers.math.G0`. That serialized form is for boundary
/// use only (diagnostics, debug output, third-party APIs); the compiler core
/// should use [`Self::qualifier`] and [`Self::member`] instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopedName {
    /// Module/path segments that qualify `member`. Empty for a local name.
    qualifier: Arc<[ModuleAliasName]>,
    /// The declaration/member name inside the qualifier scope.
    member: Arc<DeclName>,
}

impl ScopedName {
    /// Create a local name from an already-validated declaration name.
    #[must_use]
    pub fn local(member: DeclName) -> Self {
        Self {
            qualifier: Arc::from([] as [ModuleAliasName; 0]),
            member: Arc::new(member),
        }
    }

    /// Create a name qualified by one already-validated module segment.
    #[must_use]
    pub fn qualified(module: ModuleAliasName, member: DeclName) -> Self {
        Self::qualified_path([module], member)
    }

    /// Create a name qualified by an arbitrary-depth validated module path.
    #[must_use]
    pub fn qualified_path(
        qualifier: impl IntoIterator<Item = ModuleAliasName>,
        member: DeclName,
    ) -> Self {
        Self {
            qualifier: qualifier.into_iter().collect(),
            member: Arc::new(member),
        }
    }

    /// Parse canonical dot-separated display text at a serialization boundary.
    ///
    /// Each component is validated independently. Empty components such as
    /// those in `.x`, `x.`, or `a..x` are rejected rather than being retained
    /// as ambiguous identity data.
    ///
    /// # Errors
    ///
    /// Returns [`ScopedNameParseError`] when any path component is empty. A
    /// literal dot cannot occur inside a component because dots delimit the
    /// serialized path.
    #[expect(
        clippy::disallowed_methods,
        reason = "str::split always yields one segment; the empty fallback follows the normal invalid-segment error path"
    )]
    pub fn parse(display: impl AsRef<str>) -> Result<Self, ScopedNameParseError> {
        fn parse_segment(segment: &str, position: usize) -> Result<NameAtom, ScopedNameParseError> {
            NameAtom::parse(segment).map_err(|source| ScopedNameParseError::InvalidSegment {
                position,
                segment: segment.to_string(),
                source,
            })
        }

        let mut raw_segments = display.as_ref().split('.');
        // `str::split` always yields at least one item. `unwrap_or_default`
        // keeps that standard-library guarantee out of this type's invariants;
        // an empty fallback is rejected by `parse_segment` in the same way as
        // an empty input string.
        let first = parse_segment(raw_segments.next().unwrap_or_default(), 1)?;
        let rest = raw_segments
            .enumerate()
            .map(|(index, segment)| parse_segment(segment, index + 2))
            .collect::<Result<Vec<_>, _>>()?;
        let path = NamePath::new(crate::syntax::non_empty::NonEmpty::new(first, rest));
        Ok(Self::from(&path))
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
    pub fn qualifier(&self) -> &[ModuleAliasName] {
        &self.qualifier
    }

    /// Convert this semantic name to a validated syntactic path.
    #[must_use]
    pub fn to_name_path(&self) -> NamePath {
        NamePath::qualified_path(
            self.qualifier.iter().map(|segment| segment.atom().clone()),
            self.member.atom().clone(),
        )
    }

    /// Returns whether this is a qualified name.
    #[must_use]
    pub fn is_qualified(&self) -> bool {
        !self.qualifier.is_empty()
    }

    /// Qualify a name with a single-segment prefix, replacing any existing
    /// qualifier while preserving the member.
    ///
    /// `x.with_prefix(p)` → `p.x`.
    /// `m.x.with_prefix(p)` → `p.x`.
    #[must_use]
    pub(crate) fn with_prefix(&self, prefix: &ModuleAliasName) -> Self {
        Self::qualified(prefix.clone(), self.member().clone())
    }

    /// Place this name inside one additional outer instance scope.
    ///
    /// Unlike [`Self::with_prefix`], this preserves the existing qualifier:
    /// `x` becomes `outer.x`, while `inner.x` becomes `outer.inner.x`.
    #[must_use]
    pub(crate) fn within_scope(&self, outer: &ModuleAliasName) -> Self {
        Self::qualified_path(
            std::iter::once(outer.clone()).chain(self.qualifier.iter().cloned()),
            self.member().clone(),
        )
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for segment in self.qualifier.iter() {
            write!(f, "{segment}.")?;
        }
        std::fmt::Display::fmt(&self.member, f)
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

impl From<NameAtom> for ScopedName {
    /// Promote a bare atom into the declaration namespace as a local name.
    fn from(atom: NameAtom) -> Self {
        Self::local(DeclName::from_atom(atom))
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
        let (qualifier, member) = path.split_last();
        Self::qualified_path(
            qualifier.iter().cloned().map(ModuleAliasName::from_atom),
            DeclName::from_atom(member.clone()),
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
    fn scoped_name_qualified_display_uses_dot() {
        let name = ScopedName::qualified(module("module"), member("x"));
        assert_eq!(name.to_string(), "module.x");
        assert_eq!(name.member().as_str(), "x");
        assert_eq!(
            name.qualifier()
                .iter()
                .map(ModuleAliasName::as_str)
                .collect::<Vec<_>>(),
            ["module"]
        );
    }

    #[test]
    fn scoped_name_parses_nested_boundary_text() {
        let name = ScopedName::parse("helpers.math.G0").unwrap();
        assert_eq!(name.to_string(), "helpers.math.G0");
        assert_eq!(name.member().as_str(), "G0");
        assert_eq!(
            name.qualifier()
                .iter()
                .map(ModuleAliasName::as_str)
                .collect::<Vec<_>>(),
            ["helpers", "math"]
        );
    }

    #[test]
    fn scoped_name_supports_nested_typed_qualifier_path() {
        let name = ScopedName::qualified_path([module("helpers"), module("math")], member("G0"));
        assert_eq!(name.to_string(), "helpers.math.G0");
        assert_eq!(name.member().as_str(), "G0");
    }

    #[test]
    fn scoped_name_outer_scope_preserves_existing_qualifier_path() {
        let nested = ScopedName::qualified_path([module("middle"), module("leaf")], member("out"));
        assert_eq!(
            nested.within_scope(&module("upper")).to_string(),
            "upper.middle.leaf.out"
        );
        assert_eq!(
            ScopedName::local(member("out"))
                .within_scope(&module("upper"))
                .to_string(),
            "upper.out"
        );
    }

    #[test]
    fn scoped_name_rejects_empty_display_segments() {
        for invalid in ["", ".x", "x.", "helpers..x"] {
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
    fn selective_include_instance_ids_have_distinct_synthetic_scope_names() {
        let first = IncludeInstanceId::at(Span::new(10, 5));
        let second = IncludeInstanceId::at(Span::new(20, 5));

        assert_ne!(first, second);
        assert_ne!(first.synthetic_scope_name(), second.synthetic_scope_name());
        assert_eq!(first.synthetic_scope_name().as_str(), "<include@10>");
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
