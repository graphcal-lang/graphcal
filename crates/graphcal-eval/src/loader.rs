use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;
use sha2::{Digest, Sha256};

use crate::eval::CompileError;
use graphcal_compiler::dag_id::{DagId, DagPackageId};
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::{DeclKind, IncludeDecl, ModulePath};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::IncludeInstanceScope;
use graphcal_compiler::syntax::phase::Phase;
use graphcal_io::{
    ByteLimit, FileSystemEntryKind, FileSystemReadError, FileSystemReader, RealFileSystem,
    SourceTreeHashLimits, hash_source_tree,
};
use graphcal_package::{
    GitCommitHash, GitUrl, LockedPackage, PackageGraph, PackageInstanceId, PackageManifest,
    PackageSource, STDLIB_VERSION, parse_lockfile_str, parse_manifest_str,
    validate_lock_against_manifests,
};

#[cfg(test)]
thread_local! {
    static TEST_CACHE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

const MEBIBYTE: u64 = 1024 * 1024;

/// Per-artifact byte limits for one project load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoaderArtifactByteLimits {
    source_file: u64,
    manifest: u64,
    lockfile: u64,
    plugin: u64,
    source_tree_file: u64,
}

impl LoaderArtifactByteLimits {
    /// Construct explicit byte limits for every artifact category. Zero is a
    /// valid deny-all policy for a category.
    #[must_use]
    pub const fn new(
        source_file: u64,
        manifest: u64,
        lockfile: u64,
        plugin: u64,
        source_tree_file: u64,
    ) -> Self {
        Self {
            source_file,
            manifest,
            lockfile,
            plugin,
            source_tree_file,
        }
    }
}

impl Default for LoaderArtifactByteLimits {
    fn default() -> Self {
        Self::new(
            16 * MEBIBYTE,
            MEBIBYTE,
            4 * MEBIBYTE,
            16 * MEBIBYTE,
            16 * MEBIBYTE,
        )
    }
}

/// Resource policy for one complete project load.
///
/// The budget covers root/dependency source files, manifests, lockfiles,
/// plugin modules, and every regular file read while verifying a locked source
/// tree. Aggregate counters are private and are created afresh for each load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoaderBudget {
    artifacts: LoaderArtifactByteLimits,
    max_files: u64,
    max_total_bytes: u64,
}

impl LoaderBudget {
    /// Construct an explicit loader policy. Zero values are valid and reject
    /// the corresponding resource immediately.
    #[must_use]
    pub const fn new(
        artifacts: LoaderArtifactByteLimits,
        max_files: u64,
        max_total_bytes: u64,
    ) -> Self {
        Self {
            artifacts,
            max_files,
            max_total_bytes,
        }
    }
}

impl Default for LoaderBudget {
    fn default() -> Self {
        Self::new(LoaderArtifactByteLimits::default(), 10_000, 256 * MEBIBYTE)
    }
}

/// Closed loader resource category reported when a budget is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderResource {
    /// One Graphcal source file.
    SourceFileBytes,
    /// One `graphcal.toml` manifest.
    ManifestBytes,
    /// One `graphcal.lock` lockfile.
    LockfileBytes,
    /// One WASM plugin module.
    PluginBytes,
    /// One regular file in a verified dependency source tree.
    SourceTreeFileBytes,
    /// Number of loaded artifacts and verified source-tree entries.
    FileCount,
    /// Aggregate bytes read during the complete load.
    TotalBytes,
}

impl std::fmt::Display for LoaderResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceFileBytes => formatter.write_str("source-file byte"),
            Self::ManifestBytes => formatter.write_str("manifest byte"),
            Self::LockfileBytes => formatter.write_str("lockfile byte"),
            Self::PluginBytes => formatter.write_str("plugin byte"),
            Self::SourceTreeFileBytes => formatter.write_str("source-tree file byte"),
            Self::FileCount => formatter.write_str("file-count"),
            Self::TotalBytes => formatter.write_str("aggregate byte"),
        }
    }
}

/// One concrete loader-budget violation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "loader {resource} limit of {limit} exceeded while reading `{}`",
    path.display()
)]
pub struct LoaderBudgetExceeded {
    /// Artifact whose read exhausted the policy.
    pub path: PathBuf,
    /// Closed resource category.
    pub resource: LoaderResource,
    /// Configured maximum.
    pub limit: u64,
}

#[derive(Debug, Clone, Copy)]
enum LoaderArtifact {
    SourceFile,
    Manifest,
    Lockfile,
    Plugin,
}

impl LoaderArtifact {
    const fn resource(self) -> LoaderResource {
        match self {
            Self::SourceFile => LoaderResource::SourceFileBytes,
            Self::Manifest => LoaderResource::ManifestBytes,
            Self::Lockfile => LoaderResource::LockfileBytes,
            Self::Plugin => LoaderResource::PluginBytes,
        }
    }

    const fn byte_limit(self, limits: LoaderArtifactByteLimits) -> u64 {
        match self {
            Self::SourceFile => limits.source_file,
            Self::Manifest => limits.manifest,
            Self::Lockfile => limits.lockfile,
            Self::Plugin => limits.plugin,
        }
    }
}

#[derive(Debug)]
enum LoaderReadError {
    Budget(LoaderBudgetExceeded),
    Filesystem(FileSystemReadError),
}

impl std::fmt::Display for LoaderReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget(error) => error.fmt(formatter),
            Self::Filesystem(error) => error.fmt(formatter),
        }
    }
}

struct LoaderBudgetState {
    policy: LoaderBudget,
    files_read: u64,
    total_bytes: u64,
}

impl LoaderBudgetState {
    const fn new(policy: LoaderBudget) -> Self {
        Self {
            policy,
            files_read: 0,
            total_bytes: 0,
        }
    }

    fn exceeded(path: &Path, resource: LoaderResource, limit: u64) -> LoaderReadError {
        LoaderReadError::Budget(LoaderBudgetExceeded {
            path: path.to_path_buf(),
            resource,
            limit,
        })
    }

    fn read_bytes(
        &mut self,
        fs: &dyn FileSystemReader,
        path: &Path,
        artifact: LoaderArtifact,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Vec<u8>, LoaderReadError> {
        if self.files_read >= self.policy.max_files {
            return Err(Self::exceeded(
                path,
                LoaderResource::FileCount,
                self.policy.max_files,
            ));
        }
        let artifact_limit = artifact.byte_limit(self.policy.artifacts);
        let total_remaining = self.policy.max_total_bytes.saturating_sub(self.total_bytes);
        let (read_limit, exhausted_resource) = if artifact_limit <= total_remaining {
            (artifact_limit, artifact.resource())
        } else {
            (total_remaining, LoaderResource::TotalBytes)
        };
        let cancellation_signal = || cancellation.is_cancelled();
        let bytes = fs
            .read_bytes_bounded(path, ByteLimit::new(read_limit), &cancellation_signal)
            .map_err(|error| match error {
                FileSystemReadError::ByteLimitExceeded { .. } => Self::exceeded(
                    path,
                    exhausted_resource,
                    match exhausted_resource {
                        LoaderResource::TotalBytes => self.policy.max_total_bytes,
                        _ => artifact_limit,
                    },
                ),
                other => LoaderReadError::Filesystem(other),
            })?;
        self.files_read = self.files_read.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }

    fn read_text(
        &mut self,
        fs: &dyn FileSystemReader,
        path: &Path,
        artifact: LoaderArtifact,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<String, LoaderReadError> {
        let bytes = self.read_bytes(fs, path, artifact, cancellation)?;
        String::from_utf8(bytes).map_err(|error| {
            LoaderReadError::Filesystem(FileSystemReadError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })
    }

    const fn remaining_files(&self) -> u64 {
        self.policy.max_files.saturating_sub(self.files_read)
    }

    const fn remaining_bytes(&self) -> u64 {
        self.policy.max_total_bytes.saturating_sub(self.total_bytes)
    }

    const fn source_tree_limits(&self) -> SourceTreeHashLimits {
        SourceTreeHashLimits::new(
            ByteLimit::new(self.policy.artifacts.source_tree_file),
            self.remaining_bytes(),
            self.remaining_files(),
        )
    }

    fn account_source_tree(
        &mut self,
        root: &Path,
        entries: u64,
        bytes: u64,
    ) -> Result<(), LoaderBudgetExceeded> {
        let next_files = self.files_read.saturating_add(entries);
        if next_files > self.policy.max_files {
            return Err(LoaderBudgetExceeded {
                path: root.to_path_buf(),
                resource: LoaderResource::FileCount,
                limit: self.policy.max_files,
            });
        }
        let next_bytes = self.total_bytes.saturating_add(bytes);
        if next_bytes > self.policy.max_total_bytes {
            return Err(LoaderBudgetExceeded {
                path: root.to_path_buf(),
                resource: LoaderResource::TotalBytes,
                limit: self.policy.max_total_bytes,
            });
        }
        self.files_read = next_files;
        self.total_bytes = next_bytes;
        Ok(())
    }
}

fn loader_manifest_error(error: impl std::fmt::Display) -> CompileError {
    CompileError::Eval(GraphcalError::ManifestError {
        message: error.to_string(),
    })
}

fn parse_operation_error(
    error: graphcal_compiler::syntax::parser::ParseOperationError,
) -> CompileError {
    match error {
        graphcal_compiler::syntax::parser::ParseOperationError::Cancelled(cancelled) => {
            cancelled.into()
        }
        graphcal_compiler::syntax::parser::ParseOperationError::Parse(error) => error.into(),
    }
}

/// Loader-resolved identities for one module path.
///
/// A path may name a file-root DAG or an inline DAG inside a loaded file.
/// Consumers need the source file to retrieve compiled artifacts and the exact
/// module target for semantic name resolution, so both identities are retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleTarget {
    source_file: DagId,
    target: DagId,
}

impl ResolvedModuleTarget {
    const fn in_file(source_file: DagId, target: DagId) -> Self {
        Self {
            source_file,
            target,
        }
    }

    fn file_root(source_file: DagId) -> Self {
        Self::in_file(source_file.clone(), source_file)
    }

    /// Loaded file that owns the target's compiled artifacts.
    #[must_use]
    pub const fn source_file(&self) -> &DagId {
        &self.source_file
    }

    /// Exact file-root or inline-DAG module named by the source path.
    #[must_use]
    pub const fn target(&self) -> &DagId {
        &self.target
    }
}

/// Failure to associate a loader-resolved module with its owning source file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolvedModuleTargetError {
    /// The resolved identity is neither a loaded file root nor one of its inline DAGs.
    #[error("resolved module `{target}` is not owned by a loaded source file")]
    UnknownOwner { target: DagId },
}

/// Failure to construct a total module resolver from an immutable loaded project.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModuleResolverBuildError {
    /// The source template include graph contains a cycle and therefore has no
    /// finite concrete-instance expansion.
    #[error("recursive include expansion involving module `{module}`")]
    RecursiveIncludeExpansion {
        /// First repeated source module observed by the typed DFS.
        module: DagId,
        /// Canonical source-module cycle, including the repeated endpoint.
        cycle: Vec<DagId>,
    },
    /// Ordinary symbol-table construction failed.
    #[error(transparent)]
    ModuleResolve(#[from] graphcal_compiler::syntax::module_resolve::ModuleResolveError),
}

/// Span-free identity for an `import`/`include` path.
///
/// Used as a `HashMap` key in `LoadedFile::resolved_imports` /
/// `LoadedDag::resolved_imports` so that two equal logical paths always
/// produce equal keys without depending on a shared join format
/// (e.g. `.` vs `/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePathKey(Vec<String>);

impl ModulePathKey {
    /// Build a key from a parsed [`ModulePath`] AST node. Segment names are
    /// cloned and spans are dropped — span-aware lookup is never useful at
    /// this layer.
    #[must_use]
    pub(crate) fn from_path(path: &ModulePath) -> Self {
        Self(path.segments.iter().map(|s| s.name.to_string()).collect())
    }

    /// Segments in order, without separators.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

/// Loader-side resolution status for an import inside an inline DAG body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineBodyImportResolution {
    /// The module path resolved to an exact module and its owning source file.
    Resolved(ResolvedModuleTarget),
    /// The loader could not resolve the path in its current project context.
    ///
    /// The import declaration remains in the DAG body so the downstream
    /// resolver can emit the user-facing diagnostic with the original span.
    Unresolved,
}

impl std::fmt::Display for ModulePathKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, seg) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(seg)?;
        }
        Ok(())
    }
}

/// Filesystem resolution result before the owning file receives its [`DagId`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedFilePath {
    file: PathBuf,
    inline_path: Vec<DeclName>,
}

impl ResolvedFilePath {
    fn target_from(&self, source_file: &DagId) -> ResolvedModuleTarget {
        let target = self
            .inline_path
            .iter()
            .fold(source_file.clone(), |owner, name| {
                owner.child(name.as_str())
            });
        ResolvedModuleTarget::in_file(source_file.clone(), target)
    }
}

/// Validated path from a file AST root to one nested inline-DAG body.
///
/// Construction is private and requires at least one declaration index, so a
/// locator cannot accidentally denote the file root.
#[derive(Debug, Clone)]
struct DagBodyLocator {
    first: usize,
    rest: Box<[usize]>,
}

impl DagBodyLocator {
    fn at_child(parent_path: &[usize], child_index: usize) -> Self {
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
    fn declaration<'a>(
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
/// own imports and `import <self>.{...}` references back to the parent file.
#[derive(Debug, Clone)]
pub struct LoadedDag {
    /// Abstract DAG identity for this inline dag, formed by appending the
    /// dag's name to its parent file's `DagId`.
    dag_id: DagId,
    /// The enclosing file's `DagId`. Imports whose path resolves to this id
    /// are dag-body self-imports (`import <self>.{...}`).
    parent_dag_id: DagId,
    /// Stable locator into the owning file AST, which remains the single body owner.
    body_locator: DagBodyLocator,
    /// Loader-resolved DAG identities for each `import` declaration in the
    /// body, keyed by its typed span-free module path. Self-imports map to
    /// `parent_dag_id`; cross-file imports map to the dependency file's id.
    /// Imports whose path fails to resolve at load time are absent here; the
    /// downstream resolver surfaces a structured error for them.
    resolved_imports: HashMap<ModulePathKey, InlineBodyImportResolution>,
}

impl LoadedDag {
    #[must_use]
    pub(crate) const fn dag_id(&self) -> &DagId {
        &self.dag_id
    }

    #[must_use]
    pub(crate) const fn parent_dag_id(&self) -> &DagId {
        &self.parent_dag_id
    }

    #[must_use]
    pub(crate) const fn resolved_imports(
        &self,
    ) -> &HashMap<ModulePathKey, InlineBodyImportResolution> {
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

/// A single loaded and parsed file.
#[derive(Debug)]
pub struct LoadedFile {
    /// Canonical path of this file (retained for I/O: diagnostics, LSP URIs).
    path: PathBuf,
    /// Abstract DAG identity (filesystem-independent).
    dag_id: DagId,
    /// Raw source text.
    source: Arc<String>,
    /// Parsed AST.
    ast: File,
    /// Named source for diagnostics.
    named_source: NamedSource<Arc<String>>,
    /// Loader-resolved DAG identities for each import declaration, keyed by the
    /// import path's display string (e.g. `"./lib.gcl"` or `"nasa/rocket"`).
    /// Produced by the loader so that downstream consumers (evaluator, LSP) can
    /// look up resolved imports without re-resolving.
    resolved_imports: HashMap<ModulePathKey, ResolvedModuleTarget>,
    /// Inline `dag X { ... }` metadata indexed from this file, with per-DAG
    /// pre-resolved imports. Entries retain source preorder and borrow their
    /// authoritative bodies from `ast` through validated locators.
    inline_dags: Vec<LoadedDag>,
}

impl LoadedFile {
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

    /// Named source paired with this file's path and source snapshot.
    #[must_use]
    pub(crate) const fn named_source(&self) -> &NamedSource<Arc<String>> {
        &self.named_source
    }

    #[must_use]
    pub(crate) fn inline_dags(&self) -> &[LoadedDag] {
        &self.inline_dags
    }

    /// Iterate over imports together with their exact module targets and owners.
    pub fn imports_with_targets(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::desugar::desugared_ast::Declaration,
            &graphcal_compiler::syntax::ast::ImportDecl,
            &ResolvedModuleTarget,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Import(import_decl) = &decl.kind {
                self.resolved_imports
                    .get(&ModulePathKey::from_path(&import_decl.path))
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
            &ResolvedModuleTarget,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Include(include_decl) = &decl.kind {
                self.resolved_imports
                    .get(&ModulePathKey::from_path(&include_decl.path))
                    .map(|target| (decl, include_decl, target))
            } else {
                None
            }
        })
    }
}

/// A loaded project: a root file plus all transitively imported files.
#[derive(Debug)]
pub struct LoadedProject {
    /// All loaded files keyed by DAG identity.
    files: HashMap<DagId, LoadedFile>,
    /// The DAG identity of the root file.
    root: DagId,
    /// Topological load order: dependencies before dependents.
    /// The root file is last.
    load_order: Vec<DagId>,
    /// Canonical owner source file for every file-root and inline DAG identity.
    dag_owners: HashMap<DagId, DagId>,
    /// WASM plugin files referenced by `import plugin "….wasm"` declarations
    /// in root-package files, keyed by the verbatim plugin path.
    ///
    /// Paths resolve relative to the root package's source root and are
    /// sandboxed inside it. A failed read is recorded (not fatal here) so
    /// compile-only consumers keep working; the evaluation pipeline surfaces
    /// the stored error with the declaring import's span. Host-registry
    /// plugin identities (e.g. `graphcal:demo`) never appear in this map,
    /// and neither do wasm imports declared by dependency packages (those
    /// are rejected at verification time).
    plugins: HashMap<graphcal_compiler::syntax::plugin::PluginPath, PluginFileEntry>,
}

/// Outcome of locating and reading one wasm plugin file.
pub type PluginFileEntry = Result<LoadedPlugin, PluginFileError>;

/// The wasm plugin paths declared by one file's `import plugin` blocks.
fn wasm_plugin_paths(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
) -> impl Iterator<Item = &graphcal_compiler::syntax::plugin::PluginPath> {
    use graphcal_compiler::syntax::plugin::PluginSourceKind;

    ast.declarations.iter().filter_map(|decl| match &decl.kind {
        DeclKind::PluginImport(plugin)
            if plugin.path.value.source_kind() == PluginSourceKind::WasmModule =>
        {
            Some(&plugin.path.value)
        }
        _ => None,
    })
}

/// Resolve and read every wasm plugin declared by the given file ASTs.
///
/// Read failures are recorded per plugin rather than failing the load, so
/// compile-only consumers (hover, symbols) keep working; evaluation surfaces
/// the stored error at the declaring import.
fn read_wasm_plugins<'a>(
    file_asts: impl Iterator<Item = &'a graphcal_compiler::desugar::desugared_ast::File>,
    package_root: &Path,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<graphcal_compiler::syntax::plugin::PluginPath, PluginFileEntry>, CompileError> {
    let mut plugins = HashMap::new();
    for ast in file_asts {
        for path in wasm_plugin_paths(ast) {
            cancellation.checkpoint()?;
            if !plugins.contains_key(path) {
                let entry = read_plugin_file(package_root, path, fs, budget, cancellation);
                cancellation.checkpoint()?;
                plugins.insert(path.clone(), entry);
            }
        }
    }
    Ok(plugins)
}

/// Canonical regular-file path accepted by the plugin containment policy.
#[derive(Debug)]
struct PluginArtifactPath(PathBuf);

fn resolve_plugin_artifact_path(
    package_root: &Path,
    plugin: &graphcal_compiler::syntax::plugin::PluginPath,
    fs: &dyn FileSystemReader,
) -> Result<PluginArtifactPath, PluginFileError> {
    let relative = Path::new(plugin.as_str());
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(PluginFileError::OutsideRoot);
    }
    let canonical_root =
        fs.canonicalize(package_root)
            .map_err(|error| PluginFileError::Unreadable {
                resolved: package_root.to_path_buf(),
                message: error.to_string(),
            })?;
    let candidate = canonical_root.join(relative);
    match fs.entry_kind(&candidate) {
        Ok(FileSystemEntryKind::File) => {}
        Ok(FileSystemEntryKind::Symlink) => return Err(PluginFileError::OutsideRoot),
        Ok(FileSystemEntryKind::Directory | FileSystemEntryKind::Other) => {
            return Err(PluginFileError::NotRegularFile {
                resolved: candidate,
            });
        }
        Err(error) => {
            return Err(PluginFileError::Unreadable {
                resolved: candidate,
                message: error.to_string(),
            });
        }
    }
    let canonical = fs
        .canonicalize(&candidate)
        .map_err(|error| PluginFileError::Unreadable {
            resolved: candidate.clone(),
            message: error.to_string(),
        })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(PluginFileError::OutsideRoot);
    }
    Ok(PluginArtifactPath(canonical))
}

/// Resolve one plugin path against the package root and read exactly the
/// canonical regular file accepted by the containment check.
fn read_plugin_file(
    package_root: &Path,
    plugin: &graphcal_compiler::syntax::plugin::PluginPath,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> PluginFileEntry {
    let artifact = resolve_plugin_artifact_path(package_root, plugin, fs)?;
    match budget.read_bytes(fs, &artifact.0, LoaderArtifact::Plugin, cancellation) {
        Ok(bytes) => Ok(LoadedPlugin {
            sha256_hex: hex_string(&Sha256::digest(&bytes)),
            bytes: bytes.into(),
        }),
        Err(LoaderReadError::Budget(error)) => Err(PluginFileError::ResourceLimit(error)),
        Err(LoaderReadError::Filesystem(error)) => Err(PluginFileError::Unreadable {
            resolved: artifact.0,
            message: error.to_string(),
        }),
    }
}

/// A successfully read wasm plugin file, ready for the plugin host.
#[derive(Clone)]
pub struct LoadedPlugin {
    /// The raw module bytes.
    bytes: Arc<[u8]>,
    /// Lowercase-hex SHA-256 of the bytes — the form `graphcal.lock` pins.
    sha256_hex: String,
}

impl LoadedPlugin {
    /// Immutable module bytes whose digest was checked by the loader.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedPlugin")
            .field("byte_len", &self.bytes.len())
            .field("sha256_hex", &self.sha256_hex)
            .finish_non_exhaustive()
    }
}

/// Why a wasm plugin file could not be provided to the plugin host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginFileError {
    /// The plugin path is absolute or leaves the package root.
    #[error("plugin paths must be relative and stay inside the package root")]
    OutsideRoot,
    /// The resolved entry exists but is not a regular file.
    #[error("plugin artifact `{}` is not a regular file", resolved.display())]
    NotRegularFile {
        /// The path the plugin string resolved to.
        resolved: PathBuf,
    },
    /// The resolved file is missing or unreadable.
    #[error("cannot read `{}`: {message}", resolved.display())]
    Unreadable {
        /// The path the plugin string resolved to.
        resolved: PathBuf,
        /// The underlying I/O error.
        message: String,
    },
    /// Reading this artifact would exceed the project loader budget.
    #[error(transparent)]
    ResourceLimit(LoaderBudgetExceeded),
    /// The project was built from in-memory source with no filesystem.
    #[error("plugin files cannot be loaded without a project on disk")]
    NoProjectFilesystem,
    /// The project has a manifest but `graphcal.lock` does not pin this
    /// plugin.
    #[error("the plugin is not pinned in graphcal.lock; run `graphcal deps lock`")]
    NotPinned,
    /// The plugin file's hash does not match its `graphcal.lock` pin.
    #[error(
        "the plugin file's SHA-256 ({actual}) does not match the graphcal.lock pin ({expected})"
    )]
    HashMismatch {
        /// The digest recorded in `graphcal.lock`.
        expected: String,
        /// The digest of the file actually on disk.
        actual: String,
    },
}

/// Enforce `graphcal.lock` pins on successfully read plugin files.
///
/// The lockfile is the trust boundary for plugin code: in a project with a
/// `graphcal.toml` manifest, a plugin binary loads only when its bytes hash
/// to the pinned digest. Missing or mismatched pins replace the loaded
/// entry with a hard error surfaced at the declaring import.
fn apply_plugin_pins(
    plugins: &mut HashMap<graphcal_compiler::syntax::plugin::PluginPath, PluginFileEntry>,
    pins: &BTreeMap<String, String>,
) {
    for (path, entry) in plugins.iter_mut() {
        let Ok(loaded) = entry.as_ref() else {
            continue;
        };
        match pins.get(path.as_str()) {
            None => *entry = Err(PluginFileError::NotPinned),
            Some(expected) if *expected != loaded.sha256_hex => {
                *entry = Err(PluginFileError::HashMismatch {
                    expected: expected.clone(),
                    actual: loaded.sha256_hex.clone(),
                });
            }
            Some(_) => {}
        }
    }
}

impl LoadedProject {
    fn from_parts(
        files: HashMap<DagId, LoadedFile>,
        root: DagId,
        load_order: Vec<DagId>,
        plugins: HashMap<graphcal_compiler::syntax::plugin::PluginPath, PluginFileEntry>,
    ) -> Self {
        let dag_owners = files
            .iter()
            .flat_map(|(file_id, file)| {
                std::iter::once((file_id.clone(), file_id.clone())).chain(
                    file.inline_dags
                        .iter()
                        .map(|dag| (dag.dag_id.clone(), file_id.clone())),
                )
            })
            .collect();
        Self {
            files,
            root,
            load_order,
            dag_owners,
            plugins,
        }
    }

    /// All loaded files, keyed by their canonical semantic identity.
    #[must_use]
    pub const fn files(&self) -> &HashMap<DagId, LoadedFile> {
        &self.files
    }

    /// Look up one loaded source file by semantic identity.
    #[must_use]
    pub fn file(&self, dag_id: &DagId) -> Option<&LoadedFile> {
        self.files.get(dag_id)
    }

    pub(crate) fn inline_dag(&self, dag_id: &DagId) -> Option<(&LoadedFile, &LoadedDag)> {
        let owner = self.dag_owners.get(dag_id)?;
        let file = self.files.get(owner)?;
        file.inline_dags
            .iter()
            .find(|inline| inline.dag_id == *dag_id)
            .map(|inline| (file, inline))
    }

    /// Root source-file identity.
    #[must_use]
    pub const fn root_id(&self) -> &DagId {
        &self.root
    }

    /// Root source file. Construction guarantees that `root_id` is present.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "private construction preserves the root-file membership invariant"
    )]
    pub fn root_file(&self) -> &LoadedFile {
        self.files
            .get(&self.root)
            .expect("loaded project root must identify one loaded file")
    }

    /// Dependency-first source-file identities, ending with the root.
    #[must_use]
    pub(crate) fn load_order(&self) -> &[DagId] {
        &self.load_order
    }

    /// Validated plugin artifacts and deferred plugin-loading errors.
    #[must_use]
    pub const fn plugins(
        &self,
    ) -> &HashMap<graphcal_compiler::syntax::plugin::PluginPath, PluginFileEntry> {
        &self.plugins
    }

    /// Build a single-file project from in-memory source text.
    ///
    /// The file is assigned a synthetic path derived from `name` (no disk I/O).
    /// Import declarations in the source are **not** followed — this is suitable
    /// for standalone files or untitled editor buffers.
    ///
    /// # Errors
    ///
    /// Returns a [`CompileError`] if parsing fails or `name` is not a valid
    /// `.gcl` source path.
    pub fn from_source(source: &str, name: &str) -> Result<Self, CompileError> {
        Self::from_source_with_cancellation(
            source,
            name,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
    }

    /// Build a single-file project while observing cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns a [`CompileError`] for invalid source or cooperative
    /// cancellation.
    pub fn from_source_with_cancellation(
        source: &str,
        name: &str,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, CompileError> {
        cancellation.checkpoint()?;
        let source = Arc::new(source.to_string());
        let named_source = NamedSource::new(name, Arc::clone(&source));
        let raw_ast = graphcal_compiler::syntax::parser::Parser::with_name(&source, name)
            .parse_file_with_cancellation(cancellation)
            .map_err(parse_operation_error)?;
        cancellation.checkpoint()?;
        let ast = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);
        cancellation.checkpoint()?;
        let path = PathBuf::from(name);
        // `name` is also a diagnostic label and may be an absolute virtual URI
        // path. A standalone in-memory file has no filesystem hierarchy, so an
        // absolute label contributes only its leaf to semantic DAG identity.
        // Relative labels remain strict, canonical module paths.
        let semantic_path = match (path.is_absolute(), path.file_name()) {
            (true, Some(file_name)) => Path::new(file_name),
            _ => path.as_path(),
        };
        let dag_id = DagId::from_virtual_relative_path(semantic_path).map_err(|error| {
            CompileError::Eval(GraphcalError::internal_error(
                format!("invalid source name `{name}`: {error}"),
                &named_source,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
        // No project root or manifest in single-file mode — only the
        // file-stem self-reference (Concept 7) can be detected here.
        let inline_dags = lift_inline_dags_by_stem(&ast, &path, &dag_id);
        cancellation.checkpoint()?;
        // No filesystem to read wasm plugin files from; the entries carry
        // the reason so evaluation can report it at the import site.
        let plugins = wasm_plugin_paths(&ast)
            .map(|plugin| (plugin.clone(), Err(PluginFileError::NoProjectFilesystem)))
            .collect();
        cancellation.checkpoint()?;
        let loaded_file = LoadedFile {
            path,
            dag_id: dag_id.clone(),
            source,
            ast,
            named_source,
            resolved_imports: HashMap::new(),
            inline_dags,
        };
        let mut files = HashMap::new();
        files.insert(dag_id.clone(), loaded_file);
        Ok(Self::from_parts(
            files,
            dag_id.clone(),
            vec![dag_id],
            plugins,
        ))
    }

    /// Pair an exact DAG identity with the loaded source file that owns it.
    ///
    /// # Errors
    ///
    /// Returns [`ResolvedModuleTargetError`] when `resolved` is not a file root
    /// or inline DAG in this immutable project snapshot.
    pub fn resolved_module_target(
        &self,
        resolved: &DagId,
    ) -> Result<ResolvedModuleTarget, ResolvedModuleTargetError> {
        self.dag_owners
            .get(resolved)
            .cloned()
            .map(|source_file| ResolvedModuleTarget::in_file(source_file, resolved.clone()))
            .ok_or_else(|| ResolvedModuleTargetError::UnknownOwner {
                target: resolved.clone(),
            })
    }

    /// Build module-aware symbol tables for every loaded file and inline DAG.
    ///
    /// The loader remains the only layer that resolves import paths to
    /// canonical [`DagId`]s. This method hands those pre-resolved edges to the
    /// compiler's pure module resolver, which can then resolve syntactic name
    /// paths to [`graphcal_compiler::syntax::names::ResolvedName`] values.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolverBuildError`] for recursive expansion, duplicate
    /// symbols, or invalid resolved import surfaces.
    pub fn build_module_resolver(
        &self,
    ) -> Result<graphcal_compiler::syntax::module_resolve::ModuleResolver, ModuleResolverBuildError>
    {
        ensure_acyclic_include_expansion(self)?;
        let mut resolver = graphcal_compiler::syntax::module_resolve::ModuleResolver::default();

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            resolver.add_module(loaded.dag_id.clone(), &loaded.ast.declarations)?;
            for inline in &loaded.inline_dags {
                resolver.add_module(inline.dag_id.clone(), inline.body(loaded))?;
            }
        }

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            add_include_instance_modules(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                &self.files,
            )?;
            for inline in &loaded.inline_dags {
                add_include_instance_modules(
                    &mut resolver,
                    &inline.dag_id,
                    inline.body(loaded),
                    &inline.resolved_imports,
                    &self.files,
                )?;
            }
        }

        // Load order is dependency-first. Before registering one owner's
        // include edges, give each synthetic target the already-completed scope
        // of its canonical dependency; public re-exports are then selectable at
        // the next composition level.
        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            inherit_include_instance_scopes(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                &self.files,
            )?;
            register_module_imports(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
            )?;
            for inline in &loaded.inline_dags {
                inherit_include_instance_scopes(
                    &mut resolver,
                    &inline.dag_id,
                    inline.body(loaded),
                    &inline.resolved_imports,
                    &self.files,
                )?;
                register_module_imports(
                    &mut resolver,
                    &inline.dag_id,
                    inline.body(loaded),
                    &inline.resolved_imports,
                )?;
            }
        }

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            link_include_instance_indexes(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                &self.files,
            )?;
            for inline in &loaded.inline_dags {
                link_include_instance_indexes(
                    &mut resolver,
                    &inline.dag_id,
                    inline.body(loaded),
                    &inline.resolved_imports,
                    &self.files,
                )?;
            }
        }

        Ok(resolver)
    }
}

#[derive(Debug)]
enum IncludeGraphVisit {
    Enter(DagId),
    Exit(DagId),
}

fn ensure_acyclic_include_expansion(
    project: &LoadedProject,
) -> Result<(), ModuleResolverBuildError> {
    let module_ids = project.load_order.iter().flat_map(|file_id| {
        let file = &project.files[file_id];
        std::iter::once(file_id.clone())
            .chain(file.inline_dags.iter().map(|inline| inline.dag_id.clone()))
    });
    let mut complete = HashSet::new();
    let mut active_positions = HashMap::new();
    let mut active_path = Vec::new();

    for root in module_ids {
        if complete.contains(&root) {
            continue;
        }
        let mut visits = vec![IncludeGraphVisit::Enter(root)];
        while let Some(visit) = visits.pop() {
            match visit {
                IncludeGraphVisit::Enter(module) => {
                    if complete.contains(&module) {
                        continue;
                    }
                    if let Some(cycle_start) = active_positions.get(&module).copied() {
                        let mut cycle = active_path[cycle_start..].to_vec();
                        cycle.push(module.clone());
                        return Err(ModuleResolverBuildError::RecursiveIncludeExpansion {
                            module,
                            cycle,
                        });
                    }
                    active_positions.insert(module.clone(), active_path.len());
                    active_path.push(module.clone());
                    visits.push(IncludeGraphVisit::Exit(module.clone()));
                    visits.extend(
                        module_include_targets(&module, &project.files)
                            .into_iter()
                            .rev()
                            .map(IncludeGraphVisit::Enter),
                    );
                }
                IncludeGraphVisit::Exit(module) => {
                    active_positions.remove(&module);
                    let popped = active_path.pop();
                    debug_assert_eq!(popped.as_ref(), Some(&module));
                    complete.insert(module);
                }
            }
        }
    }
    Ok(())
}

fn module_include_targets(source: &DagId, files: &HashMap<DagId, LoadedFile>) -> Vec<DagId> {
    module_declarations(source, files).map_or_else(Vec::new, |declarations| {
        declarations
            .iter()
            .filter_map(|declaration| {
                let DeclKind::Include(include) = &declaration.kind else {
                    return None;
                };
                resolved_module_target_from(source, &include.path, files)
            })
            .collect()
    })
}

trait ResolvedModuleLookup {
    fn resolved_target(&self, key: &ModulePathKey) -> Option<&ResolvedModuleTarget>;
}

impl ResolvedModuleLookup for HashMap<ModulePathKey, ResolvedModuleTarget> {
    fn resolved_target(&self, key: &ModulePathKey) -> Option<&ResolvedModuleTarget> {
        self.get(key)
    }
}

impl ResolvedModuleLookup for HashMap<ModulePathKey, InlineBodyImportResolution> {
    fn resolved_target(&self, key: &ModulePathKey) -> Option<&ResolvedModuleTarget> {
        match self.get(key) {
            Some(InlineBodyImportResolution::Resolved(target)) => Some(target),
            Some(InlineBodyImportResolution::Unresolved) | None => None,
        }
    }
}

fn add_include_instance_modules(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &DagId,
    declarations: &[Declaration],
    resolved_imports: &impl ResolvedModuleLookup,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    for decl in declarations {
        let DeclKind::Include(include) = &decl.kind else {
            continue;
        };
        let instance_scope = include_instance_scope(include);
        let prefix = instance_scope.merge_scope_name();
        let Some(target) =
            resolved_imports.resolved_target(&ModulePathKey::from_path(&include.path))
        else {
            continue;
        };
        let Some(target_decls) = module_declarations(target.target(), files) else {
            continue;
        };
        let instance = owner.instance_child(prefix.as_str());
        resolver.add_module(instance.clone(), target_decls)?;
        add_nested_include_instance_modules(resolver, target.target(), &instance, files)?;
    }
    Ok(())
}

/// Give every synthetic include module the completed import scope of its
/// canonical source module.
///
/// The synthetic module starts with the source declarations, while this pass
/// adds the source's selective public re-exports and module aliases after all
/// canonical import edges have been registered.
fn inherit_include_instance_scopes(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &DagId,
    declarations: &[Declaration],
    resolved_imports: &impl ResolvedModuleLookup,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    for declaration in declarations {
        let DeclKind::Include(include) = &declaration.kind else {
            continue;
        };
        let instance_scope = include_instance_scope(include);
        let Some(source) =
            resolved_imports.resolved_target(&ModulePathKey::from_path(&include.path))
        else {
            continue;
        };
        let instance = owner.instance_child(instance_scope.merge_scope_name().as_str());
        resolver.inherit_module_scope(source.target(), &instance)?;
        inherit_nested_include_instance_scopes(resolver, source.target(), &instance, files)?;
    }
    Ok(())
}

/// Make each configured include's own indexes resolvable by their bare names
/// inside the importing module.
///
/// The synthetic include modules created by [`add_include_instance_modules`]
/// already hold the dependency's index declarations. This pass copies those
/// indexes into the importer's symbol table (skipping any the include binds or
/// overrides) so the inlined dependency bodies — `for s: Step`, `T[Step]`,
/// `Step.A` — resolve against the importer's merged registry. See
/// [`graphcal_compiler::syntax::module_resolve::ModuleResolver::inline_instantiated_include_indexes`].
fn link_include_instance_indexes(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &DagId,
    declarations: &[Declaration],
    resolved_imports: &impl ResolvedModuleLookup,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    for decl in declarations {
        let DeclKind::Include(include) = &decl.kind else {
            continue;
        };
        let instance_scope = include_instance_scope(include);
        let prefix = instance_scope.merge_scope_name();
        let Some(source) =
            resolved_imports.resolved_target(&ModulePathKey::from_path(&include.path))
        else {
            continue;
        };
        let synthetic = owner.instance_child(prefix.as_str());
        if resolver.modules().get(&synthetic).is_none() {
            // The synthetic module is only present when the include target
            // resolved to declarations; skip silently otherwise (a missing
            // target is already reported elsewhere).
            continue;
        }
        if let Some(bound_indexes) = include_index_bindings(include) {
            resolver.inline_instantiated_include_indexes(owner, &synthetic, &bound_indexes)?;
        }
        link_nested_include_instance_indexes(resolver, source.target(), &synthetic, files)?;
    }
    Ok(())
}

struct NestedIncludeInstance {
    source: DagId,
    instance: DagId,
    index_bindings: Option<HashSet<IndexName>>,
}

fn nested_include_instances(
    source: &DagId,
    instance: &DagId,
    files: &HashMap<DagId, LoadedFile>,
) -> Vec<NestedIncludeInstance> {
    module_declarations(source, files).map_or_else(Vec::new, |declarations| {
        declarations
            .iter()
            .filter_map(|declaration| {
                let DeclKind::Include(include) = &declaration.kind else {
                    return None;
                };
                let instance_scope = include_instance_scope(include);
                let source = resolved_module_target_from(source, &include.path, files)?;
                Some(NestedIncludeInstance {
                    source,
                    instance: instance.instance_child(instance_scope.merge_scope_name().as_str()),
                    index_bindings: include_index_bindings(include),
                })
            })
            .collect()
    })
}

fn add_nested_include_instance_modules(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    source: &DagId,
    instance: &DagId,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    let mut pending = nested_include_instances(source, instance, files)
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        let Some(child_declarations) = module_declarations(&child.source, files) else {
            continue;
        };
        resolver.add_module(child.instance.clone(), child_declarations)?;
        pending.extend(
            nested_include_instances(&child.source, &child.instance, files)
                .into_iter()
                .rev(),
        );
    }
    Ok(())
}

fn inherit_nested_include_instance_scopes(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    source: &DagId,
    instance: &DagId,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    let mut pending = nested_include_instances(source, instance, files)
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        resolver.inherit_module_scope(&child.source, &child.instance)?;
        pending.extend(
            nested_include_instances(&child.source, &child.instance, files)
                .into_iter()
                .rev(),
        );
    }
    Ok(())
}

fn link_nested_include_instance_indexes(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    source: &DagId,
    instance: &DagId,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    let mut pending = nested_include_instances(source, instance, files)
        .into_iter()
        .rev()
        .map(|child| (instance.clone(), child))
        .collect::<Vec<_>>();
    while let Some((parent_instance, child)) = pending.pop() {
        if let Some(bound_indexes) = &child.index_bindings {
            resolver.inline_instantiated_include_indexes(
                &parent_instance,
                &child.instance,
                bound_indexes,
            )?;
        }
        pending.extend(
            nested_include_instances(&child.source, &child.instance, files)
                .into_iter()
                .rev()
                .map(|grandchild| (child.instance.clone(), grandchild)),
        );
    }
    Ok(())
}

fn resolved_module_target_from(
    source: &DagId,
    path: &ModulePath,
    files: &HashMap<DagId, LoadedFile>,
) -> Option<DagId> {
    let key = ModulePathKey::from_path(path);
    let resolved = files.get(source).map_or_else(
        || {
            files.values().find_map(|file| {
                file.inline_dags
                    .iter()
                    .find(|inline| inline.dag_id == *source)
                    .and_then(|inline| match inline.resolved_imports.get(&key) {
                        Some(InlineBodyImportResolution::Resolved(target)) => Some(target.clone()),
                        Some(InlineBodyImportResolution::Unresolved) | None => None,
                    })
            })
        },
        |file| file.resolved_imports.get(&key).cloned(),
    )?;
    Some(resolved.target().clone())
}

fn include_instance_scope<P: Phase>(include: &IncludeDecl<P>) -> IncludeInstanceScope {
    include.instance_scope()
}

fn include_index_bindings<P: Phase>(include: &IncludeDecl<P>) -> Option<HashSet<IndexName>> {
    (!include.param_bindings.is_empty()).then(|| {
        include
            .param_bindings
            .iter()
            .map(|binding| IndexName::from_atom(binding.name.name.clone()))
            .collect()
    })
}

fn module_declarations<'a>(
    target: &DagId,
    files: &'a HashMap<DagId, LoadedFile>,
) -> Option<&'a [Declaration]> {
    if let Some(file) = files.get(target) {
        return Some(file.ast.declarations.as_slice());
    }
    files.values().find_map(|file| {
        file.inline_dags
            .iter()
            .find(|inline| inline.dag_id == *target)
            .map(|inline| inline.body(file))
    })
}

fn register_module_imports(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &DagId,
    declarations: &[Declaration],
    resolved_imports: &impl ResolvedModuleLookup,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    for decl in declarations {
        match &decl.kind {
            DeclKind::Import(import) => {
                if let Some(target) =
                    resolved_imports.resolved_target(&ModulePathKey::from_path(&import.path))
                {
                    resolver.register_import(owner, &import.path, &import.kind, target.target())?;
                }
            }
            DeclKind::Include(include)
                if resolved_imports
                    .resolved_target(&ModulePathKey::from_path(&include.path))
                    .is_some() =>
            {
                let instance_scope = include_instance_scope(include);
                let prefix = instance_scope.merge_scope_name();
                let target = owner.instance_child(prefix.as_str());
                resolver.register_include(owner, &include.path, &include.kind, &target)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Load a project starting from `root_path`, recursively loading all
/// files referenced by `import` declarations. Detects circular imports.
///
/// All filesystem access goes through the provided [`FileSystemReader`],
/// making this function I/O-free when given an in-memory implementation.
///
/// # Errors
///
/// Returns a [`CompileError`] if a file cannot be read, parsed, or
/// if circular imports are detected.
pub fn load_project<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
) -> Result<LoadedProject, CompileError> {
    load_project_with_budget_and_cancellation(
        root_path,
        project_root_override,
        fs,
        LoaderBudget::default(),
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Load a project with an explicit ingestion budget.
///
/// # Errors
///
/// Returns a [`CompileError`] for loading/parsing failures or a loader-budget
/// violation.
pub fn load_project_with_budget<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
    budget: LoaderBudget,
) -> Result<LoadedProject, CompileError> {
    load_project_with_budget_and_cancellation(
        root_path,
        project_root_override,
        fs,
        budget,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Load a project with cooperative cancellation between bounded reads, files,
/// and declarations.
///
/// # Errors
///
/// Returns a [`CompileError`] for loading/parsing failures or cooperative
/// cancellation.
pub fn load_project_with_cancellation<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    load_project_with_budget_and_cancellation(
        root_path,
        project_root_override,
        fs,
        LoaderBudget::default(),
        cancellation,
    )
}

/// Load a project with explicit resource and cancellation policies.
///
/// # Errors
///
/// Returns a [`CompileError`] for loading/parsing failures, budget violations,
/// or cooperative cancellation.
pub fn load_project_with_budget_and_cancellation<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
    budget: LoaderBudget,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    let mut budget = LoaderBudgetState::new(budget);
    load_project_with_budget_state(
        root_path,
        project_root_override,
        fs,
        &mut budget,
        cancellation,
    )
}

fn load_project_with_budget_state<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    cancellation.checkpoint()?;
    let root_canonical = fs
        .canonicalize(root_path)
        .map_err(|_| io_not_found(root_path))?;

    let root_dir = root_canonical.parent().unwrap_or(&root_canonical);
    let project_root = resolve_project_root(root_dir, project_root_override, fs)?;

    let mut files: HashMap<DagId, LoadedFile> = HashMap::new();
    let mut path_to_dag_id: HashMap<PathBuf, DagId> = HashMap::new();
    let mut load_order: Vec<DagId> = Vec::new();
    let mut loading: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();

    // Determine the package mode for the root file: real package iff a manifest
    // exists at `project_root` AND the root file lives inside the package's
    // namespace (`<source_dir>/<package_name>.gcl` or under
    // `<source_dir>/<package_name>/`). A file sitting next to a manifest but
    // outside the namespace is treated as a virtual package — cross-file
    // imports from it will be rejected. This collapses the two modes into a
    // single rule: to import across files, you must live in a real package.
    let manifest =
        load_manifest_for_root(&project_root, &root_canonical, fs, budget, cancellation)?;
    if let Some(package_manifest) = manifest.as_ref()
        && !package_manifest.dependencies.is_empty()
    {
        return load_locked_package_project(
            &root_canonical,
            &project_root,
            package_manifest.clone(),
            fs,
            budget,
            cancellation,
        );
    }
    let package_id = match manifest.as_ref() {
        Some(package_manifest) => DagPackageId::new(package_manifest.name.as_str()),
        None => virtual_package_id_for_path(&root_canonical)?,
    };

    load_file_dfs(
        &root_canonical,
        &project_root,
        &package_id,
        &mut files,
        &mut path_to_dag_id,
        &mut load_order,
        &mut loading,
        &mut stack,
        manifest.as_ref(),
        fs,
        budget,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    let root_dag_id = path_to_dag_id[&root_canonical].clone();
    cancellation.checkpoint()?;
    // Single-package project: every loaded file belongs to the root package,
    // so every declared wasm plugin resolves against the project root.
    let mut plugins = read_wasm_plugins(
        files.values().map(|file| &file.ast),
        &project_root,
        fs,
        budget,
        cancellation,
    )?;
    // A manifest opts the project into the lockfile trust regime: wasm
    // plugins must be pinned in graphcal.lock even when there are no
    // package dependencies. Virtual (manifest-less) projects load unpinned —
    // the sandbox and resource limits still bound what a plugin can do.
    if manifest.is_some() && !plugins.is_empty() {
        let pins = load_plugin_pins(&project_root, fs, budget, cancellation)?;
        apply_plugin_pins(&mut plugins, &pins);
    }
    cancellation.checkpoint()?;
    Ok(LoadedProject::from_parts(
        files,
        root_dag_id,
        load_order,
        plugins,
    ))
}

/// Read the root package's plugin pins from `graphcal.lock`, when present.
///
/// A missing lockfile yields zero pins (every plugin then reports "not
/// pinned"); an unreadable or invalid lockfile is a hard error, matching
/// the dependency-loading path.
fn load_plugin_pins(
    project_root: &Path,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<BTreeMap<String, String>, CompileError> {
    let lockfile_path = project_root.join("graphcal.lock");
    let lockfile_text =
        match budget.read_text(fs, &lockfile_path, LoaderArtifact::Lockfile, cancellation) {
            Ok(text) => text,
            Err(LoaderReadError::Filesystem(error))
                if error.io_kind() == Some(std::io::ErrorKind::NotFound) =>
            {
                return Ok(BTreeMap::new());
            }
            Err(LoaderReadError::Filesystem(FileSystemReadError::Cancelled)) => {
                cancellation.checkpoint()?;
                return Err(loader_manifest_error("plugin lockfile read cancelled"));
            }
            Err(error) => {
                return Err(loader_manifest_error(format!(
                    "could not read `{}`: {error}",
                    lockfile_path.display()
                )));
            }
        };
    let lockfile = parse_lockfile_str(&lockfile_text).map_err(|e| {
        CompileError::Eval(GraphcalError::ManifestError {
            message: e.to_string(),
        })
    })?;
    lockfile
        .validate(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)
        .map_err(|e| {
            CompileError::Eval(GraphcalError::ManifestError {
                message: e.to_string(),
            })
        })?;
    Ok(lockfile
        .plugins
        .iter()
        .map(|plugin| (plugin.path().to_string(), plugin.sha256().to_string()))
        .collect())
}

fn load_locked_package_project<F: FileSystemReader>(
    root_canonical: &Path,
    project_root: &Path,
    root_manifest: PackageManifest,
    fs: &F,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    cancellation.checkpoint()?;
    let context =
        PackageLoadContext::from_lockfile(project_root, root_manifest, fs, budget, cancellation)?;
    let root_package = context.graph.root().clone();

    let mut files: HashMap<DagId, LoadedFile> = HashMap::new();
    let mut path_to_dag_id: HashMap<(PackageInstanceId, PathBuf), DagId> = HashMap::new();
    let mut load_order: Vec<DagId> = Vec::new();
    let mut loading: HashSet<(PackageInstanceId, PathBuf)> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();

    load_package_file_dfs(
        root_canonical,
        &root_package,
        &context,
        &mut files,
        &mut path_to_dag_id,
        &mut load_order,
        &mut loading,
        &mut stack,
        budget,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    let root_dag_id = path_to_dag_id[&(root_package.clone(), root_canonical.to_path_buf())].clone();
    cancellation.checkpoint()?;
    // Wasm plugin files may only be declared by the root package (dependency
    // packages' declarations are rejected at verification); resolve and read
    // them against the root package's source root.
    let root_package_root = context.root_for(&root_package)?.to_path_buf();
    let root_dag_package = root_dag_id.package().clone();
    let root_reader = context.reader_for(&root_package)?;
    let mut plugins = read_wasm_plugins(
        files
            .values()
            .filter(|file| *file.dag_id.package() == root_dag_package)
            .map(|file| &file.ast),
        &root_package_root,
        root_reader,
        budget,
        cancellation,
    )?;
    apply_plugin_pins(&mut plugins, &context.plugin_pins);
    cancellation.checkpoint()?;
    Ok(LoadedProject::from_parts(
        files,
        root_dag_id,
        load_order,
        plugins,
    ))
}

/// Package-aware filesystem authority: root-package reads preserve the
/// caller-supplied capability (and therefore LSP overlays), while every locked
/// dependency receives its own immutable rooted capability.
struct PackageLoadContext<'a> {
    graph: PackageGraph,
    root_package: PackageInstanceId,
    root_reader: &'a dyn FileSystemReader,
    roots: BTreeMap<PackageInstanceId, PathBuf>,
    dependency_readers: BTreeMap<PackageInstanceId, RealFileSystem>,
    /// Root-package plugin pins from `graphcal.lock`: path → SHA-256.
    plugin_pins: BTreeMap<String, String>,
}

impl<'a> PackageLoadContext<'a> {
    fn from_lockfile(
        project_root: &Path,
        root_manifest: PackageManifest,
        fs: &'a dyn FileSystemReader,
        budget: &mut LoaderBudgetState,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, CompileError> {
        cancellation.checkpoint()?;
        let lockfile_path = project_root.join("graphcal.lock");
        let lockfile_text = budget
            .read_text(fs, &lockfile_path, LoaderArtifact::Lockfile, cancellation)
            .map_err(|error| {
                loader_manifest_error(format!(
                    "package dependencies require graphcal.lock; run `graphcal deps lock`: {error}"
                ))
            })?;
        let lockfile = parse_lockfile_str(&lockfile_text)
            .map_err(|error| loader_manifest_error(error.to_string()))?;
        let graph = lockfile
            .package_graph(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)
            .map_err(|error| loader_manifest_error(error.to_string()))?;
        let cache_dir = cache_dir().map_err(loader_manifest_error)?;
        let canonical_project_root = fs.canonicalize(project_root).map_err(|error| {
            loader_manifest_error(format!(
                "could not canonicalize root package `{}`: {error}",
                project_root.display()
            ))
        })?;
        let mut roots = BTreeMap::new();
        let mut dependency_readers = BTreeMap::new();

        for package in &lockfile.packages {
            cancellation.checkpoint()?;
            let candidate = source_root_candidate(project_root, &cache_dir, package);
            if package.id == lockfile.root {
                let canonical = fs
                    .canonicalize(&candidate)
                    .map_err(|error| source_root_error(package, &candidate, &error))?;
                if !canonical.starts_with(&canonical_project_root) {
                    return Err(loader_manifest_error(format!(
                        "locked root package `{}` resolves outside project root `{}`",
                        canonical.display(),
                        canonical_project_root.display()
                    )));
                }
                roots.insert(package.id.clone(), canonical);
            } else {
                let reader = RealFileSystem::rooted(&candidate)
                    .map_err(|error| source_root_error(package, &candidate, &error))?;
                let canonical = reader
                    .canonicalize(&candidate)
                    .map_err(|error| source_root_error(package, &candidate, &error))?;
                roots.insert(package.id.clone(), canonical);
                dependency_readers.insert(package.id.clone(), reader);
            }
        }

        let mut manifests = BTreeMap::new();
        for package in &lockfile.packages {
            cancellation.checkpoint()?;
            if package.id == lockfile.root {
                continue;
            }
            let root = roots.get(&package.id).ok_or_else(|| {
                loader_manifest_error(format!(
                    "lockfile package `{}` has no approved source root",
                    package.id
                ))
            })?;
            let reader = dependency_readers.get(&package.id).ok_or_else(|| {
                loader_manifest_error(format!(
                    "lockfile package `{}` has no filesystem capability",
                    package.id
                ))
            })?;
            let manifest = read_package_manifest_from_path(root, reader, budget, cancellation)?;
            verify_locked_source(root, package, &manifest, reader, budget, cancellation)?;
            manifests.insert(package.id.clone(), manifest);
        }
        manifests.insert(lockfile.root.clone(), root_manifest);
        validate_lock_against_manifests(
            &lockfile,
            &manifests,
            env!("CARGO_PKG_VERSION"),
            STDLIB_VERSION,
        )
        .map_err(|error| loader_manifest_error(error.to_string()))?;
        let plugin_pins = lockfile
            .plugins
            .iter()
            .map(|plugin| (plugin.path().to_string(), plugin.sha256().to_string()))
            .collect();
        Ok(Self {
            graph,
            root_package: lockfile.root,
            root_reader: fs,
            roots,
            dependency_readers,
            plugin_pins,
        })
    }

    fn root_for(&self, package: &PackageInstanceId) -> Result<&Path, CompileError> {
        self.roots
            .get(package)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                loader_manifest_error(format!("lockfile package `{package}` has no source root"))
            })
    }

    fn reader_for(
        &self,
        package: &PackageInstanceId,
    ) -> Result<&dyn FileSystemReader, CompileError> {
        if package == &self.root_package {
            Ok(self.root_reader)
        } else {
            self.dependency_readers
                .get(package)
                .map(|reader| reader as &dyn FileSystemReader)
                .ok_or_else(|| {
                    loader_manifest_error(format!(
                        "lockfile package `{package}` has no filesystem capability"
                    ))
                })
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "package-aware DFS state mirrors the single-package project loader"
)]
#[expect(
    clippy::too_many_lines,
    reason = "DFS body keeps package resolution and parsing in one traversal"
)]
fn load_package_file_dfs(
    canonical_path: &Path,
    package_id: &PackageInstanceId,
    context: &PackageLoadContext<'_>,
    files: &mut HashMap<DagId, LoadedFile>,
    path_to_dag_id: &mut HashMap<(PackageInstanceId, PathBuf), DagId>,
    load_order: &mut Vec<DagId>,
    loading: &mut HashSet<(PackageInstanceId, PathBuf)>,
    stack: &mut Vec<String>,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    let path_key = (package_id.clone(), canonical_path.to_path_buf());
    if path_to_dag_id.contains_key(&path_key) {
        return Ok(());
    }

    let display_name = format!("{package_id}:{}", canonical_path.display());
    if !loading.insert(path_key.clone()) {
        stack.push(display_name);
        let cycle_str = stack.join(" -> ");
        return Err(CompileError::Eval(GraphcalError::CircularImport {
            cycle: cycle_str,
        }));
    }
    stack.push(display_name.clone());

    let package_reader = context.reader_for(package_id)?;
    let source_str = budget
        .read_text(
            package_reader,
            canonical_path,
            LoaderArtifact::SourceFile,
            cancellation,
        )
        .map_err(|error| match error {
            LoaderReadError::Filesystem(filesystem)
                if filesystem.io_kind() == Some(std::io::ErrorKind::NotFound) =>
            {
                io_not_found(canonical_path)
            }
            other => loader_manifest_error(format!(
                "could not read source `{}`: {other}",
                canonical_path.display()
            )),
        })?;
    let source = Arc::new(source_str);
    let named_source = NamedSource::new(display_name.as_str(), Arc::clone(&source));
    let raw_ast = graphcal_compiler::syntax::parser::Parser::with_name(&source, &display_name)
        .parse_file_with_cancellation(cancellation)
        .map_err(parse_operation_error)?;
    cancellation.checkpoint()?;
    let ast = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);
    cancellation.checkpoint()?;
    let dag_names = collect_inline_dag_names(&ast.declarations);
    let package_root = context.root_for(package_id)?;
    let parent_dir = canonical_path.parent().unwrap_or_else(|| Path::new("."));
    let file_stem = canonical_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut resolved_imports_paths: HashMap<ModulePathKey, PackageResolvedPath> = HashMap::new();

    for decl in &ast.declarations {
        cancellation.checkpoint()?;
        let path = match &decl.kind {
            DeclKind::Import(import_decl) => &import_decl.path,
            DeclKind::Include(include_decl) => &include_decl.path,
            _ => continue,
        };
        if path.segments.len() == 1 && dag_names.contains(path.segments[0].name.as_str()) {
            continue;
        }
        if path.segments.len() == 1 && path.segments[0].name == file_stem {
            continue;
        }
        let resolved =
            resolve_package_import_path(path, package_id, context, &named_source, parent_dir)?;
        if resolved.path == canonical_path && resolved.package == *package_id {
            resolved_imports_paths.insert(ModulePathKey::from_path(path), resolved);
            continue;
        }
        load_package_file_dfs(
            &resolved.path,
            &resolved.package,
            context,
            files,
            path_to_dag_id,
            load_order,
            loading,
            stack,
            budget,
            cancellation,
        )?;
        resolved_imports_paths.insert(ModulePathKey::from_path(path), resolved);
    }

    for path in inline_dag_dependency_paths(&ast.declarations) {
        cancellation.checkpoint()?;
        if path.segments.len() == 1 && dag_names.contains(path.segments[0].name.as_str()) {
            continue;
        }
        if path.segments.len() == 1 && path.segments[0].name == file_stem {
            continue;
        }
        let Ok(resolved) =
            resolve_package_import_path(path, package_id, context, &named_source, parent_dir)
        else {
            continue;
        };
        if resolved.path == canonical_path && resolved.package == *package_id {
            continue;
        }
        load_package_file_dfs(
            &resolved.path,
            &resolved.package,
            context,
            files,
            path_to_dag_id,
            load_order,
            loading,
            stack,
            budget,
            cancellation,
        )?;
    }

    cancellation.checkpoint()?;
    let relative_path = canonical_path
        .strip_prefix(package_root)
        .unwrap_or(canonical_path);
    let dag_id = package_dag_id(package_id, relative_path, &named_source)?;
    let resolved_imports = resolved_imports_paths
        .iter()
        .map(|(key, resolved)| {
            let source_file = if resolved.package == *package_id && resolved.path == canonical_path
            {
                dag_id.clone()
            } else {
                path_to_dag_id[&(resolved.package.clone(), resolved.path.clone())].clone()
            };
            (key.clone(), resolved.target_from(&source_file))
        })
        .collect();
    let same_file_dag_ids = collect_inline_dag_ids(&ast.declarations, &dag_id);
    let inline_context = PackageInlineLiftContext {
        context,
        package_id,
        file_dag_id: &dag_id,
        same_file_dag_ids: &same_file_dag_ids,
        canonical_path,
        path_to_dag_id,
        src: &named_source,
        file_stem,
    };
    let inline_dags = lift_package_inline_dags(&ast, &dag_id, &inline_context);
    cancellation.checkpoint()?;

    load_order.push(dag_id.clone());
    loading.remove(&path_key);
    stack.pop();

    path_to_dag_id.insert(path_key, dag_id.clone());
    files.insert(
        dag_id.clone(),
        LoadedFile {
            path: canonical_path.to_path_buf(),
            dag_id,
            source,
            ast,
            named_source,
            resolved_imports,
            inline_dags,
        },
    );
    Ok(())
}

fn package_dag_id(
    package_id: &PackageInstanceId,
    relative_path: &Path,
    src: &NamedSource<Arc<String>>,
) -> Result<DagId, CompileError> {
    DagId::from_relative_path(package_id.as_str(), relative_path).map_err(|error| {
        CompileError::Eval(GraphcalError::internal_error(
            format!("invalid module path `{}`: {error}", relative_path.display()),
            src,
            DiagnosticAnchor::WholeFile,
        ))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageResolvedPath {
    package: PackageInstanceId,
    path: PathBuf,
    inline_path: Vec<DeclName>,
}

impl PackageResolvedPath {
    fn target_from(&self, source_file: &DagId) -> ResolvedModuleTarget {
        let target = self
            .inline_path
            .iter()
            .fold(source_file.clone(), |owner, name| {
                owner.child(name.as_str())
            });
        ResolvedModuleTarget::in_file(source_file.clone(), target)
    }
}

fn resolve_package_import_path(
    import_path: &ModulePath,
    current_package: &PackageInstanceId,
    context: &PackageLoadContext<'_>,
    src: &NamedSource<Arc<String>>,
    _parent_dir: &Path,
) -> Result<PackageResolvedPath, CompileError> {
    let segments = import_path
        .segments
        .iter()
        .map(|segment| segment.name.to_string())
        .collect::<Vec<_>>();
    if matches!(
        segments.first().map(String::as_str),
        Some("graphcal" | "std")
    ) {
        return Err(CompileError::Eval(GraphcalError::StdlibNotImplemented {
            path: import_path.display_path(),
            src: src.clone(),
            span: import_path.span.into(),
        }));
    }
    let resolved = context
        .graph
        .resolve_module_path(current_package, &segments)
        .map_err(|e| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!("{e}; run `graphcal deps lock` after changing dependencies"),
                src: src.clone(),
                span: import_path.span.into(),
            })
        })?;
    let package = context.graph.package(&resolved.package).ok_or_else(|| {
        CompileError::Eval(GraphcalError::ManifestError {
            message: format!("lockfile package `{}` is missing", resolved.package),
        })
    })?;
    let root = context.root_for(&resolved.package)?;
    let reader = context.reader_for(&resolved.package)?;
    let resolved_file = package_module_path(
        root,
        package,
        &resolved.module_segments,
        src,
        import_path,
        reader,
    )?;
    Ok(PackageResolvedPath {
        package: resolved.package,
        path: resolved_file.file,
        inline_path: resolved_file.inline_path,
    })
}

fn package_module_path(
    package_root: &Path,
    package: &LockedPackage,
    module_segments: &[String],
    src: &NamedSource<Arc<String>>,
    import_path: &ModulePath,
    fs: &dyn FileSystemReader,
) -> Result<ResolvedFilePath, CompileError> {
    for file_segment_count in (0..=module_segments.len()).rev() {
        let mut file_path = package_root
            .join(&package.source_dir)
            .join(package.name.as_str());
        for segment in &module_segments[..file_segment_count] {
            file_path = file_path.join(segment);
        }
        file_path.set_extension("gcl");
        let Ok(canonical) = fs.canonicalize(&file_path) else {
            continue;
        };
        let canonical = ensure_package_path(canonical, package_root, import_path, src)?;
        let inline_path = module_segments[file_segment_count..]
            .iter()
            .map(|segment| DeclName::try_new(segment.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                CompileError::Eval(GraphcalError::ManifestError {
                    message: format!("invalid locked module segment: {error}"),
                })
            })?;
        return Ok(ResolvedFilePath {
            file: canonical,
            inline_path,
        });
    }

    Err(CompileError::Eval(GraphcalError::ImportFileNotFound {
        path: import_path.display_path(),
        src: src.clone(),
        span: import_path.span.into(),
    }))
}

fn ensure_package_path(
    canonical: PathBuf,
    package_root: &Path,
    import_path: &ModulePath,
    src: &NamedSource<Arc<String>>,
) -> Result<PathBuf, CompileError> {
    if canonical.starts_with(package_root) {
        Ok(canonical)
    } else {
        Err(CompileError::Eval(GraphcalError::ImportOutsideRoot {
            path: import_path.display_path(),
            src: src.clone(),
            span: import_path.span.into(),
        }))
    }
}

fn source_root_candidate(
    project_root: &Path,
    cache_dir: &Path,
    package: &LockedPackage,
) -> PathBuf {
    match &package.source {
        PackageSource::Path { path } => project_root.join(path),
        PackageSource::Git { url, commit, .. } => {
            cache_dir.join("git").join(cache_key(url, commit))
        }
    }
}

fn source_root_error(
    package: &LockedPackage,
    candidate: &Path,
    error: &std::io::Error,
) -> CompileError {
    match &package.source {
        PackageSource::Path { .. } => loader_manifest_error(format!(
            "could not canonicalize locked path source `{}`: {error}",
            candidate.display()
        )),
        PackageSource::Git { .. } => loader_manifest_error(format!(
            "locked Git package `{}` is not materialized at `{}`; run `graphcal deps lock`: {error}",
            package.id,
            candidate.display()
        )),
    }
}

fn verify_locked_source(
    root: &Path,
    package: &LockedPackage,
    manifest: &PackageManifest,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    let PackageSource::Git { tree_hashes, .. } = &package.source else {
        return Ok(());
    };
    let cancellation_signal = || cancellation.is_cancelled();
    let actual = hash_source_tree(
        fs,
        root,
        &manifest.source_dir,
        budget.source_tree_limits(),
        &cancellation_signal,
    )
    .map_err(|error| {
        if matches!(error, graphcal_io::SourceTreeHashError::Cancelled) {
            cancellation
                .checkpoint()
                .map_or_else(CompileError::from, |()| loader_manifest_error(error))
        } else {
            loader_manifest_error(error)
        }
    })?;
    budget
        .account_source_tree(root, actual.entries(), actual.bytes())
        .map_err(loader_manifest_error)?;
    if actual.sha256() == tree_hashes.sha256 {
        Ok(())
    } else {
        Err(loader_manifest_error(format!(
            "cached package `{}` hash mismatch; expected {}, got {}; run `graphcal deps lock`",
            package.id,
            tree_hashes.sha256,
            actual.sha256()
        )))
    }
}

fn read_package_manifest_from_path(
    root: &Path,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PackageManifest, CompileError> {
    let manifest_path = root.join("graphcal.toml");
    let content = budget
        .read_text(fs, &manifest_path, LoaderArtifact::Manifest, cancellation)
        .map_err(|error| {
            loader_manifest_error(format!(
                "could not read `{}`: {error}",
                manifest_path.display()
            ))
        })?;
    parse_manifest_str(&content).map_err(|error| loader_manifest_error(error.to_string()))
}

fn cache_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(path) = TEST_CACHE_DIR.with(|slot| slot.borrow().clone()) {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("GRAPHCAL_CACHE_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path).join("graphcal"));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".cache").join("graphcal"))
        .ok_or_else(|| {
            "could not determine Graphcal cache directory; set GRAPHCAL_CACHE_DIR".to_string()
        })
}

fn cache_key(url: &GitUrl, rev: &GitCommitHash) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"git\0");
    hasher.update(url.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(rev.as_str().as_bytes());
    hex_string(&hasher.finalize())
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

struct PackageInlineLiftContext<'a> {
    context: &'a PackageLoadContext<'a>,
    package_id: &'a PackageInstanceId,
    file_dag_id: &'a DagId,
    same_file_dag_ids: &'a HashSet<DagId>,
    canonical_path: &'a Path,
    path_to_dag_id: &'a HashMap<(PackageInstanceId, PathBuf), DagId>,
    src: &'a NamedSource<Arc<String>>,
    file_stem: &'a str,
}

fn lift_package_inline_dags(
    ast: &File,
    self_dag_id: &DagId,
    context: &PackageInlineLiftContext<'_>,
) -> Vec<LoadedDag> {
    let mut out = Vec::new();
    lift_package_inline_dags_from_declarations(
        &ast.declarations,
        self_dag_id,
        context,
        &[],
        &mut out,
    );
    out
}

fn lift_package_inline_dags_from_declarations(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
    context: &PackageInlineLiftContext<'_>,
    parent_path: &[usize],
    out: &mut Vec<LoadedDag>,
) {
    for (index, decl) in declarations.iter().enumerate() {
        let DeclKind::Dag(dag) = &decl.kind else {
            continue;
        };
        let name = dag.name.value.to_string();
        let dag_id = lexical_parent_id.child(name.as_str());
        let resolved_imports = resolve_package_inline_body_imports(&dag.body, &dag_id, context);
        let mut body_path = parent_path.to_vec();
        body_path.push(index);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: context.file_dag_id.clone(),
            body_locator: DagBodyLocator::at_child(parent_path, index),
            resolved_imports,
        });
        lift_package_inline_dags_from_declarations(&dag.body, &dag_id, context, &body_path, out);
    }
}

fn resolve_package_inline_body_imports(
    body: &[Declaration],
    lexical_parent_id: &DagId,
    context: &PackageInlineLiftContext<'_>,
) -> HashMap<ModulePathKey, InlineBodyImportResolution> {
    body.iter()
        .filter_map(|body_decl| match &body_decl.kind {
            DeclKind::Import(import_decl) => Some(&import_decl.path),
            DeclKind::Include(include_decl) => Some(&include_decl.path),
            _ => None,
        })
        .map(|path| {
            let key = ModulePathKey::from_path(path);
            let resolution = resolve_package_inline_body_import(path, lexical_parent_id, context);
            (key, resolution)
        })
        .collect()
}

fn resolve_package_inline_body_import(
    path: &ModulePath,
    lexical_parent_id: &DagId,
    context: &PackageInlineLiftContext<'_>,
) -> InlineBodyImportResolution {
    if let Some(target) =
        resolve_same_file_inline_dag_path(path, lexical_parent_id, context.same_file_dag_ids)
    {
        return InlineBodyImportResolution::Resolved(ResolvedModuleTarget::in_file(
            context.file_dag_id.clone(),
            target,
        ));
    }
    if path.segments.len() == 1 && path.segments[0].name == context.file_stem {
        return InlineBodyImportResolution::Resolved(ResolvedModuleTarget::file_root(
            context.file_dag_id.clone(),
        ));
    }
    let Ok(resolved) = resolve_package_import_path(
        path,
        context.package_id,
        context.context,
        context.src,
        context
            .canonical_path
            .parent()
            .unwrap_or_else(|| Path::new(".")),
    ) else {
        return InlineBodyImportResolution::Unresolved;
    };
    let source_file =
        if resolved.path == context.canonical_path && resolved.package == *context.package_id {
            Some(context.file_dag_id.clone())
        } else {
            context
                .path_to_dag_id
                .get(&(resolved.package.clone(), resolved.path.clone()))
                .cloned()
        };
    source_file.map_or(InlineBodyImportResolution::Unresolved, |source_file| {
        InlineBodyImportResolution::Resolved(resolved.target_from(&source_file))
    })
}

/// DFS helper: load a single file and recurse into its `import` declarations.
///
/// `project_root` is the import boundary (parent directory of the entry-point
/// file). All imports must resolve to paths within this directory tree.
#[expect(
    clippy::too_many_arguments,
    reason = "DFS state requires many parameters"
)]
#[expect(
    clippy::too_many_lines,
    reason = "DFS body inlines parsing, top-level import resolution, and inline-dag self-import scan"
)]
fn load_file_dfs<F: FileSystemReader>(
    canonical_path: &Path,
    project_root: &Path,
    package_id: &DagPackageId,
    files: &mut HashMap<DagId, LoadedFile>,
    path_to_dag_id: &mut HashMap<PathBuf, DagId>,
    load_order: &mut Vec<DagId>,
    loading: &mut HashSet<PathBuf>,
    stack: &mut Vec<String>,
    manifest: Option<&PackageManifest>,
    fs: &F,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    // Already fully loaded — skip.
    if path_to_dag_id.contains_key(canonical_path) {
        return Ok(());
    }

    let display_name = canonical_path.display().to_string();

    // Cycle detection: if this file is currently being loaded, we have a cycle.
    if !loading.insert(canonical_path.to_path_buf()) {
        stack.push(display_name);
        let cycle_str = stack.join(" -> ");
        return Err(CompileError::Eval(GraphcalError::CircularImport {
            cycle: cycle_str,
        }));
    }
    stack.push(display_name.clone());

    // Read through the bounded capability before allocating or parsing.
    let source_str = budget
        .read_text(fs, canonical_path, LoaderArtifact::SourceFile, cancellation)
        .map_err(|error| match error {
            LoaderReadError::Filesystem(filesystem)
                if filesystem.io_kind() == Some(std::io::ErrorKind::NotFound) =>
            {
                io_not_found(canonical_path)
            }
            other => loader_manifest_error(format!(
                "could not read source `{}`: {other}",
                canonical_path.display()
            )),
        })?;
    let source = Arc::new(source_str);

    // Use the canonical path as the NamedSource name (not just the basename).
    // Downstream diagnostic emitters can recover the file URL via
    // `Url::from_file_path(Path::new(name))` without an external resolver,
    // and basename ambiguity (two `lib.gcl`s in different packages) cannot
    // arise. The CLI's miette renderer trims this for display anyway.
    let name = display_name.as_str();
    let named_source = NamedSource::new(name, Arc::clone(&source));
    let raw_ast = graphcal_compiler::syntax::parser::Parser::with_name(&source, name)
        .parse_file_with_cancellation(cancellation)
        .map_err(parse_operation_error)?;
    cancellation.checkpoint()?;
    let ast = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);
    cancellation.checkpoint()?;

    // Collect inline DAG names (including nested DAGs) so dependency scanning
    // can skip single-segment includes/imports that reference same-file DAG
    // modules rather than files.
    let dag_names = collect_inline_dag_names(&ast.declarations);

    // Find import and include declarations and recurse.
    let parent_dir = canonical_path.parent().unwrap_or_else(|| Path::new("."));
    let mut resolved_imports_paths: HashMap<ModulePathKey, ResolvedFilePath> = HashMap::new();
    for decl in &ast.declarations {
        cancellation.checkpoint()?;
        let path = match &decl.kind {
            DeclKind::Import(import_decl) => &import_decl.path,
            DeclKind::Include(include_decl) => &include_decl.path,
            _ => continue,
        };

        // Skip single-segment paths that reference an inline DAG declared in
        // this file, or that name the file's own virtual package (Concept 7).
        if path.segments.len() == 1 && dag_names.contains(path.segments[0].name.as_str()) {
            continue;
        }
        let file_stem = canonical_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if path.segments.len() == 1 && path.segments[0].name == file_stem {
            continue;
        }

        let resolved =
            resolve_import_path(path, parent_dir, project_root, &named_source, manifest, fs)?;

        // Path sandboxing: reject imports that resolve outside the project root.
        if !resolved.file.starts_with(project_root) {
            return Err(CompileError::Eval(GraphcalError::ImportOutsideRoot {
                path: path.display_path(),
                src: named_source,
                span: path.span().into(),
            }));
        }

        resolved_imports_paths.insert(ModulePathKey::from_path(path), resolved.clone());

        // A fully-qualified import that resolves to this very file (e.g.
        // `import pkg.main.inline_dag.{x};` inside main.gcl) is a
        // self-reference, not a dependency — recursing would trip the
        // circular-import check (mirrors the inline-dag loop below).
        if resolved.file == canonical_path {
            continue;
        }

        load_file_dfs(
            &resolved.file,
            project_root,
            package_id,
            files,
            path_to_dag_id,
            load_order,
            loading,
            stack,
            manifest,
            fs,
            budget,
            cancellation,
        )?;
    }

    // Inline DAG bodies are semantic DAG modules in their own right: their
    // imports/includes must drive project loading just like file-root
    // declarations. Resolution failures are not reported here because the
    // body import remains in the source and the module resolver can produce
    // the span-precise diagnostic later.
    for path in inline_dag_dependency_paths(&ast.declarations) {
        cancellation.checkpoint()?;
        if path.segments.len() == 1 && dag_names.contains(path.segments[0].name.as_str()) {
            continue;
        }
        let file_stem = canonical_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if path.segments.len() == 1 && path.segments[0].name == file_stem {
            continue;
        }

        let Ok(resolved) =
            resolve_import_path(path, parent_dir, project_root, &named_source, manifest, fs)
        else {
            continue;
        };
        if !resolved.file.starts_with(project_root) {
            return Err(CompileError::Eval(GraphcalError::ImportOutsideRoot {
                path: path.display_path(),
                src: named_source,
                span: path.span().into(),
            }));
        }
        if resolved.file == canonical_path {
            continue;
        }
        load_file_dfs(
            &resolved.file,
            project_root,
            package_id,
            files,
            path_to_dag_id,
            load_order,
            loading,
            stack,
            manifest,
            fs,
            budget,
            cancellation,
        )?;
    }

    cancellation.checkpoint()?;
    // Compute the DagId from the path relative to the project root.
    let relative_path = canonical_path
        .strip_prefix(project_root)
        .unwrap_or(canonical_path);
    let dag_id = DagId::from_relative_path(package_id.clone(), relative_path).map_err(|error| {
        CompileError::Eval(GraphcalError::internal_error(
            format!("invalid module path `{}`: {error}", relative_path.display()),
            &named_source,
            DiagnosticAnchor::WholeFile,
        ))
    })?;

    // Convert resolved import paths to DagIds. A self-import resolves to
    // this file's own id, which is not in `path_to_dag_id` yet (it is
    // inserted post-order, below).
    let resolved_imports = resolved_imports_paths
        .iter()
        .map(|(key, resolved)| {
            let source_file = if resolved.file == canonical_path {
                dag_id.clone()
            } else {
                path_to_dag_id[&resolved.file].clone()
            };
            (key.clone(), resolved.target_from(&source_file))
        })
        .collect();

    // Lift inline `dag X { ... }` bodies into structured `LoadedDag` entries
    // with per-dag pre-resolved imports. Self-imports map to this file's own
    // `DagId`; cross-file dag-body imports map to the dependency's id (when
    // already loaded via a file-level import). Resolution failures are
    // recorded explicitly; the dag-body import resolver runs later and will
    // surface a structured error if the path is genuinely invalid.
    let inline_dags = lift_inline_dags(
        &ast,
        &dag_id,
        canonical_path,
        parent_dir,
        project_root,
        &named_source,
        manifest,
        path_to_dag_id,
        fs,
    );
    cancellation.checkpoint()?;

    // Post-order: add this file after its dependencies.
    load_order.push(dag_id.clone());
    loading.remove(canonical_path);
    stack.pop();

    path_to_dag_id.insert(canonical_path.to_path_buf(), dag_id.clone());
    files.insert(
        dag_id.clone(),
        LoadedFile {
            path: canonical_path.to_path_buf(),
            dag_id,
            source,
            ast,
            named_source,
            resolved_imports,
            inline_dags,
        },
    );

    Ok(())
}

fn collect_inline_dag_names(declarations: &[Declaration]) -> HashSet<String> {
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

fn collect_inline_dag_ids(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
) -> HashSet<DagId> {
    declarations
        .iter()
        .flat_map(|decl| match &decl.kind {
            DeclKind::Dag(dag) => {
                let dag_id = lexical_parent_id.child(dag.name.value.as_str());
                let mut ids = collect_inline_dag_ids(&dag.body, &dag_id);
                ids.insert(dag_id);
                ids
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
                        DeclKind::Import(import_decl) => Some(&import_decl.path),
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

/// Walk inline `dag X { ... }` bodies and lift each into a [`LoadedDag`]
/// with pre-resolved imports. For each body `import` declaration:
///
/// - Single-segment file-stem reference (Concept 7) maps to `self_dag_id`.
/// - A path that resolves (via the package resolver) to `canonical_path`
///   maps to `self_dag_id` (a `import <self>.{...}` self-reference).
/// - A path that resolves to another file already loaded by the file-level
///   recursion maps to that file's `DagId` from `path_to_dag_id`.
/// - Anything else (resolution failure or a cross-file dependency that
///   wasn't pulled in at file level) is recorded as unresolved; the dag-body
///   resolver surfaces a structured error later.
#[expect(
    clippy::too_many_arguments,
    reason = "loader-side resolution needs the same context as file-level imports"
)]
fn lift_inline_dags<F: FileSystemReader>(
    ast: &File,
    self_dag_id: &DagId,
    canonical_path: &Path,
    parent_dir: &Path,
    project_root: &Path,
    src: &NamedSource<Arc<String>>,
    manifest: Option<&PackageManifest>,
    path_to_dag_id: &HashMap<PathBuf, DagId>,
    fs: &F,
) -> Vec<LoadedDag> {
    let file_stem = canonical_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut out = Vec::new();
    let same_file_dag_ids = collect_inline_dag_ids(&ast.declarations, self_dag_id);
    let context = InlineLiftContext {
        file_dag_id: self_dag_id,
        same_file_dag_ids: &same_file_dag_ids,
        canonical_path,
        parent_dir,
        project_root,
        src,
        manifest,
        path_to_dag_id,
        fs,
        file_stem,
    };
    lift_inline_dags_from_declarations(&ast.declarations, self_dag_id, &context, &[], &mut out);
    out
}

struct InlineLiftContext<'a, F: FileSystemReader> {
    file_dag_id: &'a DagId,
    same_file_dag_ids: &'a HashSet<DagId>,
    canonical_path: &'a Path,
    parent_dir: &'a Path,
    project_root: &'a Path,
    src: &'a NamedSource<Arc<String>>,
    manifest: Option<&'a PackageManifest>,
    path_to_dag_id: &'a HashMap<PathBuf, DagId>,
    fs: &'a F,
    file_stem: &'a str,
}

fn lift_inline_dags_from_declarations<F: FileSystemReader>(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
    context: &InlineLiftContext<'_, F>,
    parent_path: &[usize],
    out: &mut Vec<LoadedDag>,
) {
    for (index, decl) in declarations.iter().enumerate() {
        let DeclKind::Dag(dag) = &decl.kind else {
            continue;
        };
        let name = dag.name.value.to_string();
        let dag_id = lexical_parent_id.child(name.as_str());
        let resolved_imports = resolve_inline_body_imports(&dag.body, &dag_id, context);
        let mut body_path = parent_path.to_vec();
        body_path.push(index);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: context.file_dag_id.clone(),
            body_locator: DagBodyLocator::at_child(parent_path, index),
            resolved_imports,
        });
        lift_inline_dags_from_declarations(&dag.body, &dag_id, context, &body_path, out);
    }
}

fn resolve_inline_body_imports<F: FileSystemReader>(
    body: &[Declaration],
    lexical_parent_id: &DagId,
    context: &InlineLiftContext<'_, F>,
) -> HashMap<ModulePathKey, InlineBodyImportResolution> {
    body.iter()
        .filter_map(|body_decl| match &body_decl.kind {
            DeclKind::Import(import_decl) => Some(&import_decl.path),
            DeclKind::Include(include_decl) => Some(&include_decl.path),
            _ => None,
        })
        .map(|path| {
            let key = ModulePathKey::from_path(path);
            let resolution = resolve_inline_body_import(path, lexical_parent_id, context);
            (key, resolution)
        })
        .collect()
}

fn resolve_same_file_inline_dag_path(
    path: &ModulePath,
    lexical_parent_id: &DagId,
    same_file_dag_ids: &HashSet<DagId>,
) -> Option<DagId> {
    let [leaf] = path.segments() else {
        return None;
    };
    let child = lexical_parent_id.child(leaf.name.as_str());
    if same_file_dag_ids.contains(&child) {
        return Some(child);
    }
    lexical_parent_id.parent().and_then(|parent| {
        let sibling = parent.child(leaf.name.as_str());
        same_file_dag_ids.contains(&sibling).then_some(sibling)
    })
}

fn resolve_inline_body_import<F: FileSystemReader>(
    path: &ModulePath,
    lexical_parent_id: &DagId,
    context: &InlineLiftContext<'_, F>,
) -> InlineBodyImportResolution {
    if let Some(target) =
        resolve_same_file_inline_dag_path(path, lexical_parent_id, context.same_file_dag_ids)
    {
        return InlineBodyImportResolution::Resolved(ResolvedModuleTarget::in_file(
            context.file_dag_id.clone(),
            target,
        ));
    }
    // Single-segment file-stem reference — Concept 7 self-import.
    if path.segments.len() == 1 && path.segments[0].name == context.file_stem {
        return InlineBodyImportResolution::Resolved(ResolvedModuleTarget::file_root(
            context.file_dag_id.clone(),
        ));
    }
    let Ok(resolved) = resolve_import_path(
        path,
        context.parent_dir,
        context.project_root,
        context.src,
        context.manifest,
        context.fs,
    ) else {
        return InlineBodyImportResolution::Unresolved;
    };
    let source_file = if resolved.file == context.canonical_path {
        Some(context.file_dag_id.clone())
    } else {
        context.path_to_dag_id.get(&resolved.file).cloned()
    };
    source_file.map_or(InlineBodyImportResolution::Unresolved, |source_file| {
        InlineBodyImportResolution::Resolved(resolved.target_from(&source_file))
    })
}

/// Stem-only variant of [`lift_inline_dags`] for the single-file
/// [`LoadedProject::from_source`] path, where no project root or manifest is
/// available to drive full path resolution. Only the file-stem self-reference
/// (Concept 7) can be detected.
fn lift_inline_dags_by_stem(ast: &File, path: &Path, self_dag_id: &DagId) -> Vec<LoadedDag> {
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let same_file_dag_ids = collect_inline_dag_ids(&ast.declarations, self_dag_id);
    let mut out = Vec::new();
    lift_inline_dags_by_stem_from_declarations(
        &ast.declarations,
        self_dag_id,
        self_dag_id,
        file_stem,
        &same_file_dag_ids,
        &[],
        &mut out,
    );
    out
}

fn lift_inline_dags_by_stem_from_declarations(
    declarations: &[Declaration],
    file_dag_id: &DagId,
    lexical_parent_id: &DagId,
    file_stem: &str,
    same_file_dag_ids: &HashSet<DagId>,
    parent_path: &[usize],
    out: &mut Vec<LoadedDag>,
) {
    for (index, decl) in declarations.iter().enumerate() {
        let DeclKind::Dag(dag) = &decl.kind else {
            continue;
        };
        let name = dag.name.value.to_string();
        let dag_id = lexical_parent_id.child(name.as_str());
        let resolved_imports =
            dag.body
                .iter()
                .filter_map(|body_decl| match &body_decl.kind {
                    DeclKind::Import(import_decl) => Some(&import_decl.path),
                    DeclKind::Include(include_decl) => Some(&include_decl.path),
                    _ => None,
                })
                .map(|import_path| {
                    let key = ModulePathKey::from_path(import_path);
                    let resolution =
                        resolve_same_file_inline_dag_path(import_path, &dag_id, same_file_dag_ids)
                            .map_or_else(
                                || {
                                    if import_path.segments.len() == 1
                                        && import_path.segments[0].name == file_stem
                                    {
                                        InlineBodyImportResolution::Resolved(
                                            ResolvedModuleTarget::file_root(file_dag_id.clone()),
                                        )
                                    } else {
                                        InlineBodyImportResolution::Unresolved
                                    }
                                },
                                |target| {
                                    InlineBodyImportResolution::Resolved(
                                        ResolvedModuleTarget::in_file(file_dag_id.clone(), target),
                                    )
                                },
                            );
                    (key, resolution)
                })
                .collect();
        let mut body_path = parent_path.to_vec();
        body_path.push(index);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: file_dag_id.clone(),
            body_locator: DagBodyLocator::at_child(parent_path, index),
            resolved_imports,
        });
        lift_inline_dags_by_stem_from_declarations(
            &dag.body,
            file_dag_id,
            &dag_id,
            file_stem,
            same_file_dag_ids,
            &body_path,
            out,
        );
    }
}

/// Walk up from `start_dir` looking for a `graphcal.toml` manifest. Returns
/// the directory containing the manifest, or `None` if no ancestor has one.
///
/// Filesystem access goes through `fs` so callers using overlays, mocks, or
/// sandboxed real filesystems all share the same discovery rule.
pub fn discover_project_root<F: FileSystemReader>(start_dir: &Path, fs: &F) -> Option<PathBuf> {
    let mut dir = start_dir;
    loop {
        if fs.is_file(&dir.join("graphcal.toml")) {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Build a [`RealFileSystem`] sandboxed to the project root for compiling
/// `file`. Used by the CLI and the LSP so both discover the same root.
///
/// Resolution order:
/// 1. If `root_override` is present, root the filesystem there.
/// 2. Otherwise pick a starting directory: the parent of the canonicalized
///    file when it exists on disk, falling back to the canonicalized parent
///    of the input path (so unsaved LSP buffers can still walk up from their
///    enclosing directory).
/// 3. From that directory, walk up looking for `graphcal.toml`; if found,
///    root the filesystem there.
/// 4. Otherwise root the filesystem at the loose file's enclosing directory.
///
/// The function never returns an unrestricted filesystem: a root that cannot
/// be canonicalized is an error rather than a reason to drop the sandbox.
///
/// # Errors
///
/// Returns a [`CompileError`] when the explicit, discovered, or loose-file
/// root cannot be canonicalized.
pub fn build_rooted_filesystem(
    file: &Path,
    root_override: Option<&Path>,
) -> Result<RealFileSystem, CompileError> {
    if let Some(explicit) = root_override {
        return RealFileSystem::rooted(explicit).map_err(|_| io_not_found(explicit));
    }

    let start_dir = file
        .canonicalize()
        .ok()
        .and_then(|canonical| canonical.parent().map(Path::to_path_buf))
        .or_else(|| file.parent().and_then(|parent| parent.canonicalize().ok()))
        .ok_or_else(|| io_not_found(file))?;

    let discovery_fs = RealFileSystem::default();
    let project_root = project_root_for(&start_dir, &discovery_fs);
    RealFileSystem::rooted(&project_root).map_err(|_| io_not_found(&project_root))
}

/// Pick the project root directory for `root_file_dir`, falling back to
/// `root_file_dir` itself when no manifest is found anywhere up the tree.
///
/// This is the predictable default the loader uses for files that aren't
/// part of a `graphcal.toml`-defined package: imports can reach siblings
/// and descendants but not files above the entry-point's own directory.
fn project_root_for<F: FileSystemReader>(root_file_dir: &Path, fs: &F) -> PathBuf {
    discover_project_root(root_file_dir, fs).unwrap_or_else(|| root_file_dir.to_path_buf())
}

/// Resolve the project root, using an explicit override if provided,
/// otherwise falling back to automatic `graphcal.toml` discovery.
///
/// # Errors
///
/// Returns a [`CompileError`] if the override path does not exist or
/// cannot be canonicalized.
fn resolve_project_root<F: FileSystemReader>(
    root_file_dir: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
) -> Result<PathBuf, CompileError> {
    project_root_override.map_or_else(
        || Ok(project_root_for(root_file_dir, fs)),
        |explicit| {
            fs.canonicalize(explicit)
                .map_err(|_| io_not_found(explicit))
        },
    )
}

/// Load the manifest at `project_root` and decide whether the root file is
/// part of the real package.
///
/// Returns `Some(manifest)` only when a manifest is present AND the root file
/// lives inside the package namespace (either `<source_dir>/<pkg>.gcl` or
/// under `<source_dir>/<pkg>/`). Returns `None` for the virtual-package
/// scenarios: no manifest, or the root file sits next to a manifest but
/// outside the namespace (treated as a standalone script).
///
/// # Errors
///
/// Returns a [`CompileError`] if a manifest exists but cannot be read or
/// parsed.
fn load_manifest_for_root<F: FileSystemReader>(
    project_root: &Path,
    root_canonical: &Path,
    fs: &F,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Option<PackageManifest>, CompileError> {
    let manifest_path = project_root.join("graphcal.toml");
    if !fs.exists(&manifest_path) {
        return Ok(None);
    }
    let manifest_content = budget
        .read_text(fs, &manifest_path, LoaderArtifact::Manifest, cancellation)
        .map_err(|error| {
            loader_manifest_error(format!(
                "could not read `{}`: {error}",
                manifest_path.display()
            ))
        })?;
    let parsed = parse_manifest_str(&manifest_content)
        .map_err(|error| loader_manifest_error(error.to_string()))?;

    if root_in_package_namespace(project_root, root_canonical, &parsed, fs) {
        Ok(Some(parsed))
    } else {
        Ok(None)
    }
}

/// True iff `root_canonical` is the package's single-file module
/// (`<source_dir>/<pkg>.gcl`) or lives under the package namespace directory
/// (`<source_dir>/<pkg>/...`).
fn root_in_package_namespace<F: FileSystemReader>(
    project_root: &Path,
    root_canonical: &Path,
    manifest: &PackageManifest,
    fs: &F,
) -> bool {
    let pkg_dir = project_root
        .join(&manifest.source_dir)
        .join(manifest.name.as_str());
    let pkg_file = project_root
        .join(&manifest.source_dir)
        .join(format!("{}.gcl", manifest.name));

    if let Ok(canon_pkg_dir) = fs.canonicalize(&pkg_dir)
        && root_canonical.starts_with(&canon_pkg_dir)
    {
        return true;
    }
    if let Ok(canon_pkg_file) = fs.canonicalize(&pkg_file)
        && root_canonical == canon_pkg_file
    {
        return true;
    }
    false
}

/// Resolve a `ModulePath` to a canonical file path.
///
/// All paths are absolute from a package root (real package via
/// `graphcal.toml` manifest, or virtual package = single-file project).
/// The first segment names the package; remaining segments walk the
/// directory tree under `source_dir`. Reserved first segments `graphcal`
/// and `std` route to the (deferred) stdlib resolver.
fn resolve_import_path<F: FileSystemReader>(
    import_path: &ModulePath,
    _parent_dir: &Path,
    project_root: &Path,
    src: &NamedSource<Arc<String>>,
    manifest: Option<&PackageManifest>,
    fs: &F,
) -> Result<ResolvedFilePath, CompileError> {
    resolve_module_path(
        import_path.segments.as_slice(),
        import_path.span,
        project_root,
        src,
        manifest,
        fs,
    )
}

/// Resolve a bare module path to a canonical file path.
///
/// For `nasa/rocket`, resolves to `<project_root>/<source_dir>/nasa/rocket.gcl`.
fn resolve_module_path<F: FileSystemReader>(
    segments: &[graphcal_compiler::syntax::ast::Ident],
    span: graphcal_compiler::syntax::span::Span,
    project_root: &Path,
    src: &NamedSource<Arc<String>>,
    manifest: Option<&PackageManifest>,
    fs: &F,
) -> Result<ResolvedFilePath, CompileError> {
    let display_path = segments
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(".");

    // Stdlib namespace (deferred). Both `graphcal` and `std` first segments
    // are reserved for the standard library (per Concept §6.2 of the design).
    if !segments.is_empty() && (segments[0].name == "graphcal" || segments[0].name == "std") {
        return Err(CompileError::Eval(GraphcalError::StdlibNotImplemented {
            path: display_path,
            src: src.clone(),
            span: span.into(),
        }));
    }

    // The manifest is determined eagerly by `load_manifest_for_root` based on
    // whether the root file lives inside the package namespace. If it's
    // `Some`, we're in a real package; if it's `None`, the root is a virtual
    // package (either truly manifest-less, or a loose file sitting next to a
    // manifest but outside `<source_dir>/<pkg>/`).
    if let Some(m) = manifest {
        // Real package: first segment must match the package name.
        if !segments.is_empty() && segments[0].name != m.name.as_str() {
            return Err(CompileError::Eval(GraphcalError::PackageNameMismatch {
                path_first: segments[0].name.to_string(),
                package_name: m.name.to_string(),
                src: src.clone(),
                span: span.into(),
            }));
        }

        // Choose the longest prefix that names a physical source file. Any
        // remaining segments are an exact nested inline-DAG path in that file.
        for file_segment_count in (1..=segments.len()).rev() {
            let mut file_path = project_root.join(&m.source_dir);
            for segment in &segments[..file_segment_count] {
                file_path = file_path.join(segment.name.as_str());
            }
            file_path.set_extension("gcl");
            let Ok(canonical) = fs.canonicalize(&file_path) else {
                continue;
            };
            let inline_path = segments[file_segment_count..]
                .iter()
                .map(|segment| DeclName::from_atom(segment.name.clone()))
                .collect();
            return Ok(ResolvedFilePath {
                file: canonical,
                inline_path,
            });
        }

        return Err(CompileError::Eval(GraphcalError::ImportFileNotFound {
            path: display_path,
            src: src.clone(),
            span: span.into(),
        }));
    }

    // No manifest — virtual-package mode. The project is a single standalone
    // file. The only legal path is the file's own stem (Concept 7
    // self-reference), and that case is intercepted earlier in `load_file_dfs`
    // before resolution; reaching this point means the user asked for a
    // sibling or descendant that has no manifest-backed package to resolve
    // it.
    Err(CompileError::Eval(
        GraphcalError::CrossFileImportInVirtualPackage {
            path: display_path,
            src: src.clone(),
            span: span.into(),
        },
    ))
}

fn virtual_package_id_for_path(path: &Path) -> Result<DagPackageId, CompileError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CompileError::Eval(GraphcalError::InvalidSourcePath {
                path: path.display().to_string(),
                reason: "source path has no UTF-8 file name".to_string(),
            })
        })?;
    let stem = file_name.strip_suffix(".gcl").ok_or_else(|| {
        CompileError::Eval(GraphcalError::InvalidSourcePath {
            path: path.display().to_string(),
            reason: "source path must end with `.gcl`".to_string(),
        })
    })?;
    Ok(DagPackageId::new(stem))
}

/// Helper to create a `FileNotFound` error (used for the root file itself).
fn io_not_found(path: &Path) -> CompileError {
    CompileError::Eval(GraphcalError::FileNotFound {
        path: path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::io;

    use graphcal_compiler::syntax::non_empty::NonEmpty;
    use graphcal_io::{CancellationSignal, EntryLimit, NeverCancel, RealFileSystem};

    fn fs() -> RealFileSystem {
        RealFileSystem::default()
    }

    #[test]
    fn loaded_plugin_debug_never_exposes_module_bytes() {
        let plugin = LoadedPlugin {
            bytes: Arc::from(b"private-wasm-payload".as_slice()),
            sha256_hex: "digest".to_string(),
        };

        let rendered = format!("{plugin:#?}");
        assert!(rendered.contains("byte_len: 20"));
        assert!(rendered.contains("sha256_hex: \"digest\""));
        assert!(!rendered.contains("private-wasm-payload"));
        assert!(!rendered.contains("112, 114, 105, 118, 97, 116, 101"));
    }

    struct MutatingManifestFileSystem {
        inner: RealFileSystem,
        manifest_reads: Cell<usize>,
    }

    impl Default for MutatingManifestFileSystem {
        fn default() -> Self {
            Self {
                inner: RealFileSystem::default(),
                manifest_reads: Cell::new(0),
            }
        }
    }

    impl FileSystemReader for MutatingManifestFileSystem {
        fn read_bytes_bounded(
            &self,
            path: &Path,
            limit: ByteLimit,
            cancellation: &dyn CancellationSignal,
        ) -> Result<Vec<u8>, FileSystemReadError> {
            if path.file_name() == Some(std::ffi::OsStr::new("graphcal.toml")) {
                let read = self
                    .manifest_reads
                    .get()
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("manifest read counter overflow"))?;
                self.manifest_reads.set(read);
                if read > 1 {
                    let bytes = b"[package]\nname = \"mutated\"\n";
                    if bytes.len() as u64 > limit.get() {
                        return Err(FileSystemReadError::ByteLimitExceeded { limit });
                    }
                    return Ok(bytes.to_vec());
                }
            }
            self.inner.read_bytes_bounded(path, limit, cancellation)
        }

        fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
            self.inner.canonicalize(path)
        }

        fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
            self.inner.entry_kind(path)
        }

        fn read_directory_bounded(
            &self,
            path: &Path,
            limit: EntryLimit,
            cancellation: &dyn CancellationSignal,
        ) -> Result<Vec<std::ffi::OsString>, FileSystemReadError> {
            self.inner.read_directory_bounded(path, limit, cancellation)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }
    }

    /// Create a temporary directory with the given files and return its path.
    fn setup_temp_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
        }
        dir
    }

    fn name_path(segments: &[&str]) -> graphcal_compiler::syntax::names::NamePath {
        let atoms = segments
            .iter()
            .map(|segment| graphcal_compiler::syntax::names::NameAtom::parse(*segment).unwrap())
            .collect::<Vec<_>>();
        graphcal_compiler::syntax::names::NamePath::new(
            graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(atoms).unwrap(),
        )
    }

    #[test]
    fn rooted_filesystem_rejects_invalid_explicit_root() {
        let dir = setup_temp_dir(&[("main.gcl", "param x: Dimensionless = 1.0;")]);
        let missing_root = dir.path().join("missing");

        let error = build_rooted_filesystem(&dir.path().join("main.gcl"), Some(&missing_root))
            .expect_err("an invalid explicit root must not disable the sandbox");

        assert!(matches!(
            error,
            CompileError::Eval(GraphcalError::FileNotFound { path })
                if path == missing_root.display().to_string()
        ));
    }

    #[test]
    fn rooted_filesystem_confines_loose_file_to_its_parent() {
        let dir = setup_temp_dir(&[
            ("project/main.gcl", "param x: Dimensionless = 1.0;"),
            ("outside.gcl", "param secret: Dimensionless = 42.0;"),
        ]);
        let main = dir.path().join("project/main.gcl");
        let outside = dir.path().join("outside.gcl");
        let fs = build_rooted_filesystem(&main, None).unwrap();

        assert!(
            fs.read_bytes_bounded(&main, ByteLimit::new(1024), &NeverCancel)
                .is_ok()
        );
        assert!(
            fs.read_bytes_bounded(&outside, ByteLimit::new(1024), &NeverCancel)
                .is_err()
        );
    }

    #[test]
    fn load_standalone_file() {
        let dir = setup_temp_dir(&[("standalone.gcl", "param x: Dimensionless = 1.0;")]);
        let project = load_project(&dir.path().join("standalone.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.load_order.len(), 1);
        assert_eq!(project.root.package(), &DagPackageId::new("standalone"));
    }

    #[test]
    fn root_manifest_is_read_once_and_reused_as_one_snapshot() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"mission\"\n"),
            ("src/mission/main.gcl", "param x: Dimensionless = 1.0;"),
        ]);
        let fs = MutatingManifestFileSystem::default();

        let project = load_project(&dir.path().join("src/mission/main.gcl"), None, &fs).unwrap();

        assert_eq!(fs.manifest_reads.get(), 1);
        assert_eq!(project.root.package(), &DagPackageId::new("mission"));
    }

    #[test]
    fn load_simple_import() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/lib.gcl", "param y: Dimensionless = 2.0;"),
            (
                "src/helper/main.gcl",
                "import helper.lib.{y};\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/helper/main.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 2);
        assert_eq!(project.load_order.len(), 2);
        // helper.lib should be loaded before main (topological order)
        let lib_dag_id = DagId::new("helper", NonEmpty::new("src", vec!["helper", "lib"]));
        let main_dag_id = DagId::new("helper", NonEmpty::new("src", vec!["helper", "main"]));
        assert_eq!(project.load_order[0], lib_dag_id);
        assert_eq!(project.load_order[1], main_dag_id);
        assert_eq!(project.root.package(), &DagPackageId::new("helper"));
    }

    #[test]
    fn loaded_project_builds_module_resolver_for_qualified_index_variant() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/lib.gcl", "pub index Phase = { Burn, Coast };"),
            ("src/helper/main.gcl", "import helper.lib as lib;"),
        ]);
        let project = load_project(&dir.path().join("src/helper/main.gcl"), None, &fs()).unwrap();
        let resolver = project.build_module_resolver().unwrap();
        let lib_dag_id = DagId::new("helper", NonEmpty::new("src", vec!["helper", "lib"]));

        let resolved_variant = resolver
            .resolve_index_variant_path(&project.root, &name_path(&["lib", "Phase", "Burn"]))
            .unwrap();

        assert_eq!(resolved_variant.index().owner(), &lib_dag_id);
        assert_eq!(resolved_variant.index().as_str(), "Phase");
        assert_eq!(resolved_variant.variant().as_str(), "Burn");
    }

    #[test]
    fn load_cross_file_import_in_virtual_package_rejected() {
        // Without a `graphcal.toml`, the project is a single-file virtual
        // package; a sibling-file import is rejected with a structured
        // error pointing the user at the manifest fix.
        let dir = setup_temp_dir(&[
            ("helper.gcl", "param y: Dimensionless = 2.0;"),
            (
                "main.gcl",
                "import helper.{y};\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let result = load_project(&dir.path().join("main.gcl"), None, &fs());
        let err = result.expect_err("expected sibling import to be rejected");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("CrossFileImportInVirtualPackage"),
            "expected CrossFileImportInVirtualPackage, got: {msg}"
        );
    }

    #[test]
    fn load_circular_import_detected() {
        // Manifest layout: `package = "a"`, files at `<root>/a.gcl` and
        // `<root>/a/b.gcl`. `a` imports from `a.b` and `a.b` imports from
        // `a` — yielding a cycle through dot-paths.
        let dir = setup_temp_dir(&[
            (
                "graphcal.toml",
                "[package]\nname = \"a\"\nsource_dir = \".\"\n",
            ),
            ("a.gcl", "import a.b.{y};\nparam x: Dimensionless = 1.0;"),
            ("a/b.gcl", "import a.{x};\nparam y: Dimensionless = 2.0;"),
        ]);
        let result = load_project(&dir.path().join("a.gcl"), None, &fs());
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("circular") || err.contains("Circular"),
            "error should mention circular: {err}"
        );
    }

    #[test]
    fn load_missing_import_file() {
        let dir = setup_temp_dir(&[("main.gcl", "import nonexistent.{x};")]);
        let result = load_project(&dir.path().join("main.gcl"), None, &fs());
        assert!(result.is_err());
    }

    #[test]
    fn from_source_single_file() {
        let source = "param x: Dimensionless = 1.0;";
        let project = LoadedProject::from_source(source, "test.gcl").unwrap();
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.load_order.len(), 1);
        let root_file = &project.files[&project.root];
        assert_eq!(root_file.source.as_str(), source);
        assert_eq!(project.root.package(), &DagPackageId::new("test"));
    }

    #[test]
    fn from_source_keeps_absolute_diagnostic_label_out_of_semantic_identity() {
        let project = LoadedProject::from_source(
            "param x: Dimensionless = 1.0;",
            "/virtual/project/main.gcl",
        )
        .unwrap();
        let root_file = &project.files[&project.root];

        assert_eq!(root_file.named_source.name(), "/virtual/project/main.gcl");
        assert_eq!(project.root.package(), &DagPackageId::new("main"));
        assert_eq!(project.root.to_string(), "main");
    }

    #[test]
    fn inline_dag_unresolved_body_import_is_recorded_explicitly() {
        let source = r"
dag calc {
  import missing.{x};
  param input: Dimensionless = 1.0;
  pub node output: Dimensionless = @input;
}
";
        let project = LoadedProject::from_source(source, "test.gcl").unwrap();
        let root_file = &project.files[&project.root];
        let loaded_dag = root_file
            .inline_dags
            .iter()
            .find(|dag| dag.dag_id.name() == "calc")
            .expect("inline DAG should be lifted");

        assert!(
            loaded_dag
                .resolved_imports
                .values()
                .any(|resolution| { matches!(resolution, InlineBodyImportResolution::Unresolved) })
        );
    }

    #[test]
    fn from_source_parse_error() {
        let source = "this is not valid graphcal";
        let result = LoadedProject::from_source(source, "bad.gcl");
        assert!(result.is_err());
    }

    #[test]
    fn load_with_overlay_uses_overlay_for_root() {
        let dir = setup_temp_dir(&[("main.gcl", "param x: Dimensionless = 1.0;")]);
        let root_path = dir.path().join("main.gcl");

        let overlay_source = "param x: Dimensionless = 99.0;";
        let canonical = root_path.canonicalize().unwrap();
        let fs = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            canonical,
            overlay_source.to_string(),
        );
        let project = load_project(&root_path, None, &fs).unwrap();

        let root_file = &project.files[&project.root];
        assert_eq!(root_file.source.as_str(), overlay_source);
    }

    #[test]
    fn load_with_overlay_uses_disk_for_imports() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/lib.gcl", "param y: Dimensionless = 2.0;"),
            (
                "src/helper/main.gcl",
                "import helper.lib.{y};\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let root_path = dir.path().join("src/helper/main.gcl");

        let overlay_source = "import helper.lib.{y};\nnode z: Dimensionless = @y + 99.0;";
        let canonical = root_path.canonicalize().unwrap();
        let fs = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            canonical,
            overlay_source.to_string(),
        );
        let project = load_project(&root_path, None, &fs).unwrap();

        // Root file should use overlay content
        let root_file = &project.files[&project.root];
        assert_eq!(root_file.source.as_str(), overlay_source);

        // Helper.lib file should use disk content
        let lib_dag_id = DagId::new("helper", NonEmpty::new("src", vec!["helper", "lib"]));
        let lib_file = &project.files[&lib_dag_id];
        assert_eq!(lib_file.source.as_str(), "param y: Dimensionless = 2.0;");
    }

    #[test]
    fn load_with_overlay_parse_error_propagates() {
        let dir = setup_temp_dir(&[("main.gcl", "param x: Dimensionless = 1.0;")]);
        let root_path = dir.path().join("main.gcl");

        let bad_overlay = "this is not valid graphcal";
        let canonical = root_path.canonicalize().unwrap();
        let fs = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            canonical,
            bad_overlay.to_string(),
        );
        let result = load_project(&root_path, None, &fs);
        assert!(result.is_err());
    }

    #[test]
    fn load_diamond_import_deduplication() {
        // A imports B and C; both B and C import D. D should only be
        // loaded once.
        //
        // Manifest layout: `package = "graph"`, source_dir = ".". The
        // four files live at `<root>/graph/{a,b,c,d}.gcl` so every
        // import path starts with the package name.
        let dir = setup_temp_dir(&[
            (
                "graphcal.toml",
                "[package]\nname = \"graph\"\nsource_dir = \".\"\n",
            ),
            ("graph/d.gcl", "param w: Dimensionless = 4.0;"),
            (
                "graph/b.gcl",
                "import graph.d.{w};\nparam x: Dimensionless = @w + 1.0;",
            ),
            (
                "graph/c.gcl",
                "import graph.d.{w};\nparam y: Dimensionless = @w + 2.0;",
            ),
            (
                "graph/a.gcl",
                "import graph.b.{x};\nimport graph.c.{y};\nnode z: Dimensionless = @x + @y;",
            ),
        ]);
        let project = load_project(&dir.path().join("graph/a.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 4);
        // d should appear first in load order
        let d_dag_id = DagId::new("graph", NonEmpty::new("graph", vec!["d"]));
        assert_eq!(project.load_order[0], d_dag_id);
    }

    #[test]
    fn project_root_is_entry_point_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = project_root_for(dir.path(), &fs());
        assert_eq!(result, dir.path());
    }

    #[test]
    fn project_root_for_with_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.path().join("graphcal.toml"), "").unwrap();

        // From the subdirectory, the manifest in the parent should be found.
        let result = project_root_for(&sub, &fs());
        assert_eq!(result, dir.path());
    }

    // ---- Bare module path loader tests ----

    #[test]
    fn load_bare_import_selective() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            ("src/nasa/rocket.gcl", "param x: Dimensionless = 1.0;"),
            (
                "src/nasa/main.gcl",
                "import nasa.rocket.{x};\nnode y: Dimensionless = @x + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/nasa/main.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn load_bare_import_nested_path() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            (
                "src/nasa/orbital/transfer.gcl",
                "param dv: Dimensionless = 2460.0;",
            ),
            (
                "src/nasa/main.gcl",
                "import nasa.orbital.transfer.{dv};\nnode x: Dimensionless = @dv;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/nasa/main.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn load_bare_import_custom_source_dir() {
        let dir = setup_temp_dir(&[
            (
                "graphcal.toml",
                "[package]\nname = \"myproject\"\nsource_dir = \"lib\"\n",
            ),
            (
                "lib/myproject/helpers.gcl",
                "param x: Dimensionless = 42.0;",
            ),
            (
                "lib/myproject/main.gcl",
                "import myproject.helpers.{x};\nnode y: Dimensionless = @x + 1.0;",
            ),
        ]);
        let project =
            load_project(&dir.path().join("lib/myproject/main.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 2);
    }
    #[test]
    fn load_bare_import_package_name_mismatch_error() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            ("src/other/rocket.gcl", "param x: Dimensionless = 1.0;"),
            ("src/nasa/main.gcl", "import other.rocket.{x};"),
        ]);
        let result = load_project(&dir.path().join("src/nasa/main.gcl"), None, &fs());
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("PackageNameMismatch") || err.contains("package name"),
            "error should mention package name mismatch: {err}"
        );
    }

    #[test]
    fn load_bare_import_stdlib_deferred_error() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            ("src/nasa/main.gcl", "import graphcal.math.{sin};"),
        ]);
        let result = load_project(&dir.path().join("src/nasa/main.gcl"), None, &fs());
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("StdlibNotImplemented") || err.contains("stdlib"),
            "error should mention stdlib not implemented: {err}"
        );
    }

    #[test]
    fn load_bare_import_file_not_found_error() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            ("src/nasa/main.gcl", "import nasa.nonexistent.{x};"),
        ]);
        let result = load_project(&dir.path().join("src/nasa/main.gcl"), None, &fs());
        assert!(result.is_err());
    }

    // ---- Bare module path DAG fallback tests ----
    #[test]
    fn load_bare_module_dag_fallback_not_found() {
        // Neither `nasa/rocket/nonexistent.gcl` nor `nasa/rocket.gcl` exist.
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            (
                "src/nasa/main.gcl",
                "include nasa/rocket/nonexistent(x: 5.0) { result as y };",
            ),
        ]);
        let result = load_project(&dir.path().join("src/nasa/main.gcl"), None, &fs());
        assert!(result.is_err());
    }

    #[test]
    fn load_root_outside_package_namespace_rejects_cross_file_import() {
        // Manifest at the project root names a package `myproject`, whose
        // namespace is `<source_dir>/myproject/`. A loose `main.gcl` sitting at
        // the source-dir root is *not* in that namespace, so it's treated as a
        // virtual package and any cross-file import is rejected.
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"myproject\"\n"),
            ("src/myproject/helper.gcl", "param y: Dimensionless = 2.0;"),
            (
                "src/main.gcl",
                "import myproject.helper.{y};\nnode z: Dimensionless = @y;",
            ),
        ]);
        let result = load_project(&dir.path().join("src/main.gcl"), None, &fs());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("CrossFileImportInVirtualPackage"),
            "expected loose-entry rejection: {err}"
        );
    }

    struct CacheDirectoryOverride(Option<PathBuf>);

    impl CacheDirectoryOverride {
        fn set(path: PathBuf) -> Self {
            Self(TEST_CACHE_DIR.with(|slot| slot.replace(Some(path))))
        }
    }

    impl Drop for CacheDirectoryOverride {
        fn drop(&mut self) {
            TEST_CACHE_DIR.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }

    struct LockedPackageFixture {
        root_file: PathBuf,
        root_helper: PathBuf,
        dependency_root: PathBuf,
        _cache_directory: CacheDirectoryOverride,
        directory: tempfile::TempDir,
    }

    fn locked_package_fixture(root_source: &str, dependency_source: &str) -> LockedPackageFixture {
        use graphcal_package::{LOCK_VERSION, Lockfile, PackageInstanceId, SourceTreeHashes};

        let directory = tempfile::tempdir().unwrap();
        let project_root = directory.path().join("project");
        let cache_root = directory.path().join("cache");
        let root_source_dir = project_root.join("src/mission");
        std::fs::create_dir_all(&root_source_dir).unwrap();

        let root_manifest = r#"[package]
name = "mission"

[dependencies]
units_v1 = { package = "units", git = "https://example.com/units.git", rev = "1111111111111111111111111111111111111111" }
"#;
        let parsed_root_manifest = parse_manifest_str(root_manifest).unwrap();
        let (dependency_name, dependency_spec) = parsed_root_manifest
            .dependencies
            .first_key_value()
            .expect("fixture dependency");
        let revision = dependency_spec.git.rev.clone();
        let url = dependency_spec.git.url.clone();
        let dependency_root = cache_root.join("git").join(cache_key(&url, &revision));
        std::fs::create_dir_all(dependency_root.join("src/units")).unwrap();
        let dependency_manifest = "[package]\nname = \"units\"\n";
        std::fs::write(project_root.join("graphcal.toml"), root_manifest).unwrap();
        std::fs::write(dependency_root.join("graphcal.toml"), dependency_manifest).unwrap();
        std::fs::write(dependency_root.join("src/units/lib.gcl"), dependency_source).unwrap();

        let root_file = root_source_dir.join("main.gcl");
        let root_helper = root_source_dir.join("helper.gcl");
        std::fs::write(&root_file, root_source).unwrap();
        std::fs::write(
            &root_helper,
            "pub const node local_value: Dimensionless = 1.0;",
        )
        .unwrap();

        let dependency_manifest = parse_manifest_str(dependency_manifest).unwrap();
        let dependency_reader = RealFileSystem::rooted(&dependency_root).unwrap();
        let tree_hash = hash_source_tree(
            &dependency_reader,
            &dependency_root,
            &dependency_manifest.source_dir,
            SourceTreeHashLimits::unbounded(),
            &graphcal_io::NeverCancel,
        )
        .unwrap()
        .sha256()
        .to_string();
        let root_id = PackageInstanceId::new("pkg-mission").unwrap();
        let dependency_id = PackageInstanceId::new("pkg-units-v1").unwrap();
        let lockfile = Lockfile {
            lock_version: LOCK_VERSION,
            created_by: "graphcal-test".to_string(),
            graphcal_version: env!("CARGO_PKG_VERSION").to_string(),
            stdlib_version: STDLIB_VERSION.to_string(),
            root: root_id.clone(),
            plugins: Vec::new(),
            packages: vec![
                LockedPackage {
                    id: root_id,
                    name: parsed_root_manifest.name.clone(),
                    source_dir: PathBuf::from("src"),
                    source: PackageSource::Path {
                        path: ".".to_string(),
                    },
                    dependencies: BTreeMap::from([(
                        dependency_name.clone(),
                        dependency_id.clone(),
                    )]),
                },
                LockedPackage {
                    id: dependency_id,
                    name: dependency_manifest.name,
                    source_dir: PathBuf::from("src"),
                    source: PackageSource::Git {
                        url,
                        requested_rev: revision.clone(),
                        commit: revision,
                        tree_hashes: SourceTreeHashes { sha256: tree_hash },
                    },
                    dependencies: BTreeMap::new(),
                },
            ],
        };
        std::fs::write(
            project_root.join("graphcal.lock"),
            lockfile.to_deterministic_toml().unwrap(),
        )
        .unwrap();
        let cache_directory = CacheDirectoryOverride::set(cache_root);

        LockedPackageFixture {
            root_file,
            root_helper,
            dependency_root,
            _cache_directory: cache_directory,
            directory,
        }
    }

    #[test]
    fn dependency_enabled_loader_uses_overlay_for_root_file() {
        let fixture = locked_package_fixture(
            "node disk: Dimensionless = 1.0;",
            "pub const node one: Dimensionless = 1.0;",
        );
        let overlay_source = "node overlay: Dimensionless = 2.0;";
        let overlay = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            fixture.root_file.canonicalize().unwrap(),
            overlay_source.to_string(),
        );

        let project = load_project(&fixture.root_file, None, &overlay).unwrap();
        assert_eq!(project.files[&project.root].source.as_str(), overlay_source);
    }

    #[test]
    fn dependency_enabled_loader_uses_overlay_for_root_package_import() {
        let fixture = locked_package_fixture(
            "import mission.helper.{ local_value };\nnode result: Dimensionless = @local_value;",
            "pub const node one: Dimensionless = 1.0;",
        );
        let overlay_source = "pub const node local_value: Dimensionless = 99.0;";
        let overlay = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            fixture.root_helper.canonicalize().unwrap(),
            overlay_source.to_string(),
        );

        let project = load_project(&fixture.root_file, None, &overlay).unwrap();
        let helper = project
            .files
            .values()
            .find(|file| file.path == fixture.root_helper.canonicalize().unwrap())
            .expect("loaded helper");
        assert_eq!(helper.source.as_str(), overlay_source);
    }

    #[cfg(unix)]
    #[test]
    fn locked_source_verification_rejects_symlinks_before_reading_targets() {
        use std::os::unix::fs::symlink;

        let fixture = locked_package_fixture(
            "node root: Dimensionless = 1.0;",
            "pub const node one: Dimensionless = 1.0;",
        );
        let outside = fixture.directory.path().join("outside.gcl");
        std::fs::write(&outside, "outside").unwrap();
        symlink(
            &outside,
            fixture.dependency_root.join("src/units/escape.gcl"),
        )
        .unwrap();

        let error = load_project(&fixture.root_file, None, &RealFileSystem::default())
            .expect_err("locked source verification must reject every symlink");
        assert!(
            error.to_string().contains("symbolic links are not allowed"),
            "unexpected source-tree diagnostic: {error:?}"
        );
    }

    #[test]
    fn aggregate_loader_file_budget_covers_manifest_and_source_reads() {
        let directory = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"mission\"\n"),
            (
                "src/mission/main.gcl",
                "import mission.helper.{ value };\nnode result: Dimensionless = @value;",
            ),
            (
                "src/mission/helper.gcl",
                "pub const node value: Dimensionless = 1.0;",
            ),
        ]);
        let policy = LoaderBudget::new(LoaderArtifactByteLimits::default(), 2, 256 * MEBIBYTE);

        let error = load_project_with_budget(
            &directory.path().join("src/mission/main.gcl"),
            None,
            &RealFileSystem::default(),
            policy,
        )
        .expect_err("third loader artifact must exceed the aggregate file budget");
        assert!(
            error.to_string().contains("file-count limit of 2"),
            "unexpected loader budget diagnostic: {error:?}"
        );
    }

    #[test]
    fn aggregate_loader_byte_budget_rejects_before_the_next_allocation() {
        let directory = setup_temp_dir(&[("main.gcl", "node result: Dimensionless = 1.0;")]);
        let policy = LoaderBudget::new(LoaderArtifactByteLimits::default(), 10, 8);

        let error = load_project_with_budget(
            &directory.path().join("main.gcl"),
            None,
            &RealFileSystem::default(),
            policy,
        )
        .expect_err("source must exceed the aggregate byte budget");
        assert!(
            error.to_string().contains("aggregate byte limit of 8"),
            "unexpected loader budget diagnostic: {error:?}"
        );
    }

    #[test]
    fn package_inline_dag_import_resolves_dependency_module() {
        let fixture = locked_package_fixture(
            r"
dag calculation {
    import units_v1.lib.{ one };
    pub node out: Dimensionless = @one;
}
node result: Dimensionless = @calculation().out;
",
            "pub const node one: Dimensionless = 1.0;",
        );

        let result = crate::eval::compile_and_eval_project(
            &fixture.root_file,
            &HashMap::new(),
            None,
            &RealFileSystem::default(),
        );
        assert!(result.is_ok(), "dependency import failed: {result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn unrooted_plugin_reader_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let package_root = directory.path().join("project");
        let outside = directory.path().join("outside.wasm");
        std::fs::create_dir_all(package_root.join("plugins")).unwrap();
        std::fs::write(&outside, b"outside module bytes").unwrap();
        symlink(&outside, package_root.join("plugins/escaped.wasm")).unwrap();

        let mut budget = LoaderBudgetState::new(LoaderBudget::default());
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let result = read_plugin_file(
            &package_root,
            &graphcal_compiler::syntax::plugin::PluginPath::new("plugins/escaped.wasm"),
            &RealFileSystem::default(),
            &mut budget,
            &cancellation,
        );
        assert!(matches!(result, Err(PluginFileError::OutsideRoot)));
    }

    #[cfg(unix)]
    #[test]
    fn package_project_plugin_symlink_is_recorded_as_outside_root() {
        use std::os::unix::fs::symlink;

        let directory = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"mission\"\n"),
            (
                "src/mission/main.gcl",
                "import plugin \"plugins/escaped.wasm\" as plugin {\nfn value() -> Dimensionless;\n}",
            ),
        ]);
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::create_dir_all(directory.path().join("plugins")).unwrap();
        symlink(
            outside.path(),
            directory.path().join("plugins/escaped.wasm"),
        )
        .unwrap();

        let project = load_project(
            &directory.path().join("src/mission/main.gcl"),
            None,
            &RealFileSystem::default(),
        )
        .unwrap();
        assert!(matches!(
            project.plugins().values().next(),
            Some(Err(PluginFileError::OutsideRoot))
        ));
    }

    #[test]
    fn plugin_reader_rejects_oversized_module_before_loading() {
        const OVERSIZED_PLUGIN_BYTES: u64 = 16 * 1024 * 1024 + 1;

        let directory = tempfile::tempdir().unwrap();
        let plugin_path = directory.path().join("large.wasm");
        let file = std::fs::File::create(&plugin_path).unwrap();
        file.set_len(OVERSIZED_PLUGIN_BYTES).unwrap();

        let mut budget = LoaderBudgetState::new(LoaderBudget::default());
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let result = read_plugin_file(
            directory.path(),
            &graphcal_compiler::syntax::plugin::PluginPath::new("large.wasm"),
            &RealFileSystem::default(),
            &mut budget,
            &cancellation,
        );
        assert!(result.is_err(), "oversized plugin was read into memory");
    }

    struct LockReadFailureFileSystem(RealFileSystem);

    impl FileSystemReader for LockReadFailureFileSystem {
        fn read_bytes_bounded(
            &self,
            path: &Path,
            limit: ByteLimit,
            cancellation: &dyn CancellationSignal,
        ) -> Result<Vec<u8>, FileSystemReadError> {
            if path.file_name() == Some(std::ffi::OsStr::new("graphcal.lock")) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "lockfile denied by test filesystem",
                )
                .into());
            }
            self.0.read_bytes_bounded(path, limit, cancellation)
        }

        fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
            self.0.canonicalize(path)
        }

        fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error> {
            self.0.entry_kind(path)
        }

        fn read_directory_bounded(
            &self,
            path: &Path,
            limit: EntryLimit,
            cancellation: &dyn CancellationSignal,
        ) -> Result<Vec<std::ffi::OsString>, FileSystemReadError> {
            self.0.read_directory_bounded(path, limit, cancellation)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }

        fn exists(&self, path: &Path) -> bool {
            self.0.exists(path)
        }
    }

    #[test]
    fn unreadable_plugin_lockfile_is_not_treated_as_missing() {
        let directory = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"mission\"\n"),
            (
                "src/mission/main.gcl",
                "import plugin \"plugin.wasm\" as plugin {\nfn value() -> Dimensionless;\n}",
            ),
            ("plugin.wasm", "not wasm"),
        ]);
        let filesystem = LockReadFailureFileSystem(RealFileSystem::default());
        let result = load_project(
            &directory.path().join("src/mission/main.gcl"),
            None,
            &filesystem,
        );

        let error = result.expect_err("permission failure must be a hard loader error");
        assert!(
            error
                .to_string()
                .contains("lockfile denied by test filesystem"),
            "unexpected lockfile diagnostic: {error:?}"
        );
    }
}
