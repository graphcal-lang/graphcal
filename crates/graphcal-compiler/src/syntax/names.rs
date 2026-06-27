//! Generic typed-name infrastructure.
//!
//! This module defines the reusable machinery for representing names without
//! falling back to convention-heavy strings. It intentionally stops at generic
//! concepts: leaf atoms, namespace-tagged definition names, module-resolved
//! names, and unresolved syntactic paths. Domain-specific aliases and compound
//! name shapes live in the modules that own their semantics, such as
//! [`crate::syntax::index_name`], [`crate::syntax::dimension`],
//! [`crate::syntax::type_name`], [`crate::syntax::module_name`], and
//! [`crate::registry::time_scale`].
//!
//! # Core building blocks
//!
//! - [`NameAtom`] is a single non-empty path segment. It rejects `.` so a leaf
//!   name cannot accidentally carry a qualified path. It is the storage type for
//!   semantic names and also permits compiler-generated leaf names such as range
//!   variants (`#0`, `#1`, ...).
//! - [`NameNamespace`] is implemented by zero-sized marker types owned by the
//!   relevant domain module. It gives [`NameDef`] and [`ResolvedName`] their
//!   type-level namespace without adding runtime data.
//! - [`NameDef`]`<Ns>` is a definition-site leaf name tagged with a semantic
//!   namespace marker. Use it when the grammar already determines the namespace
//!   of the identifier being introduced.
//! - [`ResolvedName`]`<Ns>` is a reference that has passed module-aware
//!   resolution. It stores the canonical owning [`DagId`](crate::dag_id::DagId)
//!   plus the leaf [`NameAtom`], rather than preserving source qualifier text.
//! - [`NamePath`] is a syntactic non-empty dotted path with no semantic
//!   namespace assigned to any segment. Keep unresolved reference positions as a
//!   `NamePath` (or [`IdentPath`](crate::syntax::ast::IdentPath) when segment
//!   spans matter) until resolution can produce a domain-specific resolved type.
//!
//! Render these types as strings only at boundaries such as diagnostics,
//! formatting, serialization, and third-party APIs. Inside the compiler core,
//! preserve and pattern-match the typed parts.

use std::marker::PhantomData;

/// Error returned when constructing a [`NameAtom`] from invalid text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameAtomError {
    /// Name atoms are leaf segments and cannot be empty.
    #[error("name atom cannot be empty")]
    Empty,
    /// Dots separate path segments; they are not valid inside a single atom.
    #[error("name atom cannot contain `.`")]
    ContainsDot,
}

/// A single name segment with no path separators.
///
/// `NameAtom` deliberately models only the leaf/segment invariant. It does not
/// attempt to encode the full lexer grammar because some internal names, such
/// as synthetic range variants (`#0`, `#1`, ...), are not source identifiers but
/// still must never contain `.`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NameAtom(String);

impl NameAtom {
    /// Parse a raw string into a single name segment.
    ///
    /// # Errors
    ///
    /// Returns [`NameAtomError::Empty`] for empty strings and
    /// [`NameAtomError::ContainsDot`] when the text contains a path separator.
    pub fn parse(s: impl Into<String>) -> Result<Self, NameAtomError> {
        let s = s.into();
        if s.is_empty() {
            return Err(NameAtomError::Empty);
        }
        if s.contains('.') {
            return Err(NameAtomError::ContainsDot);
        }
        Ok(Self(s))
    }

    /// Construct an atom from lexer-produced identifier text.
    ///
    /// The parser has already tokenized this as a single `IDENT`, so the same
    /// invariant is asserted here without making parser code handle an
    /// impossible error path.
    #[must_use]
    pub(crate) fn new_unchecked_for_parser(s: String) -> Self {
        debug_assert!(Self::parse(s.as_str()).is_ok());
        Self(s)
    }

    /// Get the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume and return the inner `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for NameAtom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::fmt::Display for NameAtom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for NameAtom {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq<str> for NameAtom {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for NameAtom {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for NameAtom {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<NameAtom> for str {
    fn eq(&self, other: &NameAtom) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<NameAtom> for &str {
    fn eq(&self, other: &NameAtom) -> bool {
        *self == other.as_str()
    }
}

impl AsRef<str> for NameAtom {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for NameAtom {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<NameAtom> for String {
    fn from(atom: NameAtom) -> Self {
        atom.into_inner()
    }
}

impl From<&NameAtom> for String {
    fn from(atom: &NameAtom) -> Self {
        atom.as_str().to_string()
    }
}

impl From<NameAtom> for std::borrow::Cow<'_, str> {
    fn from(atom: NameAtom) -> Self {
        Self::Owned(atom.into_inner())
    }
}

impl TryFrom<String> for NameAtom {
    type Error = NameAtomError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for NameAtom {
    type Error = NameAtomError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// Marker trait for a semantic name namespace.
///
/// Namespaces are zero-sized marker types used by [`NameDef`] and
/// [`ResolvedName`] to make it impossible to mix, for example, a function name
/// with an index name. The marker's [`NameNamespace::DISPLAY_NAME`] is used
/// only for diagnostics and panic messages at construction boundaries.
pub trait NameNamespace:
    std::fmt::Debug + Clone + Copy + PartialEq + Eq + std::hash::Hash + PartialOrd + Ord + 'static
{
    /// Human-readable alias/newtype name for this namespace.
    const DISPLAY_NAME: &'static str;
}

/// A definition-site leaf name in a semantic namespace.
///
/// `NameDef<Ns>` is intentionally a single [`NameAtom`]. It is suitable for
/// names introduced by syntax positions whose namespace is fixed by the
/// grammar, such as `type Foo`, `index Phase`, or `unit m`. Reference positions
/// that may be qualified should stay as [`NamePath`]
/// / [`IdentPath`](crate::syntax::ast::IdentPath) until module-aware
/// resolution can produce a [`ResolvedName`].
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NameDef<Ns: NameNamespace> {
    atom: NameAtom,
    _ns: PhantomData<Ns>,
}

impl<Ns: NameNamespace> std::fmt::Debug for NameDef<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegate to the inner string's Debug so that Vec<DeclName> formats
        // as ["foo", "bar"] rather than [NameDef { ... }].
        std::fmt::Debug::fmt(&self.atom, f)
    }
}

impl<Ns: NameNamespace> NameDef<Ns> {
    /// Try to create a new leaf name from a string.
    ///
    /// # Errors
    ///
    /// Returns [`NameAtomError`] when the string is empty or contains a path
    /// separator.
    pub fn try_new(s: impl Into<String>) -> Result<Self, NameAtomError> {
        NameAtom::parse(s).map(Self::from_atom)
    }

    /// Create a leaf name from trusted text, panicking if invalid.
    ///
    /// Prefer [`Self::try_new`] for external input. This helper keeps panic
    /// policy explicit at trusted call sites without exposing a generic
    /// panicking `new` constructor.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "trusted constructor centralizes explicit panic policy"
    )]
    pub fn expect_valid(s: impl Into<String>) -> Self {
        Self::try_new(s).expect("trusted leaf name must be valid")
    }

    /// Create this namespace-specific name from an existing atom.
    #[must_use]
    pub const fn from_atom(atom: NameAtom) -> Self {
        Self {
            atom,
            _ns: PhantomData,
        }
    }

    /// Get the underlying atom.
    #[must_use]
    pub const fn atom(&self) -> &NameAtom {
        &self.atom
    }

    /// Get the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.atom.as_str()
    }

    /// Consume and return the inner atom.
    #[must_use]
    pub fn into_atom(self) -> NameAtom {
        self.atom
    }

    /// Consume and return the inner `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.atom.into_inner()
    }
}

impl<Ns: NameNamespace> std::fmt::Display for NameDef<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<Ns: NameNamespace> PartialEq<str> for NameDef<Ns> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<Ns: NameNamespace> PartialEq<&str> for NameDef<Ns> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl<Ns: NameNamespace> AsRef<str> for NameDef<Ns> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<Ns: NameNamespace> std::borrow::Borrow<str> for NameDef<Ns> {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl<Ns: NameNamespace> From<NameAtom> for NameDef<Ns> {
    fn from(atom: NameAtom) -> Self {
        Self::from_atom(atom)
    }
}

impl<Ns: NameNamespace> TryFrom<String> for NameDef<Ns> {
    type Error = NameAtomError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_new(s)
    }
}

impl<Ns: NameNamespace> TryFrom<&str> for NameDef<Ns> {
    type Error = NameAtomError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::try_new(s)
    }
}

/// A fully resolved reference in a semantic namespace.
///
/// Unlike [`NamePath`], this no longer stores source qualifier text. The
/// `owner` is the canonical DAG/module identity chosen by module-aware
/// resolution; `name` is the declaration leaf inside that owner.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResolvedName<Ns: NameNamespace> {
    owner: crate::dag_id::DagId,
    name: NameAtom,
    _ns: PhantomData<Ns>,
}

impl<Ns: NameNamespace> std::fmt::Debug for ResolvedName<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedName")
            .field("namespace", &Ns::DISPLAY_NAME)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .finish()
    }
}

impl<Ns: NameNamespace> ResolvedName<Ns> {
    /// Construct a resolved name from its canonical owner and leaf atom.
    #[must_use]
    pub const fn new(owner: crate::dag_id::DagId, name: NameAtom) -> Self {
        Self {
            owner,
            name,
            _ns: PhantomData,
        }
    }

    /// Resolve an existing definition-site name into a canonical owner.
    #[must_use]
    pub fn from_def(owner: crate::dag_id::DagId, name: NameDef<Ns>) -> Self {
        Self::new(owner, name.into_atom())
    }

    /// The canonical DAG/module that owns this name.
    #[must_use]
    pub const fn owner(&self) -> &crate::dag_id::DagId {
        &self.owner
    }

    /// The leaf atom inside [`Self::owner`].
    #[must_use]
    pub const fn atom(&self) -> &NameAtom {
        &self.name
    }

    /// The leaf string inside [`Self::owner`].
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }

    /// Return the unowned definition-site leaf in the same namespace.
    ///
    /// This deliberately drops the canonical owner. Use it only at explicit
    /// standalone registry, diagnostic, or serialization boundaries that
    /// cannot yet carry [`ResolvedName`] itself.
    #[must_use]
    pub fn to_unowned_def_name(&self) -> NameDef<Ns> {
        NameDef::from_atom(self.name.clone())
    }

    /// Consume this value and return the canonical owner plus leaf atom.
    #[must_use]
    pub fn into_parts(self) -> (crate::dag_id::DagId, NameAtom) {
        (self.owner, self.name)
    }
}

impl<Ns: NameNamespace> std::fmt::Display for ResolvedName<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.owner, self.name)
    }
}

/// A syntactic non-empty dot-separated name path.
///
/// `NamePath` preserves source-level path shape (`Foo`, `module.Foo`,
/// `module.Index.Variant`) without assigning a semantic namespace to any
/// segment. It is appropriate for unresolved reference positions that do not
/// need per-segment spans. Use [`crate::syntax::ast::IdentPath`] when the AST
/// must retain source spans for each segment.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NamePath {
    segments: crate::syntax::non_empty::NonEmpty<NameAtom>,
}

impl NamePath {
    /// Construct a path from already-validated atoms.
    #[must_use]
    pub const fn new(segments: crate::syntax::non_empty::NonEmpty<NameAtom>) -> Self {
        Self { segments }
    }

    /// Construct a one-segment path.
    #[must_use]
    pub fn local(atom: NameAtom) -> Self {
        Self::new(crate::syntax::non_empty::NonEmpty::singleton(atom))
    }

    /// Construct a path from qualifier atoms plus a leaf atom.
    #[must_use]
    pub fn qualified_path(qualifier: impl IntoIterator<Item = NameAtom>, leaf: NameAtom) -> Self {
        let mut segments: Vec<NameAtom> = qualifier.into_iter().collect();
        segments.push(leaf);
        let first = segments.remove(0);
        Self::new(crate::syntax::non_empty::NonEmpty::new(first, segments))
    }

    /// Borrow all path segments in source order.
    #[must_use]
    pub fn segments(&self) -> &[NameAtom] {
        self.segments.as_slice()
    }

    /// Consume and return all path segments.
    #[must_use]
    pub fn into_segments(self) -> crate::syntax::non_empty::NonEmpty<NameAtom> {
        self.segments
    }

    /// Number of path segments. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.segments.len()
    }

    /// Returns `false`; provided for API compatibility with sequence-like code.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Returns whether this is a one-segment path.
    #[must_use]
    pub const fn is_bare(&self) -> bool {
        self.segments.len() == 1
    }

    /// Returns the leaf segment.
    #[must_use]
    pub fn leaf(&self) -> &NameAtom {
        self.segments.last()
    }

    /// Returns the only segment when this is a bare path.
    #[must_use]
    pub fn as_bare(&self) -> Option<&NameAtom> {
        match self.segments.as_slice() {
            [atom] => Some(atom),
            _ => None,
        }
    }

    /// Split the path into qualifier segments and leaf segment.
    ///
    /// The qualifier slice is empty for one-segment paths.
    #[must_use]
    pub fn split_last(&self) -> (&[NameAtom], &NameAtom) {
        let (leaf, qualifier) = self.segments.split_last();
        (qualifier, leaf)
    }

    /// Returns the qualifier segments before the leaf. Empty for bare paths.
    #[must_use]
    pub fn qualifier_segments(&self) -> &[NameAtom] {
        self.split_last().0
    }

    /// Returns qualifier segments and leaf only when this path is qualified.
    #[must_use]
    pub fn qualifier_and_leaf(&self) -> Option<(&[NameAtom], &NameAtom)> {
        let (qualifier, leaf) = self.split_last();
        (!qualifier.is_empty()).then_some((qualifier, leaf))
    }

    /// Human-readable path string for diagnostics and formatting boundaries.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.segments
            .iter()
            .map(NameAtom::as_str)
            .collect::<Vec<_>>()
            .join(".")
    }
}

impl From<NameAtom> for NamePath {
    fn from(atom: NameAtom) -> Self {
        Self::local(atom)
    }
}

impl From<String> for NamePath {
    #[expect(
        clippy::panic,
        reason = "From<String> is a convenience for trusted leaf names"
    )]
    fn from(s: String) -> Self {
        Self::local(NameAtom::parse(s).unwrap_or_else(|err| {
            panic!("invalid NamePath leaf name: {err}");
        }))
    }
}

impl From<&str> for NamePath {
    fn from(s: &str) -> Self {
        Self::from(s.to_string())
    }
}

impl std::fmt::Display for NamePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (idx, segment) in self.segments.iter().enumerate() {
            if idx > 0 {
                f.write_str(".")?;
            }
            f.write_str(segment.as_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    enum TestDeclNamespace {}

    impl NameNamespace for TestDeclNamespace {
        const DISPLAY_NAME: &'static str = "TestDeclName";
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    enum TestIndexNamespace {}

    impl NameNamespace for TestIndexNamespace {
        const DISPLAY_NAME: &'static str = "TestIndexName";
    }

    type TestDeclName = NameDef<TestDeclNamespace>;
    type TestIndexName = NameDef<TestIndexNamespace>;

    #[test]
    fn name_atom_rejects_dotted_paths() {
        assert_eq!(
            NameAtom::parse("module.Value"),
            Err(NameAtomError::ContainsDot)
        );
        assert_eq!(
            TestDeclName::try_new("module.Value"),
            Err(NameAtomError::ContainsDot)
        );
    }

    #[test]
    fn name_atom_accepts_internal_leaf_names() {
        let atom = NameAtom::parse("#0").unwrap();
        assert_eq!(atom.as_str(), "#0");
    }

    #[test]
    fn newtype_display() {
        let name = TestDeclName::expect_valid("dry_mass");
        assert_eq!(format!("{name}"), "dry_mass");
    }

    #[test]
    fn newtype_as_str() {
        let name = TestDeclName::expect_valid("Length");
        assert_eq!(name.as_str(), "Length");
    }

    #[test]
    fn newtype_into_inner() {
        let name = TestDeclName::expect_valid("km");
        assert_eq!(name.into_inner(), "km");
    }

    #[test]
    fn newtype_hash_map_borrow_lookup() {
        let mut map = HashMap::new();
        map.insert(TestDeclName::expect_valid("x"), 42);
        assert_eq!(map.get("x"), Some(&42));
    }

    #[test]
    fn newtype_try_from_string() {
        let name = TestDeclName::try_from("dv1".to_string()).unwrap();
        assert_eq!(name.as_str(), "dv1");
    }

    #[test]
    fn newtype_try_from_str() {
        let name = TestDeclName::try_from("Departure").unwrap();
        assert_eq!(name.as_str(), "Departure");
    }

    #[test]
    fn newtype_equality() {
        assert_eq!(
            TestIndexName::expect_valid("Maneuver"),
            TestIndexName::expect_valid("Maneuver")
        );
        assert_ne!(
            TestIndexName::expect_valid("Maneuver"),
            TestIndexName::expect_valid("Phase")
        );
    }

    #[test]
    fn newtype_ord() {
        let a = TestDeclName::expect_valid("alpha");
        let b = TestDeclName::expect_valid("beta");
        assert!(a < b);
    }

    #[test]
    fn name_path_preserves_qualifier_and_leaf() {
        let path = NamePath::qualified_path(
            [NameAtom::parse("module").unwrap()],
            NameAtom::parse("Index").unwrap(),
        );
        assert_eq!(path.display_path(), "module.Index");
        assert_eq!(path.leaf().as_str(), "Index");
        assert_eq!(
            path.qualifier_segments()
                .iter()
                .map(NameAtom::as_str)
                .collect::<Vec<_>>(),
            ["module"]
        );
    }

    #[test]
    fn name_def_aliases_keep_namespace_and_leaf_invariant() {
        let decl = TestDeclName::expect_valid("x");
        let index = TestIndexName::expect_valid("x");

        assert_eq!(decl.as_str(), index.as_str());
        assert_eq!(
            TestDeclName::try_new("module.x"),
            Err(NameAtomError::ContainsDot)
        );
        assert_eq!(
            TestIndexName::try_new("module.x"),
            Err(NameAtomError::ContainsDot)
        );
    }

    #[test]
    fn resolved_name_carries_canonical_owner_and_leaf() {
        let name = TestDeclName::expect_valid("dry_mass");
        let resolved = ResolvedName::<TestDeclNamespace>::from_def(
            crate::dag_id::DagId::new(
                "test",
                crate::syntax::non_empty::NonEmpty::new("helpers", vec!["mass"]),
            ),
            name,
        );

        assert_eq!(resolved.owner().to_string(), "helpers.mass");
        assert_eq!(resolved.as_str(), "dry_mass");
        assert_eq!(resolved.to_string(), "helpers.mass.dry_mass");
        assert_eq!(
            resolved.to_unowned_def_name(),
            TestDeclName::expect_valid("dry_mass")
        );
    }
}
