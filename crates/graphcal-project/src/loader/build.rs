//! Pure construction of the dependency-ordered loaded files from a
//! [`SourceSnapshot`].
//!
//! The builder performs no IO. It walks the snapshot depth-first from the root
//! in the loader's load order, recording every import/include edge in a
//! [`DependencyGraph`], and reports the first failure in that order (read or
//! parse failures, unresolved or self-referencing imports). An import of a
//! file that is still being built stops the walk; the graph then names the
//! import cycle. Otherwise the graph's depth-first order (the walk's own
//! post-order) orders the loaded files. The builder assigns each file its
//! [`DagId`] and connects every import/include path to the exact module it
//! names.

use crate::load_error::LoadError;

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use super::budget::io_not_found;
use super::inline_dags::lift_inline_dags;
use super::loaded_file::{FileSite, LoadedFile};
use super::module_path::ModuleTarget;
use super::same_file_reference::{LexicalSite, PathScope, SameFileModule, SameFileScope};
use super::source_snapshot::{
    DependencySite, FetchedFile, FileRootDependencyKind, ModuleResolution, ParsedSource,
    ResolveFailure, ResolvedFile, SourceKey, SourceSnapshot, file_root_dependency, outside_root,
};
use crate::compile_error::CompileError;
use crate::dependency_ordered::DependencyOrdered;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::dependency_graph::DependencyGraph;
use graphcal_compiler::desugar::desugared_ast::File;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::import_cycle::ImportCycle;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::ast::ModulePath;
use graphcal_compiler::syntax::module_path_key::ModulePathKey;

/// Build every loaded file reachable from the snapshot root, dependencies
/// before dependents and ending with the root.
///
/// A file or dependency path the snapshot does not record is reported as not
/// found.
///
/// # Errors
///
/// Returns the first failure in load order: a recorded read/parse failure, an
/// unresolved import/include (at file root or in an inline DAG body), an
/// import outside the permitted root,
/// a file-root self import, an import cycle, or an invalid module path.
pub(super) fn build_loaded_files<K: SourceKey>(
    snapshot: SourceSnapshot<K>,
) -> Result<(DependencyOrdered<LoadedFile<ModuleTarget>>, SourceRegistry), CompileError> {
    let SourceSnapshot {
        root,
        mut files,
        sources,
    } = snapshot;
    let root_parsed = files
        .remove(&root)
        .ok_or_else(|| io_not_found(root.path()))??;
    let root_source = root_parsed.source_id();
    let mut graph = DependencyGraph::new();
    graph.add_node(root.clone());
    let mut builder = Builder {
        unbuilt: files,
        loading: Vec::new(),
        graph,
        built: HashMap::new(),
        sources: &sources,
    };
    match builder.build_parsed(&root, root_parsed) {
        Ok(_) | Err(Stop::ReEntered) => {}
        Err(Stop::Failed(error)) => return Err(*error),
    }
    let Builder {
        loading,
        graph,
        mut built,
        ..
    } = builder;
    let order = graph.into_depth_first_order().map_err(|cycle| {
        let lead_in = loading
            .iter()
            .take_while(|file| *file != cycle.entry())
            .map(SourceKey::chain_file)
            .collect();
        CompileError::Load(LoadError::CircularImport {
            cycle: ImportCycle::new(lead_in, cycle.map(|file| file.chain_file())),
        })
    })?;
    let files = DependencyOrdered::from_topo_order(order, &root, |file| built.remove(&file))
        .ok_or_else(|| {
            CompileError::semantic(
                SemanticError::internal_error(
                    "the loaded files do not match the acyclic import graph",
                    root_source,
                    DiagnosticAnchor::WholeFile,
                ),
                &sources,
            )
        })?;
    Ok((files, sources))
}

struct Builder<'s, K> {
    /// Every fetched source text, for rendering the builder's diagnostics.
    sources: &'s SourceRegistry,
    /// Fetched files not yet built.
    unbuilt: HashMap<K, FetchedFile<K>>,
    /// Files being built, from the root to the current file.
    loading: Vec<K>,
    /// Every file reached so far and the files it imports/includes, in load
    /// order.
    graph: DependencyGraph<K>,
    /// Completed files.
    built: HashMap<K, LoadedFile<ModuleTarget>>,
}

/// Why the walk stopped before building every reachable file.
enum Stop {
    /// A failure to report as is.
    Failed(Box<CompileError>),
    /// A file imported a file that is still being built: the dependency graph
    /// now has a cycle.
    ReEntered,
}

impl From<CompileError> for Stop {
    fn from(error: CompileError) -> Self {
        Self::Failed(Box::new(error))
    }
}

/// Owning file of an import/include path, known before this file's own
/// [`DagId`] is assigned.
enum ImportOwner {
    ThisFile,
    Dependency(DagId),
}

impl<K: SourceKey> Builder<'_, K> {
    /// Identity of `file`, which `dependent` imports/includes, building it
    /// (and its dependencies) first when needed.
    fn dependency(&mut self, dependent: &K, file: &K) -> Result<DagId, Stop> {
        let reached = self.graph.contains(file);
        self.graph.add_dependency(dependent.clone(), file.clone());
        match self.built.get(file) {
            Some(loaded) => Ok(loaded.dag_id.clone()),
            // Reached but not built: `file` is still being built.
            None if reached => Err(Stop::ReEntered),
            None => {
                let parsed = self
                    .unbuilt
                    .remove(file)
                    .ok_or_else(|| io_not_found(file.path()))??;
                self.build_parsed(file, parsed)
            }
        }
    }

    fn build_parsed(&mut self, file: &K, parsed: ParsedSource<K>) -> Result<DagId, Stop> {
        self.loading.push(file.clone());

        let named_source = parsed.named_source();
        let location = parsed.location();
        let module_path = location.module_path.clone();
        let dag_id = DagId::from_relative_path(location.package.clone(), &location.relative_path)
            .map_err(|error| {
            CompileError::semantic(
                SemanticError::internal_error(
                    format!(
                        "invalid module path `{}`: {error}",
                        location.relative_path.display()
                    ),
                    parsed.source_id(),
                    DiagnosticAnchor::WholeFile,
                ),
                self.sources,
            )
        })?;
        let mut resolved_imports =
            same_file_root_targets(parsed.ast(), parsed.same_file(), &dag_id, named_source)?;
        let imports = self.load_file_root_dependencies(file, &parsed)?;
        self.load_dag_body_dependencies(file, &parsed)?;

        resolved_imports.extend(imports.into_iter().map(|(key, (owner, resolved))| {
            let source_file = match owner {
                ImportOwner::ThisFile => dag_id.clone(),
                ImportOwner::Dependency(dependency) => dependency,
            };
            (key, resolved.target_from(&source_file))
        }));
        let inline_dags = lift_inline_dags(parsed.ast(), &dag_id, parsed.same_file(), |path| {
            self.dag_body_target(file, &dag_id, &parsed, path).map(Some)
        })?;

        self.loading.pop();
        self.built.insert(file.clone(), {
            let (source, source_id, ast) = parsed.into_file().into_parts();
            LoadedFile::new(
                FileSite {
                    path: file.path().to_path_buf(),
                    dag_id: dag_id.clone(),
                    module_path,
                },
                source,
                source_id,
                ast,
                resolved_imports,
                inline_dags,
            )
        });
        Ok(dag_id)
    }

    /// Load the files named by file-root imports/includes and record each
    /// path's owning file. Every resolution failure is an error here.
    fn load_file_root_dependencies<'p>(
        &mut self,
        file: &K,
        parsed: &'p ParsedSource<K>,
    ) -> Result<HashMap<ModulePathKey, (ImportOwner, &'p ResolvedFile<K>)>, Stop> {
        let src = parsed.named_source();
        let mut imports = HashMap::new();
        for dependency in parsed.loading_dependencies() {
            let DependencySite::FileRoot(kind) = dependency.site else {
                continue;
            };
            let path = dependency.path;
            // An unrecorded path did not resolve to any file.
            let resolved = match parsed.resolution(path) {
                Some(ModuleResolution::Resolved(resolved)) => resolved,
                Some(ModuleResolution::OutsideRoot) => {
                    return Err(CompileError::Load(outside_root(path, src.clone())).into());
                }
                Some(ModuleResolution::Failed(failure)) => {
                    return Err(failure.to_error(path, src).into());
                }
                None => {
                    return Err(ResolveFailure::FileNotFound.to_error(path, src).into());
                }
            };
            let owner = if resolved.file == *file {
                if kind == FileRootDependencyKind::Import && resolved.inline_path.is_empty() {
                    return Err(file_root_self_import_error(path, src).into());
                }
                ImportOwner::ThisFile
            } else {
                ImportOwner::Dependency(self.dependency(file, &resolved.file)?)
            };
            imports.insert(path.key(), (owner, resolved));
        }
        Ok(imports)
    }

    /// Load the files named by inline-DAG body imports/includes. A body path
    /// that does not resolve is reported when its DAG is lifted
    /// ([`Self::dag_body_target`]); a path outside the project root is
    /// rejected here, before any file it names is loaded.
    fn load_dag_body_dependencies(
        &mut self,
        file: &K,
        parsed: &ParsedSource<K>,
    ) -> Result<(), Stop> {
        for dependency in parsed.loading_dependencies() {
            if !matches!(dependency.site, DependencySite::DagBody { .. }) {
                continue;
            }
            match parsed.resolution(dependency.path) {
                Some(ModuleResolution::Resolved(resolved)) if resolved.file != *file => {
                    self.dependency(file, &resolved.file)?;
                }
                Some(ModuleResolution::Resolved(_) | ModuleResolution::Failed(_)) | None => {}
                Some(ModuleResolution::OutsideRoot) => {
                    return Err(CompileError::Load(outside_root(
                        dependency.path,
                        parsed.named_source().clone(),
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }

    /// Target of an inline-DAG body path of `file` (whose id is `dag_id`)
    /// that is not a same-file reference. Every such path must resolve, as a
    /// file-root path must.
    fn dag_body_target(
        &self,
        file: &K,
        dag_id: &DagId,
        parsed: &ParsedSource<K>,
        path: &ModulePath,
    ) -> Result<ModuleTarget, CompileError> {
        let src = parsed.named_source();
        let resolved = match parsed.resolution(path) {
            Some(ModuleResolution::Resolved(resolved)) => resolved,
            Some(ModuleResolution::OutsideRoot) => {
                return Err(CompileError::Load(outside_root(path, src.clone())));
            }
            Some(ModuleResolution::Failed(failure)) => return Err(failure.to_error(path, src)),
            // An unrecorded path did not resolve to any file.
            None => return Err(ResolveFailure::FileNotFound.to_error(path, src)),
        };
        if resolved.file == *file {
            return Ok(resolved.target_from(dag_id));
        }
        // Every body path classified as a package path drove loading
        // (`load_dag_body_dependencies`), so the file it names is built.
        self.built.get(&resolved.file).map_or_else(
            || {
                Err(CompileError::semantic(
                    SemanticError::internal_error(
                        format!(
                            "the file named by `{}` was not loaded before its dependent",
                            path.key()
                        ),
                        parsed.source_id(),
                        DiagnosticAnchor::WholeFile,
                    ),
                    self.sources,
                ))
            },
            |loaded| Ok(resolved.target_from(&loaded.dag_id)),
        )
    }
}

pub(super) fn file_root_self_import_error(
    path: &ModulePath,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    CompileError::Load(LoadError::FileRootSelfImport {
        path: path.key(),
        src: src.clone(),
        span: path.span().into(),
    })
}

/// Targets of the file-root paths of `ast` (the file `file`) that name a
/// module of the same file: a top-level `dag`, or the file root itself through
/// a stem self-reference.
///
/// # Errors
///
/// Rejects a file-root `import` of the file root (M033): its top-level
/// declarations are already in scope.
pub(super) fn same_file_root_targets(
    ast: &File,
    same_file: &SameFileScope,
    file: &DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ModulePathKey, ModuleTarget>, CompileError> {
    let mut targets = HashMap::new();
    for (path, kind) in ast.declarations.iter().filter_map(file_root_dependency) {
        let PathScope::SameFile(module) = same_file.classify(path, LexicalSite::FileRoot) else {
            continue;
        };
        if kind == FileRootDependencyKind::Import && module == SameFileModule::FileRoot {
            return Err(file_root_self_import_error(path, src));
        }
        targets.insert(path.key(), module.target_in(file));
    }
    Ok(targets)
}

#[cfg(test)]
mod tests;
