//! Imperative shell for `graphcal deps` commands.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use graphcal_eval::loader::discover_project_root;
use graphcal_eval::package_cache::{PackageCacheRoot, PackageCacheRootError};
use graphcal_io::{
    ByteLimit, EntryLimit, FileSystemEntryKind, FileSystemReadError, FileSystemReader, NeverCancel,
    ProjectIngestionPolicy, RealFileSystem, SourceTreeHash, SourceTreeHashLimits,
    create_file_atomically, hash_source_tree as hash_package_source_tree,
    replace_file_atomically_if_unchanged,
};
use graphcal_package::{
    DependencyName, DependencySpec, GitCommitHash, GitSourceId, GitTransport, GitUrl, LOCK_VERSION,
    LockedPackage, LockedPlugin, Lockfile, LockfileParseLimits, PackageGraph, PackageInstanceId,
    PackageManifest, PackageName, PackageSource, PackageSourceDirectory, PluginArtifactPath,
    PluginArtifactPathError, STDLIB_VERSION, Sha256Digest, Sha256DigestError, SourceTreeHashes,
    parse_lockfile_str_with_limits, parse_manifest_str,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

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
    let mut budget = PackageIngestionBudget::default();
    let prior_lock = read_prior_lock(&root, &mut budget)?;
    let package_cache = PackageCache::new(
        PackageCacheRoot::from_environment()?,
        prior_lock.trusted_sources(),
    );
    let mut resolver = LockResolver::new(package_cache);
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
    let changed = match prior_lock {
        PriorLock::Present {
            content: existing, ..
        } if existing == content => false,
        PriorLock::Present {
            content: existing, ..
        } => {
            replace_file_atomically_if_unchanged(
                &lockfile_path,
                existing.as_bytes(),
                content.as_bytes(),
            )?;
            true
        }
        PriorLock::Missing => {
            create_file_atomically(&lockfile_path, content.as_bytes())?;
            true
        }
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

enum PriorLock {
    Missing,
    Present {
        content: String,
        trusted_sources: BTreeMap<GitSourceId, Sha256Digest>,
    },
}

impl PriorLock {
    fn trusted_sources(&self) -> BTreeMap<GitSourceId, Sha256Digest> {
        match self {
            Self::Missing => BTreeMap::new(),
            Self::Present {
                trusted_sources, ..
            } => trusted_sources.clone(),
        }
    }
}

fn read_prior_lock(
    root: &Path,
    budget: &mut PackageIngestionBudget,
) -> Result<PriorLock, DepsError> {
    let path = root.join("graphcal.lock");
    let fs = RealFileSystem::rooted(root).map_err(|source| DepsError::Canonicalize {
        path: root.to_path_buf(),
        source,
    })?;
    match budget.read_text(&fs, &path, IngestionArtifact::Lockfile) {
        Ok(content) => {
            let trusted_sources = trusted_sources_from_lock(&content, budget.policy.max_entries());
            Ok(PriorLock::Present {
                content,
                trusted_sources,
            })
        }
        Err(DepsError::ArtifactRead { source, .. })
            if source.io_kind() == Some(std::io::ErrorKind::NotFound) =>
        {
            Ok(PriorLock::Missing)
        }
        Err(error) => Err(error),
    }
}

fn trusted_sources_from_lock(
    content: &str,
    max_packages: u64,
) -> BTreeMap<GitSourceId, Sha256Digest> {
    let max_packages = usize::try_from(max_packages).unwrap_or(usize::MAX);
    let Ok(lockfile) =
        parse_lockfile_str_with_limits(content, LockfileParseLimits::new(max_packages))
    else {
        return BTreeMap::new();
    };
    let Ok(validated) = lockfile.validated(env!("CARGO_PKG_VERSION"), STDLIB_VERSION) else {
        return BTreeMap::new();
    };
    validated
        .packages()
        .filter_map(|package| match &package.source {
            PackageSource::Root => None,
            PackageSource::Git {
                url,
                commit,
                tree_hashes,
                ..
            } => Some((
                GitSourceId::new(url.clone(), commit.clone()),
                tree_hashes.sha256,
            )),
        })
        .collect()
}

#[derive(Debug)]
struct ResolvedLock {
    root: PackageInstanceId,
    source_dir: PackageSourceDirectory,
    packages: Vec<LockedPackage>,
}

struct LockResolver {
    package_cache: PackageCache,
}

impl LockResolver {
    const fn new(package_cache: PackageCache) -> Self {
        Self { package_cache }
    }

    fn resolve_root(
        &mut self,
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
        &mut self,
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
        &mut self,
        parent: &PackageInstanceId,
        alias: &DependencyName,
        spec: &DependencySpec,
        state: &mut ResolveState,
        budget: &mut PackageIngestionBudget,
    ) -> Result<ResolvedDependency, DepsError> {
        let source_id = GitSourceId::new(spec.git.url.clone(), spec.git.rev.clone());
        let materialized = self.package_cache.materialize(&source_id, budget)?;
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

struct PackageCache {
    root: PackageCacheRoot,
    trusted_sources: BTreeMap<GitSourceId, Sha256Digest>,
    session: BTreeMap<GitSourceId, MaterializedGit>,
}

impl PackageCache {
    const fn new(
        root: PackageCacheRoot,
        trusted_sources: BTreeMap<GitSourceId, Sha256Digest>,
    ) -> Self {
        Self {
            root,
            trusted_sources,
            session: BTreeMap::new(),
        }
    }

    fn materialize(
        &mut self,
        source: &GitSourceId,
        budget: &mut PackageIngestionBudget,
    ) -> Result<MaterializedGit, DepsError> {
        self.materialize_with(source, budget, |path| {
            materialize_git_revision(source.url(), source.commit(), path)
        })
    }

    fn materialize_with(
        &mut self,
        source: &GitSourceId,
        budget: &mut PackageIngestionBudget,
        fetch: impl FnOnce(&Path) -> Result<(), DepsError>,
    ) -> Result<MaterializedGit, DepsError> {
        if let Some(materialized) = self.session.get(source) {
            return Ok(materialized.clone());
        }

        let _writer_lock = CacheWriterLock::acquire(&self.root, source)?;
        let expected = self.trusted_sources.get(source).copied();
        let trusted_checkout = match expected {
            Some(expected) => {
                let path = self.root.git_checkout(source, &expected);
                let existing = inspect_existing_checkout(&path, budget)?;
                if let Some(inspected) = &existing
                    && inspected.sha256 == expected
                {
                    let materialized = inspected.clone().into_materialized();
                    self.session.insert(source.clone(), materialized.clone());
                    return Ok(materialized);
                }
                existing
            }
            None => None,
        };

        let parent = self.root.git_source_directory(source);
        std::fs::create_dir_all(&parent).map_err(|source| DepsError::CreateDir {
            path: parent.clone(),
            source,
        })?;
        let temporary = tempfile::Builder::new()
            .prefix("graphcal-git-")
            .tempdir_in(&parent)
            .map_err(|source| DepsError::CreateTempDir {
                path: parent.clone(),
                source,
            })?;
        fetch(temporary.path())?;
        let fetched = inspect_materialized_package(temporary.path(), budget)?;
        if let Some(expected) = expected
            && fetched.sha256 != expected
        {
            return Err(DepsError::CacheIntegrity {
                url: source.url().to_string(),
                commit: source.commit().as_str().to_string(),
                expected,
                actual: fetched.sha256,
            });
        }

        let path = self.root.git_checkout(source, &fetched.sha256);
        let destination_checkout = if expected == Some(fetched.sha256) {
            trusted_checkout
        } else {
            inspect_existing_checkout(&path, budget)?
        };
        let materialized = match destination_checkout {
            Some(existing) if existing.sha256 == fetched.sha256 => existing.into_materialized(),
            Some(_) | None => {
                publish_checkout(&temporary, &path)?;
                fetched.into_materialized()
            }
        };
        self.session.insert(source.clone(), materialized.clone());
        Ok(materialized)
    }
}

struct CacheWriterLock {
    file: std::fs::File,
}

impl CacheWriterLock {
    fn acquire(root: &PackageCacheRoot, source: &GitSourceId) -> Result<Self, DepsError> {
        let path = root.git_writer_lock(source);
        let parent = path.parent().ok_or_else(|| {
            DepsError::Internal(format!("cache lock path {} has no parent", path.display()))
        })?;
        std::fs::create_dir_all(parent).map_err(|source| DepsError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| DepsError::CacheLock {
                path: path.clone(),
                source,
            })?;
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|source| DepsError::CacheLock { path, source })?;
        Ok(Self { file })
    }
}

impl Drop for CacheWriterLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn inspect_existing_checkout(
    path: &Path,
    budget: &mut PackageIngestionBudget,
) -> Result<Option<InspectedPackage>, DepsError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => match inspect_materialized_package(path, budget) {
            Ok(inspected) => Ok(Some(inspected)),
            Err(error) if is_ingestion_policy_error(&error) => Err(error),
            Err(_) => Ok(None),
        },
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DepsError::Metadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

const fn is_ingestion_policy_error(error: &DepsError) -> bool {
    matches!(
        error,
        DepsError::IngestionLimit { .. }
            | DepsError::SourceTreeHash(
                graphcal_io::SourceTreeHashError::ResourceLimit { .. }
                    | graphcal_io::SourceTreeHashError::ReadFile {
                        source: FileSystemReadError::ByteLimitExceeded { .. },
                        ..
                    }
            )
    )
}

fn publish_checkout(temporary: &tempfile::TempDir, destination: &Path) -> Result<(), DepsError> {
    let temporary_path = temporary.path().to_path_buf();
    let parent = destination.parent().ok_or_else(|| {
        DepsError::Internal(format!(
            "cache checkout path {} has no parent",
            destination.display()
        ))
    })?;
    match std::fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::rename(&temporary_path, destination).map_err(|source| DepsError::Rename {
                from: temporary_path,
                to: destination.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        Err(source) => Err(DepsError::Metadata {
            path: destination.to_path_buf(),
            source,
        }),
        Ok(_) => {
            let backup = tempfile::Builder::new()
                .prefix("graphcal-cache-backup-")
                .tempdir_in(parent)
                .map_err(|source| DepsError::CreateTempDir {
                    path: parent.to_path_buf(),
                    source,
                })?;
            let backup_checkout = backup.path().join("checkout");
            std::fs::rename(destination, &backup_checkout).map_err(|source| DepsError::Rename {
                from: destination.to_path_buf(),
                to: backup_checkout.clone(),
                source,
            })?;
            match std::fs::rename(&temporary_path, destination) {
                Ok(()) => Ok(()),
                Err(publish) => match std::fs::rename(&backup_checkout, destination) {
                    Ok(()) => Err(DepsError::Rename {
                        from: temporary_path,
                        to: destination.to_path_buf(),
                        source: publish,
                    }),
                    Err(rollback) => {
                        let preserved = backup.keep().join("checkout");
                        Err(DepsError::CacheRollback {
                            destination: destination.to_path_buf(),
                            preserved,
                            publish,
                            rollback,
                        })
                    }
                },
            }
        }
    }
}

#[derive(Debug, Clone)]
struct MaterializedGit {
    sha256: Sha256Digest,
    manifest: PackageManifest,
}

#[derive(Clone)]
struct InspectedPackage {
    sha256: Sha256Digest,
    manifest: PackageManifest,
}

impl InspectedPackage {
    fn into_materialized(self) -> MaterializedGit {
        MaterializedGit {
            sha256: self.sha256,
            manifest: self.manifest,
        }
    }
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
    /// Package cache root discovery failed.
    #[error(transparent)]
    CacheRoot(#[from] PackageCacheRootError),
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
    /// A refetched immutable source contradicted the prior validated lock.
    #[error(
        "Git dependency `{url}` at immutable commit `{commit}` has source-tree digest `{actual}`, but the prior lock requires `{expected}`"
    )]
    CacheIntegrity {
        url: String,
        commit: String,
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    /// Per-source cache writer lock failed.
    #[error("could not lock package cache key at `{}`: {source}", path.display())]
    CacheLock {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Publishing and rolling back a replacement both failed; the prior
    /// checkout is retained at `preserved` rather than deleted.
    #[error(
        "could not publish cache checkout `{}` ({publish}) or restore it ({rollback}); prior checkout preserved at `{}`",
        destination.display(),
        preserved.display()
    )]
    CacheRollback {
        destination: PathBuf,
        preserved: PathBuf,
        publish: std::io::Error,
        rollback: std::io::Error,
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

    fn test_git_source() -> GitSourceId {
        let manifest = parse_manifest_str(
            r#"
[package]
name = "root"

[dependencies]
units = { git = "https://example.com/acme/units.git", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
"#,
        )
        .unwrap();
        let dependency = manifest.dependencies.into_values().next().unwrap();
        GitSourceId::new(dependency.git.url, dependency.git.rev)
    }

    fn write_cached_package(root: &Path, source: &str) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("graphcal.toml"),
            "[package]\nname = \"units\"\nsource_dir = \"src\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/units.gcl"), source).unwrap();
    }

    fn inspect_for_test(root: &Path) -> InspectedPackage {
        inspect_materialized_package(root, &mut PackageIngestionBudget::default()).unwrap()
    }

    fn install_cached_package(
        root: &PackageCacheRoot,
        source: &GitSourceId,
        content: &str,
    ) -> (PathBuf, InspectedPackage) {
        let parent = root.git_source_directory(source);
        std::fs::create_dir_all(&parent).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("package-test-")
            .tempdir_in(parent)
            .unwrap();
        write_cached_package(temporary.path(), content);
        let inspected = inspect_for_test(temporary.path());
        let checkout = root.git_checkout(source, &inspected.sha256);
        std::fs::rename(temporary.path(), &checkout).unwrap();
        (checkout, inspected)
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
    fn package_cache_reuses_a_trusted_checkout_without_network_access() {
        use std::cell::Cell;

        let directory = tempfile::tempdir().unwrap();
        let root = PackageCacheRoot::from_path(directory.path()).unwrap();
        let source = test_git_source();
        let (_checkout, inspected) =
            install_cached_package(&root, &source, "node x: Dimensionless = 1.0;\n");
        let mut cache =
            PackageCache::new(root, BTreeMap::from([(source.clone(), inspected.sha256)]));
        let fetches = Cell::new(0_u8);

        let materialized = cache
            .materialize_with(&source, &mut PackageIngestionBudget::default(), |_path| {
                fetches.set(fetches.get().saturating_add(1));
                Err(DepsError::Internal("network must stay offline".to_string()))
            })
            .unwrap();

        assert_eq!(fetches.get(), 0);
        assert_eq!(materialized.sha256, inspected.sha256);
        assert_eq!(materialized.manifest.name.as_str(), "units");
    }

    #[test]
    fn package_cache_materializes_a_diamond_source_once_per_session() {
        use std::cell::Cell;

        let directory = tempfile::tempdir().unwrap();
        let root = PackageCacheRoot::from_path(directory.path()).unwrap();
        let source = test_git_source();
        let mut cache = PackageCache::new(root, BTreeMap::new());
        let fetches = Cell::new(0_u8);
        let mut budget = PackageIngestionBudget::default();

        let first = cache
            .materialize_with(&source, &mut budget, |path| {
                fetches.set(fetches.get().saturating_add(1));
                write_cached_package(path, "node x: Dimensionless = 1.0;\n");
                Ok(())
            })
            .unwrap();
        let second = cache
            .materialize_with(&source, &mut budget, |_path| {
                fetches.set(fetches.get().saturating_add(1));
                Err(DepsError::Internal("duplicate fetch".to_string()))
            })
            .unwrap();

        assert_eq!(fetches.get(), 1);
        assert_eq!(first.sha256, second.sha256);
    }

    #[test]
    fn failed_cache_refresh_preserves_the_previous_checkout() {
        let directory = tempfile::tempdir().unwrap();
        let root = PackageCacheRoot::from_path(directory.path()).unwrap();
        let source = test_git_source();
        let old_source = "node old: Dimensionless = 1.0;\n";
        let (checkout, _inspected) = install_cached_package(&root, &source, old_source);
        let contradictory = Sha256Digest::new("f".repeat(64)).unwrap();
        let mut cache = PackageCache::new(root, BTreeMap::from([(source.clone(), contradictory)]));

        let error = cache
            .materialize_with(&source, &mut PackageIngestionBudget::default(), |_path| {
                Err(DepsError::Internal("offline".to_string()))
            })
            .unwrap_err();

        assert!(matches!(error, DepsError::Internal(message) if message == "offline"));
        assert_eq!(
            std::fs::read_to_string(checkout.join("src/units.gcl")).unwrap(),
            old_source
        );
    }

    #[test]
    fn refetched_immutable_source_must_match_the_prior_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = PackageCacheRoot::from_path(directory.path()).unwrap();
        let source = test_git_source();
        let (checkout, inspected) =
            install_cached_package(&root, &source, "node old: Dimensionless = 1.0;\n");
        let corrupted = "node corrupt: Dimensionless = 99.0;\n";
        std::fs::write(checkout.join("src/units.gcl"), corrupted).unwrap();
        let mut cache =
            PackageCache::new(root, BTreeMap::from([(source.clone(), inspected.sha256)]));

        let error = cache
            .materialize_with(&source, &mut PackageIngestionBudget::default(), |path| {
                write_cached_package(path, "node changed: Dimensionless = 2.0;\n");
                Ok(())
            })
            .unwrap_err();

        assert!(matches!(
            error,
            DepsError::CacheIntegrity {
                expected,
                actual,
                ..
            } if expected == inspected.sha256 && actual != expected
        ));
        assert_eq!(
            std::fs::read_to_string(checkout.join("src/units.gcl")).unwrap(),
            corrupted
        );
    }

    #[test]
    fn cache_refresh_keeps_existing_rooted_readers_alive() {
        let directory = tempfile::tempdir().unwrap();
        let root = PackageCacheRoot::from_path(directory.path()).unwrap();
        let source = test_git_source();
        let old_source = "node old: Dimensionless = 1.0;\n";
        let new_source = "node new: Dimensionless = 2.0;\n";
        let (checkout, _inspected) = install_cached_package(&root, &source, old_source);
        let existing_reader = RealFileSystem::rooted(&checkout).unwrap();
        let mut cache = PackageCache::new(root, BTreeMap::new());

        let materialized = cache
            .materialize_with(&source, &mut PackageIngestionBudget::default(), |path| {
                write_cached_package(path, new_source);
                Ok(())
            })
            .unwrap();

        assert_eq!(
            existing_reader
                .read_to_string_bounded(
                    &checkout.join("src/units.gcl"),
                    ByteLimit::new(1024),
                    &NeverCancel,
                )
                .unwrap(),
            old_source
        );
        let published = PackageCacheRoot::from_path(directory.path())
            .unwrap()
            .git_checkout(&source, &materialized.sha256);
        assert_eq!(
            std::fs::read_to_string(published.join("src/units.gcl")).unwrap(),
            new_source
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("src/units.gcl")).unwrap(),
            old_source
        );
    }

    #[test]
    fn package_cache_serializes_writers_per_source_key() {
        let directory = tempfile::tempdir().unwrap();
        let root = PackageCacheRoot::from_path(directory.path()).unwrap();
        let source = test_git_source();
        let first = CacheWriterLock::acquire(&root, &source).unwrap();
        let lock_path = root.git_writer_lock(&source);
        let second = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();

        let contention = fs2::FileExt::try_lock_exclusive(&second).unwrap_err();
        assert_eq!(contention.kind(), std::io::ErrorKind::WouldBlock);
        drop(first);
        fs2::FileExt::try_lock_exclusive(&second).unwrap();
        fs2::FileExt::unlock(&second).unwrap();
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
