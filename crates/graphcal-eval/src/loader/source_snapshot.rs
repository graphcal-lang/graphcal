//! Immutable source snapshot fetched by the loader's IO shell.
//!
//! The shell reads, parses, and resolves every source file reachable from the
//! root (reads, `canonicalize` probes, and package lookups are IO). Everything
//! the pure project builder needs afterwards is recorded here as data: the
//! parsed file, and the span-free outcome of resolving each import/include
//! path it contains. Diagnostics are rendered only when the builder reaches an
//! offending path, so the reported error follows the builder's traversal.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use super::{ModulePathKey, ResolvedModuleTarget};
use crate::eval::CompileError;
use graphcal_compiler::dag_id::{DagId, DagPackageId};
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::import_cycle::ImportChainFile;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::{DeclKind, ModulePath};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_package::PackageInstanceId;

/// Identity of one source file within a snapshot.
pub(super) trait SourceKey: Clone + Eq + Hash {
    /// Canonical path of the file (retained for I/O and diagnostics).
    fn path(&self) -> &Path;

    /// Label of this file on an import chain.
    fn chain_file(&self) -> ImportChainFile;
}

/// Single-package projects identify files by canonical path alone.
impl SourceKey for PathBuf {
    fn path(&self) -> &Path {
        self
    }

    fn chain_file(&self) -> ImportChainFile {
        ImportChainFile::Path(self.clone())
    }
}

/// A file inside one locked package instance's source authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PackageFileKey {
    pub(super) package: PackageInstanceId,
    pub(super) path: PathBuf,
}

impl SourceKey for PackageFileKey {
    fn path(&self) -> &Path {
        &self.path
    }

    fn chain_file(&self) -> ImportChainFile {
        ImportChainFile::Package {
            package: DagPackageId::new(self.package.as_str()),
            path: self.path.clone(),
        }
    }
}

/// Every source the shell fetched, keyed by file identity.
///
/// The shell stops at the first file it cannot read or parse, so later files
/// in load order may be absent; the builder reports that file's failure before
/// it would need them.
#[derive(Debug)]
pub(super) struct SourceSnapshot<K> {
    pub(super) root: K,
    pub(super) files: HashMap<K, FetchedFile<K>>,
}

/// One fetched file: its parsed content, or the read/parse failure the
/// builder reports when it reaches this file.
pub(super) type FetchedFile<K> = Result<ParsedSource<K>, CompileError>;

/// Semantic module location of a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModuleLocation {
    /// Package owning the file's semantic identity.
    pub(super) package: DagPackageId,
    /// File path relative to the package's module root.
    pub(super) relative_path: PathBuf,
}

/// One parsed and desugared source text with its diagnostic name.
#[derive(Debug)]
pub(super) struct ParsedFile {
    pub(super) source: Arc<String>,
    pub(super) named_source: NamedSource<Arc<String>>,
    pub(super) ast: File,
}

/// Parsed file plus the recorded resolution of each of its dependency paths.
///
/// Construction resolves every path yielded by [`dependency_paths`], so each
/// dependency path of the AST has a recorded outcome.
#[derive(Debug)]
pub(super) struct ParsedSource<K> {
    location: ModuleLocation,
    file: ParsedFile,
    resolutions: HashMap<ModulePathKey, ModuleResolution<K>>,
}

impl<K: SourceKey> ParsedSource<K> {
    /// Record `resolve`'s outcome for every dependency path of `file` (once
    /// per span-free path). `resolve` may abort, e.g. on cancellation.
    pub(super) fn resolve<E>(
        location: ModuleLocation,
        file: ParsedFile,
        mut resolve: impl FnMut(&ModulePath) -> Result<ModuleResolution<K>, E>,
    ) -> Result<Self, E> {
        let mut resolutions = HashMap::new();
        for dependency in dependency_paths(&file.ast) {
            if let Entry::Vacant(slot) =
                resolutions.entry(ModulePathKey::from_path(dependency.path))
            {
                slot.insert(resolve(dependency.path)?);
            }
        }
        Ok(Self {
            location,
            file,
            resolutions,
        })
    }

    pub(super) const fn location(&self) -> &ModuleLocation {
        &self.location
    }

    pub(super) const fn named_source(&self) -> &NamedSource<Arc<String>> {
        &self.file.named_source
    }

    pub(super) const fn ast(&self) -> &File {
        &self.file.ast
    }

    /// Recorded resolution for a dependency path of this file. Only a path
    /// that is not a dependency path of this file can be absent.
    pub(super) fn resolution(&self, path: &ModulePath) -> Option<&ModuleResolution<K>> {
        self.resolutions.get(&ModulePathKey::from_path(path))
    }

    /// Other files this file loads, in load order (duplicates retained).
    pub(super) fn dependency_files<'a>(&'a self, this_file: &K) -> Vec<&'a K> {
        loading_dependency_paths(self.ast(), file_stem(this_file.path()))
            .into_iter()
            .filter_map(|dependency| match self.resolution(dependency.path) {
                Some(ModuleResolution::Resolved(resolved)) if resolved.file != *this_file => {
                    Some(&resolved.file)
                }
                _ => None,
            })
            .collect()
    }

    /// The parsed file, dropping the recorded resolutions.
    pub(super) fn into_file(self) -> ParsedFile {
        self.file
    }
}

/// Span-free outcome of resolving one module path from one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ModuleResolution<K> {
    /// The path names this file (possibly the file itself) and, for any
    /// remaining segments, a nested inline DAG inside it.
    Resolved(ResolvedFile<K>),
    /// The path resolved to a file outside a single-package project's root.
    /// Unlike [`ResolveFailure`]s, this is rejected in inline-DAG bodies too.
    OutsideProjectRoot,
    /// The path could not be resolved.
    Failed(ResolveFailure),
}

/// Resolved owning file plus the exact inline-DAG path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedFile<K> {
    pub(super) file: K,
    pub(super) inline_path: Vec<DeclName>,
}

impl<K> ResolvedFile<K> {
    /// Exact module target once the owning file has its [`DagId`].
    pub(super) fn target_from(&self, source_file: &DagId) -> ResolvedModuleTarget {
        let target = self
            .inline_path
            .iter()
            .fold(source_file.clone(), |owner, name| {
                owner.inline_dag_child(name.clone())
            });
        ResolvedModuleTarget::in_file(source_file.clone(), target)
    }
}

/// Why a module path did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResolveFailure {
    /// The path names the reserved (unimplemented) standard library.
    StdlibNotImplemented,
    /// The first segment does not name the importing package.
    PackageNameMismatch { package_name: String },
    /// No source file matches any prefix of the path.
    FileNotFound,
    /// A file without a package manifest cannot import other files.
    CrossFileImportInVirtualPackage,
    /// The resolved file leaves its package's source root.
    OutsidePackageRoot,
    /// The locked package graph cannot resolve the path.
    NotLocked { message: String },
    /// The lockfile or package authority is inconsistent.
    Manifest { message: String },
}

impl ResolveFailure {
    /// Render this failure at the importing `path` of the file `src`.
    pub(super) fn to_error(
        &self,
        path: &ModulePath,
        src: &NamedSource<Arc<String>>,
    ) -> CompileError {
        let src = src.clone();
        let span = path.span().into();
        CompileError::Eval(match self {
            Self::StdlibNotImplemented => GraphcalError::StdlibNotImplemented {
                path: path.display_path(),
                src,
                span,
            },
            Self::PackageNameMismatch { package_name } => GraphcalError::PackageNameMismatch {
                path_first: path.segments.first().name.to_string(),
                package_name: package_name.clone(),
                src,
                span,
            },
            Self::FileNotFound => GraphcalError::ImportFileNotFound {
                path: path.display_path(),
                src,
                span,
            },
            Self::CrossFileImportInVirtualPackage => {
                GraphcalError::CrossFileImportInVirtualPackage {
                    path: path.display_path(),
                    src,
                    span,
                }
            }
            Self::OutsidePackageRoot => outside_root(path, src),
            Self::NotLocked { message } => GraphcalError::EvalError {
                message: format!("{message}; run `graphcal deps lock` after changing dependencies"),
                src,
                span,
            },
            Self::Manifest { message } => GraphcalError::ManifestError {
                message: message.clone(),
            },
        })
    }
}

/// Error for a path that resolved outside the permitted source root.
pub(super) fn outside_root(path: &ModulePath, src: NamedSource<Arc<String>>) -> GraphcalError {
    GraphcalError::ImportOutsideRoot {
        path: path.display_path(),
        src,
        span: path.span().into(),
    }
}

/// Whether a file-root dependency is an `import` or an `include`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileRootDependencyKind {
    Import,
    Include,
}

/// Where a dependency path is declared within its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DependencySite {
    FileRoot(FileRootDependencyKind),
    DagBody,
}

/// One import/include path together with its declaration site.
#[derive(Debug, Clone, Copy)]
pub(super) struct DependencyPath<'a> {
    pub(super) path: &'a ModulePath,
    pub(super) site: DependencySite,
}

pub(super) const fn file_root_dependency(
    declaration: &Declaration,
) -> Option<(&ModulePath, FileRootDependencyKind)> {
    match &declaration.kind {
        DeclKind::Import(import) => Some((import.path(), FileRootDependencyKind::Import)),
        DeclKind::Include(include) => Some((&include.path, FileRootDependencyKind::Include)),
        _ => None,
    }
}

/// Every import/include path in `ast`: file-root declarations in order, then
/// inline-DAG bodies in preorder.
pub(super) fn dependency_paths(ast: &File) -> Vec<DependencyPath<'_>> {
    ast.declarations
        .iter()
        .filter_map(|declaration| {
            file_root_dependency(declaration).map(|(path, kind)| DependencyPath {
                path,
                site: DependencySite::FileRoot(kind),
            })
        })
        .chain(
            inline_dag_dependency_paths(&ast.declarations)
                .into_iter()
                .map(|path| DependencyPath {
                    path,
                    site: DependencySite::DagBody,
                }),
        )
        .collect()
}

/// Dependency paths that can name another file, in load order. Single-segment
/// paths naming a same-file inline DAG or the file's own stem are same-file
/// references and never drive loading.
pub(super) fn loading_dependency_paths<'a>(
    ast: &'a File,
    file_stem: &str,
) -> Vec<DependencyPath<'a>> {
    let dag_names = collect_inline_dag_names(&ast.declarations);
    dependency_paths(ast)
        .into_iter()
        .filter(|dependency| !is_same_file_reference(dependency.path, &dag_names, file_stem))
        .collect()
}

fn is_same_file_reference(path: &ModulePath, dag_names: &HashSet<String>, file_stem: &str) -> bool {
    let [segment] = path.segments() else {
        return false;
    };
    dag_names.contains(segment.name.as_str()) || segment.name.as_str() == file_stem
}

pub(super) fn collect_inline_dag_names(declarations: &[Declaration]) -> HashSet<String> {
    declarations
        .iter()
        .flat_map(|decl| match &decl.kind {
            DeclKind::Dag(dag) => {
                let mut names = collect_inline_dag_names(&dag.body);
                names.insert(dag.name.value.to_string());
                names
            }
            _ => HashSet::new(),
        })
        .collect()
}

fn inline_dag_dependency_paths(declarations: &[Declaration]) -> Vec<&ModulePath> {
    declarations
        .iter()
        .flat_map(|decl| match &decl.kind {
            DeclKind::Dag(dag) => {
                let body_paths = dag
                    .body
                    .iter()
                    .filter_map(|body_decl| match &body_decl.kind {
                        DeclKind::Import(import_decl) => Some(import_decl.path()),
                        DeclKind::Include(include_decl) => Some(&include_decl.path),
                        _ => None,
                    });
                body_paths
                    .chain(inline_dag_dependency_paths(&dag.body))
                    .collect::<Vec<_>>()
            }
            _ => Vec::new(),
        })
        .collect()
}

/// UTF-8 file stem used for file-root self references (empty when absent).
pub(super) fn file_stem(path: &Path) -> &str {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
}
