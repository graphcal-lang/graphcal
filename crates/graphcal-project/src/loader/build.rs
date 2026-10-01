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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use super::budget::io_not_found;
use super::inline_dags::lift_inline_dags;
use super::loaded_file::LoadedFile;
use super::module_path::ModulePathKey;
use super::source_snapshot::{
    DependencySite, FetchedFile, FileRootDependencyKind, ModuleResolution, ParsedSource,
    ResolveFailure, ResolvedFile, SourceKey, SourceSnapshot, collect_inline_dag_names,
    file_root_dependency, file_stem, loading_dependency_paths, outside_root,
};
use crate::compile_error::CompileError;
use crate::dependency_ordered::DependencyOrdered;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::dependency_graph::DependencyGraph;
use graphcal_compiler::desugar::desugared_ast::Declaration;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::import_cycle::ImportCycle;
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::ast::ModulePath;

/// Build every loaded file reachable from the snapshot root, dependencies
/// before dependents and ending with the root.
///
/// A file or dependency path the snapshot does not record is reported as not
/// found.
///
/// # Errors
///
/// Returns the first failure in load order: a recorded read/parse failure, an
/// unresolved file-root import/include, an import outside the permitted root,
/// a file-root self import, an import cycle, or an invalid module path.
pub(super) fn build_loaded_files<K: SourceKey>(
    snapshot: SourceSnapshot<K>,
) -> Result<(DependencyOrdered<LoadedFile>, SourceRegistry), CompileError> {
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
        Err(Stop::Failed(error)) => return Err(error),
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
                GraphcalError::internal_error(
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
    built: HashMap<K, LoadedFile>,
}

/// Why the walk stopped before building every reachable file.
enum Stop {
    /// A failure to report as is.
    Failed(CompileError),
    /// A file imported a file that is still being built: the dependency graph
    /// now has a cycle.
    ReEntered,
}

impl From<CompileError> for Stop {
    fn from(error: CompileError) -> Self {
        Self::Failed(error)
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

        let stem = file_stem(file.path());
        let named_source = parsed.named_source();
        reject_file_root_stem_imports(&parsed.ast().declarations, stem, named_source)?;
        let imports = self.load_file_root_dependencies(file, &parsed)?;
        self.load_dag_body_dependencies(file, &parsed)?;

        let location = parsed.location();
        let dag_id = DagId::from_relative_path(location.package.clone(), &location.relative_path)
            .map_err(|error| {
            CompileError::semantic(
                GraphcalError::internal_error(
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
        let resolved_imports = imports
            .into_iter()
            .map(|(key, (owner, resolved))| {
                let source_file = match owner {
                    ImportOwner::ThisFile => dag_id.clone(),
                    ImportOwner::Dependency(dependency) => dependency,
                };
                (key, resolved.target_from(&source_file))
            })
            .collect();
        let inline_dags = lift_inline_dags(parsed.ast(), &dag_id, stem, |path| {
            let Some(ModuleResolution::Resolved(resolved)) = parsed.resolution(path) else {
                return None;
            };
            let source_file = if resolved.file == *file {
                Some(dag_id.clone())
            } else {
                self.built
                    .get(&resolved.file)
                    .map(|loaded| loaded.dag_id.clone())
            }?;
            Some(resolved.target_from(&source_file))
        });

        self.loading.pop();
        self.built.insert(file.clone(), {
            let (source, source_id, ast) = parsed.into_file().into_parts();
            LoadedFile::new(
                file.path().to_path_buf(),
                dag_id.clone(),
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
        for dependency in loading_dependency_paths(parsed.ast(), file_stem(file.path())) {
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
                    return Err(failure
                        .to_error(path, src, parsed.source_id(), self.sources)
                        .into());
                }
                None => {
                    return Err(ResolveFailure::FileNotFound
                        .to_error(path, src, parsed.source_id(), self.sources)
                        .into());
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
            imports.insert(ModulePathKey::from_path(path), (owner, resolved));
        }
        Ok(imports)
    }

    /// Load the files named by inline-DAG body imports/includes. Unresolved
    /// body paths stay in the AST for the module resolver to report with their
    /// spans; only a path outside the project root is an error here.
    fn load_dag_body_dependencies(
        &mut self,
        file: &K,
        parsed: &ParsedSource<K>,
    ) -> Result<(), Stop> {
        for dependency in loading_dependency_paths(parsed.ast(), file_stem(file.path())) {
            if dependency.site != DependencySite::DagBody {
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
}

pub(super) fn file_root_self_import_error(
    path: &ModulePath,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    CompileError::Load(LoadError::FileRootSelfImport {
        path: path.display_path(),
        src: src.clone(),
        span: path.span().into(),
    })
}

/// Reject `import <file stem>` at file root: a file cannot import itself as a
/// module, unless the stem also names one of its inline DAGs.
pub(super) fn reject_file_root_stem_imports(
    declarations: &[Declaration],
    file_stem: &str,
    src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    let dag_names: HashSet<String> = collect_inline_dag_names(declarations);
    declarations.iter().try_for_each(|declaration| {
        let Some((path, FileRootDependencyKind::Import)) = file_root_dependency(declaration) else {
            return Ok(());
        };
        let is_file_root_self_import = path.segments.len() == 1
            && path.segments[0].name.as_str() == file_stem
            && !dag_names.contains(path.segments[0].name.as_str());
        if is_file_root_self_import {
            Err(file_root_self_import_error(path, src))
        } else {
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests;
