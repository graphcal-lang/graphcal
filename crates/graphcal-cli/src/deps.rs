//! Imperative shell for `graphcal deps` commands.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use graphcal_eval::loader::discover_project_root;
use graphcal_io::{
    ByteLimit, EntryLimit, FileSystemEntryKind, FileSystemReadError, FileSystemReader, NeverCancel,
    ProjectIngestionPolicy, RealFileSystem, SourceTreeHash, SourceTreeHashLimits,
    create_file_atomically, hash_source_tree as hash_package_source_tree,
    replace_file_atomically_if_unchanged,
};
use graphcal_package::{
    DependencyName, DependencySpec, GitCommitHash, GitTransport, GitUrl, LOCK_VERSION,
    LockedPackage, LockedPlugin, Lockfile, PackageGraph, PackageInstanceId, PackageManifest,
    PackageName, PackageSource, PackageSourceDirectory, PluginArtifactPath,
    PluginArtifactPathError, STDLIB_VERSION, Sha256Digest, Sha256DigestError, SourceTreeHashes,
    parse_manifest_str,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

const CACHE_ENV: &str = "GRAPHCAL_CACHE_DIR";
const FETCHED_COMMIT_REF: &str = "refs/remotes/origin/graphcal-lock";

type BoxError = Box<dyn StdError + Send + Sync>;

#[derive(Debug, Clone, Copy)]
enum IngestionArtifact {
    Source,
    Manifest,
    Lockfile,
}

impl IngestionArtifact {
    const fn limit(self, policy: ProjectIngestionPolicy) -> ByteLimit {
        match self {
            Self::Source => policy.source_file(),
            Self::Manifest => policy.manifest(),
            Self::Lockfile => policy.lockfile(),
        }
    }

    const fn resource(self) -> IngestionResource {
        match self {
            Self::Source => IngestionResource::SourceFileBytes,
            Self::Manifest => IngestionResource::ManifestBytes,
            Self::Lockfile => IngestionResource::LockfileBytes,
        }
    }
}

/// Package-ingestion resource category exhausted during lock generation.
#[derive(Debug, Clone, Copy)]
pub enum IngestionResource {
    /// One Graphcal source file.
    SourceFileBytes,
    /// One package manifest.
    ManifestBytes,
    /// One lockfile.
    LockfileBytes,
    /// One WASM plugin artifact.
    PluginBytes,
    /// Aggregate files, directories, and artifacts.
    Entries,
    /// Aggregate bytes across package ingestion.
    TotalBytes,
}

impl std::fmt::Display for IngestionResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceFileBytes => formatter.write_str("source-file byte"),
            Self::ManifestBytes => formatter.write_str("manifest byte"),
            Self::LockfileBytes => formatter.write_str("lockfile byte"),
            Self::PluginBytes => formatter.write_str("plugin byte"),
            Self::Entries => formatter.write_str("entry-count"),
            Self::TotalBytes => formatter.write_str("aggregate byte"),
        }
    }
}

struct PackageIngestionBudget {
    policy: ProjectIngestionPolicy,
    entries: u64,
    bytes: u64,
}

impl Default for PackageIngestionBudget {
    fn default() -> Self {
        Self::new(ProjectIngestionPolicy::default())
    }
}

impl PackageIngestionBudget {
    const fn new(policy: ProjectIngestionPolicy) -> Self {
        Self {
            policy,
            entries: 0,
            bytes: 0,
        }
    }

    const fn remaining_entries(&self) -> u64 {
        self.policy.max_entries().saturating_sub(self.entries)
    }

    const fn remaining_bytes(&self) -> u64 {
        self.policy.max_total_bytes().saturating_sub(self.bytes)
    }

    fn account_entry(&mut self, path: &Path) -> Result<(), DepsError> {
        if self.entries >= self.policy.max_entries() {
            return Err(DepsError::IngestionLimit {
                path: path.to_path_buf(),
                resource: IngestionResource::Entries,
                limit: self.policy.max_entries(),
            });
        }
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| DepsError::IngestionLimit {
                path: path.to_path_buf(),
                resource: IngestionResource::Entries,
                limit: self.policy.max_entries(),
            })?;
        Ok(())
    }

    fn account_bytes(&mut self, path: &Path, bytes: u64) -> Result<(), DepsError> {
        let total = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| DepsError::IngestionLimit {
                path: path.to_path_buf(),
                resource: IngestionResource::TotalBytes,
                limit: self.policy.max_total_bytes(),
            })?;
        if total > self.policy.max_total_bytes() {
            return Err(DepsError::IngestionLimit {
                path: path.to_path_buf(),
                resource: IngestionResource::TotalBytes,
                limit: self.policy.max_total_bytes(),
            });
        }
        self.bytes = total;
        Ok(())
    }

    fn read_text(
        &mut self,
        fs: &dyn FileSystemReader,
        path: &Path,
        artifact: IngestionArtifact,
    ) -> Result<String, DepsError> {
        if self.remaining_entries() == 0 {
            return Err(DepsError::IngestionLimit {
                path: path.to_path_buf(),
                resource: IngestionResource::Entries,
                limit: self.policy.max_entries(),
            });
        }
        let artifact_limit = artifact.limit(self.policy);
        let remaining = self.remaining_bytes();
        let (limit, exhausted) = if artifact_limit.get() <= remaining {
            (artifact_limit, artifact.resource())
        } else {
            (ByteLimit::new(remaining), IngestionResource::TotalBytes)
        };
        let content = fs
            .read_to_string_bounded(path, limit, &NeverCancel)
            .map_err(|source| match source {
                FileSystemReadError::ByteLimitExceeded { .. } => DepsError::IngestionLimit {
                    path: path.to_path_buf(),
                    resource: exhausted,
                    limit: match exhausted {
                        IngestionResource::TotalBytes => self.policy.max_total_bytes(),
                        _ => artifact_limit.get(),
                    },
                },
                other => DepsError::ArtifactRead {
                    path: path.to_path_buf(),
                    source: other,
                },
            })?;
        self.account_entry(path)?;
        self.account_bytes(path, content.len() as u64)?;
        Ok(content)
    }

    fn hash_plugin(
        &mut self,
        fs: &dyn FileSystemReader,
        path: &Path,
    ) -> Result<Sha256Digest, DepsError> {
        if self.remaining_entries() == 0 {
            return Err(DepsError::IngestionLimit {
                path: path.to_path_buf(),
                resource: IngestionResource::Entries,
                limit: self.policy.max_entries(),
            });
        }
        let plugin_limit = self.policy.plugin();
        let remaining = self.remaining_bytes();
        let (limit, exhausted) = if plugin_limit.get() <= remaining {
            (plugin_limit, IngestionResource::PluginBytes)
        } else {
            (ByteLimit::new(remaining), IngestionResource::TotalBytes)
        };
        let hash = fs
            .hash_file_sha256_bounded(path, limit, &NeverCancel)
            .map_err(|source| match source {
                FileSystemReadError::ByteLimitExceeded { .. } => DepsError::IngestionLimit {
                    path: path.to_path_buf(),
                    resource: exhausted,
                    limit: match exhausted {
                        IngestionResource::TotalBytes => self.policy.max_total_bytes(),
                        _ => plugin_limit.get(),
                    },
                },
                other => DepsError::PluginFileUnreadable {
                    path: path.to_path_buf(),
                    source: other,
                },
            })?;
        self.account_entry(path)?;
        self.account_bytes(path, hash.bytes())?;
        Ok(Sha256Digest::from_bytes(hash.sha256()))
    }

    const fn source_tree_limits(&self) -> SourceTreeHashLimits {
        SourceTreeHashLimits::new(
            self.policy.source_tree_file(),
            self.remaining_bytes(),
            self.remaining_entries(),
        )
    }

    fn account_source_tree(&mut self, root: &Path, hash: &SourceTreeHash) -> Result<(), DepsError> {
        let entries =
            self.entries
                .checked_add(hash.entries())
                .ok_or_else(|| DepsError::IngestionLimit {
                    path: root.to_path_buf(),
                    resource: IngestionResource::Entries,
                    limit: self.policy.max_entries(),
                })?;
        if entries > self.policy.max_entries() {
            return Err(DepsError::IngestionLimit {
                path: root.to_path_buf(),
                resource: IngestionResource::Entries,
                limit: self.policy.max_entries(),
            });
        }
        self.account_bytes(root, hash.bytes())?;
        self.entries = entries;
        Ok(())
    }
}

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
    let mut budget = PackageIngestionBudget::default();
    let resolved_lock = resolver.resolve_root(&root, &mut budget)?;
    let plugins = resolve_plugin_pins(&root, &resolved_lock.source_dir, &mut budget)?;
    let lockfile = Lockfile {
        lock_version: LOCK_VERSION,
        created_by: "graphcal".to_string(),
        graphcal_version: env!("CARGO_PKG_VERSION").to_string(),
        stdlib_version: STDLIB_VERSION.to_string(),
        root: resolved_lock.root,
        packages: resolved_lock.packages,
        plugins,
    };

    let graph = lockfile.package_graph(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)?;
    ensure_lock_graph_loadable(&graph)?;

    let content = lockfile.to_deterministic_toml();
    let lockfile_path = root.join("graphcal.lock");
    let root_fs = RealFileSystem::rooted(&root).map_err(|source| DepsError::Canonicalize {
        path: root.clone(),
        source,
    })?;
    let changed = match budget.read_text(&root_fs, &lockfile_path, IngestionArtifact::Lockfile) {
        Ok(existing) if existing == content => false,
        Ok(existing) => {
            replace_file_atomically_if_unchanged(
                &lockfile_path,
                existing.as_bytes(),
                content.as_bytes(),
            )?;
            true
        }
        Err(DepsError::ArtifactRead { source, .. })
            if source.io_kind() == Some(std::io::ErrorKind::NotFound) =>
        {
            create_file_atomically(&lockfile_path, content.as_bytes())?;
            true
        }
        Err(error) => return Err(error),
    };

    Ok(LockOutcome {
        lockfile_path,
        changed,
    })
}

/// Scan the root package's `.gcl` sources for wasm plugin imports and pin
/// each referenced file by its SHA-256.
///
/// Scanning covers every `.gcl` file under the manifest's source directory
/// (not just files reachable from one entry point — packages have no single
/// entry), so a pin exists for every plugin any file of the package can
/// load. Host-registry plugin identities (no `.wasm` suffix) are not
/// artifacts and get no pins.
fn resolve_plugin_pins(
    root: &Path,
    source_dir: &PackageSourceDirectory,
    budget: &mut PackageIngestionBudget,
) -> Result<Vec<LockedPlugin>, DepsError> {
    use graphcal_compiler::syntax::plugin::PluginSourceKind;

    let fs = RealFileSystem::rooted(root).map_err(|source| DepsError::Canonicalize {
        path: root.to_path_buf(),
        source,
    })?;
    let scan_dir = source_dir.join_to(root);
    let gcl_files = collect_gcl_files_bounded(&fs, &scan_dir, budget)?;
    let mut plugin_paths = BTreeSet::new();
    for file in &gcl_files {
        let source = budget.read_text(&fs, file, IngestionArtifact::Source)?;
        let display = file.display().to_string();
        let raw = graphcal_compiler::syntax::parser::Parser::with_name(&source, &display)
            .parse_file()
            .map_err(|err| DepsError::PluginScanParse {
                path: file.clone(),
                message: err.to_string(),
            })?;
        let ast = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw);
        for decl in &ast.declarations {
            if let graphcal_compiler::desugar::desugared_ast::DeclKind::PluginImport(plugin) =
                &decl.kind
                && plugin.path.value.source_kind() == PluginSourceKind::WasmModule
            {
                let path = PluginArtifactPath::new(plugin.path.value.as_str())
                    .map_err(DepsError::PluginPath)?;
                plugin_paths.insert(path);
            }
        }
    }

    plugin_paths
        .into_iter()
        .map(|path| {
            let file = path.join_to(root);
            match fs.entry_kind(&file) {
                Ok(FileSystemEntryKind::File) => {}
                Ok(
                    FileSystemEntryKind::Directory
                    | FileSystemEntryKind::Symlink
                    | FileSystemEntryKind::Other,
                ) => {
                    return Err(DepsError::UnsupportedSourceEntry { path: file });
                }
                Err(source) => {
                    return Err(DepsError::PluginFileUnreadable {
                        path: file,
                        source: FileSystemReadError::Io(source),
                    });
                }
            }
            let sha256 = budget.hash_plugin(&fs, &file)?;
            Ok(LockedPlugin::from_parts(path, sha256))
        })
        .collect()
}

fn collect_gcl_files_bounded(
    fs: &dyn FileSystemReader,
    root: &Path,
    budget: &mut PackageIngestionBudget,
) -> Result<Vec<PathBuf>, DepsError> {
    if !fs.exists(root) {
        return Ok(Vec::new());
    }
    if fs.entry_kind(root).map_err(|source| DepsError::Metadata {
        path: root.to_path_buf(),
        source,
    })? != FileSystemEntryKind::Directory
    {
        return Err(DepsError::UnsupportedSourceEntry {
            path: root.to_path_buf(),
        });
    }

    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut names = fs
            .read_directory_bounded(
                &directory,
                EntryLimit::new(budget.remaining_entries()),
                &NeverCancel,
            )
            .map_err(|source| match source {
                FileSystemReadError::EntryLimitExceeded { .. } => DepsError::IngestionLimit {
                    path: directory.clone(),
                    resource: IngestionResource::Entries,
                    limit: budget.policy.max_entries(),
                },
                other => DepsError::ArtifactRead {
                    path: directory.clone(),
                    source: other,
                },
            })?;
        names.sort();
        for name in names.into_iter().rev() {
            let path = directory.join(name);
            budget.account_entry(&path)?;
            match fs.entry_kind(&path).map_err(|source| DepsError::Metadata {
                path: path.clone(),
                source,
            })? {
                FileSystemEntryKind::Directory => pending.push(path),
                FileSystemEntryKind::File
                    if path.extension() == Some(std::ffi::OsStr::new("gcl")) =>
                {
                    files.push(path);
                }
                FileSystemEntryKind::File => {}
                FileSystemEntryKind::Symlink | FileSystemEntryKind::Other => {
                    return Err(DepsError::UnsupportedSourceEntry { path });
                }
            }
        }
    }
    files.sort();
    Ok(files)
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
    source_dir: PackageSourceDirectory,
    packages: Vec<LockedPackage>,
}

struct LockResolver {
    cache_dir: PathBuf,
}

impl LockResolver {
    const fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    fn resolve_root(
        &self,
        root: &Path,
        budget: &mut PackageIngestionBudget,
    ) -> Result<ResolvedLock, DepsError> {
        let manifest = read_manifest(root, budget)?;
        let root_id = root_package_id(&manifest.name)?;
        let source_dir = manifest.source_dir.clone();
        let mut state = ResolveState::default();
        let root_package = self.resolve_package(
            root_id.clone(),
            manifest,
            PackageSource::Root,
            &mut state,
            budget,
        )?;
        state.insert(root_package);
        let mut packages = state.packages.into_values().collect::<Vec<_>>();
        packages.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(ResolvedLock {
            root: root_id,
            source_dir,
            packages,
        })
    }

    fn resolve_package(
        &self,
        id: PackageInstanceId,
        manifest: PackageManifest,
        source: PackageSource,
        state: &mut ResolveState,
        budget: &mut PackageIngestionBudget,
    ) -> Result<LockedPackage, DepsError> {
        if let Some(existing) = state.packages.get(&id) {
            return Ok(existing.clone());
        }
        if !state.visiting.insert(id.clone()) {
            return Err(DepsError::DependencyCycle { package: id });
        }

        let mut dependencies = BTreeMap::new();
        for (alias, spec) in &manifest.dependencies {
            let dep = self.resolve_dependency(&id, alias, spec, state, budget)?;
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
        Ok(package)
    }

    fn resolve_dependency(
        &self,
        parent: &PackageInstanceId,
        alias: &DependencyName,
        spec: &DependencySpec,
        state: &mut ResolveState,
        budget: &mut PackageIngestionBudget,
    ) -> Result<ResolvedDependency, DepsError> {
        let materialized = self.materialize_git(&spec.git.url, &spec.git.rev, budget)?;
        let manifest = materialized.manifest;
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
        let package = self.resolve_package(id.clone(), manifest, source, state, budget)?;
        state.insert(package);
        Ok(ResolvedDependency { id })
    }

    fn materialize_git(
        &self,
        url: &GitUrl,
        rev: &GitCommitHash,
        budget: &mut PackageIngestionBudget,
    ) -> Result<MaterializedGit, DepsError> {
        let path = self.cache_dir.join("git").join(cache_key(url, rev));
        remove_existing_cache_checkout(&path)?;

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
        materialize_git_revision(url, rev, &tmp_path)?;
        let inspected = inspect_materialized_package(&tmp_path, budget)?;
        match std::fs::rename(&tmp_path, &path) {
            Ok(()) => Ok(MaterializedGit {
                sha256: inspected.sha256,
                manifest: inspected.manifest,
            }),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
                let inspected = inspect_materialized_package(&path, budget)?;
                Ok(MaterializedGit {
                    sha256: inspected.sha256,
                    manifest: inspected.manifest,
                })
            }
            Err(source) => Err(DepsError::Rename {
                from: tmp_path,
                to: path.clone(),
                source,
            }),
        }
    }
}

fn remove_existing_cache_checkout(path: &Path) -> Result<(), DepsError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir_all(path).map_err(|source| DepsError::RemoveDir {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        Ok(_) => Err(DepsError::UnsupportedSourceEntry {
            path: path.to_path_buf(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DepsError::Metadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn materialize_git_revision(
    url: &GitUrl,
    rev: &GitCommitHash,
    path: &Path,
) -> Result<(), DepsError> {
    let url_text = url.to_string();
    let commit_id = gix::hash::ObjectId::from_hex(rev.as_str().as_bytes()).map_err(|source| {
        DepsError::GitMaterialize {
            url: url_text.clone(),
            rev: rev.as_str().to_string(),
            source: Box::new(source),
        }
    })?;
    let fetch_refspec = format!("+{}:{FETCHED_COMMIT_REF}", rev.as_str());
    let should_interrupt = AtomicBool::new(false);
    let open_options = isolated_git_open_options(url.transport());
    let mut prepare_fetch = gix::clone::PrepareFetch::new(
        url_text.as_str(),
        path,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        open_options,
    )
    .map_err(|source| git_materialize_error(url, rev, source))?
    .configure_remote(move |remote| {
        remote
            .with_refspecs([fetch_refspec.as_str()], gix::remote::Direction::Fetch)
            .map(|remote| remote.with_fetch_tags(gix::remote::fetch::Tags::None))
            .map_err(|source| Box::new(source) as BoxError)
    });
    let (repo, _) = prepare_fetch
        .fetch_only(gix::progress::Discard, &should_interrupt)
        .map_err(|source| git_materialize_error(url, rev, source))?;

    checkout_git_commit(&repo, commit_id).map_err(|source| DepsError::GitMaterialize {
        url: url_text,
        rev: rev.as_str().to_string(),
        source,
    })
}

fn isolated_git_open_options(transport: GitTransport) -> gix::open::Options {
    let overrides = match transport {
        GitTransport::Https => None,
        GitTransport::Ssh | GitTransport::Scp => std::env::var_os("GIT_SSH").map(|command| {
            format!(
                "gitoxide.ssh.commandWithoutShellFallback={}",
                command.to_string_lossy()
            )
        }),
    }
    .into_iter();
    gix::open::Options::isolated()
        .config_overrides(overrides)
        .strict_config(true)
}

fn checkout_git_commit(
    repo: &gix::Repository,
    commit_id: gix::hash::ObjectId,
) -> Result<(), BoxError> {
    let workdir = repo.workdir().ok_or_else(|| {
        Box::new(std::io::Error::other(
            "gix clone unexpectedly produced a bare repository",
        )) as BoxError
    })?;
    let commit = repo.find_commit(commit_id)?;
    let tree_id = commit.tree_id()?.detach();
    let mut index = repo.index_from_tree(&tree_id)?;
    let mut checkout_options =
        repo.checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)?;
    checkout_options.destination_is_initially_empty = true;

    gix::worktree::state::checkout(
        &mut index,
        workdir,
        repo.objects.clone().into_arc()?,
        &gix::progress::Discard,
        &gix::progress::Discard,
        &AtomicBool::new(false),
        checkout_options,
    )?;
    index.write(gix::index::write::Options::default())?;
    Ok(())
}

fn git_materialize_error(
    url: &GitUrl,
    rev: &GitCommitHash,
    source: impl StdError + Send + Sync + 'static,
) -> DepsError {
    DepsError::GitMaterialize {
        url: url.to_string(),
        rev: rev.as_str().to_string(),
        source: Box::new(source),
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
    sha256: Sha256Digest,
    manifest: PackageManifest,
}

struct InspectedPackage {
    sha256: Sha256Digest,
    manifest: PackageManifest,
}

fn inspect_materialized_package(
    root: &Path,
    budget: &mut PackageIngestionBudget,
) -> Result<InspectedPackage, DepsError> {
    let manifest = read_manifest(root, budget)?;
    let sha256 = hash_source_tree(root, &manifest.source_dir, budget)?;
    Ok(InspectedPackage { sha256, manifest })
}

fn read_manifest(
    root: &Path,
    budget: &mut PackageIngestionBudget,
) -> Result<PackageManifest, DepsError> {
    let fs = RealFileSystem::rooted(root).map_err(|source| DepsError::Canonicalize {
        path: root.to_path_buf(),
        source,
    })?;
    let path = root.join("graphcal.toml");
    match fs.entry_kind(&path) {
        Ok(FileSystemEntryKind::File) => {}
        Ok(
            FileSystemEntryKind::Directory
            | FileSystemEntryKind::Symlink
            | FileSystemEntryKind::Other,
        ) => return Err(DepsError::UnsupportedSourceEntry { path }),
        Err(source) => {
            return Err(DepsError::Metadata { path, source });
        }
    }
    let content = budget.read_text(&fs, &path, IngestionArtifact::Manifest)?;
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
    key_hash.update(url.to_string().as_bytes());
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
    hasher.update(url.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(rev.as_str().as_bytes());
    hex_string(&hasher.finalize())
}

fn hash_source_tree(
    root: &Path,
    source_dir: &PackageSourceDirectory,
    budget: &mut PackageIngestionBudget,
) -> Result<Sha256Digest, DepsError> {
    let fs = RealFileSystem::rooted(root).map_err(|source| DepsError::Canonicalize {
        path: root.to_path_buf(),
        source,
    })?;
    let hash = hash_package_source_tree(
        &fs,
        root,
        &source_dir.to_path_buf(),
        budget.source_tree_limits(),
        &NeverCancel,
    )?;
    budget.account_source_tree(root, &hash)?;
    Sha256Digest::new(hash.sha256()).map_err(DepsError::Sha256Digest)
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
    /// Deterministic source-tree traversal or hashing failed.
    #[error(transparent)]
    SourceTreeHash(#[from] graphcal_io::SourceTreeHashError),
    /// A generated source-tree digest was not canonical.
    #[error(transparent)]
    Sha256Digest(#[from] Sha256DigestError),
    /// A bounded package artifact read failed.
    #[error("could not read `{}`: {source}", path.display())]
    ArtifactRead {
        path: PathBuf,
        source: FileSystemReadError,
    },
    /// Lock generation exhausted its explicit ingestion policy.
    #[error("package ingestion {resource} limit of {limit} exceeded at `{}`", path.display())]
    IngestionLimit {
        path: PathBuf,
        resource: IngestionResource,
        limit: u64,
    },
    /// Metadata read failed.
    #[error("could not inspect `{}`: {source}", path.display())]
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Atomic lockfile replacement failed.
    #[error(transparent)]
    AtomicWrite(#[from] graphcal_io::AtomicWriteError),
    /// A source file failed to parse while scanning for plugin imports.
    #[error("could not scan `{}` for plugin imports: {message}", path.display())]
    PluginScanParse { path: PathBuf, message: String },
    /// A referenced plugin file could not be read for pinning.
    #[error("could not read plugin artifact `{}` to pin it: {source}", path.display())]
    PluginFileUnreadable {
        path: PathBuf,
        source: FileSystemReadError,
    },
    /// A plugin artifact path failed validation before filesystem access.
    #[error(transparent)]
    PluginPath(#[from] PluginArtifactPathError),
    /// Cache materialization rename failed.
    #[error("could not move `{}` to `{}`: {source}", from.display(), to.display())]
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    /// Directory removal failed.
    #[error("could not remove directory `{}`: {source}", path.display())]
    RemoveDir {
        path: PathBuf,
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
    /// Git materialization failed.
    #[error("could not materialize Git dependency `{url}` at `{rev}`: {source}")]
    GitMaterialize {
        url: String,
        rev: String,
        source: BoxError,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_budget(
        source_file: u64,
        manifest: u64,
        plugin: u64,
        source_tree_file: u64,
        max_entries: u64,
        max_total_bytes: u64,
    ) -> PackageIngestionBudget {
        PackageIngestionBudget::new(ProjectIngestionPolicy::new(
            ByteLimit::new(source_file),
            ByteLimit::new(manifest),
            ByteLimit::new(1024),
            ByteLimit::new(plugin),
            ByteLimit::new(source_tree_file),
            max_entries,
            max_total_bytes,
        ))
    }

    fn unique_temp_dir() -> PathBuf {
        // Parallel test threads can observe the same wall-clock tick, so a
        // monotonically increasing counter makes the directory unique even
        // when two tests start in the same nanosecond.
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "graphcal-deps-test-{}-{nanos}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn resolve_plugin_pins_scans_sources_and_hashes_files() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src/mission")).unwrap();
        std::fs::create_dir_all(root.join("plugins")).unwrap();
        std::fs::write(
            root.join("src/mission/main.gcl"),
            r#"
import plugin "plugins/demo.wasm" as demo {
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}
import plugin "graphcal:demo" as native {
    fn inverse<D: Dim>(x: D) -> D^-1;
}
node x: Dimensionless = demo.lerp(0.0, 1.0, 0.5);
"#,
        )
        .unwrap();
        let plugin_bytes = b"not-really-wasm; pinning hashes bytes only";
        std::fs::write(root.join("plugins/demo.wasm"), plugin_bytes).unwrap();

        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = PackageIngestionBudget::default();
        let pins = resolve_plugin_pins(&root, &source_dir, &mut budget).unwrap();
        assert_eq!(pins.len(), 1, "host-registry identities get no pins");
        assert_eq!(pins[0].path().to_string(), "plugins/demo.wasm");
        assert_eq!(
            pins[0].sha256().to_string(),
            hex_string(&Sha256::digest(plugin_bytes))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_plugin_pins_errors_on_missing_plugin_file() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.gcl"),
            r#"
import plugin "plugins/nope.wasm" as demo {
    fn f(x: Dimensionless) -> Dimensionless;
}
"#,
        )
        .unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = PackageIngestionBudget::default();
        assert!(matches!(
            resolve_plugin_pins(&root, &source_dir, &mut budget).unwrap_err(),
            DepsError::PluginFileUnreadable { path, .. }
                if path.ends_with("plugins/nope.wasm")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_plugin_pins_errors_on_unparsable_source() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/broken.gcl"), "import plugin \"x.wasm").unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = PackageIngestionBudget::default();
        assert!(matches!(
            resolve_plugin_pins(&root, &source_dir, &mut budget).unwrap_err(),
            DepsError::PluginScanParse { .. }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_paths_are_validated_before_filesystem_access() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.gcl"),
            r#"
import plugin "../outside.wasm" as outside {
    fn f(x: Dimensionless) -> Dimensionless;
}
"#,
        )
        .unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = PackageIngestionBudget::default();

        assert!(matches!(
            resolve_plugin_pins(&root, &source_dir, &mut budget),
            Err(DepsError::PluginPath(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_pinning_rejects_sparse_artifact_before_allocating_it() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("plugins")).unwrap();
        std::fs::write(
            root.join("src/lib.gcl"),
            r#"
import plugin "plugins/large.wasm" as large {
    fn f(x: Dimensionless) -> Dimensionless;
}
"#,
        )
        .unwrap();
        std::fs::File::create(root.join("plugins/large.wasm"))
            .unwrap()
            .set_len(9)
            .unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = test_budget(1024, 1024, 8, 1024, 100, 4096);

        assert!(matches!(
            resolve_plugin_pins(&root, &source_dir, &mut budget),
            Err(DepsError::IngestionLimit {
                resource: IngestionResource::PluginBytes,
                limit: 8,
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_scan_rejects_oversized_source_before_allocation() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::File::create(root.join("src/large.gcl"))
            .unwrap()
            .set_len(9)
            .unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = test_budget(8, 1024, 1024, 1024, 100, 4096);

        assert!(matches!(
            resolve_plugin_pins(&root, &source_dir, &mut budget),
            Err(DepsError::IngestionLimit {
                resource: IngestionResource::SourceFileBytes,
                limit: 8,
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_pinning_rejects_symlink_before_reading_target() {
        let root = unique_temp_dir();
        let outside = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("plugins")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            root.join("src/lib.gcl"),
            r#"
import plugin "plugins/linked.wasm" as linked {
    fn f(x: Dimensionless) -> Dimensionless;
}
"#,
        )
        .unwrap();
        std::fs::write(outside.join("secret.wasm"), b"secret").unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.wasm"),
            root.join("plugins/linked.wasm"),
        )
        .unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = PackageIngestionBudget::default();

        assert!(matches!(
            resolve_plugin_pins(&root, &source_dir, &mut budget),
            Err(DepsError::UnsupportedSourceEntry { path })
                if path.ends_with("plugins/linked.wasm")
        ));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn manifest_read_obeys_shared_per_file_limit() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::File::create(root.join("graphcal.toml"))
            .unwrap()
            .set_len(9)
            .unwrap();
        let mut budget = test_budget(1024, 8, 1024, 1024, 100, 4096);

        assert!(matches!(
            read_manifest(&root, &mut budget),
            Err(DepsError::IngestionLimit {
                resource: IngestionResource::ManifestBytes,
                limit: 8,
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn source_tree_hash_uses_loader_compatible_file_and_aggregate_limits() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("graphcal.toml"), "x").unwrap();
        std::fs::File::create(root.join("src/large.gcl"))
            .unwrap()
            .set_len(9)
            .unwrap();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let mut budget = test_budget(1024, 1024, 1024, 8, 100, 4096);

        let error = hash_source_tree(&root, &source_dir, &mut budget).unwrap_err();
        assert!(
            matches!(
                error,
                DepsError::SourceTreeHash(graphcal_io::SourceTreeHashError::ReadFile {
                    source: FileSystemReadError::ByteLimitExceeded { .. },
                    ..
                })
            ),
            "unexpected error: {error:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn hash_source_tree_rejects_symlinked_source_file() {
        let root = unique_temp_dir();
        let outside = unique_temp_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            root.join("graphcal.toml"),
            "[package]\nname = \"mission\"\nsource_dir = \"src\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/mission.gcl"),
            "node x: Dimensionless = 1.0;\n",
        )
        .unwrap();
        std::fs::write(
            outside.join("secret.gcl"),
            "node secret: Dimensionless = 2.0;\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.join("secret.gcl"), root.join("src/evil.gcl")).unwrap();

        let mut budget = PackageIngestionBudget::default();
        let source_dir = PackageSourceDirectory::new("src").unwrap();
        let err = hash_source_tree(&root, &source_dir, &mut budget).unwrap_err();
        assert!(matches!(
            err,
            DepsError::SourceTreeHash(graphcal_io::SourceTreeHashError::Symlink { path })
                if path.ends_with("src/evil.gcl")
        ));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
