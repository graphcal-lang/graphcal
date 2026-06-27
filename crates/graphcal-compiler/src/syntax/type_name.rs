//! Names owned by Graphcal's type and constructor syntax.

use crate::syntax::names::{NameDef, NameNamespace, ResolvedName};

/// Struct/tagged-union type namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StructTypeNameNamespace {}

impl NameNamespace for StructTypeNameNamespace {
    const DISPLAY_NAME: &'static str = "StructTypeName";
}

/// Struct/constructor field namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldNameNamespace {}

impl NameNamespace for FieldNameNamespace {
    const DISPLAY_NAME: &'static str = "FieldName";
}

/// Tagged-union constructor namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConstructorNameNamespace {}

impl NameNamespace for ConstructorNameNamespace {
    const DISPLAY_NAME: &'static str = "ConstructorName";
}

/// Generic parameter namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GenericParamNameNamespace {}

impl NameNamespace for GenericParamNameNamespace {
    const DISPLAY_NAME: &'static str = "GenericParamName";
}

/// Name of a struct type (e.g., `"TransferResult"`).
pub type StructTypeName = NameDef<StructTypeNameNamespace>;

/// Module-resolved struct/tagged-union type name.
pub type ResolvedStructTypeName = ResolvedName<StructTypeNameNamespace>;

/// Name of a struct or constructor field (e.g., `"dv1"`, `"altitude"`).
pub type FieldName = NameDef<FieldNameNamespace>;

/// Name of a tagged-union constructor (e.g., `"LowThrust"`, `"Coast"`).
///
/// Constructors live in a separate namespace from types: a single lexeme can
/// name both a type and a constructor. Keeping these distinct marker namespaces
/// enforces the boundary at the type level.
pub type ConstructorName = NameDef<ConstructorNameNamespace>;

/// Module-resolved tagged-union constructor name.
pub type ResolvedConstructorName = ResolvedName<ConstructorNameNamespace>;

/// Name of a generic type parameter (e.g., `"D"`, `"I"`).
pub type GenericParamName = NameDef<GenericParamNameNamespace>;
