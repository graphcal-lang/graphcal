//! Syntax-level dimension and unit names.

use std::fmt;

use crate::syntax::names::{
    NameAtom, NameDef, NameNamespace, NamePath, NamespacePath, ResolvedName,
};

/// Dimension namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DimNameNamespace {}

impl NameNamespace for DimNameNamespace {
    const DISPLAY_NAME: &'static str = "DimName";
}

/// Unit namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UnitNameNamespace {}

impl NameNamespace for UnitNameNamespace {
    const DISPLAY_NAME: &'static str = "UnitName";
}

/// Built-in dimension-variable namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DimVarNameNamespace {}

impl NameNamespace for DimVarNameNamespace {
    const DISPLAY_NAME: &'static str = "DimVarName";
}

/// Name of a dimension (e.g., `"Length"`, `"Velocity"`).
pub type DimName = NameDef<DimNameNamespace>;

/// Module-resolved dimension name.
pub type ResolvedDimName = ResolvedName<DimNameNamespace>;

/// Name of a unit (e.g., `"m"`, `"km"`, `"h"`).
pub type UnitName = NameDef<UnitNameNamespace>;

/// Module-resolved unit name.
pub type ResolvedUnitName = ResolvedName<UnitNameNamespace>;

/// Name of a dimension variable in a built-in function signature (e.g., `"D"`).
///
/// Built-in signatures use these variables to relate argument and result
/// dimensions, such as `sqrt: D -> D^(1/2)` or `least: (D, D) -> D`.
pub type DimVarName = NameDef<DimVarNameNamespace>;

/// A Unit name selected locally or after a dotted DAG path and `::`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitRef {
    owner: Option<NamespacePath>,
    name: UnitName,
}

impl UnitRef {
    /// Create an unqualified local/selective/prelude Unit reference.
    #[must_use]
    pub fn local(name: impl Into<UnitName>) -> Self {
        Self {
            owner: None,
            name: name.into(),
        }
    }

    /// Create a Unit member selected after `::`.
    #[must_use]
    pub const fn qualified(owner: NamespacePath, name: UnitName) -> Self {
        Self {
            owner: Some(owner),
            name,
        }
    }

    /// Construct from the namespace-neutral syntax reference parser.
    #[must_use]
    pub fn from_name_path(path: NamePath) -> Self {
        let (owner, name) = path.into_parts();
        Self {
            owner,
            name: UnitName::from_atom(name),
        }
    }

    /// The dotted DAG/module owner before `::`, if any.
    #[must_use]
    pub const fn qualifier(&self) -> Option<&NamespacePath> {
        self.owner.as_ref()
    }

    /// Convert to the namespace-neutral path consumed by the resolver.
    #[must_use]
    pub(crate) fn to_name_path(&self) -> NamePath {
        self.owner.as_ref().map_or_else(
            || NamePath::local(self.name.atom().clone()),
            |owner| NamePath::member(owner.clone(), self.name.atom().clone()),
        )
    }

    /// The unit leaf name.
    #[must_use]
    pub const fn name(&self) -> &UnitName {
        &self.name
    }

    /// Returns whether this reference is module-qualified.
    #[must_use]
    pub const fn is_qualified(&self) -> bool {
        self.owner.is_some()
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
    /// `crate::syntax::ast::Ident::into_spanned` uses to lift parser
    /// identifiers into the typed reference.
    fn from(atom: NameAtom) -> Self {
        Self::local(UnitName::from_atom(atom))
    }
}

impl fmt::Display for UnitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(owner) = &self.owner {
            write!(f, "{owner}::")?;
        }
        write!(f, "{}", self.name)
    }
}
