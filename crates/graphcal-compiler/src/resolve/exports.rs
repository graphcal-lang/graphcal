//! Public module surface: exported spellings paired with canonical targets.

use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::ast::UnitConstness;
use crate::syntax::names::NameAtom;

use super::category::{DeclSymbolKind, ExportedImportItemKind};

/// Canonical semantic target of one public module-surface spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExportedBindingTarget {
    Decl {
        identity: ResolvedDeclName,
        kind: DeclSymbolKind,
    },
    Constructor(ResolvedConstructorName),
    Dimension(ResolvedDimName),
    Unit {
        identity: ResolvedUnitName,
        constness: UnitConstness,
    },
    Type(ResolvedStructTypeName),
    Index(ResolvedIndexName),
}

impl ExportedBindingTarget {
    /// Exact selective-import category of this canonical target.
    #[must_use]
    pub const fn kind(&self) -> ExportedImportItemKind {
        match self {
            Self::Decl { kind, .. } => ExportedImportItemKind::Decl(*kind),
            Self::Constructor(_) => ExportedImportItemKind::Constructor,
            Self::Dimension(_) => ExportedImportItemKind::Dimension,
            Self::Unit { constness, .. } => ExportedImportItemKind::Unit(*constness),
            Self::Type(_) => ExportedImportItemKind::Type,
            Self::Index(_) => ExportedImportItemKind::Index,
        }
    }

    /// Canonical declaration identity when this target inhabits the Term declaration namespace.
    #[must_use]
    pub const fn declaration(&self) -> Option<&ResolvedDeclName> {
        match self {
            Self::Decl { identity, .. } => Some(identity),
            _ => None,
        }
    }
}

/// One source-visible public spelling paired atomically with category and canonical target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExportedBinding {
    pub name: NameAtom,
    pub target: ExportedBindingTarget,
}

/// One public symbol rendered in the exact category used by selective imports.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExportedImportItem {
    pub name: NameAtom,
    pub kind: ExportedImportItemKind,
}

impl ExportedImportItem {
    /// Canonical source spelling that can be pasted into an import brace list.
    #[must_use]
    pub fn render(&self) -> String {
        self.kind.namespace().render_item(self.name.as_str())
    }
}
