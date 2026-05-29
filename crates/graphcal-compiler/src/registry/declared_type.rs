//! Declared type of a const/param/node.

use crate::syntax::dimension::Dimension;
use crate::syntax::names::{NameDef, NameNamespace, ResolvedName, namespace};

use crate::registry::time_scale::TimeScale;
use crate::registry::types::DimensionRegistry;

/// A type-level reference to a named compiler entity.
///
/// The leaf `name` is retained for standalone/legacy registry compatibility
/// and diagnostic rendering. The optional `resolved` identity is populated by
/// module-aware type resolution and must be used by semantic comparisons when
/// both sides carry an owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeNameRef<Ns: NameNamespace> {
    name: NameDef<Ns>,
    resolved: Option<ResolvedName<Ns>>,
}

impl<Ns: NameNamespace> TypeNameRef<Ns> {
    /// Create a type reference from a leaf name plus an optional canonical owner.
    #[must_use]
    pub const fn new(name: NameDef<Ns>, resolved: Option<ResolvedName<Ns>>) -> Self {
        Self { name, resolved }
    }

    /// Create a standalone/legacy reference with no canonical owner.
    #[must_use]
    pub const fn legacy(name: NameDef<Ns>) -> Self {
        Self {
            name,
            resolved: None,
        }
    }

    /// Create a module-aware reference from a canonical resolved name.
    #[must_use]
    pub fn from_resolved(resolved: ResolvedName<Ns>) -> Self {
        Self {
            name: resolved.to_unowned_def_name(),
            resolved: Some(resolved),
        }
    }

    /// The leaf definition name used by legacy standalone registries and diagnostics.
    #[must_use]
    pub const fn name(&self) -> &NameDef<Ns> {
        &self.name
    }

    /// The canonical owner/name identity, when module-aware resolution supplied one.
    #[must_use]
    pub const fn resolved(&self) -> Option<&ResolvedName<Ns>> {
        self.resolved.as_ref()
    }

    /// Compare this reference against an expected leaf plus optional owner.
    ///
    /// If both sides carry canonical owners, the owner-qualified identity is
    /// authoritative. Otherwise this intentionally falls back to the leaf name;
    /// use it only for semantic matching at compatibility boundaries, not as a
    /// `HashMap` key equality relation.
    #[must_use]
    pub fn matches_resolved_or_name(
        &self,
        name: &NameDef<Ns>,
        resolved: Option<&ResolvedName<Ns>>,
    ) -> bool {
        match (self.resolved(), resolved) {
            (Some(actual), Some(expected)) => actual == expected,
            _ => self.name() == name,
        }
    }

    /// Compare this reference against another type reference using semantic
    /// compatibility rules.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        self.matches_resolved_or_name(other.name(), other.resolved())
    }

    /// Borrow the leaf string for diagnostic/display-only formatting.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }

    /// Clone the leaf definition name, explicitly dropping any canonical owner.
    ///
    /// This is a temporary adapter for standalone/legacy APIs that still key by
    /// leaf name. Do not feed the returned value back into module-aware semantic
    /// comparisons.
    #[must_use]
    pub fn to_legacy_name(&self) -> NameDef<Ns> {
        self.name.clone()
    }
}

impl<Ns: NameNamespace> From<NameDef<Ns>> for TypeNameRef<Ns> {
    fn from(name: NameDef<Ns>) -> Self {
        Self::legacy(name)
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
