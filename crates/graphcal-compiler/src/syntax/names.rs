//! Typed name atoms and namespace-specific name wrappers.
//!
//! Source identifiers are path segments first; semantic namespace wrappers are
//! layered on top only at definition or resolution boundaries. The wrappers in
//! this module therefore store a [`NameAtom`] rather than an arbitrary flat
//! string, making it impossible to represent a dotted path as a leaf name.

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
        debug_assert!(NameAtom::parse(s.as_str()).is_ok());
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

impl<'a> From<NameAtom> for std::borrow::Cow<'a, str> {
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

/// Macro to define a namespace-specific wrapper around [`NameAtom`] with
/// standard trait impls.
macro_rules! define_name_type {
    (
        $(#[$meta:meta])*
        $vis:vis struct $Name:ident;
    ) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis struct $Name(NameAtom);

        impl std::fmt::Debug for $Name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                // Delegate to the inner String's Debug so that Vec<$Name> formats
                // as ["foo", "bar"] rather than [TypeName("foo"), TypeName("bar")].
                std::fmt::Debug::fmt(&self.0, f)
            }
        }

        impl $Name {
            /// Create a new leaf name from a string.
            ///
            /// # Panics
            ///
            /// Panics if the string is empty or contains `.`. Use
            /// [`Self::try_new`] when validating external input.
            #[must_use]
            pub fn new(s: impl Into<String>) -> Self {
                Self::try_new(s).unwrap_or_else(|err| {
                    panic!("invalid {} leaf name: {err}", stringify!($Name));
                })
            }

            /// Try to create a new leaf name from a string.
            ///
            /// # Errors
            ///
            /// Returns [`NameAtomError`] when the string is empty or contains
            /// a path separator.
            pub fn try_new(s: impl Into<String>) -> Result<Self, NameAtomError> {
                NameAtom::parse(s).map(Self)
            }

            /// Create this namespace-specific name from an existing atom.
            #[must_use]
            pub const fn from_atom(atom: NameAtom) -> Self {
                Self(atom)
            }

            /// Get the underlying atom.
            #[must_use]
            pub const fn atom(&self) -> &NameAtom {
                &self.0
            }

            /// Get the underlying string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }

            /// Consume and return the inner atom.
            #[must_use]
            pub fn into_atom(self) -> NameAtom {
                self.0
            }

            /// Consume and return the inner `String`.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0.into_inner()
            }
        }

        impl std::fmt::Display for $Name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl PartialEq<str> for $Name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $Name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl AsRef<str> for $Name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::borrow::Borrow<str> for $Name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl From<NameAtom> for $Name {
            fn from(atom: NameAtom) -> Self {
                Self::from_atom(atom)
            }
        }

        impl From<String> for $Name {
            fn from(s: String) -> Self {
                Self::new(s)
            }
        }

        impl From<&str> for $Name {
            fn from(s: &str) -> Self {
                Self::new(s)
            }
        }
    };
}

define_name_type! {
    /// Name of a const, param, or node declaration (e.g., `"G0"`, `"dry_mass"`, `"dv_total"`).
    pub struct DeclName;
}

define_name_type! {
    /// Name of a dimension (e.g., `"Length"`, `"Velocity"`).
    pub struct DimName;
}

define_name_type! {
    /// Name of a unit (e.g., `"m"`, `"km"`, `"hour"`).
    pub struct UnitName;
}

define_name_type! {
    /// Name of a struct type (e.g., `"TransferResult"`).
    pub struct StructTypeName;
}

define_name_type! {
    /// Name of an index type (e.g., `"Maneuver"`).
    pub struct IndexName;
}

define_name_type! {
    /// Name of a function (e.g., `"sqrt"`, `"lerp"`).
    pub struct FnName;
}

define_name_type! {
    /// Name of a struct field (e.g., `"dv1"`, `"altitude"`).
    pub struct FieldName;
}

define_name_type! {
    /// Name of an index variant (e.g., `"Departure"`, `"Correction"`).
    pub struct IndexVariantName;
}

impl IndexVariantName {
    /// Build the variant name for the `n`-th step of a range index
    /// (`#0`, `#1`, …). Centralises the `"#"`-prefix format so registry,
    /// parser, and evaluator can't disagree on it.
    #[must_use]
    pub fn range_step(n: impl std::fmt::Display) -> Self {
        Self::new(format!("#{n}"))
    }

    /// Pair this variant with its index name for qualified rendering.
    #[must_use]
    pub fn qualified_by(&self, index: &IndexName) -> QualifiedIndexVariantName {
        QualifiedIndexVariantName::new(index.clone(), self.clone())
    }
}

/// A fully qualified index variant name, rendered as `Index.Variant`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedIndexVariantName {
    index: IndexName,
    variant: IndexVariantName,
}

impl QualifiedIndexVariantName {
    /// Create a qualified index variant name from its index and variant parts.
    #[must_use]
    pub const fn new(index: IndexName, variant: IndexVariantName) -> Self {
        Self { index, variant }
    }

    /// The index/type part of the qualified variant.
    #[must_use]
    pub const fn index(&self) -> &IndexName {
        &self.index
    }

    /// The variant/constructor part of the qualified variant.
    #[must_use]
    pub const fn variant(&self) -> &IndexVariantName {
        &self.variant
    }
}

impl std::fmt::Display for QualifiedIndexVariantName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.index, self.variant)
    }
}

define_name_type! {
    /// Name of a tagged-union constructor (e.g., `"LowThrust"`, `"Coast"`).
    ///
    /// Constructors live in a *separate namespace* from types: a single
    /// lexeme can name both a type and a constructor (and will, once the
    /// single-variant sugar lands). Keeping these as distinct newtypes
    /// enforces the namespace boundary at the type level.
    pub struct ConstructorName;
}

define_name_type! {
    /// Name of a generic type parameter (e.g., `"D"`, `"I"`).
    pub struct GenericParamName;
}

define_name_type! {
    /// Name of a dimension variable in a built-in function signature (e.g., `"D"`).
    ///
    /// Built-in signatures use these variables to relate argument and result
    /// dimensions, such as `sqrt: D -> D^(1/2)` or `min: (D, D) -> D`.
    pub struct DimVarName;
}

define_name_type! {
    /// Name of a local expression binding (e.g., `"x"`, `"stage_mass"`).
    pub struct LocalName;
}

define_name_type! {
    /// Name of a module alias introduced by an import/include declaration (e.g., `"constants"`, `"std"`).
    pub struct ModuleAliasName;
}

define_name_type! {
    /// Name of an open plot/figure/layer property (e.g., `"title"`, `"width"`, `"stroke_width"`).
    pub struct PlotPropertyName;
}

/// Name of a built-in datetime time scale (e.g., `"UTC"`, `"TAI"`, `"TDB"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeScaleName(crate::registry::time_scale::TimeScale);

impl TimeScaleName {
    /// Create a time-scale name from an already-validated time scale.
    #[must_use]
    pub const fn new(scale: crate::registry::time_scale::TimeScale) -> Self {
        Self(scale)
    }

    /// Get the underlying time scale.
    #[must_use]
    pub const fn scale(self) -> crate::registry::time_scale::TimeScale {
        self.0
    }

    /// Get the canonical time-scale name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0.name()
    }
}

impl std::fmt::Display for TimeScaleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for TimeScaleName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

// --- Module-scoped names ---

use std::sync::Arc;

/// A declaration name that may optionally be qualified by a module path.
///
/// The qualifier is stored as structured path segments, not as a flat
/// dot-separated string. This allows arbitrary-depth qualification such as
/// `helpers.math.G0` while keeping the declaration member (`G0`) directly
/// accessible and distinct from the qualifier.
///
/// The `Display` impl renders `qualifier: ["helpers", "math"], member: "G0"`
/// as `helpers.math.G0`. That serialized form is for boundary use only
/// (diagnostics, debug output, third-party APIs); the compiler core should use
/// the typed accessors instead of splitting strings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopedName {
    /// Module/path segments that qualify `member`. Empty for a local name.
    qualifier: Arc<[Arc<str>]>,
    /// The declaration/member name inside the qualifier scope.
    member: Arc<str>,
}

impl ScopedName {
    /// Create an unqualified local name.
    #[must_use]
    pub fn local(member: impl Into<Arc<str>>) -> Self {
        Self {
            qualifier: Arc::from([] as [Arc<str>; 0]),
            member: member.into(),
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
    pub fn with_prefix(&self, prefix: &str) -> Self {
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
    /// [`crate::syntax::ast::Ident::into_spanned`] uses to lift parser
    /// identifiers into the typed name; qualified forms are constructed
    /// explicitly via [`ScopedName::qualified`] or [`ScopedName::qualified_path`].
    fn from(atom: NameAtom) -> Self {
        Self::local(atom.into_inner())
    }
}

impl From<String> for ScopedName {
    /// Wrap a bare string as a local `ScopedName`.
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
        Self::new(
            crate::syntax::non_empty::NonEmpty::try_from_vec(segments)
                .expect("qualified_path always pushes a leaf segment"),
        )
    }

    /// Borrow all path segments in source order.
    #[must_use]
    pub fn segments(&self) -> &[NameAtom] {
        self.segments.as_slice()
    }

    /// Mutably borrow all path segments in source order.
    #[must_use]
    pub fn segments_mut(&mut self) -> &mut [NameAtom] {
        self.segments.as_mut_slice()
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

    /// Returns the leaf segment as a string slice.
    ///
    /// Use this only at legacy boundaries that still key local registries by
    /// leaf names. Do not use it to recover structure from a qualified path.
    #[must_use]
    pub fn leaf_str(&self) -> &str {
        self.leaf().as_str()
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
        let segments = self.segments.as_slice();
        let (leaf, qualifier) = segments
            .split_last()
            .expect("NamePath is backed by NonEmpty");
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

impl From<IndexName> for NamePath {
    fn from(name: IndexName) -> Self {
        Self::local(name.into_atom())
    }
}

impl From<String> for NamePath {
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

    #[test]
    fn name_atom_rejects_dotted_paths() {
        assert_eq!(
            NameAtom::parse("module.Value"),
            Err(NameAtomError::ContainsDot)
        );
        assert_eq!(
            DeclName::try_new("module.Value"),
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
        let name = DeclName::new("dry_mass");
        assert_eq!(format!("{name}"), "dry_mass");
    }

    #[test]
    fn newtype_as_str() {
        let name = DimName::new("Length");
        assert_eq!(name.as_str(), "Length");
    }

    #[test]
    fn newtype_into_inner() {
        let name = UnitName::new("km");
        assert_eq!(name.into_inner(), "km");
    }

    #[test]
    fn newtype_hash_map_borrow_lookup() {
        let mut map = HashMap::new();
        map.insert(DeclName::new("x"), 42);
        // Lookup with &str via Borrow<str>
        assert_eq!(map.get("x"), Some(&42));
    }

    #[test]
    fn newtype_from_string() {
        let name: FieldName = "dv1".to_string().into();
        assert_eq!(name.as_str(), "dv1");
    }

    #[test]
    fn newtype_from_str() {
        let name: IndexVariantName = "Departure".into();
        assert_eq!(name.as_str(), "Departure");
    }

    #[test]
    fn newtype_equality() {
        assert_eq!(IndexName::new("Maneuver"), IndexName::new("Maneuver"));
        assert_ne!(IndexName::new("Maneuver"), IndexName::new("Phase"));
    }

    #[test]
    fn newtype_ord() {
        let a = FnName::new("alpha");
        let b = FnName::new("beta");
        assert!(a < b);
    }

    #[test]
    fn name_path_preserves_qualifier_and_leaf() {
        let path = NamePath::qualified_path(
            [NameAtom::parse("module").unwrap()],
            NameAtom::parse("Index").unwrap(),
        );
        assert_eq!(path.display_path(), "module.Index");
        assert_eq!(path.leaf_str(), "Index");
        assert_eq!(
            path.qualifier_segments()
                .iter()
                .map(NameAtom::as_str)
                .collect::<Vec<_>>(),
            ["module"]
        );
    }

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
