//! Declared type of a const/param/node.

use crate::dag_id::DagId;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{NameDef, NameNamespace, ResolvedName, namespace};

use crate::registry::time_scale::TimeScale;
use crate::registry::types::{DimensionRegistry, NatRangeIndex, NatRangeIndexError};
use crate::syntax::nat::NatPolyForm;

/// A type-level reference to a named compiler entity.
///
/// Every semantic type reference has a canonical owner. Leaf-only names belong
/// at syntax/display boundaries; once a value crosses into the functional core,
/// it must carry a [`ResolvedName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    pub fn to_unowned_name(&self) -> NameDef<Ns> {
        self.name.clone()
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

/// Type-level reference to a compiler-generated Nat range index.
///
/// Concrete Nat ranges carry a validated in-memory size. Symbolic Nat ranges
/// carry the normalized type-level arithmetic form directly; no fake resolved
/// name or parseable display string is used as semantic identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NatRangeIndexRef {
    Concrete(NatRangeIndex),
    Symbolic(NatPolyForm),
}

impl NatRangeIndexRef {
    /// Create a Nat range reference from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid Nat range size.
    pub fn from_form(form: NatPolyForm) -> Result<Self, NatRangeIndexError> {
        if form.is_constant() {
            NatRangeIndex::try_from_u64(form.constant()).map(Self::Concrete)
        } else {
            Ok(Self::Symbolic(form))
        }
    }

    /// Return the concrete Nat range identity, if this reference is concrete.
    #[must_use]
    pub const fn concrete_index(&self) -> Option<NatRangeIndex> {
        match self {
            Self::Concrete(index) => Some(*index),
            Self::Symbolic(_) => None,
        }
    }

    /// Return the normalized Nat form for this reference.
    #[must_use]
    pub fn form(&self) -> NatPolyForm {
        match self {
            Self::Concrete(index) => NatPolyForm::from_constant(index.size_u64()),
            Self::Symbolic(form) => form.clone(),
        }
    }

    /// Render this Nat range as a source-like display name for diagnostics.
    #[must_use]
    pub fn display_name(&self) -> NameDef<namespace::Index> {
        match self {
            Self::Concrete(index) => index.display_name(),
            Self::Symbolic(form) => NameDef::new(format!("range({})", form.format())),
        }
    }

    /// Compare Nat range references by typed identity.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Concrete(lhs), Self::Concrete(rhs)) => lhs == rhs,
            (Self::Symbolic(lhs), Self::Symbolic(rhs)) => lhs == rhs,
            _ => false,
        }
    }
}

/// Type-level reference to an index definition.
///
/// Declared indexes are owner-qualified names. Compiler-generated `range(N)`
/// axes are typed Nat-range identities and do not have declared resolved names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexTypeRef {
    Declared(TypeNameRef<namespace::Index>),
    NatRange(NatRangeIndexRef),
}

impl IndexTypeRef {
    /// Create a module-aware reference from a canonical resolved declared-index name.
    #[must_use]
    pub fn from_resolved(resolved: ResolvedName<namespace::Index>) -> Self {
        Self::Declared(TypeNameRef::from_resolved(resolved))
    }

    /// Resolve a declared-index definition-site leaf into the given owner.
    #[must_use]
    pub fn with_owner(owner: DagId, name: NameDef<namespace::Index>) -> Self {
        Self::Declared(TypeNameRef::with_owner(owner, name))
    }

    /// Create a reference with a display leaf that differs from the canonical
    /// owner-qualified identity.
    #[must_use]
    pub const fn with_display_leaf(
        name: NameDef<namespace::Index>,
        resolved: ResolvedName<namespace::Index>,
    ) -> Self {
        Self::Declared(TypeNameRef::with_display_leaf(name, resolved))
    }

    /// Create a concrete compiler-generated Nat range reference.
    #[must_use]
    pub const fn from_nat_range(index: NatRangeIndex) -> Self {
        Self::NatRange(NatRangeIndexRef::Concrete(index))
    }

    /// Create a Nat range reference from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid Nat range size.
    pub fn from_nat_range_form(form: NatPolyForm) -> Result<Self, NatRangeIndexError> {
        NatRangeIndexRef::from_form(form).map(Self::NatRange)
    }

    /// Wrap an already validated Nat range reference.
    #[must_use]
    pub const fn from_nat_range_ref(reference: NatRangeIndexRef) -> Self {
        Self::NatRange(reference)
    }

    /// The declared leaf name, when this is a declared index.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&NameDef<namespace::Index>> {
        match self {
            Self::Declared(reference) => Some(reference.name()),
            Self::NatRange(_) => None,
        }
    }

    /// The canonical declared owner/name identity, when this is a declared index.
    #[must_use]
    pub const fn declared_resolved(&self) -> Option<&ResolvedName<namespace::Index>> {
        match self {
            Self::Declared(reference) => Some(reference.resolved()),
            Self::NatRange(_) => None,
        }
    }

    /// Return the Nat range reference, when this is a compiler-generated Nat range.
    #[must_use]
    pub const fn nat_range_ref(&self) -> Option<&NatRangeIndexRef> {
        match self {
            Self::Declared(_) => None,
            Self::NatRange(reference) => Some(reference),
        }
    }

    /// Return the typed concrete Nat range identity, if this reference has one.
    #[must_use]
    pub const fn nat_range(&self) -> Option<NatRangeIndex> {
        match self {
            Self::NatRange(reference) => reference.concrete_index(),
            Self::Declared(_) => None,
        }
    }

    /// Return the normalized Nat form, when this is a Nat range reference.
    #[must_use]
    pub fn nat_range_form(&self) -> Option<NatPolyForm> {
        self.nat_range_ref().map(NatRangeIndexRef::form)
    }

    /// Render a display-only leaf name for diagnostics and formatting.
    #[must_use]
    pub fn display_name(&self) -> NameDef<namespace::Index> {
        match self {
            Self::Declared(reference) => reference.to_unowned_name(),
            Self::NatRange(reference) => reference.display_name(),
        }
    }

    /// Compare this reference against another type reference.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Declared(lhs), Self::Declared(rhs)) => lhs.matches_ref(rhs),
            (Self::NatRange(lhs), Self::NatRange(rhs)) => lhs.matches_ref(rhs),
            _ => false,
        }
    }

    /// Clone the leaf definition/display name for diagnostic/display boundaries.
    #[must_use]
    pub fn to_unowned_name(&self) -> NameDef<namespace::Index> {
        self.display_name()
    }
}

impl From<ResolvedName<namespace::Index>> for IndexTypeRef {
    fn from(resolved: ResolvedName<namespace::Index>) -> Self {
        Self::from_resolved(resolved)
    }
}

impl std::fmt::Display for IndexTypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.display_name().fmt(f)
    }
}

/// Type-level reference to a struct/tagged-union definition.
pub type StructTypeRef = TypeNameRef<namespace::StructType>;

/// The declared type of a const/param/node: either a scalar with a dimension, a bool, or a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale. `Datetime(UTC)` is the default for civil use.
    Datetime(TimeScale),
    /// A label of a named index (e.g., `Maneuver.Departure` has type `Label(Maneuver)`).
    Label(IndexTypeRef),
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeRef, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexTypeRef,
    },
}

impl DeclaredType {
    /// Format as a human-readable string for diagnostics (e.g. `"Length / Time"`, `"Bool"`).
    #[must_use]
    pub fn format(&self, dims: &DimensionRegistry) -> String {
        match self {
            Self::Scalar(d) => dims.format_dimension(d),
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Label(index) => format!("Label({index})"),
            Self::Struct(name, args) => {
                if args.is_empty() {
                    name.to_string()
                } else {
                    let args_str: Vec<String> = args.iter().map(|a| a.format(dims)).collect();
                    format!("{name}<{}>", args_str.join(", "))
                }
            }
            Self::Indexed { element, index } => {
                format!("{}[{index}]", element.format(dims))
            }
        }
    }
}
