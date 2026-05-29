//! Declared type of a const/param/node.

use crate::dag_id::DagId;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{NameDef, NameNamespace, ResolvedName, namespace};

use crate::registry::time_scale::TimeScale;
use crate::registry::types::DimensionRegistry;

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

/// Type-level reference to an index definition.
pub type IndexTypeRef = TypeNameRef<namespace::Index>;

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
