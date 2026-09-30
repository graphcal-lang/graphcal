//! Loaded source files and the inline DAG bodies indexed within them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::syntax::ast::DeclKind;

use super::module_path::{InlineBodyImportResolution, ModulePathKey, ResolvedModuleTarget};

/// Validated path from a file AST root to one nested inline-DAG body.
///
/// Construction is private and requires at least one declaration index, so a
/// locator cannot accidentally denote the file root.
#[derive(Debug, Clone)]
pub(super) struct DagBodyLocator {
    pub(super) first: usize,
    pub(super) rest: Box<[usize]>,
}

impl DagBodyLocator {
    pub(super) fn at_child(parent_path: &[usize], child_index: usize) -> Self {
        match parent_path.split_first() {
            None => Self {
                first: child_index,
                rest: Box::new([]),
            },
            Some((first, rest)) => Self {
                first: *first,
                rest: rest
                    .iter()
                    .copied()
                    .chain(std::iter::once(child_index))
                    .collect(),
            },
        }
    }

    #[expect(
        clippy::expect_used,
        clippy::unreachable,
        reason = "private locators are validated while traversing the immutable owning AST"
    )]
    pub(super) fn declaration<'a>(
        &self,
        ast: &'a File,
    ) -> &'a graphcal_compiler::desugar::desugared_ast::DagDecl {
        let declaration = ast
            .declarations
            .get(self.first)
            .expect("loader-created inline DAG locator must remain in bounds");
        let DeclKind::Dag(first_dag) = &declaration.kind else {
            unreachable!("loader-created inline DAG locator must address DAG declarations")
        };
        let mut dag = first_dag;
        for index in &self.rest {
            let declaration = dag
                .body
                .get(*index)
                .expect("loader-created inline DAG locator must remain in bounds");
            let DeclKind::Dag(child) = &declaration.kind else {
                unreachable!("loader-created inline DAG locator must address DAG declarations")
            };
            dag = child;
        }
        dag
    }
}

/// A single inline `dag X { ... }` block indexed within its enclosing file.
///
/// Produced by the loader so that downstream stages can iterate inline DAGs
/// uniformly with file DAGs, looking up `resolved_imports` for both the body's
/// own imports and `import <self>::{...}` references back to the parent file.
#[derive(Debug, Clone)]
pub struct LoadedDag {
    /// Abstract DAG identity for this inline dag, formed by appending the
    /// dag's name to its parent file's `DagId`.
    pub(super) dag_id: DagId,
    /// The enclosing file's `DagId`. Imports whose path resolves to this id
    /// are dag-body self-imports (`import <self>::{...}`).
    pub(super) parent_dag_id: DagId,
    /// Stable locator into the owning file AST, which remains the single body owner.
    pub(super) body_locator: DagBodyLocator,
    /// Loader-resolved DAG identities for each `import` declaration in the
    /// body, keyed by its typed span-free module path. Self-imports map to
    /// `parent_dag_id`; cross-file imports map to the dependency file's id.
    /// Imports whose path fails to resolve at load time are absent here; the
    /// downstream resolver surfaces a structured error for them.
    pub(super) resolved_imports: HashMap<ModulePathKey, InlineBodyImportResolution>,
    /// Declared interface of the body, computed once at load.
    pub(super) interface: ModuleInterface,
}

impl LoadedDag {
    /// Declared interface of this inline DAG's body.
    #[must_use]
    pub(crate) const fn interface(&self) -> &ModuleInterface {
        &self.interface
    }

    /// This inline DAG as a module of its owning `file`.
    #[must_use]
    pub(crate) fn module<'a>(&'a self, file: &'a LoadedFile) -> LoadedModule<'a> {
        debug_assert_eq!(self.parent_dag_id, file.dag_id);
        LoadedModule::InlineDag { file, dag: self }
    }

    #[must_use]
    pub(crate) const fn dag_id(&self) -> &DagId {
        &self.dag_id
    }

    #[must_use]
    pub(crate) const fn parent_dag_id(&self) -> &DagId {
        &self.parent_dag_id
    }

    #[must_use]
    pub(crate) const fn resolved_imports(
        &self,
    ) -> &HashMap<ModulePathKey, InlineBodyImportResolution> {
        &self.resolved_imports
    }

    #[must_use]
    pub(crate) fn declaration<'a>(
        &self,
        file: &'a LoadedFile,
    ) -> &'a graphcal_compiler::desugar::desugared_ast::DagDecl {
        debug_assert_eq!(self.parent_dag_id, file.dag_id);
        self.body_locator.declaration(&file.ast)
    }

    /// Borrow this DAG's authoritative body from its owning file AST.
    #[must_use]
    pub(crate) fn body<'a>(&self, file: &'a LoadedFile) -> &'a [Declaration] {
        &self.declaration(file).body
    }
}

/// One loaded DAG module (a file root or an inline `dag`), borrowed from the
/// source file that owns it.
///
/// Holding the owning file (and inline-DAG entry) itself, rather than their
/// identities, makes every later lookup of the module's body, interface, and
/// owning source file infallible.
#[derive(Debug, Clone, Copy)]
pub enum LoadedModule<'a> {
    FileRoot(&'a LoadedFile),
    InlineDag {
        file: &'a LoadedFile,
        dag: &'a LoadedDag,
    },
}

impl<'a> LoadedModule<'a> {
    /// Authoritative declarations of this module's body.
    #[must_use]
    pub fn declarations(self) -> &'a [Declaration] {
        match self {
            Self::FileRoot(file) => &file.ast.declarations,
            Self::InlineDag { file, dag } => dag.body(file),
        }
    }

    /// Declared interface computed at load from exactly [`Self::declarations`].
    #[must_use]
    pub const fn interface(self) -> &'a ModuleInterface {
        match self {
            Self::FileRoot(file) => &file.interface,
            Self::InlineDag { dag, .. } => &dag.interface,
        }
    }

    /// Exact file-root or inline-DAG identity of this module.
    #[must_use]
    pub const fn dag_id(self) -> &'a DagId {
        match self {
            Self::FileRoot(file) => &file.dag_id,
            Self::InlineDag { dag, .. } => &dag.dag_id,
        }
    }
}

/// A single loaded and parsed file.
#[derive(Debug)]
pub struct LoadedFile {
    /// Canonical path of this file (retained for I/O: diagnostics, LSP URIs).
    pub(super) path: PathBuf,
    /// Abstract DAG identity (filesystem-independent).
    pub(super) dag_id: DagId,
    /// Raw source text.
    pub(super) source: Arc<String>,
    /// Parsed AST.
    pub(super) ast: File,
    /// Named source for diagnostics.
    pub(super) named_source: NamedSource<Arc<String>>,
    /// Loader-resolved DAG identities for each import declaration, keyed by the
    /// import path's display string (e.g. `"./lib.gcl"` or `"nasa/rocket"`).
    /// Produced by the loader so that downstream consumers (evaluator, LSP) can
    /// look up resolved imports without re-resolving.
    pub(super) resolved_imports: HashMap<ModulePathKey, ResolvedModuleTarget>,
    /// Inline `dag X { ... }` metadata indexed from this file, with per-DAG
    /// pre-resolved imports. Entries retain source preorder and borrow their
    /// authoritative bodies from `ast` through validated locators.
    pub(super) inline_dags: Vec<LoadedDag>,
    /// Declared interface of the file root, computed once at load.
    pub(super) interface: ModuleInterface,
}

impl LoadedFile {
    /// Assemble a loaded file from its parsed source; the declared interface
    /// is computed here, once, from exactly the parsed declarations.
    pub(super) fn new(
        path: PathBuf,
        dag_id: DagId,
        source: Arc<String>,
        named_source: NamedSource<Arc<String>>,
        ast: File,
        resolved_imports: HashMap<ModulePathKey, ResolvedModuleTarget>,
        inline_dags: Vec<LoadedDag>,
    ) -> Self {
        Self {
            path,
            dag_id,
            source,
            interface: ModuleInterface::new(&ast.declarations),
            ast,
            named_source,
            resolved_imports,
            inline_dags,
        }
    }

    /// Declared interface of this file root module.
    #[must_use]
    pub(crate) const fn interface(&self) -> &ModuleInterface {
        &self.interface
    }

    /// This file root as a module.
    #[must_use]
    pub(crate) const fn module(&self) -> LoadedModule<'_> {
        LoadedModule::FileRoot(self)
    }

    /// Canonical path used for I/O and diagnostic URI mapping.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Abstract, filesystem-independent identity of this source file.
    #[must_use]
    pub const fn dag_id(&self) -> &DagId {
        &self.dag_id
    }

    /// Raw source text parsed into this immutable loader artifact.
    #[must_use]
    pub const fn source(&self) -> &Arc<String> {
        &self.source
    }

    /// Parsed, desugared AST from the same source snapshot.
    #[must_use]
    pub const fn ast(&self) -> &File {
        &self.ast
    }

    /// Named source paired with this file's path and source snapshot.
    #[must_use]
    pub(crate) const fn named_source(&self) -> &NamedSource<Arc<String>> {
        &self.named_source
    }

    #[must_use]
    pub(crate) fn inline_dags(&self) -> &[LoadedDag] {
        &self.inline_dags
    }

    /// Iterate over imports together with their exact module targets and owners.
    pub fn imports_with_targets(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::desugar::desugared_ast::Declaration,
            &graphcal_compiler::syntax::ast::ImportDecl,
            &ResolvedModuleTarget,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Import(import_decl) = &decl.kind {
                self.resolved_imports
                    .get(&ModulePathKey::from_path(import_decl.path()))
                    .map(|target| (decl, import_decl, target))
            } else {
                None
            }
        })
    }

    /// Iterate over includes together with their exact module targets and owners.
    pub fn includes_with_targets(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::desugar::desugared_ast::Declaration,
            &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
            &ResolvedModuleTarget,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Include(include_decl) = &decl.kind {
                self.resolved_imports
                    .get(&ModulePathKey::from_path(&include_decl.path))
                    .map(|target| (decl, include_decl, target))
            } else {
                None
            }
        })
    }
}
