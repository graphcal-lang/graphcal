//! Errors produced while building or using module-aware symbol tables.

use thiserror::Error;

use crate::dag_id::DagId;
use crate::resolved_name::{ResolvedDeclName, ResolvedIndexName, ResolvedStructTypeName};
use crate::syntax::import_category::ImportItemCategoryMismatch;
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::span::Span;

use super::category::{DeclSymbolKind, ExportedImportItemKind, SurfaceNameKind};

/// Errors produced while building or using module-aware symbol tables.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModuleResolveError {
    /// A module was added twice.
    #[error("duplicate module `{owner}`")]
    DuplicateModule { owner: DagId },
    /// No symbol table exists for a canonical module identity.
    #[error("unknown module `{owner}`")]
    UnknownModule { owner: DagId },
    /// A module qualifier's first segment is not an alias in the current module.
    #[error("module alias `{alias}` is not in scope of `{owner}`")]
    UnknownModuleAlias {
        owner: DagId,
        alias: ModuleAliasName,
    },
    /// A call path is ambiguous between a local DAG and an imported module alias.
    #[error("DAG name `{name}` is ambiguous in `{owner}`")]
    AmbiguousCallableModule {
        owner: DagId,
        name: ModuleAliasName,
        targets: Vec<DagId>,
    },
    /// An include alias denotes an existing instance, not a reusable DAG blueprint.
    #[error("included instance `{alias}` is not callable in `{owner}`")]
    IncludedInstanceNotCallable {
        owner: DagId,
        alias: ModuleAliasName,
    },
    /// Duplicate definition in one namespace.
    #[error("duplicate {namespace} `{name}` in module `{owner}`")]
    DuplicateSymbol {
        owner: DagId,
        namespace: &'static str,
        name: String,
        first: Span,
        duplicate: Span,
    },
    /// Duplicate local import/alias in one namespace.
    #[error("duplicate imported {namespace} `{name}` in module `{owner}`")]
    DuplicateImportName {
        owner: DagId,
        namespace: &'static str,
        name: String,
        first: Span,
        duplicate: Span,
    },
    /// A name was not found in the requested namespace.
    #[error("unknown {namespace} `{name}` in module `{owner}`")]
    UnknownName {
        owner: DagId,
        namespace: &'static str,
        name: String,
    },
    /// A selective import name exists, but not under the marked category.
    #[error("in module `{owner}`, {mismatch}")]
    WrongImportCategory {
        owner: DagId,
        mismatch: ImportItemCategoryMismatch,
        span: Span,
    },
    /// A name exists, but in a semantic universe that is not valid here.
    #[error("in module `{owner}`, `{name}` is {actual}, not {expected}")]
    WrongUniverseName {
        owner: DagId,
        name: String,
        expected: SurfaceNameKind,
        actual: SurfaceNameKind,
    },
    /// A selective include chose an importable blueprint that is not an instance member.
    #[error("cannot project `{name}` from configured instance `{owner}` ({kind:?})")]
    IncludeItemNotProjectable {
        owner: DagId,
        name: String,
        kind: ExportedImportItemKind,
        span: Span,
    },
    /// A constructor cannot survive replacement of its owning nominal type.
    #[error(
        "cannot project constructor `{constructor}` because its owner `{owner_type}` is rebound in `{owner}`"
    )]
    ConstructorOwnerRebound {
        owner: DagId,
        constructor: String,
        owner_type: ResolvedStructTypeName,
        span: Span,
    },
    /// A name exists but has the wrong declaration kind for the use site.
    #[error("expected {expected} declaration `{name}`, found {actual}")]
    UnexpectedDeclKind {
        name: ResolvedDeclName,
        expected: &'static str,
        actual: DeclSymbolKind,
    },
    /// A name exists but is not public across module boundaries.
    #[error("private {namespace} `{name}` in module `{owner}`")]
    PrivateName {
        owner: DagId,
        namespace: &'static str,
        name: String,
    },
    /// A path did not have enough segments to denote `Index#Variant`.
    #[error("expected index-variant path in module `{owner}`, got `{path}`")]
    ExpectedIndexVariantPath { owner: DagId, path: String },
    /// The index exists, but the requested variant is absent.
    #[error("unknown variant `{variant}` for index `{index}`")]
    UnknownIndexVariant {
        index: ResolvedIndexName,
        variant: IndexVariantName,
    },
    /// A bare variant exists on more than one visible index.
    #[error("ambiguous index label `{variant}` in module `{owner}`; qualify it with an index name")]
    AmbiguousIndexVariant {
        owner: DagId,
        variant: IndexVariantName,
        indexes: Vec<ResolvedIndexName>,
    },
}
