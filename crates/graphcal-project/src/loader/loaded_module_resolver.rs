//! The module resolver of one loaded project, with the resolver handle of
//! every loaded module.

use graphcal_compiler::resolve::error::{ModuleResolveError, ScopeError};
use graphcal_compiler::resolve::{ModuleHandle, ModuleRef, ModuleResolver};
use graphcal_compiler::source_id::SourceId;

use super::loaded_project::LoadedProject;
use super::module_path::{LoadedModuleId, ResolvedModuleTarget};

/// The resolver handles of one loaded file: its root's, then each inline
/// DAG's in source preorder.
#[derive(Debug)]
struct FileModuleHandles {
    root: ModuleHandle,
    inline_dags: Vec<ModuleHandle>,
}

/// The module resolver of one [`LoadedProject`], which knows the handle of
/// every loaded module (file root or inline DAG).
///
/// Built only from the project it serves, so every loaded module of that
/// project has a handle, and a [`ResolvedModuleTarget`] of the project names
/// a module of the resolver.
#[derive(Debug)]
pub struct LoadedModuleResolver {
    resolver: ModuleResolver,
    /// Per loaded file, in dependency order (the order of
    /// [`LoadedModuleId`]).
    handles: Vec<FileModuleHandles>,
}

/// A failure to build the module resolver of a loaded project.
#[derive(Debug)]
pub struct ModuleResolverBuildError {
    /// The loaded source the error's spans belong to, or `None` when the
    /// error concerns the include graph as a whole.
    pub source: Option<SourceId>,
    pub error: ModuleResolveError,
}

impl LoadedModuleResolver {
    /// Build the module resolver of `project`.
    ///
    /// # Errors
    ///
    /// Returns the [`ModuleResolveError`] of
    /// [`LoadedProject::build_module_resolver`], with the loaded source it
    /// was raised in.
    pub(crate) fn build(project: &LoadedProject) -> Result<Self, ModuleResolverBuildError> {
        let mut tables = graphcal_compiler::resolve::builder::SymbolTables::default();
        let handles = project
            .files()
            .iter()
            .map(|loaded| {
                let in_file = |error| ModuleResolverBuildError {
                    source: Some(loaded.source_id()),
                    error,
                };
                let root = tables
                    .add_module(loaded.dag_id.clone(), &loaded.ast.declarations)
                    .map_err(in_file)?;
                let inline_dags = loaded
                    .inline_dags
                    .iter()
                    .map(|inline| tables.add_module(inline.dag_id.clone(), inline.body(loaded)))
                    .collect::<Result<_, _>>()
                    .map_err(in_file)?;
                Ok(FileModuleHandles { root, inline_dags })
            })
            .collect::<Result<_, _>>()?;
        let resolver = tables
            .scopes(project)
            .map_err(|error| ModuleResolverBuildError {
                source: None,
                error,
            })?
            .freeze()
            .map_err(|ScopeError { module, error }| ModuleResolverBuildError {
                source: project
                    .file(&module.file_root())
                    .map(super::loaded_file::LoadedFile::source_id),
                error,
            })?;
        Ok(Self { resolver, handles })
    }

    /// The resolver itself.
    pub(crate) const fn resolver(&self) -> &ModuleResolver {
        &self.resolver
    }

    /// The resolver, without the loaded modules' handles.
    pub(crate) fn into_resolver(self) -> ModuleResolver {
        self.resolver
    }

    /// The resolver module of the loaded module `id`.
    #[expect(
        clippy::expect_used,
        reason = "module ids are issued only by the placement of the project this resolver was built from"
    )]
    pub(crate) fn module(&self, id: LoadedModuleId) -> ModuleRef<'_> {
        let file = self
            .handles
            .get(id.file)
            .expect("a placed module id names a loaded file");
        let handle = id.inline.map_or(file.root, |inline| {
            *file
                .inline_dags
                .get(inline)
                .expect("a placed module id names an inline DAG of its file")
        });
        self.resolver.module(handle)
    }

    /// The resolver module a resolved module path names.
    pub(crate) fn target(&self, target: &ResolvedModuleTarget) -> ModuleRef<'_> {
        self.module(target.module())
    }
}
