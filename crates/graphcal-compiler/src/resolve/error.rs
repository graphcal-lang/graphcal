//! Errors produced while building or using module-aware symbol tables.
//!
//! Every payload is typed; text appears only in the `Display` rendering, which
//! keeps the resolver's established diagnostic wording.

use thiserror::Error;

use crate::dag_id::DagId;
use crate::resolved_name::{ResolvedDeclName, ResolvedIndexName, ResolvedStructTypeName};
use crate::syntax::function_name::FnName;
use crate::syntax::import_category::ImportItemCategoryMismatch;
use crate::syntax::index_name::{IndexVariantName, QualifiedIndexVariantName};
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;
use crate::syntax::type_name::ConstructorName;

use super::category::{DeclSymbolKind, ExportedImportItemKind, SurfaceNameKind, SymbolTable};
use super::namespace::Namespace;

/// What a failed lookup or visibility check searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameCategory {
    /// One declaration / selective-import table.
    Table(SymbolTable),
    /// A whole collision unit.
    Namespace(Namespace),
    /// The Term category of a selective import, which may name a
    /// declaration or a constructor.
    TermImport,
    /// A module alias reached through a qualifier.
    DagAlias,
    /// A `dag` declaration on a module path.
    Dag,
}

impl std::fmt::Display for NameCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Table(table) => table.fmt(f),
            Self::Namespace(namespace) => namespace.fmt(f),
            Self::TermImport => f.write_str("term import namespace"),
            Self::DagAlias => f.write_str("dag alias"),
            Self::Dag => f.write_str("dag"),
        }
    }
}

/// The declaration kind a use site requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpectedDeclKind {
    /// A `const` declaration.
    Const,
    /// A `const` declaration reached through an imported (not instantiated) DAG.
    InstanceIndependentConst,
    /// A value that participates in the graph (`const`, `param`, or `node`).
    GraphValue,
}

impl std::fmt::Display for ExpectedDeclKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Const => "const",
            Self::InstanceIndependentConst => "instance-independent const",
            Self::GraphValue => "graph value",
        })
    }
}

/// Errors produced while building or using module-aware symbol tables.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModuleResolveError {
    /// A module was added twice.
    #[error("duplicate module `{owner}`")]
    DuplicateModule { owner: DagId },
    /// Two source modules share one module-path spelling, e.g. the file
    /// `lib/x.gcl` and an inline `dag x` in `lib.gcl`. Neither is preferred.
    #[error(
        "module path `{first}` is ambiguous: it names a module in file `{}` and a module in file `{}`",
        .first.file_root(),
        .second.file_root()
    )]
    AmbiguousModulePath { first: DagId, second: DagId },
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
    /// Two local declarations occupy one slot of the module's collision unit.
    #[error("duplicate {namespace} `{name}` in module `{owner}`")]
    DuplicateSymbol {
        owner: DagId,
        namespace: Namespace,
        name: NameAtom,
        first: Span,
        duplicate: Span,
    },
    /// One index declaration lists a variant twice.
    #[error("duplicate IndexVariantName `{variant}` in module `{owner}`")]
    DuplicateIndexVariant {
        owner: DagId,
        variant: QualifiedIndexVariantName,
        first: Span,
        duplicate: Span,
    },
    /// One plugin block declares a function twice.
    #[error("duplicate FnName `{function}` in module `{owner}`")]
    DuplicatePluginFunction {
        owner: DagId,
        function: FnName,
        first: Span,
        duplicate: Span,
    },
    /// An alias or import claims a slot another binding already occupies.
    #[error("duplicate imported {namespace} `{name}` in module `{owner}`")]
    DuplicateImportName {
        owner: DagId,
        namespace: Namespace,
        name: NameAtom,
        first: Span,
        duplicate: Span,
    },
    /// A name was not found where the lookup searched.
    #[error("unknown {category} `{name}` in module `{owner}`")]
    UnknownName {
        owner: DagId,
        category: NameCategory,
        name: NameAtom,
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
        name: NamePath,
        expected: SurfaceNameKind,
        actual: SurfaceNameKind,
    },
    /// A selective include chose an importable blueprint that is not an instance member.
    #[error("cannot project `{name}` from configured instance `{owner}` ({kind:?})")]
    IncludeItemNotProjectable {
        owner: DagId,
        name: NameAtom,
        kind: ExportedImportItemKind,
        span: Span,
    },
    /// A constructor cannot survive replacement of its owning nominal type.
    #[error(
        "cannot project constructor `{constructor}` because its owner `{owner_type}` is rebound in `{owner}`"
    )]
    ConstructorOwnerRebound {
        owner: DagId,
        constructor: ConstructorName,
        owner_type: ResolvedStructTypeName,
        span: Span,
    },
    /// A name exists but has the wrong declaration kind for the use site.
    #[error("expected {expected} declaration `{name}`, found {actual}")]
    UnexpectedDeclKind {
        name: ResolvedDeclName,
        expected: ExpectedDeclKind,
        actual: DeclSymbolKind,
    },
    /// A name exists but is not public across module boundaries.
    #[error("private {category} `{name}` in module `{owner}`")]
    PrivateName {
        owner: DagId,
        category: NameCategory,
        name: NameAtom,
    },
    /// The index exists, but the requested variant is absent.
    #[error("unknown variant `{variant}` for index `{index}`")]
    UnknownIndexVariant {
        index: ResolvedIndexName,
        variant: IndexVariantName,
    },
}
