//! Syntax-level dimension and unit names.

use std::fmt;

use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace, ResolvedName};

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

/// Name of a unit (e.g., `"m"`, `"km"`, `"hour"`).
pub type UnitName = NameDef<UnitNameNamespace>;

/// Module-resolved unit name.
pub type ResolvedUnitName = ResolvedName<UnitNameNamespace>;

/// Name of a dimension variable in a built-in function signature (e.g., `"D"`).
///
/// Built-in signatures use these variables to relate argument and result
/// dimensions, such as `sqrt: D -> D^(1/2)` or `min: (D, D) -> D`.
pub type DimVarName = NameDef<DimVarNameNamespace>;

/// A unit reference, optionally qualified by a module alias.
///
/// Unit references follow the same scoping rules as every other imported
/// category: a bare name (`mile`) refers to a local declaration, a selective
/// import, or a prelude unit; a qualified name (`u.mile`) refers to a `pub`
/// unit of the module imported as `u`. The qualifier is at most one module
/// alias — unit references never nest deeper.
///
/// The `Display` impl renders `u.mile` / `mile` for diagnostics and formatting
/// boundaries only; the compiler core matches on the typed parts.
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

impl fmt::Display for UnitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(qualifier) = &self.qualifier {
            write!(f, "{qualifier}.")?;
        }
        write!(f, "{}", self.name)
    }
}
