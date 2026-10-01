//! Module source authorities and the loader's one IO walk.
//!
//! A [`ModuleSourceAuthority`] answers exactly two project-mode questions:
//! which package source tree owns a file, and which package the first segment
//! of a module path selects. Everything else is one generic algorithm here:
//! reading and parsing a file, the reserved standard-library namespace,
//! locating the longest file prefix of a module path inside the selected
//! package tree, the source-root sandbox, and the depth-first fetch order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::dag_id::DagPackageId;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::syntax::ast::ModulePath;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_io::{FileSystemReadError, FileSystemReader};
use graphcal_package::PackageManifest;

use super::budget::{
    LoaderArtifact, LoaderBudgetState, LoaderReadError, PackageAuthorityError, io_not_found,
    loader_manifest_error,
};
use super::source_snapshot::{
    FetchedFile, ModuleLocation, ModuleResolution, ParsedFile, ParsedSource, ResolveFailure,
    ResolvedFile, SourceKey, SourceSnapshot,
};
use crate::compile_error::CompileError;

/// Read capability and semantic identity of one package's source tree.
pub(super) struct SourceTree<'a> {
    /// Capability through which the tree's files are read and canonicalized.
    pub(super) reader: &'a dyn FileSystemReader,
    /// Sandbox root: every module file must canonicalize below it.
    pub(super) root: &'a Path,
    /// Package owning the semantic identity of the tree's files.
    pub(super) package: DagPackageId,
}

/// Package selected by the first segment of a module path.
pub(super) struct SelectedPackage<P> {
    /// Package tree that owns the rest of the path.
    pub(super) package: P,
    /// Directory of the package's module namespace relative to its tree root
    /// (`<source_dir>/<package name>`); the remaining path segments walk it.
    pub(super) namespace_dir: PathBuf,
}

/// Package-aware source authority of one project load.
pub(super) trait ModuleSourceAuthority {
    type Key: SourceKey;

    /// Source tree of `package`.
    ///
    /// # Errors
    ///
    /// Returns [`PackageAuthorityError`] when the package has no captured
    /// source authority.
    fn tree(
        &self,
        package: &<Self::Key as SourceKey>::Package,
    ) -> Result<SourceTree<'_>, PackageAuthorityError>;

    /// Package whose module namespace the first segment of `path` names when
    /// imported from the file `from`.
    ///
    /// # Errors
    ///
    /// Returns the [`ResolveFailure`] recorded for `path` when the first
    /// segment names no package visible from `from`.
    fn select_package(
        &self,
        path: &ModulePath,
        from: &Self::Key,
    ) -> Result<SelectedPackage<<Self::Key as SourceKey>::Package>, ResolveFailure>;
}

/// Fetch every source file reachable from `root` in load order (depth-first
/// preorder, the order in which the builder visits files), stopping after the
/// first file that cannot be read or parsed.
///
/// # Errors
///
/// Returns only cooperative cancellation; read and parse failures are
/// recorded in the snapshot.
pub(super) fn fetch_source_snapshot<A: ModuleSourceAuthority>(
    authority: &A,
    root: A::Key,
    budget: &mut LoaderBudgetState,
    cancellation: &CancellationToken,
) -> Result<SourceSnapshot<A::Key>, Cancelled> {
    let mut files = HashMap::new();
    let mut pending = vec![root.clone()];
    while let Some(file) = pending.pop() {
        if files.contains_key(&file) {
            continue;
        }
        let fetched = fetch_file(authority, &file, budget, cancellation)?;
        let Ok(parsed) = &fetched else {
            files.insert(file, fetched);
            break;
        };
        pending.extend(parsed.dependency_files(&file).into_iter().rev().cloned());
        files.insert(file, fetched);
    }
    Ok(SourceSnapshot { root, files })
}

/// Read, parse, and resolve the dependency paths of one file. Read and parse
/// failures are recorded in the fetched file; `Err` aborts the load
/// (cooperative cancellation).
fn fetch_file<A: ModuleSourceAuthority>(
    authority: &A,
    file: &A::Key,
    budget: &mut LoaderBudgetState,
    cancellation: &CancellationToken,
) -> Result<FetchedFile<A::Key>, Cancelled> {
    cancellation.checkpoint()?;
    let tree = match authority.tree(file.package()) {
        Ok(tree) => tree,
        Err(error) => return Ok(Err(loader_manifest_error(error))),
    };
    let parsed = match read_source_file(
        tree.reader,
        file.path(),
        &file.diagnostic_name(),
        budget,
        cancellation,
    ) {
        Ok(parsed) => parsed,
        Err(Outcome::Cancelled) => return Err(Cancelled),
        Err(Outcome::Failed(error)) => return Ok(Err(error)),
    };
    cancellation.checkpoint()?;
    let location = ModuleLocation {
        package: tree.package,
        relative_path: file
            .path()
            .strip_prefix(tree.root)
            .unwrap_or_else(|_| file.path())
            .to_path_buf(),
    };
    ParsedSource::resolve(location, parsed, |path| {
        cancellation.checkpoint()?;
        Ok(resolve_module(authority, path, file))
    })
    .map(Ok)
}

/// Read one source file through the bounded capability, then parse and
/// desugar it under the diagnostic `name`.
fn read_source_file(
    reader: &dyn FileSystemReader,
    path: &Path,
    name: &str,
    budget: &mut LoaderBudgetState,
    cancellation: &CancellationToken,
) -> Result<ParsedFile, Outcome<CompileError>> {
    let source = budget
        .read_text(reader, path, LoaderArtifact::SourceFile, cancellation)
        .map_err(|error| match error {
            LoaderReadError::Filesystem(FileSystemReadError::Cancelled) => Outcome::Cancelled,
            LoaderReadError::Filesystem(filesystem) if is_not_found(&filesystem) => {
                io_not_found(path).into()
            }
            other => loader_manifest_error(format!(
                "could not read source `{}`: {other}",
                path.display()
            ))
            .into(),
        })?;
    ParsedFile::parse(name, Arc::new(source), cancellation)
}

fn is_not_found(error: &FileSystemReadError) -> bool {
    error.io_kind() == Some(std::io::ErrorKind::NotFound)
}

/// Resolve a module path imported from `from` to a canonical file and the
/// exact inline-DAG path inside it.
///
/// The path is absolute from a package namespace: the first segment selects
/// the package (through `authority`), and the longest prefix of the remaining
/// segments that names a physical source file under that package's namespace
/// directory wins (`nasa.rocket` resolves to
/// `<root>/<source_dir>/nasa/rocket.gcl`). Any remaining segments are an exact
/// nested inline-DAG path in that file.
pub(super) fn resolve_module<A: ModuleSourceAuthority>(
    authority: &A,
    path: &ModulePath,
    from: &A::Key,
) -> ModuleResolution<A::Key> {
    if names_stdlib(path) {
        return ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented);
    }
    let selected = match authority.select_package(path, from) {
        Ok(selected) => selected,
        Err(failure) => return ModuleResolution::Failed(failure),
    };
    let tree = match authority.tree(&selected.package) {
        Ok(tree) => tree,
        Err(error) => {
            return ModuleResolution::Failed(ResolveFailure::Manifest {
                message: error.to_string(),
            });
        }
    };
    let namespace_dir = tree.root.join(&selected.namespace_dir);
    let module_segments = &path.segments()[1..];
    for file_segment_count in (0..=module_segments.len()).rev() {
        let mut file_path = module_segments[..file_segment_count]
            .iter()
            .fold(namespace_dir.clone(), |file_path, segment| {
                file_path.join(segment.name.as_str())
            });
        file_path.set_extension("gcl");
        let Ok(canonical) = tree.reader.canonicalize(&file_path) else {
            continue;
        };
        // Path sandboxing: modules must stay inside their package's root.
        if !canonical.starts_with(tree.root) {
            return ModuleResolution::OutsideRoot;
        }
        let inline_path = module_segments[file_segment_count..]
            .iter()
            .map(|segment| DeclName::classify(segment.name.atom().clone()))
            .collect();
        return ModuleResolution::Resolved(ResolvedFile {
            file: A::Key::in_package(selected.package, canonical),
            inline_path,
        });
    }
    ModuleResolution::Failed(ResolveFailure::FileNotFound)
}

/// Whether a module path names the reserved (deferred) standard library.
/// Both `graphcal` and `std` first segments are reserved (Concept §6.2).
fn names_stdlib(path: &ModulePath) -> bool {
    matches!(path.segments.first().name.as_str(), "graphcal" | "std")
}

/// Filesystem authority of a single-package project: a real package whose
/// manifest has no dependencies, or a virtual (manifest-less) package.
pub(super) struct ProjectSources<'a> {
    /// Import boundary: every import must resolve inside this directory tree.
    pub(super) project_root: &'a Path,
    pub(super) package_id: &'a DagPackageId,
    /// The package manifest, present only when the root file lives inside the
    /// package namespace.
    pub(super) manifest: Option<&'a PackageManifest>,
    pub(super) fs: &'a dyn FileSystemReader,
}

impl ModuleSourceAuthority for ProjectSources<'_> {
    type Key = PathBuf;

    fn tree(&self, &(): &()) -> Result<SourceTree<'_>, PackageAuthorityError> {
        Ok(SourceTree {
            reader: self.fs,
            root: self.project_root,
            package: self.package_id.clone(),
        })
    }

    fn select_package(
        &self,
        path: &ModulePath,
        _from: &PathBuf,
    ) -> Result<SelectedPackage<()>, ResolveFailure> {
        // Without a manifest the project is a single standalone file whose
        // only legal path is its own stem (Concept 7 self-reference), which
        // never reaches resolution.
        let Some(manifest) = self.manifest else {
            return Err(ResolveFailure::CrossFileImportInVirtualPackage);
        };
        // Real package: the first segment must match the package name.
        if path.segments.first().name.as_str() != manifest.name.as_str() {
            return Err(ResolveFailure::PackageNameMismatch {
                package_name: manifest.name.to_string(),
            });
        }
        Ok(SelectedPackage {
            package: (),
            namespace_dir: manifest
                .source_dir
                .to_path_buf()
                .join(manifest.name.as_str()),
        })
    }
}
