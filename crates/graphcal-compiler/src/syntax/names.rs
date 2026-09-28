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
//!   semantic names and also permits generated display leaves at diagnostic
//!   boundaries. Structured index-entry positions remain typed values rather
//!   than `NameAtom`s.
//! - [`NameNamespace`] is implemented by zero-sized marker types owned by the
//!   relevant domain module. It gives [`NameDef`] and [`ResolvedName`] their
//!   type-level namespace without adding runtime data.
//! - <code>[NameDef]&lt;Ns&gt;</code> is a definition-site leaf name tagged with a semantic
//!   namespace marker. Use it when the grammar already determines the namespace
//!   of the identifier being introduced.
//! - <code>[ResolvedName]&lt;Ns&gt;</code> is a reference that has passed module-aware
//!   resolution. It stores the canonical owning [`DagId`](crate::dag_id::DagId)
//!   plus the leaf [`NameAtom`], rather than preserving source qualifier text.
//! - <code>[Qualified]&lt;Seg, Leaf&gt;</code> is the single source-path shape
//!   `leaf` / `seg.seg::leaf`. [`NamePath`] is its span-less,
//!   namespace-neutral instance. Keep unresolved reference positions as a
//!   `NamePath` (or [`IdentPath`](crate::syntax::ast::IdentPath) when segment
//!   spans matter) until resolution can produce a domain-specific resolved type.
//!
//! Render these types as strings only at boundaries such as diagnostics,
//! formatting, serialization, and third-party APIs. Inside the compiler core,
//! preserve and pattern-match the typed parts.

use std::marker::PhantomData;

use crate::syntax::non_empty::NonEmpty;

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
/// attempt to encode the full lexer grammar because resolver-owned and wire
/// names may be broader than source identifiers while still excluding `.`.
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
        NameAtom::parse(s).map(Self::classify)
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

    /// Classify an unnamespaced atom into this namespace.
    ///
    /// This is the only way to attach a namespace to an existing
    /// [`NameAtom`]. Call it where the grammar position or a resolver lookup
    /// fixes the namespace of a source identifier. `NameAtom` and `NameDef`
    /// deliberately have no implicit `&str` view (`Deref`, `Borrow`,
    /// `PartialEq<str>`) or `From` conversion, so a namespace can neither be
    /// erased for a string-keyed lookup nor silently swapped for another.
    #[must_use]
    pub const fn classify(atom: NameAtom) -> Self {
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
    pub(crate) fn into_atom(self) -> NameAtom {
        self.atom
    }
}

impl<Ns: NameNamespace> std::fmt::Display for NameDef<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    pub(crate) const fn new(owner: crate::dag_id::DagId, name: NameAtom) -> Self {
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
        NameDef::classify(self.name.clone())
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

/// A leaf name selected locally or after a dotted owner path and `::`.
///
/// This is the one source-path shape of the language: `leaf` or
/// `seg.seg::leaf`. Dots separate owner segments and exactly one `::`
/// separates the owner from the selected leaf, so the owner is either absent
/// or non-empty and the two parts can never be confused. Consumers pick the
/// segment and leaf types for their phase and namespace:
///
/// - [`NamePath`] (`Qualified<NameAtom, NameAtom>`) is span-less and
///   namespace-neutral;
/// - [`IdentPath`](crate::syntax::ast::IdentPath) keeps per-segment spans;
/// - [`UnitRef`](crate::syntax::dimension::UnitRef) and
///   [`DimRef`](crate::syntax::dimension::DimRef) classify the leaf;
/// - [`ScopedName`](crate::syntax::module_name::ScopedName) classifies both
///   the owner segments (module aliases) and the leaf (a declaration).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Qualified<Seg, Leaf> {
    owner: Option<NonEmpty<Seg>>,
    leaf: Leaf,
}

impl<Seg, Leaf> Qualified<Seg, Leaf> {
    /// Construct a local name with no `::` boundary.
    #[must_use]
    pub const fn local(leaf: Leaf) -> Self {
        Self { owner: None, leaf }
    }

    /// Construct a leaf selected after a non-empty dotted owner and `::`.
    #[must_use]
    #[expect(
        clippy::self_named_constructors,
        reason = "`local` / `qualified` name the two path shapes symmetrically"
    )]
    pub const fn qualified(owner: NonEmpty<Seg>, leaf: Leaf) -> Self {
        Self {
            owner: Some(owner),
            leaf,
        }
    }

    /// Construct from an optional owner (local when `None`).
    #[must_use]
    pub const fn from_parts(owner: Option<NonEmpty<Seg>>, leaf: Leaf) -> Self {
        Self { owner, leaf }
    }

    /// The dotted owner before `::`, when this is a qualified name.
    #[must_use]
    pub const fn owner(&self) -> Option<&NonEmpty<Seg>> {
        self.owner.as_ref()
    }

    /// The owner segments before `::`; empty for a local name.
    #[must_use]
    pub fn qualifier(&self) -> &[Seg] {
        self.owner.as_ref().map_or(&[], NonEmpty::as_slice)
    }

    /// The selected local or member leaf.
    #[must_use]
    pub const fn leaf(&self) -> &Leaf {
        &self.leaf
    }

    /// The leaf only when this is a local name.
    #[must_use]
    pub const fn as_bare(&self) -> Option<&Leaf> {
        match &self.owner {
            None => Some(&self.leaf),
            Some(_) => None,
        }
    }

    /// The owner and leaf only when this is a qualified name.
    #[must_use]
    pub const fn qualifier_and_leaf(&self) -> Option<(&NonEmpty<Seg>, &Leaf)> {
        match &self.owner {
            Some(owner) => Some((owner, &self.leaf)),
            None => None,
        }
    }

    /// Returns whether this name crosses a `::` boundary.
    #[must_use]
    pub const fn is_qualified(&self) -> bool {
        self.owner.is_some()
    }

    /// Consume into the optional owner and the leaf.
    #[must_use]
    pub fn into_parts(self) -> (Option<NonEmpty<Seg>>, Leaf) {
        (self.owner, self.leaf)
    }

    /// Transform segments and leaf while preserving the path shape.
    #[must_use]
    pub fn map<S, L>(
        self,
        segment: impl FnMut(Seg) -> S,
        leaf: impl FnOnce(Leaf) -> L,
    ) -> Qualified<S, L> {
        Qualified {
            owner: self.owner.map(|owner| owner.map(segment)),
            leaf: leaf(self.leaf),
        }
    }

    /// Transform borrowed segments and leaf while preserving the path shape.
    #[must_use]
    pub fn map_ref<S, L>(
        &self,
        segment: impl FnMut(&Seg) -> S,
        leaf: impl FnOnce(&Leaf) -> L,
    ) -> Qualified<S, L> {
        Qualified {
            owner: self.owner.as_ref().map(|owner| owner.map_ref(segment)),
            leaf: leaf(&self.leaf),
        }
    }
}

impl<Seg, Leaf> From<Leaf> for Qualified<Seg, Leaf> {
    fn from(leaf: Leaf) -> Self {
        Self::local(leaf)
    }
}

impl<Seg: std::fmt::Display, Leaf: std::fmt::Display> std::fmt::Display for Qualified<Seg, Leaf> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(owner) = &self.owner {
            for (index, segment) in owner.iter().enumerate() {
                if index > 0 {
                    f.write_str(".")?;
                }
                write!(f, "{segment}")?;
            }
            f.write_str("::")?;
        }
        write!(f, "{}", self.leaf)
    }
}

/// A span-less, namespace-neutral source path (`leaf` or `a.b::leaf`).
///
/// Keep unresolved reference positions as a `NamePath` until module-aware
/// resolution can produce a domain-specific resolved type.
pub type NamePath = Qualified<NameAtom, NameAtom>;

impl NamePath {
    /// Construct a local name from trusted leaf text, panicking if invalid.
    #[expect(
        clippy::panic,
        reason = "trusted constructor centralizes explicit panic policy"
    )]
    pub(crate) fn expect_local(s: impl Into<String>) -> Self {
        NameAtom::parse(s).map_or_else(
            |err| panic!("trusted NamePath leaf must be valid: {err}"),
            Self::local,
        )
    }

    /// Classify the leaf into a namespace while keeping the owner neutral.
    #[must_use]
    pub fn classify_leaf<Ns: NameNamespace>(self) -> Qualified<NameAtom, NameDef<Ns>> {
        self.map(|segment| segment, NameDef::classify)
    }

    /// Human-readable source spelling for diagnostics and formatting.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.to_string()
    }
}

impl<Ns: NameNamespace> Qualified<NameAtom, NameDef<Ns>> {
    /// Drop the leaf's namespace to obtain the neutral path consumed by the
    /// resolver.
    #[must_use]
    pub fn to_name_path(&self) -> NamePath {
        self.map_ref(Clone::clone, |leaf| leaf.atom().clone())
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
    fn newtype_hash_map_lookup_uses_classified_key() {
        let mut map = HashMap::new();
        map.insert(TestDeclName::expect_valid("x"), 42);
        let atom = NameAtom::parse("x").unwrap();
        assert_eq!(map.get(&TestDeclName::classify(atom.clone())), Some(&42));
        assert_eq!(TestIndexName::classify(atom).as_str(), "x");
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

    fn atom(s: &str) -> NameAtom {
        NameAtom::parse(s).unwrap()
    }

    #[test]
    fn qualified_path_preserves_owner_and_leaf() {
        let path = NamePath::qualified(NonEmpty::new(atom("a"), vec![atom("b")]), atom("Index"));
        assert_eq!(path.display_path(), "a.b::Index");
        assert_eq!(path.leaf().as_str(), "Index");
        assert!(path.is_qualified());
        assert_eq!(path.as_bare(), None);
        assert_eq!(
            path.qualifier()
                .iter()
                .map(NameAtom::as_str)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let (owner, leaf) = path.qualifier_and_leaf().unwrap();
        assert_eq!(owner.len(), 2);
        assert_eq!(leaf.as_str(), "Index");
    }

    #[test]
    fn local_path_has_no_owner() {
        let path = NamePath::local(atom("x"));
        assert_eq!(path.to_string(), "x");
        assert!(!path.is_qualified());
        assert!(path.qualifier().is_empty());
        assert_eq!(path.as_bare().map(NameAtom::as_str), Some("x"));
        assert!(path.qualifier_and_leaf().is_none());
        assert_eq!(NamePath::from(atom("x")), path);
        assert_eq!(NamePath::from_parts(None, atom("x")), path);
    }

    #[test]
    fn qualified_map_and_classify_keep_shape() {
        let path = NamePath::qualified(NonEmpty::singleton(atom("m")), atom("x"));
        let classified = path.clone().classify_leaf::<TestDeclNamespace>();
        assert_eq!(classified.leaf(), &TestDeclName::expect_valid("x"));
        assert_eq!(classified.to_name_path(), path);
        let lengths = path.map_ref(|segment| segment.as_str().len(), |leaf| leaf.as_str().len());
        assert_eq!(lengths.qualifier(), &[1]);
        assert_eq!(*lengths.leaf(), 1);
        let (owner, leaf) = path.into_parts();
        assert_eq!(owner.map(|owner| owner.len()), Some(1));
        assert_eq!(leaf.as_str(), "x");
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
