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

use std::marker::PhantomData;

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

/// Semantic name namespace markers.
///
/// These are marker types only; values of these types are never constructed.
pub mod namespace {
    use super::NameNamespace;

    macro_rules! define_namespace {
        ($($(#[$meta:meta])* $Name:ident => $display:literal;)+) => {
            $(
                $(#[$meta])*
                #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
                pub enum $Name {}

                impl NameNamespace for $Name {
                    const DISPLAY_NAME: &'static str = $display;
                }
            )+
        };
    }

    define_namespace! {
        /// Const/param/node declaration namespace.
        Decl => "DeclName";
        /// Dimension namespace.
        Dim => "DimName";
        /// Unit namespace.
        Unit => "UnitName";
        /// Struct/tagged-union type namespace.
        StructType => "StructTypeName";
        /// Index type namespace.
        Index => "IndexName";
        /// Function namespace.
        Fn => "FnName";
        /// Struct/constructor field namespace.
        Field => "FieldName";
        /// Index variant namespace.
        IndexVariant => "IndexVariantName";
        /// Tagged-union constructor namespace.
        Constructor => "ConstructorName";
        /// Generic parameter namespace.
        GenericParam => "GenericParamName";
        /// Built-in dimension-variable namespace.
        DimVar => "DimVarName";
        /// Local expression-binding namespace.
        Local => "LocalName";
        /// Module alias namespace.
        ModuleAlias => "ModuleAliasName";
        /// Plot/figure/layer property namespace.
        PlotProperty => "PlotPropertyName";
    }
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
    /// Create a new leaf name from a string.
    ///
    /// # Panics
    ///
    /// Panics if the string is empty or contains `.`. Use [`Self::try_new`]
    /// when validating external input.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "infallible constructor documents invalid input panic"
    )]
    pub fn new(s: impl Into<String>) -> Self {
        Self::try_new(s).unwrap_or_else(|err| {
            panic!("invalid {} leaf name: {err}", Ns::DISPLAY_NAME);
        })
    }

    /// Try to create a new leaf name from a string.
    ///
    /// # Errors
    ///
    /// Returns [`NameAtomError`] when the string is empty or contains a path
    /// separator.
    pub fn try_new(s: impl Into<String>) -> Result<Self, NameAtomError> {
        NameAtom::parse(s).map(Self::from_atom)
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

impl<Ns: NameNamespace> From<String> for NameDef<Ns> {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl<Ns: NameNamespace> From<&str> for NameDef<Ns> {
    fn from(s: &str) -> Self {
        Self::new(s)
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

/// Name of a const, param, or node declaration (e.g., `"G0"`, `"dry_mass"`, `"dv_total"`).
pub type DeclName = NameDef<namespace::Decl>;

/// Name of a dimension (e.g., `"Length"`, `"Velocity"`).
pub type DimName = NameDef<namespace::Dim>;

/// Name of a unit (e.g., `"m"`, `"km"`, `"hour"`).
pub type UnitName = NameDef<namespace::Unit>;

/// Name of a struct type (e.g., `"TransferResult"`).
pub type StructTypeName = NameDef<namespace::StructType>;

/// Name of an index type (e.g., `"Maneuver"`).
pub type IndexName = NameDef<namespace::Index>;

/// Name of a function (e.g., `"sqrt"`, `"lerp"`).
pub type FnName = NameDef<namespace::Fn>;

/// Name of a struct field (e.g., `"dv1"`, `"altitude"`).
pub type FieldName = NameDef<namespace::Field>;

/// Name of an index variant (e.g., `"Departure"`, `"Correction"`).
pub type IndexVariantName = NameDef<namespace::IndexVariant>;

/// Name of a tagged-union constructor (e.g., `"LowThrust"`, `"Coast"`).
///
/// Constructors live in a *separate namespace* from types: a single lexeme can
/// name both a type and a constructor (and will, once the single-variant sugar
/// lands). Keeping these distinct marker namespaces enforces the boundary at
/// the type level.
pub type ConstructorName = NameDef<namespace::Constructor>;

/// Name of a generic type parameter (e.g., `"D"`, `"I"`).
pub type GenericParamName = NameDef<namespace::GenericParam>;

/// Name of a dimension variable in a built-in function signature (e.g., `"D"`).
///
/// Built-in signatures use these variables to relate argument and result
/// dimensions, such as `sqrt: D -> D^(1/2)` or `min: (D, D) -> D`.
pub type DimVarName = NameDef<namespace::DimVar>;

/// Name of a local expression binding (e.g., `"x"`, `"stage_mass"`).
pub type LocalName = NameDef<namespace::Local>;

/// Name of a module alias introduced by an import/include declaration (e.g., `"constants"`, `"std"`).
pub type ModuleAliasName = NameDef<namespace::ModuleAlias>;

/// Name of an open plot/figure/layer property (e.g., `"title"`, `"width"`, `"stroke_width"`).
pub type PlotPropertyName = NameDef<namespace::PlotProperty>;

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

/// A fully resolved index variant reference.
///
/// Index variants are owned by an index declaration rather than directly by a
/// DAG/module. This type therefore resolves the index itself to a canonical
/// owner, then stores the variant as a leaf in that index's variant set.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResolvedIndexVariant {
    index: ResolvedName<namespace::Index>,
    variant: IndexVariantName,
}

impl ResolvedIndexVariant {
    /// Create a resolved index-variant reference from its resolved index and
    /// variant leaf.
    #[must_use]
    pub const fn new(index: ResolvedName<namespace::Index>, variant: IndexVariantName) -> Self {
        Self { index, variant }
    }

    /// The resolved index that owns this variant.
    #[must_use]
    pub const fn index(&self) -> &ResolvedName<namespace::Index> {
        &self.index
    }

    /// The variant leaf inside [`Self::index`].
    #[must_use]
    pub const fn variant(&self) -> &IndexVariantName {
        &self.variant
    }

    /// Consume this value and return its typed parts.
    #[must_use]
    pub fn into_parts(self) -> (ResolvedName<namespace::Index>, IndexVariantName) {
        (self.index, self.variant)
    }
}

impl std::fmt::Debug for ResolvedIndexVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedIndexVariant")
            .field("index", &self.index)
            .field("variant", &self.variant)
            .finish()
    }
}

impl std::fmt::Display for ResolvedIndexVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.index, self.variant)
    }
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

/// A unit reference, optionally qualified by a module alias.
///
/// Unit references follow the same scoping rules as every other imported
/// category: a bare name (`mile`) refers to a local declaration, a selective
/// import, or a prelude unit; a qualified name (`u.mile`) refers to a `pub`
/// unit of the module imported as `u`. The qualifier is at most one module
/// alias — unit references never nest deeper.
///
/// The `Display` impl renders `u.mile` / `mile` for diagnostics and
/// formatting boundaries only; the compiler core matches on the typed parts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitRef {
    /// Module alias qualifying `name`, or `None` for a file-local reference.
    qualifier: Option<ModuleAliasName>,
    /// The unit leaf name inside the qualifier scope.
    name: UnitName,
}

impl UnitRef {
    /// Create an unqualified (file-local, selective-import, or prelude) unit reference.
    #[must_use]
    pub fn local(name: impl Into<UnitName>) -> Self {
        Self {
            qualifier: None,
            name: name.into(),
        }
    }

    /// Create a unit reference qualified by a module alias (`u.mile`).
    #[must_use]
    pub const fn qualified(qualifier: ModuleAliasName, name: UnitName) -> Self {
        Self {
            qualifier: Some(qualifier),
            name,
        }
    }

    /// The module alias qualifying this reference, if any.
    #[must_use]
    pub const fn qualifier(&self) -> Option<&ModuleAliasName> {
        self.qualifier.as_ref()
    }

    /// The unit leaf name.
    #[must_use]
    pub const fn name(&self) -> &UnitName {
        &self.name
    }

    /// Returns whether this reference is module-qualified.
    #[must_use]
    pub const fn is_qualified(&self) -> bool {
        self.qualifier.is_some()
    }
}

impl From<UnitName> for UnitRef {
    /// Wrap a bare unit name as a local reference. Definition sites always
    /// produce local references; qualified forms are constructed explicitly
    /// via [`UnitRef::qualified`].
    fn from(name: UnitName) -> Self {
        Self::local(name)
    }
}

impl From<NameAtom> for UnitRef {
    /// Wrap a bare atom as a local unit reference. This is what
    /// [`crate::syntax::ast::Ident::into_spanned`] uses to lift parser
    /// identifiers into the typed reference.
    fn from(atom: NameAtom) -> Self {
        Self::local(UnitName::from_atom(atom))
    }
}

impl std::fmt::Display for UnitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(qualifier) = &self.qualifier {
            write!(f, "{qualifier}.")?;
        }
        write!(f, "{}", self.name)
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

impl From<IndexName> for NamePath {
    fn from(name: IndexName) -> Self {
        Self::local(name.into_atom())
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
        let decl = DeclName::new("x");
        let index = IndexName::new("x");

        assert_eq!(decl.as_str(), index.as_str());
        assert_eq!(
            DeclName::try_new("module.x"),
            Err(NameAtomError::ContainsDot)
        );
        assert_eq!(
            IndexName::try_new("module.x"),
            Err(NameAtomError::ContainsDot)
        );
    }

    #[test]
    fn resolved_name_carries_canonical_owner_and_leaf() {
        let name = DeclName::new("dry_mass");
        let resolved = ResolvedName::<namespace::Decl>::from_def(
            crate::dag_id::DagId::new_in_package("test", "helpers", ["mass"]),
            name,
        );

        assert_eq!(resolved.owner().to_string(), "helpers.mass");
        assert_eq!(resolved.as_str(), "dry_mass");
        assert_eq!(resolved.to_string(), "helpers.mass.dry_mass");
        assert_eq!(resolved.to_unowned_def_name(), DeclName::new("dry_mass"));
    }

    #[test]
    fn resolved_index_variant_carries_resolved_index_owner() {
        let index = ResolvedName::<namespace::Index>::from_def(
            crate::dag_id::DagId::root_in_package("test", "mission"),
            IndexName::new("Phase"),
        );
        let variant = ResolvedIndexVariant::new(index, IndexVariantName::new("Burn"));

        assert_eq!(variant.index().owner().to_string(), "mission");
        assert_eq!(variant.index().as_str(), "Phase");
        assert_eq!(variant.variant().as_str(), "Burn");
        assert_eq!(variant.to_string(), "mission.Phase.Burn");
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
