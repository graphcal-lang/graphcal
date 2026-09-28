//! Syntax-level dimension and unit names.

use crate::syntax::names::{NameAtom, NameDef, NameNamespace, Qualified};

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

/// Name of a unit (e.g., `"m"`, `"km"`, `"h"`).
pub type UnitName = NameDef<UnitNameNamespace>;

/// Name of a dimension variable in a built-in function signature (e.g., `"D"`).
///
/// Built-in signatures use these variables to relate argument and result
/// dimensions, such as `sqrt: D -> D^(1/2)` or `least: (D, D) -> D`.
pub type DimVarName = NameDef<DimVarNameNamespace>;

/// A Unit name selected locally or after a dotted DAG path and `::`.
pub type UnitRef = Qualified<NameAtom, UnitName>;

/// A Dimension name selected locally or after a dotted DAG path and `::`.
///
/// Mirrors [`UnitRef`]: flat per-file registries key source-visible dimension
/// names by this reference so that `a::Rate` and a local or imported `Rate`
/// remain distinct entries instead of colliding on the leaf spelling.
pub type DimRef = Qualified<NameAtom, DimName>;
