//! Imperative shell for `graphcal deps` commands.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use graphcal_eval::loader::discover_project_root;
use graphcal_io::RealFileSystem;
use graphcal_package::{
    DependencyName, DependencySpec, GitCommitHash, GitUrl, LOCK_VERSION, LockedPackage, Lockfile,
    PackageGraph, PackageInstanceId, PackageManifest, PackageName, PackageSource, STDLIB_VERSION,
    SourceTreeHashes, parse_manifest_str,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

const CACHE_ENV: &str = "GRAPHCAL_CACHE_DIR";

/// Result of a successful `graphcal deps lock` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockOutcome {
    /// Path to the project lockfile.
    pub lockfile_path: PathBuf,
    /// Whether the command wrote new lockfile contents.
    pub changed: bool,
}

/// Run `graphcal deps lock`.
///
/// # Errors
///
/// Returns [`DepsError`] when root discovery, manifest parsing, Git
/// materialization, tree hashing, or lockfile writing fails.
pub fn lock(root_override: Option<&Path>) -> Result<LockOutcome, DepsError> {
    let root = resolve_root(root_override)?;
    let cache = cache_dir()?;
    std::fs::create_dir_all(&cache).map_err(|source| DepsError::CreateDir {
        path: cache.clone(),
        source,
    })?;

    let resolver = LockResolver::new(cache);
    let resolved = resolver.resolve_root(&root)?;
    let lockfile = Lockfile {
        lock_version: LOCK_VERSION,
        created_by: "graphcal".to_string(),
        graphcal_version: env!("CARGO_PKG_VERSION").to_string(),
        stdlib_version: STDLIB_VERSION.to_string(),
        root: resolved.root,
        packages: resolved.packages,
    };

    let graph = lockfile.package_graph(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)?;
    ensure_lock_graph_loadable(&graph)?;

    let content = lockfile.to_deterministic_toml();
    let lockfile_path = root.join("graphcal.lock");
    let changed = match std::fs::read_to_string(&lockfile_path) {
        Ok(existing) if existing == content => false,
        Ok(_) | Err(_) => {
            std::fs::write(&lockfile_path, content).map_err(|source| DepsError::WriteFile {
                path: lockfile_path.clone(),
                source,
            })?;
            true
        }
    };

    Ok(LockOutcome {
        lockfile_path,
        changed,
    })
}

fn ensure_lock_graph_loadable(graph: &PackageGraph) -> Result<(), DepsError> {
    graph
        .package(graph.root())
        .ok_or_else(|| DepsError::Internal("validated lock graph has no root".to_string()))?;
    Ok(())
}

fn resolve_root(root_override: Option<&Path>) -> Result<PathBuf, DepsError> {
    let fs = RealFileSystem::default();
    if let Some(root) = root_override {
        let canonical = std::fs::canonicalize(root).map_err(|source| DepsError::Canonicalize {
            path: root.to_path_buf(),
            source,
        })?;
        let manifest = canonical.join("graphcal.toml");
        if !manifest.is_file() {
            return Err(DepsError::MissingManifest { root: canonical });
        }
        return Ok(canonical);
    }

    let current_dir = std::env::current_dir().map_err(DepsError::CurrentDir)?;
    discover_project_root(&current_dir, &fs).ok_or(DepsError::NoProjectRoot { start: current_dir })
}

fn cache_dir() -> Result<PathBuf, DepsError> {
    if let Some(path) = std::env::var_os(CACHE_ENV) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path).join("graphcal"));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".cache").join("graphcal"))
        .ok_or(DepsError::CacheDirUnavailable)
}

#[derive(Debug)]
struct ResolvedLock {
    root: PackageInstanceId,
    packages: Vec<LockedPackage>,
}

struct LockResolver {
    cache_dir: PathBuf,
}

impl LockResolver {
    fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    fn resolve_root(&self, root: &Path) -> Result<ResolvedLock, DepsError> {
        let manifest = read_manifest(root)?;
        let root_id = root_package_id(&manifest.name)?;
        let mut state = ResolveState::default();
        let root_package = self.resolve_package(
            root_id.clone(),
            manifest,
            PackageSource::Path {
                path: ".".to_string(),
            },
            root.to_path_buf(),
            &mut state,
        )?;
        state.insert(root_package);
        let mut packages = state.packages.into_values().collect::<Vec<_>>();
        packages.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(ResolvedLock {
            root: root_id,
            packages,
        })
    }

    fn resolve_package(
        &self,
        id: PackageInstanceId,
        manifest: PackageManifest,
        source: PackageSource,
        package_root: PathBuf,
        state: &mut ResolveState,
    ) -> Result<LockedPackage, DepsError> {
        if let Some(existing) = state.packages.get(&id) {
            return Ok(existing.clone());
        }
        if !state.visiting.insert(id.clone()) {
            return Err(DepsError::DependencyCycle { package: id });
        }

        let mut dependencies = BTreeMap::new();
        for (alias, spec) in &manifest.dependencies {
            let dep = self.resolve_dependency(&id, alias, spec, state)?;
            dependencies.insert(alias.clone(), dep.id);
        }

        state.visiting.remove(&id);
        let package = LockedPackage {
            id,
            name: manifest.name,
            source_dir: manifest.source_dir,
            source,
            dependencies,
        };
        let _ = package_root;
        Ok(package)
    }

    fn resolve_dependency(
        &self,
        parent: &PackageInstanceId,
        alias: &DependencyName,
        spec: &DependencySpec,
        state: &mut ResolveState,
    ) -> Result<ResolvedDependency, DepsError> {
        let materialized = self.materialize_git(&spec.git.url, &spec.git.rev)?;
        let manifest = read_manifest(&materialized.root)?;
        let expected = spec.expected_package_name(alias);
        if manifest.name != expected {
            return Err(DepsError::DependencyPackageNameMismatch {
                parent: parent.clone(),
                dependency: alias.clone(),
                expected,
                actual: manifest.name,
            });
        }
        let id = git_package_id(&manifest.name, &spec.git.url, &spec.git.rev)?;
        let source = PackageSource::Git {
            url: spec.git.url.clone(),
            requested_rev: spec.git.rev.clone(),
            commit: spec.git.rev.clone(),
            tree_hashes: SourceTreeHashes {
                sha256: materialized.sha256,
            },
        };
        let package =
            self.resolve_package(id.clone(), manifest, source, materialized.root, state)?;
        state.insert(package);
        Ok(ResolvedDependency { id })
    }

    fn materialize_git(
        &self,
        url: &GitUrl,
        rev: &GitCommitHash,
    ) -> Result<MaterializedGit, DepsError> {
        let path = self.cache_dir.join("git").join(cache_key(url, rev));
        if path.is_dir() {
            let sha256 = hash_source_tree(&path)?;
            return Ok(MaterializedGit { root: path, sha256 });
        }

        let parent = path.parent().ok_or_else(|| {
            DepsError::Internal(format!("cache path {} has no parent", path.display()))
        })?;
        std::fs::create_dir_all(parent).map_err(|source| DepsError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
        let tmp = tempfile::Builder::new()
            .prefix("graphcal-git-")
            .tempdir_in(parent)
            .map_err(|source| DepsError::CreateTempDir {
                path: parent.to_path_buf(),
                source,
            })?;
        let tmp_path = tmp.path().to_path_buf();
        git(["init", "--quiet"], Some(&tmp_path))?;
        git(["remote", "add", "origin", url.as_str()], Some(&tmp_path))?;
        git(
            ["fetch", "--quiet", "--depth=1", "origin", rev.as_str()],
            Some(&tmp_path),
        )?;
        git(
            ["checkout", "--quiet", "--detach", rev.as_str()],
            Some(&tmp_path),
        )?;
        let sha256 = hash_source_tree(&tmp_path)?;
        std::fs::rename(&tmp_path, &path).map_err(|source| DepsError::Rename {
            from: tmp_path,
            to: path.clone(),
            source,
        })?;

        Ok(MaterializedGit { root: path, sha256 })
    }
}

#[derive(Debug, Default)]
struct ResolveState {
    packages: BTreeMap<PackageInstanceId, LockedPackage>,
    visiting: BTreeSet<PackageInstanceId>,
}

impl ResolveState {
    fn insert(&mut self, package: LockedPackage) {
        self.packages.insert(package.id.clone(), package);
    }
}

#[derive(Debug)]
struct ResolvedDependency {
    id: PackageInstanceId,
}

#[derive(Debug)]
struct MaterializedGit {
    root: PathBuf,
    sha256: String,
}

fn read_manifest(root: &Path) -> Result<PackageManifest, DepsError> {
    let path = root.join("graphcal.toml");
    let content = std::fs::read_to_string(&path).map_err(|source| DepsError::ReadFile {
        path: path.clone(),
        source,
    })?;
    parse_manifest_str(&content).map_err(|source| DepsError::Manifest { path, source })
}

fn root_package_id(name: &PackageName) -> Result<PackageInstanceId, DepsError> {
    PackageInstanceId::new(format!("pkg-{}", name.as_str())).map_err(DepsError::PackageId)
}

fn git_package_id(
    name: &PackageName,
    url: &GitUrl,
    rev: &GitCommitHash,
) -> Result<PackageInstanceId, DepsError> {
    let mut key_hash = Sha256::new();
    key_hash.update(name.as_str().as_bytes());
    key_hash.update([0]);
    key_hash.update(url.as_str().as_bytes());
    key_hash.update([0]);
    key_hash.update(rev.as_str().as_bytes());
    let key = hex_string(&key_hash.finalize());
    let short_rev = rev.as_str().get(..12).ok_or_else(|| {
        DepsError::Internal(format!("validated Git rev `{}` is too short", rev.as_str()))
    })?;
    PackageInstanceId::new(format!("pkg-{}-{}-{}", name.as_str(), short_rev, &key[..8]))
        .map_err(DepsError::PackageId)
}

fn cache_key(url: &GitUrl, rev: &GitCommitHash) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"git\0");
    hasher.update(url.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(rev.as_str().as_bytes());
    hex_string(&hasher.finalize())
}

fn hash_source_tree(root: &Path) -> Result<String, DepsError> {
    let manifest = read_manifest(root)?;
    let mut files = BTreeMap::new();
    collect_hash_files(root, Path::new("graphcal.toml"), &mut files)?;
    collect_hash_files(root, &manifest.source_dir, &mut files)?;

    let mut hasher = Sha256::new();
    for (relative, path) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        let bytes =
            std::fs::read(&path).map_err(|source| DepsError::ReadFileBytes { path, source })?;
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        hasher.update([0]);
    }
    Ok(hex_string(&hasher.finalize()))
}

fn collect_hash_files(
    root: &Path,
    relative: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), DepsError> {
    if relative
        .components()
        .any(|c| matches!(c, std::path::Component::Normal(name) if name == OsStr::new(".git")))
    {
        return Ok(());
    }
    let path = root.join(relative);
    let metadata = std::fs::metadata(&path).map_err(|source| DepsError::Metadata {
        path: path.clone(),
        source,
    })?;
    if metadata.is_file() {
        files.insert(normalize_relative_path(relative), path);
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(&path).map_err(|source| DepsError::ReadDir {
            path: path.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| DepsError::ReadDir {
                path: path.clone(),
                source,
            })?;
            let child = relative.join(entry.file_name());
            collect_hash_files(root, &child, files)?;
        }
        return Ok(());
    }
    Err(DepsError::UnsupportedSourceEntry { path })
}

fn normalize_relative_path(path: &Path) -> String {
    let mut out = String::new();
    for component in path.components() {
        let std::path::Component::Normal(part) = component else {
            continue;
        };
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&part.to_string_lossy());
    }
    out
}

fn git<I, S>(args: I, cwd: Option<&Path>) -> Result<(), DepsError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.arg("-C").arg(cwd);
    }
    command.args(args);
    let output = command.output().map_err(DepsError::GitSpawn)?;
    if output.status.success() {
        return Ok(());
    }
    Err(DepsError::GitFailed {
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// `graphcal deps` command failure.
#[derive(Debug, Error)]
pub enum DepsError {
    /// Could not read the current directory.
    #[error("could not determine current directory: {0}")]
    CurrentDir(std::io::Error),
    /// No project root was found.
    #[error("could not find graphcal.toml from `{}` or its ancestors", start.display())]
    NoProjectRoot { start: PathBuf },
    /// Explicit root did not contain a manifest.
    #[error("project root `{}` does not contain graphcal.toml", root.display())]
    MissingManifest { root: PathBuf },
    /// Path canonicalization failed.
    #[error("could not canonicalize `{}`: {source}", path.display())]
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
    /// No cache directory could be derived.
    #[error("could not determine Graphcal cache directory; set {CACHE_ENV}")]
    CacheDirUnavailable,
    /// Directory creation failed.
    #[error("could not create directory `{}`: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Temporary directory creation failed.
    #[error("could not create temporary directory under `{}`: {source}", path.display())]
    CreateTempDir {
        path: PathBuf,
        source: std::io::Error,
    },
    /// File read failed.
    #[error("could not read `{}`: {source}", path.display())]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    /// File byte read failed.
    #[error("could not read `{}`: {source}", path.display())]
    ReadFileBytes {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Directory read failed.
    #[error("could not read directory `{}`: {source}", path.display())]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Metadata read failed.
    #[error("could not inspect `{}`: {source}", path.display())]
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Lockfile write failed.
    #[error("could not write `{}`: {source}", path.display())]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Cache materialization rename failed.
    #[error("could not move `{}` to `{}`: {source}", from.display(), to.display())]
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    /// Unsupported file type in source hash input.
    #[error("unsupported source entry `{}`", path.display())]
    UnsupportedSourceEntry { path: PathBuf },
    /// Manifest parse failed.
    #[error("invalid manifest `{}`: {source}", path.display())]
    Manifest {
        path: PathBuf,
        source: graphcal_package::ManifestError,
    },
    /// Lock validation failed.
    #[error("{0}")]
    LockValidation(#[from] graphcal_package::LockValidationError),
    /// Generated package id was invalid.
    #[error(transparent)]
    PackageId(graphcal_package::PackageInstanceIdError),
    /// Git process could not start.
    #[error("could not run git: {0}")]
    GitSpawn(std::io::Error),
    /// Git command failed.
    #[error("git failed with status {status:?}: {stderr}")]
    GitFailed { status: Option<i32>, stderr: String },
    /// Recursive dependency cycle.
    #[error("dependency cycle while resolving package `{package}`")]
    DependencyCycle { package: PackageInstanceId },
    /// Fetched package did not match the requested real package name.
    #[error(
        "package `{parent}` dependency `{dependency}` expected package `{expected}` but fetched `{actual}`"
    )]
    DependencyPackageNameMismatch {
        parent: PackageInstanceId,
        dependency: DependencyName,
        expected: PackageName,
        actual: PackageName,
    },
    /// Internal invariant violation.
    #[error("internal deps lock error: {0}")]
    Internal(String),
}
