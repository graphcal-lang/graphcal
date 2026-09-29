//! Declared type of a const/param/node.

use crate::dag_id::DagId;
use crate::dimension::Dimension;
use crate::resolved_name::{ResolvedIndexName, ResolvedName};
use crate::syntax::index_name::{IndexName, IndexNameNamespace};
use crate::syntax::names::{NameDef, NameNamespace};
use crate::syntax::type_name::StructTypeNameNamespace;

use crate::nat::NatPolyForm;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{DimensionFormattingRegistry, FiniteIndex, IndexCardinalityError};

/// A type-level reference to a named compiler entity.
///
/// Every semantic type reference has a canonical owner. Leaf-only names belong
/// at syntax/display boundaries; once a value crosses into the functional core,
/// it must carry a [`ResolvedName`].
#[derive(Debug, Clone)]
pub struct TypeNameRef<Ns: NameNamespace> {
    name: NameDef<Ns>,
    resolved: ResolvedName<Ns>,
}

impl<Ns: NameNamespace> TypeNameRef<Ns> {
    /// Create a module-aware reference from a canonical resolved name.
    #[must_use]
    pub fn from_resolved(resolved: ResolvedName<Ns>) -> Self {
        Self {
            name: resolved.to_unowned_def_name(),
            resolved,
        }
    }

    /// Create a reference with a display leaf that differs from the canonical
    /// owner-qualified identity.
    ///
    /// This is used at value-display boundaries such as tagged-union
    /// constructor values, where the semantic type is the owning union but the
    /// rendered value should show the constructor leaf.
    #[must_use]
    pub const fn with_display_leaf(name: NameDef<Ns>, resolved: ResolvedName<Ns>) -> Self {
        Self { name, resolved }
    }

    /// Resolve a definition-site leaf into the given owner.
    #[must_use]
    pub fn with_owner(owner: DagId, name: NameDef<Ns>) -> Self {
        Self::from_resolved(ResolvedName::from_def(owner, name))
    }

    /// The leaf definition name used by registries and diagnostics.
    #[must_use]
    pub const fn name(&self) -> &NameDef<Ns> {
        &self.name
    }

    /// The canonical owner/name identity.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Compare this reference against another type reference by owner-qualified identity.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        self.resolved() == other.resolved()
    }

    /// Borrow the leaf string for diagnostic/display-only formatting.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }

    /// Clone the leaf definition name for diagnostic/display boundaries.
    #[must_use]
    fn to_unowned_name(&self) -> NameDef<Ns> {
        self.name.clone()
    }
}

impl<Ns: NameNamespace> PartialEq for TypeNameRef<Ns> {
    fn eq(&self, other: &Self) -> bool {
        self.resolved == other.resolved
    }
}

impl<Ns: NameNamespace> Eq for TypeNameRef<Ns> {}

impl<Ns: NameNamespace> std::hash::Hash for TypeNameRef<Ns> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.resolved.hash(state);
    }
}

impl<Ns: NameNamespace> From<ResolvedName<Ns>> for TypeNameRef<Ns> {
    fn from(resolved: ResolvedName<Ns>) -> Self {
        Self::from_resolved(resolved)
    }
}

impl<Ns: NameNamespace> std::fmt::Display for TypeNameRef<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

/// Type-level reference to a compiler-generated structural `Fin(N)` index.
///
/// The representation is private and canonical: a constant cardinality is
/// always a validated concrete identity, and only a genuinely symbolic Nat
/// form stays symbolic. Equality is therefore structural identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FiniteIndexRef(FiniteIndexRepr);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FiniteIndexRepr {
    Concrete(FiniteIndex),
    Symbolic(NatPolyForm),
}

impl FiniteIndexRef {
    /// Reference a validated concrete structural identity.
    #[must_use]
    pub const fn concrete(index: FiniteIndex) -> Self {
        Self(FiniteIndexRepr::Concrete(index))
    }

    /// Create a finite structural reference from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a constant that is not a valid
    /// finite cardinality.
    pub(crate) fn from_form(form: NatPolyForm) -> Result<Self, IndexCardinalityError> {
        if form.is_constant() {
            FiniteIndex::try_from_u64(form.constant()).map(Self::concrete)
        } else {
            Ok(Self(FiniteIndexRepr::Symbolic(form)))
        }
    }

    /// Return the concrete finite structural identity, if this reference is concrete.
    #[must_use]
    pub(crate) const fn concrete_index(&self) -> Option<FiniteIndex> {
        match &self.0 {
            FiniteIndexRepr::Concrete(index) => Some(*index),
            FiniteIndexRepr::Symbolic(_) => None,
        }
    }

    /// Return the normalized Nat form for this reference.
    #[must_use]
    pub(crate) fn form(&self) -> NatPolyForm {
        match &self.0 {
            FiniteIndexRepr::Concrete(index) => NatPolyForm::from_constant(index.size_u64()),
            FiniteIndexRepr::Symbolic(form) => form.clone(),
        }
    }
}

/// Renders the source spelling `Fin(N)` or `Fin(<Nat expression>)`.
impl std::fmt::Display for FiniteIndexRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        IndexDisplayName::Finite(self.form()).fmt(f)
    }
}

/// Source-facing spelling of an index for diagnostics and value display.
///
/// Declared indexes render by their definition-site leaf name; structural
/// axes keep their Nat cardinality form and render as `Fin(...)` only through
/// `Display`, never as a fabricated [`IndexName`]. The form is display-only
/// and may describe a cardinality that failed validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexDisplayName {
    Declared(IndexName),
    Finite(NatPolyForm),
}

impl IndexDisplayName {
    /// The declared leaf name, when this spells a declared index.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&IndexName> {
        match self {
            Self::Declared(name) => Some(name),
            Self::Finite(_) => None,
        }
    }
}

impl From<IndexName> for IndexDisplayName {
    fn from(name: IndexName) -> Self {
        Self::Declared(name)
    }
}

impl std::fmt::Display for IndexDisplayName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(name) => name.fmt(f),
            Self::Finite(form) => write!(f, "Fin({})", form.format()),
        }
    }
}

/// Type-level reference to an index definition.
///
/// Declared indexes are owner-qualified names. Compiler-generated `Fin(N)`
/// axes are typed structural identities and have no declared resolved names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexTypeRef {
    Declared(TypeNameRef<IndexNameNamespace>),
    Finite(FiniteIndexRef),
}

impl IndexTypeRef {
    /// Create a module-aware reference from a canonical resolved declared-index name.
    #[must_use]
    pub fn from_resolved(resolved: ResolvedIndexName) -> Self {
        Self::Declared(TypeNameRef::from_resolved(resolved))
    }

    /// Resolve a declared-index definition-site leaf into the given owner.
    #[must_use]
    pub fn with_owner(owner: DagId, name: IndexName) -> Self {
        Self::Declared(TypeNameRef::with_owner(owner, name))
    }

    /// Create a reference with a display leaf that differs from the canonical
    /// owner-qualified identity.
    #[must_use]
    pub const fn with_display_leaf(name: IndexName, resolved: ResolvedIndexName) -> Self {
        Self::Declared(TypeNameRef::with_display_leaf(name, resolved))
    }

    /// Create a concrete compiler-generated finite structural reference.
    #[must_use]
    pub const fn from_finite_index(index: FiniteIndex) -> Self {
        Self::Finite(FiniteIndexRef::concrete(index))
    }

    /// Create a finite structural reference from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid finite structural size.
    pub(crate) fn from_finite_index_form(form: NatPolyForm) -> Result<Self, IndexCardinalityError> {
        FiniteIndexRef::from_form(form).map(Self::Finite)
    }

    /// Wrap an already validated finite structural reference.
    #[must_use]
    pub const fn from_finite_index_ref(reference: FiniteIndexRef) -> Self {
        Self::Finite(reference)
    }

    /// The declared leaf name, when this is a declared index.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&IndexName> {
        match self {
            Self::Declared(reference) => Some(reference.name()),
            Self::Finite(_) => None,
        }
    }

    /// The canonical declared owner/name identity, when this is a declared index.
    #[must_use]
    pub const fn declared_resolved(&self) -> Option<&ResolvedIndexName> {
        match self {
            Self::Declared(reference) => Some(reference.resolved()),
            Self::Finite(_) => None,
        }
    }

    /// Return the finite structural reference, when this is a compiler-generated finite structural.
    #[must_use]
    pub(crate) const fn finite_index_ref(&self) -> Option<&FiniteIndexRef> {
        match self {
            Self::Declared(_) => None,
            Self::Finite(reference) => Some(reference),
        }
    }

    /// Return the typed concrete finite structural identity, if this reference has one.
    #[must_use]
    pub const fn finite_index(&self) -> Option<FiniteIndex> {
        match self {
            Self::Finite(reference) => reference.concrete_index(),
            Self::Declared(_) => None,
        }
    }

    /// Return the normalized Nat form, when this is a finite structural reference.
    #[must_use]
    pub(crate) fn finite_index_form(&self) -> Option<NatPolyForm> {
        self.finite_index_ref().map(FiniteIndexRef::form)
    }

    /// The display-only spelling for diagnostics and formatting.
    #[must_use]
    pub fn display_name(&self) -> IndexDisplayName {
        match self {
            Self::Declared(reference) => IndexDisplayName::Declared(reference.to_unowned_name()),
            Self::Finite(reference) => IndexDisplayName::Finite(reference.form()),
        }
    }

    /// Compare this reference against another type reference.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Declared(lhs), Self::Declared(rhs)) => lhs.matches_ref(rhs),
            (Self::Finite(lhs), Self::Finite(rhs)) => lhs == rhs,
            _ => false,
        }
    }
}

impl From<ResolvedIndexName> for IndexTypeRef {
    fn from(resolved: ResolvedIndexName) -> Self {
        Self::from_resolved(resolved)
    }
}

impl std::fmt::Display for IndexTypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(reference) => reference.fmt(f),
            Self::Finite(reference) => reference.fmt(f),
        }
    }
}

/// Type-level reference to a struct/tagged-union definition.
pub type StructTypeRef = TypeNameRef<StructTypeNameNamespace>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic_param::test_support::type_param;
    use crate::syntax::type_name::StructTypeName;

    fn symbolic_n_plus_one() -> NatPolyForm {
        NatPolyForm::from_var(type_param("N"))
            .add(&NatPolyForm::from_constant(1))
            .unwrap()
    }

    #[test]
    fn finite_index_ref_is_canonical_for_constant_forms() {
        let three = FiniteIndex::try_from_u64(3).unwrap();
        let from_form = FiniteIndexRef::from_form(NatPolyForm::from_constant(3)).unwrap();
        assert_eq!(from_form, FiniteIndexRef::concrete(three));
        assert_eq!(from_form.concrete_index(), Some(three));
        assert_eq!(from_form.form(), NatPolyForm::from_constant(3));
        assert_eq!(from_form.to_string(), "Fin(3)");
        assert_eq!(
            FiniteIndexRef::from_form(NatPolyForm::from_constant(0)),
            Err(IndexCardinalityError::Empty)
        );
    }

    #[test]
    fn finite_index_ref_keeps_symbolic_forms() {
        let symbolic = FiniteIndexRef::from_form(symbolic_n_plus_one()).unwrap();
        assert_eq!(symbolic.concrete_index(), None);
        assert_eq!(symbolic.form(), symbolic_n_plus_one());
        assert_eq!(symbolic.to_string(), "Fin(N + 1)");
        assert_ne!(
            symbolic,
            FiniteIndexRef::concrete(FiniteIndex::try_from_u64(1).unwrap())
        );
    }

    #[test]
    fn index_display_names_render_declared_leaves_and_finite_forms() {
        let phase = IndexName::expect_valid("Phase");
        let declared = IndexDisplayName::from(phase.clone());
        assert_eq!(declared.declared_name(), Some(&phase));
        assert_eq!(declared.to_string(), "Phase");

        let finite = IndexDisplayName::Finite(symbolic_n_plus_one());
        assert_eq!(finite.declared_name(), None);
        assert_eq!(finite.to_string(), "Fin(N + 1)");
        // Display-only: an invalid cardinality still renders.
        assert_eq!(
            IndexDisplayName::Finite(NatPolyForm::from_constant(0)).to_string(),
            "Fin(0)"
        );
    }

    #[test]
    fn index_type_ref_display_matches_its_display_name() {
        let owner = DagId::root_in_package("test", "main");
        let declared = IndexTypeRef::with_owner(owner, IndexName::expect_valid("Phase"));
        assert_eq!(declared.to_string(), "Phase");
        assert_eq!(
            declared.display_name(),
            IndexDisplayName::Declared(IndexName::expect_valid("Phase"))
        );

        let finite = IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(4).unwrap());
        assert_eq!(finite.to_string(), "Fin(4)");
        assert_eq!(
            finite.display_name(),
            IndexDisplayName::Finite(NatPolyForm::from_constant(4))
        );
        assert!(finite.matches_ref(
            &IndexTypeRef::from_finite_index_form(NatPolyForm::from_constant(4)).unwrap()
        ));
        assert!(!finite.matches_ref(&declared));
    }

    #[test]
    fn type_name_ref_equality_uses_canonical_identity_not_display_leaf() {
        let owner = DagId::root_in_package("test", "main");
        let resolved = ResolvedName::from_def(owner, StructTypeName::expect_valid("Result"));
        let canonical = StructTypeRef::from_resolved(resolved.clone());
        let display_variant =
            StructTypeRef::with_display_leaf(StructTypeName::expect_valid("Success"), resolved);

        assert_eq!(canonical, display_variant);
        let mut set = std::collections::HashSet::new();
        set.insert(canonical);
        assert!(set.contains(&display_variant));
    }
}

/// A concrete generic argument classified by its declared sort.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeclaredGenericArg {
    Dim(Dimension),
    Index(IndexTypeRef),
    Nat(NatPolyForm),
    Type(DeclaredType),
}

#[derive(Clone, Copy)]
enum DiagnosticNameQualification {
    Leaf,
    OwnerQualified,
}

impl DiagnosticNameQualification {
    fn dimension(self, dimension: &Dimension, dims: &DimensionFormattingRegistry) -> String {
        match self {
            Self::Leaf => dims.format_dimension(dimension),
            Self::OwnerQualified => dimension.owner_qualified().to_string(),
        }
    }

    fn index(self, index: &IndexTypeRef) -> String {
        match (self, index.declared_resolved()) {
            (Self::OwnerQualified, Some(resolved)) => resolved.to_string(),
            (Self::Leaf | Self::OwnerQualified, None) | (Self::Leaf, Some(_)) => index.to_string(),
        }
    }

    fn struct_type(self, name: &StructTypeRef) -> String {
        match self {
            Self::Leaf => name.to_string(),
            Self::OwnerQualified => name.resolved().to_string(),
        }
    }
}

impl DeclaredGenericArg {
    fn format(
        &self,
        dims: &DimensionFormattingRegistry,
        qualification: DiagnosticNameQualification,
    ) -> String {
        match self {
            Self::Dim(dimension) => qualification.dimension(dimension, dims),
            Self::Index(index) => qualification.index(index),
            Self::Nat(value) => value.format(),
            Self::Type(type_expr) => type_expr.format_with(dims, qualification),
        }
    }
}

/// A concrete declared type: primitive, struct, or indexed value type.
/// Index arguments are never types; generic struct metadata carries them as
/// [`DeclaredGenericArg::Index`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeclaredType {
    Quantity(Dimension),
    /// A complex quantity whose real and imaginary components share one dimension.
    Complex(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale. `Datetime(UTC)` is the default for civil use.
    Datetime(TimeScale),
    /// An index-key value type `Key<I>`: element keys of axis `I`.
    Key(IndexTypeRef),
    /// A struct type, optionally with concrete sorted generic arguments.
    Struct(StructTypeRef, Vec<DeclaredGenericArg>),
    Indexed {
        element: Box<Self>,
        index: IndexTypeRef,
    },
}

impl DeclaredType {
    /// Format as a human-readable string for diagnostics (e.g. `"Length / Time"`, `"Bool"`).
    #[must_use]
    pub fn format(&self, dims: &DimensionFormattingRegistry) -> String {
        self.format_with(dims, DiagnosticNameQualification::Leaf)
    }

    /// Format every nominal identity with its canonical owner.
    ///
    /// Used only when two unequal semantic types would otherwise render with
    /// the same leaf-only diagnostic spelling.
    #[must_use]
    pub(crate) fn format_owner_qualified(&self, dims: &DimensionFormattingRegistry) -> String {
        self.format_with(dims, DiagnosticNameQualification::OwnerQualified)
    }

    fn format_with(
        &self,
        dims: &DimensionFormattingRegistry,
        qualification: DiagnosticNameQualification,
    ) -> String {
        match self {
            Self::Quantity(d) => qualification.dimension(d, dims),
            Self::Complex(d) => format!("Complex<{}>", qualification.dimension(d, dims)),
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Key(index) => format!("Key<{}>", qualification.index(index)),
            Self::Struct(name, args) => {
                let name = qualification.struct_type(name);
                if args.is_empty() {
                    name
                } else {
                    let args_str: Vec<String> = args
                        .iter()
                        .map(|arg| arg.format(dims, qualification))
                        .collect();
                    format!("{name}<{}>", args_str.join(", "))
                }
            }
            Self::Indexed { .. } => {
                // Multi-axis source syntax uses one bracket list (`T[I, J]`),
                // while the concrete type stores one outer-to-inner layer per
                // axis. Flatten those layers at the diagnostic boundary rather
                // than exposing the invalid internal spelling `T[J][I]`.
                let mut indexes = Vec::new();
                let mut base = self;
                while let Self::Indexed { element, index } = base {
                    indexes.push(qualification.index(index));
                    base = element;
                }
                format!(
                    "{}[{}]",
                    base.format_with(dims, qualification),
                    indexes.join(", ")
                )
            }
        }
    }
}
