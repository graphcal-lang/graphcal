//! Categories of resolved symbols: declaration kinds, selective-import
//! categories, include projections, and the surface kinds diagnostics report.

use crate::syntax::ast::UnitConstness;
use crate::syntax::import_category::ImportItemNamespace;

/// Semantic kind of a value/declaration namespace symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclSymbolKind {
    Const,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
    Dag,
}

impl DeclSymbolKind {
    /// Returns whether this declaration can be referenced from const-like
    /// expression positions.
    #[must_use]
    pub(crate) const fn is_const(self) -> bool {
        matches!(self, Self::Const)
    }
}

impl std::fmt::Display for DeclSymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Const => "const",
            Self::Param => "param",
            Self::Node => "node",
            Self::Assert => "assert",
            Self::Plot => "plot",
            Self::Figure => "figure",
            Self::Layer => "layer",
            Self::Dag => "dag",
        };
        f.write_str(label)
    }
}

/// Semantic category of one exported symbol as seen by import tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportedImportItemKind {
    Decl(DeclSymbolKind),
    Constructor,
    Dimension,
    Unit(UnitConstness),
    Type,
    Index,
}

impl std::fmt::Display for ExportedImportItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decl(kind) => kind.fmt(f),
            Self::Constructor => f.write_str("constructor"),
            Self::Dimension => f.write_str("dimension"),
            Self::Unit(_) => f.write_str("unit"),
            Self::Type => f.write_str("type"),
            Self::Index => f.write_str("index"),
        }
    }
}

/// Semantic target category produced by crossing an include projection boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IncludeProjection {
    StaticDeclaration,
    ConstNode,
    Constructor,
    RuntimeTerm,
    Assertion,
    Visualization,
    StaticUnit,
    RuntimeUnit,
}

/// The single capability table for selective include projection.
///
/// Reusable DAGs are importable blueprints, but they are not members of a
/// configured instance. Every resolver and external-surface check must consume
/// this classification rather than maintaining its own declaration-kind list.
#[must_use]
pub const fn include_projection(kind: ExportedImportItemKind) -> Option<IncludeProjection> {
    match kind {
        ExportedImportItemKind::Decl(DeclSymbolKind::Const) => Some(IncludeProjection::ConstNode),
        ExportedImportItemKind::Decl(DeclSymbolKind::Param | DeclSymbolKind::Node) => {
            Some(IncludeProjection::RuntimeTerm)
        }
        ExportedImportItemKind::Decl(DeclSymbolKind::Assert) => Some(IncludeProjection::Assertion),
        ExportedImportItemKind::Decl(
            DeclSymbolKind::Plot | DeclSymbolKind::Figure | DeclSymbolKind::Layer,
        ) => Some(IncludeProjection::Visualization),
        ExportedImportItemKind::Decl(DeclSymbolKind::Dag) => None,
        ExportedImportItemKind::Constructor => Some(IncludeProjection::Constructor),
        ExportedImportItemKind::Dimension
        | ExportedImportItemKind::Type
        | ExportedImportItemKind::Index => Some(IncludeProjection::StaticDeclaration),
        ExportedImportItemKind::Unit(UnitConstness::Const) => Some(IncludeProjection::StaticUnit),
        ExportedImportItemKind::Unit(UnitConstness::Dynamic) => {
            Some(IncludeProjection::RuntimeUnit)
        }
    }
}

impl ExportedImportItemKind {
    /// Selective-import namespace required for this symbol.
    #[must_use]
    pub const fn namespace(self) -> ImportItemNamespace {
        match self {
            Self::Decl(_) | Self::Constructor => ImportItemNamespace::Term,
            Self::Dimension => ImportItemNamespace::Dimension,
            Self::Unit(_) => ImportItemNamespace::Unit,
            Self::Type => ImportItemNamespace::Type,
            Self::Index => ImportItemNamespace::Index,
        }
    }

    pub(super) const fn sort_rank(self) -> u8 {
        match self.namespace() {
            ImportItemNamespace::Term => 0,
            ImportItemNamespace::Type => 1,
            ImportItemNamespace::Dimension => 2,
            ImportItemNamespace::Unit => 3,
            ImportItemNamespace::Index => 4,
        }
    }
}

/// One of the resolver's per-namespace symbol tables.
///
/// Diagnostics render it with the user-facing category of its declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolTable {
    Decl,
    Constructor,
    Dimension,
    StructType,
    Index,
    Unit,
}

impl std::fmt::Display for SymbolTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Decl => "declaration",
            Self::Constructor => "constructor",
            Self::Dimension => "dimension",
            Self::StructType => "type",
            Self::Index => "index",
            Self::Unit => "unit",
        })
    }
}

/// Surface category for diagnostics that cross namespace boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceNameKind {
    Value,
    Dimension,
    Unit,
    Type,
    Index,
    IndexLabel,
    Constructor,
}

impl std::fmt::Display for SurfaceNameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Value => "a value",
            Self::Dimension => "a dimension",
            Self::Unit => "a unit",
            Self::Type => "a type",
            Self::Index => "an index",
            Self::IndexLabel => "an index label",
            Self::Constructor => "a constructor",
        };
        f.write_str(text)
    }
}
