//! Loaded source files and the inline DAG bodies indexed within them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use graphcal_compiler::source_id::SourceId;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::syntax::ast::DeclKind;

use graphcal_compiler::syntax::module_path_key::ModulePathKey;

use super::module_path::{LoadedModuleId, ModuleTarget, ResolvedModuleTarget};

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
pub struct LoadedDag<T = ResolvedModuleTarget> {
    /// Abstract DAG identity for this inline dag, formed by appending the
    /// dag's name to its parent file's `DagId`.
    pub(super) dag_id: DagId,
    /// The enclosing file's `DagId`. Imports whose path resolves to this id
    /// are dag-body self-imports (`import <self>::{...}`).
    pub(super) parent_dag_id: DagId,
    /// Stable locator into the owning file AST, which remains the single body owner.
    pub(super) body_locator: DagBodyLocator,
    /// Loader-resolved DAG identities for each `import`/`include` declaration
    /// in the body, keyed by its typed span-free module path. Self-imports map
    /// to `parent_dag_id`; cross-file imports map to the dependency file's id.
    /// A project load rejects a body path it cannot resolve; only a
    /// single-buffer load (no loader) leaves cross-file paths absent.
    pub(super) resolved_imports: HashMap<ModulePathKey, T>,
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
    pub(crate) const fn resolved_imports(&self) -> &HashMap<ModulePathKey, ResolvedModuleTarget> {
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
pub struct LoadedFile<T = ResolvedModuleTarget> {
    /// Canonical path of this file (retained for I/O: diagnostics, LSP URIs).
    pub(super) path: PathBuf,
    /// Abstract DAG identity (filesystem-independent).
    pub(super) dag_id: DagId,
    /// Raw source text.
    pub(super) source: Arc<String>,
    /// Parsed AST.
    pub(super) ast: File,
    /// Identity of the source text in the project's source registry.
    pub(super) source_id: SourceId,
    /// Loader-resolved DAG identities for each import declaration, keyed by the
    /// import path's display string (e.g. `"./lib.gcl"` or `"nasa/rocket"`).
    /// Produced by the loader so that downstream consumers (evaluator, LSP) can
    /// look up resolved imports without re-resolving.
    pub(super) resolved_imports: HashMap<ModulePathKey, T>,
    /// Inline `dag X { ... }` metadata indexed from this file, with per-DAG
    /// pre-resolved imports. Entries retain source preorder and borrow their
    /// authoritative bodies from `ast` through validated locators.
    pub(super) inline_dags: Vec<LoadedDag<T>>,
    /// Declared interface of the file root, computed once at load.
    pub(super) interface: ModuleInterface,
}

impl LoadedFile<ModuleTarget> {
    /// Assemble a loaded file from its parsed source; the declared interface
    /// is computed here, once, from exactly the parsed declarations.
    pub(super) fn new(
        path: PathBuf,
        dag_id: DagId,
        source: Arc<String>,
        source_id: SourceId,
        ast: File,
        resolved_imports: HashMap<ModulePathKey, ModuleTarget>,
        inline_dags: Vec<LoadedDag<ModuleTarget>>,
    ) -> Self {
        Self {
            path,
            dag_id,
            source,
            interface: ModuleInterface::new(&ast.declarations),
            ast,
            source_id,
            resolved_imports,
            inline_dags,
        }
    }

    /// Place every module target of this file with `place`, which knows the
    /// loaded module of each identity.
    ///
    /// Targets are placed in source order: the file root's declarations,
    /// then each inline DAG body in source preorder.
    ///
    /// # Errors
    ///
    /// Returns the first target that names no loaded module (an inline DAG
    /// the named file does not declare), at its module path.
    pub(super) fn place(
        self,
        place: &impl Fn(&DagId) -> Option<LoadedModuleId>,
    ) -> Result<LoadedFile, UnplacedTarget> {
        let Self {
            path,
            dag_id,
            source,
            ast,
            source_id,
            resolved_imports,
            inline_dags,
            interface,
        } = self;
        let resolved_imports =
            place_targets(&ast.declarations, resolved_imports, place, source_id)?;
        let inline_dags = inline_dags
            .into_iter()
            .map(|dag| {
                let LoadedDag {
                    dag_id,
                    parent_dag_id,
                    body_locator,
                    resolved_imports,
                    interface,
                } = dag;
                let body = &body_locator.declaration(&ast).body;
                Ok(LoadedDag {
                    resolved_imports: place_targets(body, resolved_imports, place, source_id)?,
                    dag_id,
                    parent_dag_id,
                    body_locator,
                    interface,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(LoadedFile {
            path,
            dag_id,
            source,
            ast,
            source_id,
            resolved_imports,
            inline_dags,
            interface,
        })
    }
}

/// A module path whose loaded file declares no inline DAG it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UnplacedTarget {
    /// The source declaring the module path.
    pub(super) source: SourceId,
    /// The module path.
    pub(super) span: graphcal_compiler::syntax::span::Span,
    /// The module the path names.
    pub(super) target: DagId,
}

/// Place the target of every `import` / `include` path among `declarations`.
fn place_targets(
    declarations: &[Declaration],
    mut targets: HashMap<ModulePathKey, ModuleTarget>,
    place: &impl Fn(&DagId) -> Option<LoadedModuleId>,
    source: SourceId,
) -> Result<HashMap<ModulePathKey, ResolvedModuleTarget>, UnplacedTarget> {
    let mut placed = HashMap::with_capacity(targets.len());
    for path in declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Import(import) => Some(import.path()),
            DeclKind::Include(include) => Some(&include.path),
            _ => None,
        })
    {
        let key = path.key();
        // A path written twice is placed once; an unresolved path has no target.
        let Some(target) = targets.remove(&key) else {
            continue;
        };
        let module = place(target.target()).ok_or_else(|| UnplacedTarget {
            source,
            span: path.span(),
            target: target.target().clone(),
        })?;
        placed.insert(key, target.placed_at(module));
    }
    Ok(placed)
}

impl LoadedFile {
    /// This file root as a module.
    #[must_use]
    pub(crate) const fn module(&self) -> LoadedModule<'_> {
        LoadedModule::FileRoot(self)
    }
}

impl<T> LoadedFile<T> {
    /// Declared interface of this file root module.
    #[must_use]
    pub(crate) const fn interface(&self) -> &ModuleInterface {
        &self.interface
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

    /// Identity of this file's text in the project's source registry.
    #[must_use]
    pub(crate) const fn source_id(&self) -> SourceId {
        self.source_id
    }

    #[must_use]
    pub(crate) fn inline_dags(&self) -> &[LoadedDag<T>] {
        &self.inline_dags
    }

    /// Iterate over imports together with their exact module targets and owners.
    pub fn imports_with_targets(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::desugar::desugared_ast::Declaration,
            &graphcal_compiler::syntax::ast::ImportDecl,
            &T,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Import(import_decl) = &decl.kind {
                self.resolved_imports
                    .get(&import_decl.path().key())
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
            &T,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Include(include_decl) = &decl.kind {
                self.resolved_imports
                    .get(&include_decl.path.key())
                    .map(|target| (decl, include_decl, target))
            } else {
                None
            }
        })
    }
}
