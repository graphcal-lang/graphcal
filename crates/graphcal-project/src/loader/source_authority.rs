//! Module source authorities and the loader's one IO walk.
//!
//! A [`ModuleSourceAuthority`] answers exactly two project-mode questions:
//! which package source tree owns a file, and which package the first segment
//! of a module path selects. Everything else is one generic algorithm here:
//! reading and parsing a file, the reserved standard-library namespace,
//! locating the longest file prefix of a module path inside the selected
//! package tree, the source-root sandbox, and the depth-first fetch order.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::dag_id::DagPackageId;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::source_registry::{SourceOrigin, SourceRegistry};
use graphcal_compiler::syntax::ast::ModulePath;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_io::{
    EntryLimit, FileSystemReadError, FileSystemReader, OutsideRootError, ProjectIngestionPolicy,
};
use graphcal_package::PackageManifest;

use super::budget::{
    LoaderArtifact, LoaderBudgetState, LoaderReadError, PackageAuthorityError, io_not_found,
    loader_manifest_error,
};
use super::module_file::{ModuleFileFailure, ModuleFilePath};
use super::module_path::PackageSelector;
use super::source_snapshot::{
    FetchedFile, ModuleLocation, ModuleResolution, ModuleRoot, ParsedFile, ParsedSource,
    ResolveFailure, ResolvedFile, SourceKey, SourceSnapshot,
};
use crate::compile_error::CompileError;
use crate::load_error::LoadError;

/// Read capability and semantic identity of one package's source tree.
pub(super) struct SourceTree<'a> {
    /// Capability through which the tree's files are read and canonicalized.
    pub(super) reader: &'a dyn FileSystemReader,
    /// Sandbox root: every module file must canonicalize below it.
    pub(super) root: &'a Path,
    /// Package owning the semantic identity of the tree's files.
    pub(super) package: DagPackageId,
    /// Where the module paths of the tree's files are spelled from.
    pub(super) module_root: ModuleRoot,
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

    /// Package whose module namespace `selector` names when imported from the
    /// file `from`.
    ///
    /// # Errors
    ///
    /// Returns the [`ResolveFailure`] recorded for the path when the selector
    /// names no package visible from `from`.
    fn select_package(
        &self,
        selector: PackageSelector<'_>,
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
    let mut sources = SourceRegistry::new();
    let mut entries = DirectoryEntries::default();
    let mut pending = vec![root.clone()];
    while let Some(file) = pending.pop() {
        if files.contains_key(&file) {
            continue;
        }
        let fetched = fetch_file(
            authority,
            &file,
            &mut sources,
            &mut entries,
            budget,
            cancellation,
        )?;
        let Ok(parsed) = &fetched else {
            files.insert(file, fetched);
            break;
        };
        pending.extend(parsed.dependency_files(&file).into_iter().rev().cloned());
        files.insert(file, fetched);
    }
    Ok(SourceSnapshot {
        root,
        files,
        sources,
    })
}

/// Read, parse, and resolve the dependency paths of one file. Read and parse
/// failures are recorded in the fetched file; `Err` aborts the load
/// (cooperative cancellation).
fn fetch_file<A: ModuleSourceAuthority>(
    authority: &A,
    file: &A::Key,
    sources: &mut SourceRegistry,
    entries: &mut DirectoryEntries,
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
        sources,
        budget,
        cancellation,
    ) {
        Ok(parsed) => parsed,
        Err(Outcome::Cancelled) => return Err(Cancelled),
        Err(Outcome::Failed(error)) => return Ok(Err(error)),
    };
    cancellation.checkpoint()?;
    let location = ModuleLocation {
        module_path: tree.module_root.module_path_of(file.path()),
        package: tree.package,
        relative_path: tree.module_root.identity_path(file.path(), tree.root),
    };
    let stem = tree.module_root.stem_reference(file.path());
    ParsedSource::resolve(location, parsed, stem, |path| {
        cancellation.checkpoint()?;
        resolve_module(authority, path, file, entries, cancellation)
    })
    .map(Ok)
}

/// Read one source file through the bounded capability, then parse and
/// desugar it under the diagnostic `name`.
fn read_source_file(
    reader: &dyn FileSystemReader,
    path: &Path,
    name: &str,
    sources: &mut SourceRegistry,
    budget: &mut LoaderBudgetState,
    cancellation: &CancellationToken,
) -> Result<ParsedFile, Outcome<CompileError>> {
    let source = budget
        .read_text(reader, path, LoaderArtifact::SourceFile, cancellation)
        .map_err(|outcome| {
            outcome.map_failed(|error| match error {
                LoaderReadError::Filesystem(filesystem) if is_not_found(&filesystem) => {
                    io_not_found(path)
                }
                other => CompileError::Load(LoadError::SourceReadFailed {
                    path: path.to_path_buf(),
                    reason: other.to_string(),
                }),
            })
        })?;
    ParsedFile::parse(
        sources,
        SourceOrigin::File(path.to_path_buf()),
        name,
        Arc::new(source),
        cancellation,
    )
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
///
/// A candidate must be spelled exactly as its directory entries are, and the
/// file must have a module name ([`verify_module_file`]), so resolution is
/// the same on case-sensitive and case-insensitive filesystems and one file
/// never loads under two identities.
///
/// # Errors
///
/// Returns only cooperative cancellation; resolution failures are values.
pub(super) fn resolve_module<A: ModuleSourceAuthority>(
    authority: &A,
    path: &ModulePath,
    from: &A::Key,
    entries: &mut DirectoryEntries,
    cancellation: &CancellationToken,
) -> Result<ModuleResolution<A::Key>, Cancelled> {
    let Ok(selector) = PackageSelector::classify(path) else {
        return Ok(ModuleResolution::Failed(
            ResolveFailure::StdlibNotImplemented,
        ));
    };
    let selected = match authority.select_package(selector, from) {
        Ok(selected) => selected,
        Err(failure) => return Ok(ModuleResolution::Failed(failure)),
    };
    let tree = match authority.tree(&selected.package) {
        Ok(tree) => tree,
        Err(error) => {
            return Ok(ModuleResolution::Failed(ResolveFailure::PackageAuthority(
                error,
            )));
        }
    };
    let namespace_dir = tree.root.join(&selected.namespace_dir);
    let module_segments = selector.module_segments();
    let mut mismatch = None;
    for file_segment_count in (0..=module_segments.len()).rev() {
        let mut file_path = module_segments[..file_segment_count]
            .iter()
            .fold(namespace_dir.clone(), |file_path, segment| {
                file_path.join(segment.name.as_str())
            });
        file_path.set_extension("gcl");
        // Only a missing file moves on to a shorter prefix. A sandbox refusal
        // or an unreadable entry is reported, never treated as absence: an
        // escaping symlink must not resolve to an inline DAG of a shorter
        // prefix instead.
        let canonical = match tree.reader.canonicalize(&file_path) {
            Ok(canonical) => canonical,
            Err(error) if OutsideRootError::find(&error).is_some() => {
                return Ok(ModuleResolution::OutsideRoot);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(error) => {
                return Ok(ModuleResolution::Failed(ResolveFailure::Unreadable {
                    file: file_path,
                    reason: error.to_string(),
                }));
            }
        };
        // Path sandboxing: modules must stay inside their package's root.
        if !canonical.starts_with(tree.root) {
            return Ok(ModuleResolution::OutsideRoot);
        }
        if let ModuleRoot::SourceDir(source_dir) = &tree.module_root {
            // A candidate found only under another letter case does not exist
            // on a case-sensitive filesystem, so it is skipped like a missing
            // one: resolution is the same on every host. It is reported only
            // if no shorter prefix resolves either.
            if let Ok(relative) = file_path.strip_prefix(source_dir) {
                match verify_spelling(tree.reader, source_dir, relative, entries, cancellation)? {
                    Ok(()) => {}
                    Err(failure @ ModuleFileFailure::SpellingMismatch { .. }) => {
                        mismatch.get_or_insert(failure);
                        continue;
                    }
                    Err(failure) => {
                        return Ok(ModuleResolution::Failed(ResolveFailure::ModuleFile(
                            failure,
                        )));
                    }
                }
            }
            if let Err(failure) =
                verify_module_file(tree.reader, source_dir, &canonical, entries, cancellation)?
            {
                return Ok(ModuleResolution::Failed(ResolveFailure::ModuleFile(
                    failure,
                )));
            }
        }
        let inline_path = module_segments[file_segment_count..]
            .iter()
            .map(|segment| DeclName::classify(segment.name.atom().clone()))
            .collect();
        return Ok(ModuleResolution::Resolved(ResolvedFile {
            file: A::Key::in_package(selected.package, canonical),
            inline_path,
        }));
    }
    Ok(ModuleResolution::Failed(mismatch.map_or(
        ResolveFailure::FileNotFound,
        ResolveFailure::ModuleFile,
    )))
}

/// Directory listings fetched while checking on-disk spellings, so a load
/// lists each directory at most once.
#[derive(Debug, Default)]
pub(super) struct DirectoryEntries(HashMap<PathBuf, HashSet<OsString>>);

impl DirectoryEntries {
    /// Names in `directory`, listed through `reader` on first use.
    fn names(
        &mut self,
        reader: &dyn FileSystemReader,
        directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<&HashSet<OsString>, Outcome<FileSystemReadError>> {
        if !self.0.contains_key(directory) {
            let limit = EntryLimit::new(ProjectIngestionPolicy::default().max_entries());
            let names = reader.read_directory_bounded(directory, limit, cancellation)?;
            self.0
                .insert(directory.to_path_buf(), names.into_iter().collect());
        }
        Ok(&self.0[directory])
    }
}

/// Check a package file canonicalized to `canonical` before it is loaded:
///
/// - its spelling must match its directory entries exactly below
///   `source_dir`, so a case-insensitive filesystem (macOS, Windows) cannot
///   load one file under two canonical spellings;
/// - its path below `source_dir` must name a module ([`ModuleFilePath`]).
///
/// A file reached through an in-root symlink that leaves the source
/// directory is not checked.
///
/// # Errors
///
/// Returns only cooperative cancellation; a failed check is the inner
/// [`ModuleFileFailure`].
pub(super) fn verify_module_file(
    reader: &dyn FileSystemReader,
    source_dir: &Path,
    canonical: &Path,
    entries: &mut DirectoryEntries,
    cancellation: &CancellationToken,
) -> Result<Result<(), ModuleFileFailure>, Cancelled> {
    let Ok(relative) = canonical.strip_prefix(source_dir) else {
        return Ok(Ok(()));
    };
    if let Err(failure) = verify_spelling(reader, source_dir, relative, entries, cancellation)? {
        return Ok(Err(failure));
    }
    Ok(ModuleFilePath::parse(relative)
        .map(drop)
        .map_err(|error| ModuleFileFailure::Invalid {
            path: canonical.to_path_buf(),
            error,
        }))
}

/// Check the root file of a package load like an imported module file, so
/// the entry is rejected under a name or spelling an import could not use
/// (`notes.txt`, `MyModule.gcl`, `Lib.gcl` for `lib.gcl`).
///
/// # Errors
///
/// Returns the failed check's diagnostic, or cooperative cancellation.
pub(super) fn verify_root_module_file(
    reader: &dyn FileSystemReader,
    source_dir: &Path,
    root_canonical: &Path,
    cancellation: &CancellationToken,
) -> Result<(), Outcome<CompileError>> {
    let source_dir = reader
        .canonicalize(source_dir)
        .unwrap_or_else(|_| source_dir.to_path_buf());
    verify_module_file(
        reader,
        &source_dir,
        root_canonical,
        &mut DirectoryEntries::default(),
        cancellation,
    )
    .map_err(|Cancelled| Outcome::Cancelled)?
    .map_err(|failure| Outcome::Failed(failure.to_load_error().into()))
}

/// Check that every component of `relative` below `base` is spelled exactly
/// as its directory entry.
fn verify_spelling(
    reader: &dyn FileSystemReader,
    base: &Path,
    relative: &Path,
    entries: &mut DirectoryEntries,
    cancellation: &CancellationToken,
) -> Result<Result<(), ModuleFileFailure>, Cancelled> {
    let mut directory = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let names = match entries.names(reader, &directory, cancellation) {
            Ok(names) => names,
            Err(Outcome::Cancelled) => return Err(Cancelled),
            Err(Outcome::Failed(error)) => {
                return Ok(Err(ModuleFileFailure::Unreadable {
                    path: directory,
                    reason: error.to_string(),
                }));
            }
        };
        if !names.contains(name) {
            let folded = name.to_string_lossy().to_lowercase();
            let on_disk = names
                .iter()
                .find(|entry| entry.to_string_lossy().to_lowercase() == folded)
                .cloned();
            return Ok(Err(ModuleFileFailure::SpellingMismatch {
                path: directory.join(name),
                on_disk,
            }));
        }
        directory.push(name);
    }
    Ok(Ok(()))
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
            module_root: self.manifest.map_or(ModuleRoot::Standalone, |manifest| {
                ModuleRoot::SourceDir(manifest.source_dir.join_to(self.project_root))
            }),
        })
    }

    fn select_package(
        &self,
        selector: PackageSelector<'_>,
        _from: &PathBuf,
    ) -> Result<SelectedPackage<()>, ResolveFailure> {
        // Without a manifest the project is a single standalone file whose
        // only legal path is its own stem (Concept 7 self-reference), which
        // never reaches resolution.
        let Some(manifest) = self.manifest else {
            return Err(ResolveFailure::CrossFileImportInVirtualPackage);
        };
        // Real package: the first segment must match the package name.
        if !selector.names(&manifest.name) {
            return Err(ResolveFailure::PackageNameMismatch {
                package_name: manifest.name.clone(),
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
