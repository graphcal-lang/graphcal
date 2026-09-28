use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;
use sha2::{Digest, Sha256};

use crate::dependency_ordered::DependencyOrdered;
use crate::eval::CompileError;
use graphcal_compiler::dag_id::{DagId, DagPackageId};
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::plugin_identity::{ExternFnKey, PluginIdentity};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::{DeclKind, IncludeDecl, ModulePath};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::module_name::IncludeInstanceScope;
use graphcal_compiler::syntax::phase::Phase;
use graphcal_compiler::syntax::plugin::PluginPath;
#[cfg(test)]
use graphcal_io::hash_source_tree;
use graphcal_io::{
    ByteLimit, FileSystemEntryKind, FileSystemReadError, FileSystemReader, ProjectIngestionPolicy,
    RealFileSystem, SourceTreeHashLimits,
};
mod build;
mod inline_dags;
mod source_snapshot;

use build::{build_loaded_files, reject_file_root_stem_imports};
use inline_dags::lift_inline_dags;
use source_snapshot::{
    FetchedFile, ModuleLocation, ModuleResolution, PackageFileKey, ParsedFile, ParsedSource,
    ResolveFailure, ResolvedFile, SourceKey, SourceSnapshot, file_stem,
};

use graphcal_package::{
    GitSourceId, LockedPackage, LockfileParseLimits, PackageInstanceId, PackageManifest,
    PackageSource, STDLIB_VERSION, ValidatedPackageGraph, parse_lockfile_str_with_limits,
    parse_manifest_str,
};

#[cfg(test)]
thread_local! {
    static TEST_CACHE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
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

    /// Maximum bytes accepted for one Graphcal source document.
    #[must_use]
    pub const fn source_file_bytes(self) -> u64 {
        self.source_file
    }
}

impl Default for LoaderArtifactByteLimits {
    fn default() -> Self {
        let policy = ProjectIngestionPolicy::default();
        Self::new(
            policy.source_file().get(),
            policy.manifest().get(),
            policy.lockfile().get(),
            policy.plugin().get(),
            policy.source_tree_file().get(),
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
        let policy = ProjectIngestionPolicy::default();
        Self::new(
            LoaderArtifactByteLimits::default(),
            policy.max_entries(),
            policy.max_total_bytes(),
        )
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

    fn lockfile_parse_limits(&self) -> LockfileParseLimits {
        LockfileParseLimits::new(usize::try_from(self.policy.max_files).unwrap_or(usize::MAX))
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

/// Fold a parse outcome into the loader's error type.
///
/// Transitional boundary: until the loader returns `Outcome<_>` itself,
/// cancellation still travels inside `CompileError` (as `GraphcalError::Cancelled`).
fn parse_outcome_error(
    outcome: Outcome<graphcal_compiler::syntax::parser::ParseError>,
) -> CompileError {
    match outcome {
        Outcome::Cancelled => graphcal_compiler::cancellation::Cancelled.into(),
        Outcome::Failed(error) => error.into(),
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
/// own imports and `import <self>::{...}` references back to the parent file.
#[derive(Debug, Clone)]
pub struct LoadedDag {
    /// Abstract DAG identity for this inline dag, formed by appending the
    /// dag's name to its parent file's `DagId`.
    dag_id: DagId,
    /// The enclosing file's `DagId`. Imports whose path resolves to this id
    /// are dag-body self-imports (`import <self>::{...}`).
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

/// Fuel policies resolved from each owning package manifest into compiler-owned
/// plugin and function identities.
#[derive(Debug, Clone, Default)]
pub struct PluginCallPolicy {
    default_fuel_per_call: HashMap<DagPackageId, u64>,
    function_fuel_per_call: HashMap<ExternFnKey, u64>,
}

impl PluginCallPolicy {
    fn from_manifest(
        policy: &graphcal_package::PluginExecutionPolicy,
        package: &DagPackageId,
    ) -> Self {
        let function_fuel_per_call = policy
            .function_limits()
            .map(|(selector, budget)| {
                (
                    ExternFnKey {
                        plugin: PluginIdentity::resolve(
                            &PluginPath::new(selector.plugin().to_string()),
                            package,
                        ),
                        name: FnName::expect_valid(selector.function().as_str()),
                    },
                    budget.get(),
                )
            })
            .collect();
        Self {
            default_fuel_per_call: policy
                .default_fuel_per_call()
                .map(|fuel| (package.clone(), fuel.get()))
                .into_iter()
                .collect(),
            function_fuel_per_call,
        }
    }

    /// Resolve one function's configured budget, with a function override
    /// taking precedence over the project-wide default.
    #[must_use]
    pub fn fuel_per_call(&self, plugin: &PluginIdentity, function: &FnName) -> Option<u64> {
        self.function_fuel_per_call
            .get(&ExternFnKey {
                plugin: plugin.clone(),
                name: function.clone(),
            })
            .copied()
            .or_else(|| {
                plugin
                    .package()
                    .and_then(|package| self.default_fuel_per_call.get(package).copied())
            })
    }
}

/// A loaded project: a root file plus all transitively imported files.
#[derive(Debug)]
pub struct LoadedProject {
    /// All loaded files in topological load order, ending with the root file.
    files: LoadedFiles,
    /// WASM plugin files referenced by `import plugin "….wasm"` declarations
    /// keyed by the declaring package instance and artifact path.
    ///
    /// Paths resolve within the owning package root. Dependency bytes come
    /// exclusively from the authenticated snapshot. Read failures are reported
    /// at import spans; global host-registry identities never appear here.
    plugins: HashMap<PluginIdentity, PluginFileEntry>,
    /// Package-scoped plugin fuel settings resolved to typed function identities.
    plugin_call_policy: PluginCallPolicy,
    package_closure: Option<LoadedPackageClosure>,
}

/// Loaded source files in topological load order (dependencies before
/// dependents, ending with the root file), indexed by semantic identity.
#[derive(Debug)]
pub struct LoadedFiles {
    ordered: DependencyOrdered<LoadedFile>,
    /// Position (in `ordered`) of the owner source file for every file-root
    /// and inline DAG identity. Derived from `ordered` at construction.
    owners: HashMap<DagId, usize>,
}

impl LoadedFiles {
    fn new(ordered: DependencyOrdered<LoadedFile>) -> Self {
        let owners = ordered
            .iter()
            .enumerate()
            .flat_map(|(position, file)| {
                std::iter::once((file.dag_id.clone(), position)).chain(
                    file.inline_dags
                        .iter()
                        .map(move |dag| (dag.dag_id.clone(), position)),
                )
            })
            .collect();
        Self { ordered, owners }
    }

    /// Files in dependency order, ending with the root file.
    #[must_use]
    pub const fn ordered(&self) -> &DependencyOrdered<LoadedFile> {
        &self.ordered
    }

    /// Iterate dependencies first, ending with the root file.
    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &LoadedFile> + Clone {
        self.ordered.iter()
    }

    /// Number of loaded source files. Always at least 1.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.ordered.len()
    }

    /// Always `false`: the root file is always present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Look up one loaded source file by its file-root identity.
    #[must_use]
    pub fn get(&self, dag_id: &DagId) -> Option<&LoadedFile> {
        self.owner(dag_id).filter(|file| file.dag_id == *dag_id)
    }

    /// Source file that owns a file-root or inline DAG identity.
    fn owner(&self, dag_id: &DagId) -> Option<&LoadedFile> {
        self.owners
            .get(dag_id)
            .and_then(|position| self.ordered.get(*position))
    }
}

impl std::ops::Index<&DagId> for LoadedFiles {
    type Output = LoadedFile;

    /// Keyed lookup with the same contract as indexing the former
    /// `HashMap<DagId, LoadedFile>`: callers index only identities produced by
    /// this project's loader.
    #[expect(
        clippy::panic,
        reason = "preserves the former map-indexing contract of loader-produced identities"
    )]
    fn index(&self, dag_id: &DagId) -> &LoadedFile {
        self.get(dag_id)
            .unwrap_or_else(|| panic!("`{dag_id}` is not a loaded source file"))
    }
}

impl<'a> IntoIterator for &'a LoadedFiles {
    type Item = &'a LoadedFile;
    type IntoIter = <&'a DependencyOrdered<LoadedFile> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&self.ordered).into_iter()
    }
}

/// Lockfile and exact verified dependency bytes used by this loaded project.
#[derive(Debug)]
pub struct LoadedPackageClosure {
    pub lockfile: String,
    pub dependencies: BTreeMap<PackageInstanceId, LoadedDependency>,
}

/// An authenticated dependency snapshot and its filesystem provenance.
#[derive(Debug)]
pub struct LoadedDependency {
    pub root: PathBuf,
    pub snapshot: graphcal_io::SourceTreeSnapshot,
}

/// Outcome of locating and reading one wasm plugin file.
pub type PluginFileEntry = Result<LoadedPlugin, PluginFileError>;

/// The wasm plugin paths declared by one file's `import plugin` blocks,
/// including blocks inside nested `dag` bodies.
fn wasm_plugin_paths(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
) -> impl Iterator<Item = &graphcal_compiler::syntax::plugin::PluginPath> {
    use graphcal_compiler::syntax::plugin::PluginSourceKind;

    ast.plugin_imports()
        .into_iter()
        .map(|plugin| &plugin.path.value)
        .filter(|path| path.source_kind() == PluginSourceKind::WasmModule)
}

fn validate_plugin_call_policy(
    files: &DependencyOrdered<LoadedFile>,
    policy: &PluginCallPolicy,
) -> Result<(), CompileError> {
    let declared_functions = files
        .iter()
        .flat_map(|file| {
            file.ast
                .plugin_imports()
                .into_iter()
                .flat_map(move |plugin| {
                    plugin.functions.iter().map(move |function| ExternFnKey {
                        plugin: PluginIdentity::resolve(&plugin.path.value, file.dag_id.package()),
                        name: function.name.value.clone(),
                    })
                })
        })
        .collect::<HashSet<_>>();

    policy
        .function_fuel_per_call
        .keys()
        .find(|configured| {
            declared_functions
                .iter()
                .any(|declared| declared.plugin == configured.plugin)
                && !declared_functions.contains(*configured)
        })
        .map_or(Ok(()), |configured| {
            Err(loader_manifest_error(format!(
                "plugin fuel limit selects undeclared function `{}.{}`",
                configured.plugin, configured.name
            )))
        })
}

/// Resolve and read every wasm plugin declared by the given file ASTs.
///
/// Read failures are recorded per plugin rather than failing the load, so
/// compile-only consumers (hover, symbols) keep working; evaluation surfaces
/// the stored error at the declaring import.
fn read_wasm_plugins<'a>(
    file_asts: impl Iterator<
        Item = (
            &'a DagPackageId,
            &'a graphcal_compiler::desugar::desugared_ast::File,
        ),
    >,
    package_root: &Path,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<PluginIdentity, PluginFileEntry>, CompileError> {
    let mut plugins = HashMap::new();
    for (package, ast) in file_asts {
        for path in wasm_plugin_paths(ast) {
            cancellation.checkpoint()?;
            let identity = PluginIdentity::resolve(path, package);
            match plugins.entry(identity) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let entry = read_plugin_file(package_root, path, fs, budget, cancellation);
                    cancellation.checkpoint()?;
                    slot.insert(entry);
                }
                std::collections::hash_map::Entry::Occupied(_) => {}
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
    plugins: &mut HashMap<PluginIdentity, PluginFileEntry>,
    pins: &BTreeMap<String, String>,
) {
    for (path, entry) in plugins.iter_mut() {
        let Ok(loaded) = entry.as_ref() else {
            continue;
        };
        match pins.get(path.path().as_str()) {
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
        files: DependencyOrdered<LoadedFile>,
        plugins: HashMap<PluginIdentity, PluginFileEntry>,
        plugin_call_policy: PluginCallPolicy,
    ) -> Self {
        Self {
            files: LoadedFiles::new(files),
            plugins,
            plugin_call_policy,
            package_closure: None,
        }
    }

    /// Exact verified dependency closure, when the project has dependencies.
    #[must_use]
    pub const fn package_closure(&self) -> Option<&LoadedPackageClosure> {
        self.package_closure.as_ref()
    }

    /// All loaded files in dependency order, ending with the root file.
    #[must_use]
    pub const fn files(&self) -> &LoadedFiles {
        &self.files
    }

    /// Look up one loaded source file by semantic identity.
    #[must_use]
    pub fn file(&self, dag_id: &DagId) -> Option<&LoadedFile> {
        self.files.get(dag_id)
    }

    pub(crate) fn inline_dag(&self, dag_id: &DagId) -> Option<(&LoadedFile, &LoadedDag)> {
        let file = self.files.owner(dag_id)?;
        file.inline_dags
            .iter()
            .find(|inline| inline.dag_id == *dag_id)
            .map(|inline| (file, inline))
    }

    /// Root source-file identity.
    #[must_use]
    pub const fn root_id(&self) -> &DagId {
        &self.files.ordered.root().dag_id
    }

    /// Root source file.
    #[must_use]
    pub const fn root_file(&self) -> &LoadedFile {
        self.files.ordered.root()
    }

    /// Validated plugin artifacts and deferred plugin-loading errors.
    #[must_use]
    pub const fn plugins(&self) -> &HashMap<PluginIdentity, PluginFileEntry> {
        &self.plugins
    }

    /// Root-package fuel policy for vendored plugin calls.
    #[must_use]
    pub const fn plugin_call_policy(&self) -> &PluginCallPolicy {
        &self.plugin_call_policy
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
            .map_err(parse_outcome_error)?;
        cancellation.checkpoint()?;
        let ast = graphcal_compiler::desugar::desugared_ast::File::from(raw_ast);
        cancellation.checkpoint()?;
        let path = PathBuf::from(name);
        let stem = file_stem(&path);
        reject_file_root_stem_imports(&ast.declarations, stem, &named_source)?;
        // `name` is also a diagnostic label and may be an absolute virtual URI
        // path. A standalone in-memory file has no filesystem hierarchy, so a
        // rooted label contributes only its leaf to semantic DAG identity.
        // Relative labels remain strict, canonical module paths.
        //
        // This tests for a root/prefix component rather than calling
        // `Path::is_absolute`. Virtual URI labels are POSIX-style
        // (`/playground/main.gcl`) regardless of host OS, but `is_absolute` is
        // host-dependent: Windows requires a drive prefix, so it reports those
        // labels as relative and they then fail the strict relative-path
        // validation below.
        let is_rooted = path.components().next().is_some_and(|component| {
            matches!(component, Component::RootDir | Component::Prefix(_))
        });
        let semantic_path = match (is_rooted, path.file_name()) {
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
        let inline_dags = lift_inline_dags(&ast, &dag_id, stem, |_| None);
        cancellation.checkpoint()?;
        // No filesystem to read wasm plugin files from; the entries carry
        // the reason so evaluation can report it at the import site.
        let plugins = wasm_plugin_paths(&ast)
            .map(|plugin| {
                (
                    PluginIdentity::resolve(plugin, dag_id.package()),
                    Err(PluginFileError::NoProjectFilesystem),
                )
            })
            .collect();
        cancellation.checkpoint()?;
        let loaded_file = LoadedFile {
            path,
            dag_id,
            source,
            ast,
            named_source,
            resolved_imports: HashMap::new(),
            inline_dags,
        };
        Ok(Self::from_parts(
            DependencyOrdered::new(Vec::new(), loaded_file),
            plugins,
            PluginCallPolicy::default(),
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
        self.files
            .owner(resolved)
            .map(|source_file| {
                ResolvedModuleTarget::in_file(source_file.dag_id.clone(), resolved.clone())
            })
            .ok_or_else(|| ResolvedModuleTargetError::UnknownOwner {
                target: resolved.clone(),
            })
    }

    /// Build module-aware symbol tables for every loaded file and inline DAG.
    ///
    /// The loader resolves filesystem and package import paths to canonical
    /// [`DagId`]s. This method hands those edges to the compiler's pure module
    /// resolver; same-file and synthetic include scopes may then be resolved
    /// against the module scopes already constructed here.
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

        for loaded in &self.files {
            resolver.add_module(loaded.dag_id.clone(), &loaded.ast.declarations)?;
            for inline in &loaded.inline_dags {
                resolver.add_module(inline.dag_id.clone(), inline.body(loaded))?;
            }
        }

        for loaded in &self.files {
            add_include_instance_modules(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                self,
            )?;
            for inline in &loaded.inline_dags {
                add_include_instance_modules(
                    &mut resolver,
                    &inline.dag_id,
                    inline.body(loaded),
                    &inline.resolved_imports,
                    self,
                )?;
            }
        }

        // Load order is dependency-first. Before registering one owner's
        // include edges, give each synthetic target the already-completed scope
        // of its canonical dependency; public re-exports are then selectable at
        // the next composition level.
        for loaded in &self.files {
            inherit_include_instance_scopes(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                self,
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
                    self,
                )?;
                register_module_imports(
                    &mut resolver,
                    &inline.dag_id,
                    inline.body(loaded),
                    &inline.resolved_imports,
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
    let module_ids = project.files.iter().flat_map(|file| {
        std::iter::once(file.dag_id.clone())
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
                        module_include_targets(&module, project)
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

fn module_include_targets(source: &DagId, project: &LoadedProject) -> Vec<DagId> {
    module_declarations(source, project).map_or_else(Vec::new, |declarations| {
        declarations
            .iter()
            .filter_map(|declaration| {
                let DeclKind::Include(include) = &declaration.kind else {
                    return None;
                };
                resolved_module_target_from(source, &include.path, project)
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
    project: &LoadedProject,
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
        let Some(target_decls) = module_declarations(target.target(), project) else {
            continue;
        };
        let instance = owner.instance_child(prefix.as_str());
        resolver.add_module(instance.clone(), target_decls)?;
        add_nested_include_instance_modules(resolver, target.target(), &instance, project)?;
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
    project: &LoadedProject,
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
        inherit_nested_include_instance_scopes(resolver, source.target(), &instance, project)?;
    }
    Ok(())
}

struct NestedIncludeInstance {
    source: DagId,
    instance: DagId,
}

fn nested_include_instances(
    source: &DagId,
    instance: &DagId,
    project: &LoadedProject,
) -> Vec<NestedIncludeInstance> {
    module_declarations(source, project).map_or_else(Vec::new, |declarations| {
        declarations
            .iter()
            .filter_map(|declaration| {
                let DeclKind::Include(include) = &declaration.kind else {
                    return None;
                };
                let instance_scope = include_instance_scope(include);
                let source = resolved_module_target_from(source, &include.path, project)?;
                Some(NestedIncludeInstance {
                    source,
                    instance: instance.instance_child(instance_scope.merge_scope_name().as_str()),
                })
            })
            .collect()
    })
}

fn add_nested_include_instance_modules(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    source: &DagId,
    instance: &DagId,
    project: &LoadedProject,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    let mut pending = nested_include_instances(source, instance, project)
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        let Some(child_declarations) = module_declarations(&child.source, project) else {
            continue;
        };
        resolver.add_module(child.instance.clone(), child_declarations)?;
        pending.extend(
            nested_include_instances(&child.source, &child.instance, project)
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
    project: &LoadedProject,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    let mut pending = nested_include_instances(source, instance, project)
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        resolver.inherit_module_scope(&child.source, &child.instance)?;
        pending.extend(
            nested_include_instances(&child.source, &child.instance, project)
                .into_iter()
                .rev(),
        );
    }
    Ok(())
}

fn resolved_module_target_from(
    source: &DagId,
    path: &ModulePath,
    project: &LoadedProject,
) -> Option<DagId> {
    let key = ModulePathKey::from_path(path);
    let resolved = project.file(source).map_or_else(
        || {
            project.inline_dag(source).and_then(|(_, inline)| {
                match inline.resolved_imports.get(&key) {
                    Some(InlineBodyImportResolution::Resolved(target)) => Some(target.clone()),
                    Some(InlineBodyImportResolution::Unresolved) | None => None,
                }
            })
        },
        |file| file.resolved_imports.get(&key).cloned(),
    )?;
    Some(resolved.target().clone())
}

fn include_instance_scope<P: Phase>(include: &IncludeDecl<P>) -> IncludeInstanceScope {
    include.instance_scope()
}

fn module_declarations<'a>(
    target: &DagId,
    project: &'a LoadedProject,
) -> Option<&'a [Declaration]> {
    if let Some(file) = project.file(target) {
        return Some(file.ast.declarations.as_slice());
    }
    project
        .inline_dag(target)
        .map(|(file, inline)| inline.body(file))
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
                    resolver.register_import_decl(owner, import, target.target())?;
                }
            }
            DeclKind::Include(include) => {
                let resolved_edge =
                    resolved_imports.resolved_target(&ModulePathKey::from_path(&include.path));
                let source_target = resolved_edge
                    .map(|target| target.target().clone())
                    .or_else(|| resolver.resolve_module_path(owner, &include.path).ok());
                if resolved_edge.is_some() {
                    let instance_scope = include_instance_scope(include);
                    let prefix = instance_scope.merge_scope_name();
                    let target = owner.instance_child(prefix.as_str());
                    resolver.register_include(owner, &include.path, &include.kind, &target)?;
                }
                resolver.apply_include_static_projection_bindings(
                    owner,
                    source_target.as_ref(),
                    include,
                )?;
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
    load_project_with_dependency_sources(
        root_path,
        project_root_override,
        fs,
        crate::package_sources::DependencySources::NativeCache,
        budget,
        cancellation,
    )
}

/// Load with explicit dependency authority. Embedded authority cannot access the
/// native package cache and must supply exactly the validated locked closure.
///
/// # Errors
/// Reports missing, extra, or unauthenticated packages, malformed sources,
/// budget exhaustion, and cancellation through the normal compiler diagnostics.
pub fn load_project_with_dependency_sources<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
    sources: crate::package_sources::DependencySources<'_>,
    budget: LoaderBudget,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    let mut budget = LoaderBudgetState::new(budget);
    load_project_with_budget_state(
        root_path,
        project_root_override,
        fs,
        sources,
        &mut budget,
        cancellation,
    )
}

fn load_project_with_budget_state<F: FileSystemReader>(
    root_path: &Path,
    project_root_override: Option<&Path>,
    fs: &F,
    sources: crate::package_sources::DependencySources<'_>,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    cancellation.checkpoint()?;
    let root_canonical = fs
        .canonicalize(root_path)
        .map_err(|_| io_not_found(root_path))?;

    let root_dir = root_canonical.parent().unwrap_or(&root_canonical);
    let project_root = resolve_project_root(root_dir, project_root_override, fs)?;

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
            sources,
            budget,
            cancellation,
        );
    }
    if matches!(sources, crate::package_sources::DependencySources::Embedded(packages) if !packages.is_empty())
    {
        return Err(loader_manifest_error(
            "embedded packages were supplied but the root has no dependencies",
        ));
    }
    let package_id = match manifest.as_ref() {
        Some(package_manifest) => DagPackageId::new(package_manifest.name.as_str()),
        None => virtual_package_id_for_path(&root_canonical)?,
    };

    let authority = ProjectSources {
        project_root: &project_root,
        package_id: &package_id,
        manifest: manifest.as_ref(),
        fs,
    };
    let snapshot = fetch_source_snapshot(&authority, root_canonical, budget, cancellation)?;
    cancellation.checkpoint()?;
    let files = build_loaded_files(snapshot)?;
    cancellation.checkpoint()?;
    // Single-package project: every loaded file belongs to the root package,
    // so every declared wasm plugin resolves against the project root.
    let mut plugins = read_wasm_plugins(
        files.iter().map(|file| (file.dag_id.package(), &file.ast)),
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
    let plugin_call_policy = manifest
        .as_ref()
        .map_or_else(PluginCallPolicy::default, |manifest| {
            PluginCallPolicy::from_manifest(&manifest.plugin_execution_policy, &package_id)
        });
    validate_plugin_call_policy(&files, &plugin_call_policy)?;
    cancellation.checkpoint()?;
    Ok(LoadedProject::from_parts(
        files,
        plugins,
        plugin_call_policy,
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
    let lockfile = parse_lockfile_str_with_limits(&lockfile_text, budget.lockfile_parse_limits())
        .map_err(|error| {
        CompileError::Eval(GraphcalError::ManifestError {
            message: error.to_string(),
        })
    })?;
    let validated = lockfile
        .validated(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)
        .map_err(|error| {
            CompileError::Eval(GraphcalError::ManifestError {
                message: error.to_string(),
            })
        })?;
    Ok(validated
        .plugins()
        .map(|plugin| (plugin.path().to_string(), plugin.sha256().to_string()))
        .collect())
}

fn load_locked_package_project<F: FileSystemReader>(
    root_canonical: &Path,
    project_root: &Path,
    root_manifest: PackageManifest,
    fs: &F,
    sources: crate::package_sources::DependencySources<'_>,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<LoadedProject, CompileError> {
    cancellation.checkpoint()?;
    let root_policy = root_manifest.plugin_execution_policy.clone();
    let context = PackageLoadContext::from_lockfile(
        project_root,
        root_manifest,
        fs,
        sources,
        budget,
        cancellation,
    )?;
    let root_package = context.graph.root().clone();
    let mut plugin_call_policy =
        PluginCallPolicy::from_manifest(&root_policy, &DagPackageId::new(root_package.as_str()));
    for (package, policy) in &context.dependency_plugin_policies {
        let policy = PluginCallPolicy::from_manifest(policy, &DagPackageId::new(package.as_str()));
        plugin_call_policy
            .default_fuel_per_call
            .extend(policy.default_fuel_per_call);
        plugin_call_policy
            .function_fuel_per_call
            .extend(policy.function_fuel_per_call);
    }

    let root_file = PackageFileKey {
        package: root_package.clone(),
        path: root_canonical.to_path_buf(),
    };
    let snapshot = fetch_source_snapshot(&context, root_file, budget, cancellation)?;
    cancellation.checkpoint()?;
    let files = build_loaded_files(snapshot)?;
    cancellation.checkpoint()?;
    // Each artifact resolves within its declaring package's authority. Root
    // plugins use explicit pins; dependency binaries need verified coverage.
    let mut plugins = HashMap::new();
    for package in context.roots.keys() {
        let owner = DagPackageId::new(package.as_str());
        let (package_root, reader) = context
            .authority_for(package)
            .map_err(loader_manifest_error)?;
        let mut package_plugins = read_wasm_plugins(
            files
                .iter()
                .filter(|file| file.dag_id.package() == &owner)
                .map(|file| (file.dag_id.package(), &file.ast)),
            package_root,
            reader,
            budget,
            cancellation,
        )?;
        if package == &root_package {
            apply_plugin_pins(&mut package_plugins, &context.plugin_pins);
        }
        plugins.extend(package_plugins);
    }
    validate_plugin_call_policy(&files, &plugin_call_policy)?;
    cancellation.checkpoint()?;
    let mut project = LoadedProject::from_parts(files, plugins, plugin_call_policy);
    project.package_closure = Some(context.closure);
    Ok(project)
}

/// Package-aware filesystem authority: root-package reads preserve the
/// caller-supplied capability (and therefore LSP overlays), while every locked
/// dependency receives its own immutable rooted capability.
struct PackageLoadContext<'a> {
    graph: ValidatedPackageGraph,
    root_package: PackageInstanceId,
    root_reader: &'a dyn FileSystemReader,
    roots: BTreeMap<PackageInstanceId, PathBuf>,
    dependency_readers: BTreeMap<PackageInstanceId, graphcal_io::InMemoryFileSystem>,
    dependency_plugin_policies:
        BTreeMap<PackageInstanceId, graphcal_package::PluginExecutionPolicy>,
    closure: LoadedPackageClosure,
    /// Root-package plugin pins from `graphcal.lock`: path → SHA-256.
    plugin_pins: BTreeMap<String, String>,
}

impl<'a> PackageLoadContext<'a> {
    fn from_lockfile(
        project_root: &Path,
        root_manifest: PackageManifest,
        fs: &'a dyn FileSystemReader,
        sources: crate::package_sources::DependencySources<'_>,
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
        let lockfile =
            parse_lockfile_str_with_limits(&lockfile_text, budget.lockfile_parse_limits())
                .map_err(|error| loader_manifest_error(error.to_string()))?;
        let validated = lockfile
            .validated(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)
            .map_err(|error| loader_manifest_error(error.to_string()))?;
        let canonical_project_root = fs
            .canonicalize(project_root)
            .map_err(loader_manifest_error)?;
        let mut roots = BTreeMap::from([(validated.root().clone(), canonical_project_root)]);
        if let crate::package_sources::DependencySources::Embedded(packages) = sources {
            let expected: std::collections::BTreeSet<_> = validated
                .packages()
                .filter(|package| &package.id != validated.root())
                .map(|package| &package.id)
                .collect();
            if expected != packages.keys().collect() {
                return Err(loader_manifest_error(
                    "embedded package identities must exactly match the locked dependency closure",
                ));
            }
        }

        let mut manifests = BTreeMap::new();
        let mut dependency_readers = BTreeMap::new();
        let mut dependencies = BTreeMap::new();
        for package in validated.packages() {
            cancellation.checkpoint()?;
            if &package.id == validated.root() {
                continue;
            }
            let verified = capture_dependency_from_authority(
                project_root,
                package,
                sources,
                budget,
                cancellation,
            )?;
            roots.insert(package.id.clone(), verified.dependency.root.clone());
            dependencies.insert(package.id.clone(), verified.dependency);
            dependency_readers.insert(package.id.clone(), verified.filesystem);
            manifests.insert(package.id.clone(), verified.manifest);
        }
        let dependency_plugin_policies = manifests
            .iter()
            .map(|(id, manifest)| (id.clone(), manifest.plugin_execution_policy.clone()))
            .collect();
        manifests.insert(validated.root().clone(), root_manifest);
        let graph = validated
            .bind_manifests(&manifests)
            .map_err(|error| loader_manifest_error(error.to_string()))?;
        let plugin_pins = validated
            .plugins()
            .map(|plugin| (plugin.path().to_string(), plugin.sha256().to_string()))
            .collect();
        Ok(Self {
            graph,
            root_package: validated.root().clone(),
            root_reader: fs,
            roots,
            dependency_readers,
            dependency_plugin_policies,
            closure: LoadedPackageClosure {
                lockfile: lockfile_text,
                dependencies,
            },
            plugin_pins,
        })
    }

    /// Source root and filesystem capability of one locked package instance.
    fn authority_for(
        &self,
        package: &PackageInstanceId,
    ) -> Result<(&Path, &dyn FileSystemReader), PackageAuthorityError> {
        let root = self
            .roots
            .get(package)
            .ok_or_else(|| PackageAuthorityError::NoSourceRoot(package.clone()))?;
        let reader = if package == &self.root_package {
            self.root_reader
        } else {
            self.dependency_readers
                .get(package)
                .map(|reader| reader as &dyn FileSystemReader)
                .ok_or_else(|| PackageAuthorityError::NoFilesystem(package.clone()))?
        };
        Ok((root, reader))
    }
}

/// A locked package instance without a captured source authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum PackageAuthorityError {
    #[error("lockfile package `{0}` has no source root")]
    NoSourceRoot(PackageInstanceId),
    #[error("lockfile package `{0}` has no filesystem capability")]
    NoFilesystem(PackageInstanceId),
}

/// Source authority through which the IO shell fetches one project's files.
trait SnapshotSource {
    type Key: SourceKey;

    /// Read, parse, and resolve the dependency paths of one file. Read and
    /// parse failures are recorded in the fetched file; `Err` aborts the load
    /// (cooperative cancellation).
    fn fetch(
        &self,
        file: &Self::Key,
        budget: &mut LoaderBudgetState,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<FetchedFile<Self::Key>, CompileError>;
}

/// Fetch every source file reachable from `root` in load order (depth-first
/// preorder, the order in which the builder visits files), stopping after the
/// first file that cannot be read or parsed.
fn fetch_source_snapshot<S: SnapshotSource>(
    authority: &S,
    root: S::Key,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<SourceSnapshot<S::Key>, CompileError> {
    let mut files = HashMap::new();
    let mut pending = vec![root.clone()];
    while let Some(file) = pending.pop() {
        if files.contains_key(&file) {
            continue;
        }
        let fetched = authority.fetch(&file, budget, cancellation)?;
        let Ok(parsed) = &fetched else {
            files.insert(file, fetched);
            break;
        };
        pending.extend(parsed.dependency_files(&file).into_iter().rev().cloned());
        files.insert(file, fetched);
    }
    Ok(SourceSnapshot { root, files })
}

/// Read one source file through the bounded capability, then parse and
/// desugar it under the diagnostic `name`.
fn read_source_file(
    fs: &dyn FileSystemReader,
    path: &Path,
    name: &str,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ParsedFile, CompileError> {
    let source_str = budget
        .read_text(fs, path, LoaderArtifact::SourceFile, cancellation)
        .map_err(|error| match error {
            LoaderReadError::Filesystem(filesystem)
                if filesystem.io_kind() == Some(std::io::ErrorKind::NotFound) =>
            {
                io_not_found(path)
            }
            other => loader_manifest_error(format!(
                "could not read source `{}`: {other}",
                path.display()
            )),
        })?;
    let source = Arc::new(source_str);
    let named_source = NamedSource::new(name, Arc::clone(&source));
    let raw_ast = graphcal_compiler::syntax::parser::Parser::with_name(&source, name)
        .parse_file_with_cancellation(cancellation)
        .map_err(parse_outcome_error)?;
    let ast = graphcal_compiler::desugar::desugared_ast::File::from(raw_ast);
    Ok(ParsedFile {
        source,
        named_source,
        ast,
    })
}

/// Filesystem authority of a single-package project: a real package whose
/// manifest has no dependencies, or a virtual (manifest-less) package.
struct ProjectSources<'a, F> {
    /// Import boundary: every import must resolve inside this directory tree.
    project_root: &'a Path,
    package_id: &'a DagPackageId,
    manifest: Option<&'a PackageManifest>,
    fs: &'a F,
}

impl<F: FileSystemReader> SnapshotSource for ProjectSources<'_, F> {
    type Key = PathBuf;

    fn fetch(
        &self,
        file: &PathBuf,
        budget: &mut LoaderBudgetState,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<FetchedFile<PathBuf>, CompileError> {
        cancellation.checkpoint()?;
        // Use the canonical path as the NamedSource name (not just the
        // basename). Downstream diagnostic emitters can recover the file URL
        // via `Url::from_file_path(Path::new(name))` without an external
        // resolver, and basename ambiguity (two `lib.gcl`s in different
        // packages) cannot arise. The CLI's miette renderer trims this for
        // display anyway.
        let name = file.display().to_string();
        let parsed = match read_source_file(self.fs, file, &name, budget, cancellation) {
            Ok(parsed) => parsed,
            Err(error) => return Ok(Err(error)),
        };
        cancellation.checkpoint()?;
        let location = ModuleLocation {
            package: self.package_id.clone(),
            relative_path: file
                .strip_prefix(self.project_root)
                .unwrap_or(file)
                .to_path_buf(),
        };
        ParsedSource::resolve(location, parsed, |path| {
            cancellation.checkpoint()?;
            Ok(resolve_project_module(
                path,
                self.project_root,
                self.manifest,
                self.fs,
            ))
        })
        .map(Ok)
    }
}

impl SnapshotSource for PackageLoadContext<'_> {
    type Key = PackageFileKey;

    fn fetch(
        &self,
        file: &PackageFileKey,
        budget: &mut LoaderBudgetState,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<FetchedFile<PackageFileKey>, CompileError> {
        cancellation.checkpoint()?;
        let (package_root, reader) = match self.authority_for(&file.package) {
            Ok(authority) => authority,
            Err(error) => return Ok(Err(loader_manifest_error(error))),
        };
        let name = format!("{}:{}", file.package, file.path.display());
        let parsed = match read_source_file(reader, &file.path, &name, budget, cancellation) {
            Ok(parsed) => parsed,
            Err(error) => return Ok(Err(error)),
        };
        cancellation.checkpoint()?;
        let location = ModuleLocation {
            package: DagPackageId::new(file.package.as_str()),
            relative_path: file
                .path
                .strip_prefix(package_root)
                .unwrap_or(&file.path)
                .to_path_buf(),
        };
        ParsedSource::resolve(location, parsed, |path| {
            cancellation.checkpoint()?;
            Ok(resolve_package_module(path, &file.package, self))
        })
        .map(Ok)
    }
}
/// Resolve a module path from a file of `current_package` through the locked
/// package graph to a canonical file inside the owning package's authority.
fn resolve_package_module(
    path: &ModulePath,
    current_package: &PackageInstanceId,
    context: &PackageLoadContext<'_>,
) -> ModuleResolution<PackageFileKey> {
    if names_stdlib(path) {
        return ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented);
    }
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.name.to_string())
        .collect::<Vec<_>>();
    let resolved = match context
        .graph
        .resolve_module_path(current_package, &segments)
    {
        Ok(resolved) => resolved,
        Err(error) => {
            return ModuleResolution::Failed(ResolveFailure::NotLocked {
                message: error.to_string(),
            });
        }
    };
    let Some(package) = context.graph.package(&resolved.package) else {
        return ModuleResolution::Failed(ResolveFailure::Manifest {
            message: format!("lockfile package `{}` is missing", resolved.package),
        });
    };
    let (root, reader) = match context.authority_for(&resolved.package) {
        Ok(authority) => authority,
        Err(error) => {
            return ModuleResolution::Failed(ResolveFailure::Manifest {
                message: error.to_string(),
            });
        }
    };
    for file_segment_count in (0..=resolved.module_segments.len()).rev() {
        let mut file_path = package.source_dir.join_to(root).join(package.name.as_str());
        for segment in &resolved.module_segments[..file_segment_count] {
            file_path = file_path.join(segment);
        }
        file_path.set_extension("gcl");
        let Ok(canonical) = reader.canonicalize(&file_path) else {
            continue;
        };
        if !canonical.starts_with(root) {
            return ModuleResolution::Failed(ResolveFailure::OutsidePackageRoot);
        }
        let inline_path = match resolved.module_segments[file_segment_count..]
            .iter()
            .map(|segment| DeclName::try_new(segment.clone()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(inline_path) => inline_path,
            Err(error) => {
                return ModuleResolution::Failed(ResolveFailure::Manifest {
                    message: format!("invalid locked module segment: {error}"),
                });
            }
        };
        return ModuleResolution::Resolved(ResolvedFile {
            file: PackageFileKey {
                package: resolved.package,
                path: canonical,
            },
            inline_path,
        });
    }
    ModuleResolution::Failed(ResolveFailure::FileNotFound)
}

fn source_root_candidate(
    project_root: &Path,
    cache_root: &crate::package_cache::PackageCacheRoot,
    package: &LockedPackage,
) -> PathBuf {
    match &package.source {
        PackageSource::Root => project_root.to_path_buf(),
        PackageSource::Git {
            url,
            commit,
            tree_hashes,
            ..
        } => cache_root.git_checkout(
            &GitSourceId::new(url.clone(), commit.clone()),
            &tree_hashes.sha256,
        ),
    }
}

fn source_root_error(
    package: &LockedPackage,
    candidate: &Path,
    error: &std::io::Error,
) -> CompileError {
    match &package.source {
        PackageSource::Root => loader_manifest_error(format!(
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

fn capture_dependency_from_authority(
    project_root: &Path,
    package: &LockedPackage,
    sources: crate::package_sources::DependencySources<'_>,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<VerifiedDependency, CompileError> {
    match sources {
        crate::package_sources::DependencySources::NativeCache => {
            let cache = package_cache_root().map_err(loader_manifest_error)?;
            let candidate = source_root_candidate(project_root, &cache, package);
            let reader = RealFileSystem::rooted(&candidate)
                .map_err(|error| source_root_error(package, &candidate, &error))?;
            let root = reader
                .canonicalize(&candidate)
                .map_err(|error| source_root_error(package, &candidate, &error))?;
            load_verified_dependency(&root, package, &reader, budget, cancellation)
        }
        crate::package_sources::DependencySources::Embedded(packages) => {
            let embedded = packages.get(&package.id).ok_or_else(|| {
                loader_manifest_error(format!("missing embedded package `{}`", package.id))
            })?;
            let verified = load_verified_dependency(
                &embedded.root,
                package,
                &embedded.filesystem,
                budget,
                cancellation,
            )?;
            let expected: std::collections::BTreeSet<_> = verified
                .dependency
                .snapshot
                .files()
                .map(|(path, _)| embedded.root.join(path))
                .collect();
            let supplied: std::collections::BTreeSet<_> = embedded
                .filesystem
                .file_paths()
                .map(|path| path.as_path().to_path_buf())
                .collect();
            if expected != supplied {
                return Err(loader_manifest_error(format!(
                    "embedded package `{}` contains files outside its authenticated closure",
                    package.id
                )));
            }
            Ok(verified)
        }
    }
}

struct VerifiedDependency {
    manifest: PackageManifest,
    filesystem: graphcal_io::InMemoryFileSystem,
    dependency: LoadedDependency,
}

fn load_verified_dependency(
    root: &Path,
    package: &LockedPackage,
    reader: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<VerifiedDependency, CompileError> {
    let manifest = read_package_manifest_from_path(root, reader, budget, cancellation)?;
    let snapshot = verify_locked_source(root, package, &manifest, reader, budget, cancellation)?;
    let filesystem = snapshot.mount(root).map_err(loader_manifest_error)?;
    let captured_manifest =
        read_package_manifest_from_path(root, &filesystem, budget, cancellation)?;
    if captured_manifest != manifest {
        return Err(loader_manifest_error(
            "package manifest changed while capturing its authenticated snapshot",
        ));
    }
    Ok(VerifiedDependency {
        manifest,
        filesystem,
        dependency: LoadedDependency {
            root: root.to_path_buf(),
            snapshot,
        },
    })
}

fn verify_locked_source(
    root: &Path,
    package: &LockedPackage,
    manifest: &PackageManifest,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_io::SourceTreeSnapshot, CompileError> {
    let PackageSource::Git { tree_hashes, .. } = &package.source else {
        return Err(loader_manifest_error(
            "only Git dependencies have authenticated source snapshots",
        ));
    };
    let cancellation_signal = || cancellation.is_cancelled();
    let source_dir = manifest.source_dir.to_path_buf();
    let snapshot = crate::package_snapshot::capture_package(
        fs,
        root,
        &source_dir,
        budget.source_tree_limits(),
        &cancellation_signal,
    )
    .map_err(|error| {
        if matches!(
            error,
            crate::package_snapshot::PackageSnapshotError::Tree(
                graphcal_io::SourceTreeHashError::Cancelled
            )
        ) {
            cancellation
                .checkpoint()
                .map_or_else(CompileError::from, |()| loader_manifest_error(error))
        } else {
            loader_manifest_error(error)
        }
    })?;
    let actual = snapshot.hash();
    budget
        .account_source_tree(root, actual.entries(), actual.bytes())
        .map_err(loader_manifest_error)?;
    if actual.sha256() == tree_hashes.sha256.to_string() {
        Ok(snapshot)
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

fn package_cache_root() -> Result<crate::package_cache::PackageCacheRoot, String> {
    #[cfg(test)]
    if let Some(path) = TEST_CACHE_DIR.with(|slot| slot.borrow().clone()) {
        return crate::package_cache::PackageCacheRoot::from_path(path)
            .map_err(|error| error.to_string());
    }
    crate::package_cache::PackageCacheRoot::from_environment().map_err(|error| error.to_string())
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
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
    let source_root = manifest.source_dir.join_to(project_root);
    let pkg_dir = source_root.join(manifest.name.as_str());
    let pkg_file = source_root.join(format!("{}.gcl", manifest.name));

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

/// Whether a module path names the reserved (deferred) standard library.
/// Both `graphcal` and `std` first segments are reserved (Concept §6.2).
fn names_stdlib(path: &ModulePath) -> bool {
    matches!(path.segments.first().name.as_str(), "graphcal" | "std")
}

/// Resolve a module path of a single-package project to a canonical file.
///
/// All paths are absolute from a package root (real package via
/// `graphcal.toml` manifest, or virtual package = single-file project).
/// The first segment names the package; remaining segments walk the
/// directory tree under `source_dir`, so `nasa.rocket` resolves to
/// `<project_root>/<source_dir>/nasa/rocket.gcl`.
fn resolve_project_module<F: FileSystemReader>(
    path: &ModulePath,
    project_root: &Path,
    manifest: Option<&PackageManifest>,
    fs: &F,
) -> ModuleResolution<PathBuf> {
    if names_stdlib(path) {
        return ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented);
    }
    // The manifest is determined eagerly by `load_manifest_for_root` based on
    // whether the root file lives inside the package namespace. Without one
    // the project is a single standalone file whose only legal path is its
    // own stem (Concept 7 self-reference), which never reaches resolution.
    let Some(manifest) = manifest else {
        return ModuleResolution::Failed(ResolveFailure::CrossFileImportInVirtualPackage);
    };
    let segments = path.segments();
    // Real package: first segment must match the package name.
    if segments[0].name != manifest.name.as_str() {
        return ModuleResolution::Failed(ResolveFailure::PackageNameMismatch {
            package_name: manifest.name.to_string(),
        });
    }

    // Choose the longest prefix that names a physical source file. Any
    // remaining segments are an exact nested inline-DAG path in that file.
    for file_segment_count in (1..=segments.len()).rev() {
        let mut file_path = manifest.source_dir.join_to(project_root);
        for segment in &segments[..file_segment_count] {
            file_path = file_path.join(segment.name.as_str());
        }
        file_path.set_extension("gcl");
        let Ok(canonical) = fs.canonicalize(&file_path) else {
            continue;
        };
        // Path sandboxing: imports must stay inside the project root.
        if !canonical.starts_with(project_root) {
            return ModuleResolution::OutsideProjectRoot;
        }
        let inline_path = segments[file_segment_count..]
            .iter()
            .map(|segment| DeclName::from_atom(segment.name.clone()))
            .collect();
        return ModuleResolution::Resolved(ResolvedFile {
            file: canonical,
            inline_path,
        });
    }
    ModuleResolution::Failed(ResolveFailure::FileNotFound)
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
    use graphcal_package::Sha256Digest;

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

    #[test]
    fn plugin_call_policy_prefers_function_budget_then_project_default() {
        let manifest = parse_manifest_str(
            r#"
[package]
name = "project"

[plugins]
fuel_per_call = 200000000

[[plugins.function_limits]]
plugin = "plugins/solver.wasm"
function = "heavy"
fuel_per_call = 900000000
"#,
        )
        .unwrap();
        let owner = DagPackageId::new("test");
        let policy = PluginCallPolicy::from_manifest(&manifest.plugin_execution_policy, &owner);
        let plugin = PluginIdentity::resolve(&PluginPath::new("plugins/solver.wasm"), &owner);

        assert_eq!(
            policy.fuel_per_call(&plugin, &FnName::expect_valid("heavy")),
            Some(900_000_000)
        );
        assert_eq!(
            policy.fuel_per_call(&plugin, &FnName::expect_valid("light")),
            Some(200_000_000)
        );
    }

    struct CanonicalizeCountingFileSystem {
        inner: RealFileSystem,
        canonicalize_calls: Cell<usize>,
    }

    impl Default for CanonicalizeCountingFileSystem {
        fn default() -> Self {
            Self {
                inner: RealFileSystem::default(),
                canonicalize_calls: Cell::new(0),
            }
        }
    }

    impl FileSystemReader for CanonicalizeCountingFileSystem {
        fn read_bytes_bounded(
            &self,
            path: &Path,
            limit: ByteLimit,
            cancellation: &dyn CancellationSignal,
        ) -> Result<Vec<u8>, FileSystemReadError> {
            self.inner.read_bytes_bounded(path, limit, cancellation)
        }

        fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error> {
            let next = self
                .canonicalize_calls
                .get()
                .checked_add(1)
                .ok_or_else(|| io::Error::other("canonicalization counter overflow"))?;
            self.canonicalize_calls.set(next);
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
    fn malformed_outside_path_lock_is_rejected_before_source_root_access() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root_manifest = parse_manifest_str(
            r#"
[package]
name = "mission"
"#,
        )
        .unwrap();
        let escaped_outside = outside
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let lockfile = format!(
            r#"lock_version = 1
created_by = "test"
graphcal_version = "{}"
stdlib_version = "{}"
root = "pkg-mission"

[[package]]
id = "pkg-mission"
name = "mission"
source_dir = "src"

[package.source]
type = "path"
path = "."

[[package]]
id = "pkg-outside"
name = "outside"
source_dir = "src"

[package.source]
type = "path"
path = "{escaped_outside}"
"#,
            env!("CARGO_PKG_VERSION"),
            STDLIB_VERSION,
        );
        fs::write(project.path().join("graphcal.lock"), lockfile).unwrap();
        let file_system = CanonicalizeCountingFileSystem::default();
        let mut budget = LoaderBudgetState::new(LoaderBudget::default());
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();

        let result = PackageLoadContext::from_lockfile(
            project.path(),
            root_manifest,
            &file_system,
            crate::package_sources::DependencySources::NativeCache,
            &mut budget,
            &cancellation,
        );

        let error = result.err().expect("outside path source must be rejected");
        assert!(error.to_string().contains("unsupported path source"));
        assert_eq!(
            file_system.canonicalize_calls.get(),
            0,
            "source-root canonicalization must not start for an invalid lock"
        );
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
        assert_eq!(project.files.len(), 1);
        assert_eq!(
            project.root_id().package(),
            &DagPackageId::new("standalone")
        );
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
        assert_eq!(project.root_id().package(), &DagPackageId::new("mission"));
    }

    #[test]
    fn load_simple_import() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/lib.gcl", "param y: Dimensionless = 2.0;"),
            (
                "src/helper/main.gcl",
                "import helper.lib::{y};\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/helper/main.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 2);
        assert_eq!(project.files.len(), 2);
        // helper.lib should be loaded before main (topological order)
        let lib_dag_id = DagId::new("helper", NonEmpty::new("src", vec!["helper", "lib"]));
        let main_dag_id = DagId::new("helper", NonEmpty::new("src", vec!["helper", "main"]));
        assert_eq!(project.files.ordered().get(0).unwrap().dag_id, lib_dag_id);
        assert_eq!(project.files.ordered().get(1).unwrap().dag_id, main_dag_id);
        assert_eq!(project.root_id().package(), &DagPackageId::new("helper"));
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
            .resolve_index_variant_parts(
                project.root_id(),
                &name_path(&["lib", "Phase"]),
                &graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid("Burn"),
            )
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
                "import helper::{y};\nnode z: Dimensionless = @y + 1.0;",
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
        // Manifest layout: `package = "a"`, files at `<root>/src/a.gcl`
        // and `<root>/src/a/b.gcl`. `a` imports from `a.b` and `a.b` imports
        // from `a` — yielding a cycle through dot-paths.
        let dir = setup_temp_dir(&[
            (
                "graphcal.toml",
                "[package]\nname = \"a\"\nsource_dir = \"src\"\n",
            ),
            (
                "src/a.gcl",
                "import a.b::{y};\nparam x: Dimensionless = 1.0;",
            ),
            (
                "src/a/b.gcl",
                "import a::{x};\nparam y: Dimensionless = 2.0;",
            ),
        ]);
        let result = load_project(&dir.path().join("src/a.gcl"), None, &fs());
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("circular") || err.contains("Circular"),
            "error should mention circular: {err}"
        );
    }

    #[test]
    fn load_missing_import_file() {
        let dir = setup_temp_dir(&[("main.gcl", "import nonexistent::{x};")]);
        let result = load_project(&dir.path().join("main.gcl"), None, &fs());
        assert!(result.is_err());
    }

    #[test]
    fn from_source_single_file() {
        let source = "param x: Dimensionless = 1.0;";
        let project = LoadedProject::from_source(source, "test.gcl").unwrap();
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.files.len(), 1);
        let root_file = project.root_file();
        assert_eq!(root_file.source.as_str(), source);
        assert_eq!(project.root_id().package(), &DagPackageId::new("test"));
    }

    #[test]
    fn from_source_keeps_absolute_diagnostic_label_out_of_semantic_identity() {
        let project = LoadedProject::from_source(
            "param x: Dimensionless = 1.0;",
            "/virtual/project/main.gcl",
        )
        .unwrap();
        let root_file = project.root_file();

        assert_eq!(root_file.named_source.name(), "/virtual/project/main.gcl");
        assert_eq!(project.root_id().package(), &DagPackageId::new("main"));
        assert_eq!(project.root_id().to_string(), "main");
    }

    #[test]
    fn inline_dag_unresolved_body_import_is_recorded_explicitly() {
        let source = r"
dag calc {
  import missing::{x};
  param input: Dimensionless = 1.0;
  pub node output: Dimensionless = @input;
}
";
        let project = LoadedProject::from_source(source, "test.gcl").unwrap();
        let root_file = project.root_file();
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
        )
        .unwrap();
        let project = load_project(&root_path, None, &fs).unwrap();

        let root_file = project.root_file();
        assert_eq!(root_file.source.as_str(), overlay_source);
    }

    #[test]
    fn load_with_overlay_uses_disk_for_imports() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"helper\"\n"),
            ("src/helper/lib.gcl", "param y: Dimensionless = 2.0;"),
            (
                "src/helper/main.gcl",
                "import helper.lib::{y};\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let root_path = dir.path().join("src/helper/main.gcl");

        let overlay_source = "import helper.lib::{y};\nnode z: Dimensionless = @y + 99.0;";
        let canonical = root_path.canonicalize().unwrap();
        let fs = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            canonical,
            overlay_source.to_string(),
        )
        .unwrap();
        let project = load_project(&root_path, None, &fs).unwrap();

        // Root file should use overlay content
        let root_file = project.root_file();
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
        )
        .unwrap();
        let result = load_project(&root_path, None, &fs);
        assert!(result.is_err());
    }

    #[test]
    fn load_diamond_import_deduplication() {
        // A imports B and C; both B and C import D. D should only be
        // loaded once.
        //
        // The four files live at `<root>/src/graph/{a,b,c,d}.gcl` so every
        // import path starts with the package name.
        let dir = setup_temp_dir(&[
            (
                "graphcal.toml",
                "[package]\nname = \"graph\"\nsource_dir = \"src\"\n",
            ),
            ("src/graph/d.gcl", "param w: Dimensionless = 4.0;"),
            (
                "src/graph/b.gcl",
                "import graph.d::{w};\nparam x: Dimensionless = @w + 1.0;",
            ),
            (
                "src/graph/c.gcl",
                "import graph.d::{w};\nparam y: Dimensionless = @w + 2.0;",
            ),
            (
                "src/graph/a.gcl",
                "import graph.b::{x};\nimport graph.c::{y};\nnode z: Dimensionless = @x + @y;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/graph/a.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 4);
        // d should appear first in load order
        let d_dag_id = DagId::new("graph", NonEmpty::new("src", vec!["graph", "d"]));
        assert_eq!(project.files.ordered().get(0).unwrap().dag_id, d_dag_id);
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
                "import nasa.rocket::{x};\nnode y: Dimensionless = @x + 1.0;",
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
                "import nasa.orbital.transfer::{dv};\nnode x: Dimensionless = @dv;",
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
                "import myproject.helpers::{x};\nnode y: Dimensionless = @x + 1.0;",
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
            ("src/nasa/main.gcl", "import other.rocket::{x};"),
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
            ("src/nasa/main.gcl", "import graphcal.math::{sin};"),
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
            ("src/nasa/main.gcl", "import nasa.nonexistent::{x};"),
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
                "import myproject.helper::{y};\nnode z: Dimensionless = @y;",
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

    fn install_dependency_cache_generation(
        cache_root: &Path,
        source: &GitSourceId,
        dependency_source: &str,
    ) -> (PathBuf, PackageManifest, Sha256Digest) {
        let staging = cache_root.join("dependency-staging");
        std::fs::create_dir_all(staging.join("src/units")).unwrap();
        let manifest_text = "[package]\nname = \"units\"\n";
        std::fs::write(staging.join("graphcal.toml"), manifest_text).unwrap();
        std::fs::write(staging.join("src/units/lib.gcl"), dependency_source).unwrap();
        let manifest = parse_manifest_str(manifest_text).unwrap();
        let reader = RealFileSystem::rooted(&staging).unwrap();
        let tree_hash = Sha256Digest::new(
            hash_source_tree(
                &reader,
                &staging,
                &manifest.source_dir.to_path_buf(),
                SourceTreeHashLimits::unbounded(),
                &graphcal_io::NeverCancel,
            )
            .unwrap()
            .sha256(),
        )
        .unwrap();
        drop(reader);
        let dependency_root = crate::package_cache::PackageCacheRoot::from_path(cache_root)
            .unwrap()
            .git_checkout(source, &tree_hash);
        std::fs::create_dir_all(dependency_root.parent().unwrap()).unwrap();
        std::fs::rename(staging, &dependency_root).unwrap();
        (dependency_root, manifest, tree_hash)
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
        let source = GitSourceId::new(url.clone(), revision.clone());
        let (dependency_root, dependency_manifest, tree_hash) =
            install_dependency_cache_generation(&cache_root, &source, dependency_source);
        std::fs::write(project_root.join("graphcal.toml"), root_manifest).unwrap();

        let root_file = root_source_dir.join("main.gcl");
        let root_helper = root_source_dir.join("helper.gcl");
        std::fs::write(&root_file, root_source).unwrap();
        std::fs::write(
            &root_helper,
            "pub const node local_value: Dimensionless = 1.0;",
        )
        .unwrap();

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
                    source_dir: parsed_root_manifest.source_dir.clone(),
                    source: PackageSource::Root,
                    dependencies: BTreeMap::from([(
                        dependency_name.clone(),
                        dependency_id.clone(),
                    )]),
                },
                LockedPackage {
                    id: dependency_id,
                    name: dependency_manifest.name,
                    source_dir: dependency_manifest.source_dir,
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
            lockfile.to_deterministic_toml(),
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
        )
        .unwrap();

        let project = load_project(&fixture.root_file, None, &overlay).unwrap();
        assert_eq!(project.root_file().source.as_str(), overlay_source);
    }

    #[test]
    fn dependency_enabled_loader_rejects_file_root_self_import() {
        let fixture = locked_package_fixture(
            "import mission.main::{ ghost };",
            "pub const node one: Dimensionless = 1.0;",
        );

        let error = load_project(&fixture.root_file, None, &RealFileSystem::default())
            .expect_err("the package-aware loader must reject an exact self-file target");
        assert!(matches!(
            error,
            CompileError::Eval(GraphcalError::FileRootSelfImport { .. })
        ));
    }

    #[test]
    fn dependency_enabled_loader_uses_overlay_for_root_package_import() {
        let fixture = locked_package_fixture(
            "import mission.helper::{ local_value };\nnode result: Dimensionless = @local_value;",
            "pub const node one: Dimensionless = 1.0;",
        );
        let overlay_source = "pub const node local_value: Dimensionless = 99.0;";
        let overlay = graphcal_io::OverlayFileSystem::new(
            RealFileSystem::default(),
            fixture.root_helper.canonicalize().unwrap(),
            overlay_source.to_string(),
        )
        .unwrap();

        let project = load_project(&fixture.root_file, None, &overlay).unwrap();
        let helper = project
            .files
            .iter()
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
    fn lock_source_directory_cannot_select_an_unhashed_tree() {
        let fixture = locked_package_fixture(
            "import units_v1.lib::{ value };\nnode result: Dimensionless = @value;",
            "pub const node value: Dimensionless = 1.0;",
        );
        std::fs::create_dir_all(fixture.dependency_root.join("unhashed/units")).unwrap();
        std::fs::write(
            fixture.dependency_root.join("unhashed/units/lib.gcl"),
            "pub const node value: Dimensionless = 999.0;",
        )
        .unwrap();

        let lockfile_path = fixture.directory.path().join("project/graphcal.lock");
        let mut lockfile =
            graphcal_package::parse_lockfile_str(&std::fs::read_to_string(&lockfile_path).unwrap())
                .expect("fixture lockfile parses");
        let dependency = lockfile
            .packages
            .iter_mut()
            .find(|package| matches!(package.source, PackageSource::Git { .. }))
            .expect("fixture dependency lock entry");
        dependency.source_dir = graphcal_package::PackageSourceDirectory::new("unhashed").unwrap();
        std::fs::write(&lockfile_path, lockfile.to_deterministic_toml()).unwrap();

        let error = load_project(&fixture.root_file, None, &RealFileSystem::default())
            .expect_err("lock and manifest source directories must match before resolution");
        assert!(
            error
                .to_string()
                .contains("uses source_dir `unhashed`, but its manifest uses `src`"),
            "unexpected lock/manifest diagnostic: {error:?}"
        );
    }

    #[test]
    fn aggregate_loader_file_budget_covers_manifest_and_source_reads() {
        let directory = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"mission\"\n"),
            (
                "src/mission/main.gcl",
                "import mission.helper::{ value };\nnode result: Dimensionless = @value;",
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
    import units_v1.lib::{ one };
    pub node out: Dimensionless = @one;
}
node result: Dimensionless = @calculation()::out;
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

    /// In-memory authority recording the order in which files are fetched.
    struct ScriptedSources {
        files: HashMap<PathBuf, &'static str>,
        manifest: PackageManifest,
        filesystem: graphcal_io::InMemoryFileSystem,
        fetched: std::cell::RefCell<Vec<PathBuf>>,
    }

    impl ScriptedSources {
        fn new(files: &[(&str, &'static str)]) -> Self {
            let mut filesystem = graphcal_io::InMemoryFileSystem::new();
            let files = files
                .iter()
                .map(|(name, text)| {
                    let path = PathBuf::from(format!("/p/src/pkg/{name}.gcl"));
                    filesystem
                        .add_file(
                            graphcal_io::VirtualAbsolutePath::new(path.clone()).unwrap(),
                            (*text).to_string(),
                        )
                        .unwrap();
                    (path, *text)
                })
                .collect();
            Self {
                files,
                manifest: parse_manifest_str("[package]\nname = \"pkg\"\n").unwrap(),
                filesystem,
                fetched: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl SnapshotSource for ScriptedSources {
        type Key = PathBuf;

        fn fetch(
            &self,
            file: &PathBuf,
            _budget: &mut LoaderBudgetState,
            _cancellation: &graphcal_compiler::cancellation::CancellationToken,
        ) -> Result<FetchedFile<PathBuf>, CompileError> {
            self.fetched.borrow_mut().push(file.clone());
            let Some(text) = self.files.get(file) else {
                return Ok(Err(io_not_found(file)));
            };
            let name = file.display().to_string();
            let source = Arc::new((*text).to_string());
            let parsed = match graphcal_compiler::syntax::parser::Parser::with_name(&source, &name)
                .parse_file()
            {
                Ok(raw) => ParsedFile {
                    named_source: NamedSource::new(name.as_str(), Arc::clone(&source)),
                    source,
                    ast: graphcal_compiler::desugar::desugared_ast::File::from(raw),
                },
                Err(error) => return Ok(Err(error.into())),
            };
            let location = ModuleLocation {
                package: DagPackageId::new("pkg"),
                relative_path: file.strip_prefix("/p").unwrap().to_path_buf(),
            };
            ParsedSource::resolve(location, parsed, |path| {
                Ok(resolve_project_module(
                    path,
                    Path::new("/p"),
                    Some(&self.manifest),
                    &self.filesystem,
                ))
            })
            .map(Ok)
        }
    }

    fn fetch_scripted(sources: &ScriptedSources, root: &str) -> SourceSnapshot<PathBuf> {
        fetch_source_snapshot(
            sources,
            PathBuf::from(format!("/p/src/pkg/{root}.gcl")),
            &mut LoaderBudgetState::new(LoaderBudget::default()),
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
        .unwrap()
    }

    fn scripted_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/p/src/pkg/{name}.gcl"))
    }

    #[test]
    fn snapshot_fetch_follows_depth_first_load_order_once_per_file() {
        let sources = ScriptedSources::new(&[
            (
                "main",
                "import pkg.b::{y};\nimport pkg.c::{z};\ndag inner { import pkg.d::{w}; }",
            ),
            ("b", "import pkg.c::{z};"),
            ("c", "param z: Dimensionless = 1.0;"),
            ("d", "param w: Dimensionless = 1.0;"),
        ]);
        let snapshot = fetch_scripted(&sources, "main");

        assert_eq!(
            *sources.fetched.borrow(),
            ["main", "b", "c", "d"].map(scripted_path)
        );
        assert_eq!(snapshot.files.len(), 4);
        let files = build_loaded_files(snapshot).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            ["c", "b", "d", "main"].map(scripted_path)
        );
    }

    #[test]
    fn snapshot_fetch_stops_after_first_unreadable_file() {
        let sources = ScriptedSources::new(&[
            ("main", "import pkg.b::{y};\nimport pkg.c::{z};"),
            ("b", "this is not graphcal"),
            ("c", "param z: Dimensionless = 1.0;"),
        ]);
        let snapshot = fetch_scripted(&sources, "main");

        assert_eq!(*sources.fetched.borrow(), ["main", "b"].map(scripted_path));
        assert!(snapshot.files[&scripted_path("b")].is_err());
        assert!(matches!(
            build_loaded_files(snapshot),
            Err(CompileError::Parse(_))
        ));
    }

    #[test]
    fn snapshot_fetch_skips_unresolved_and_self_paths() {
        let sources = ScriptedSources::new(&[(
            "main",
            "import pkg.missing::{y};\nimport pkg.main.inner::{x};\ndag inner { param x: Dimensionless = 1.0; }",
        )]);
        let snapshot = fetch_scripted(&sources, "main");

        assert_eq!(*sources.fetched.borrow(), [scripted_path("main")]);
        assert!(matches!(
            build_loaded_files(snapshot),
            Err(CompileError::Eval(GraphcalError::ImportFileNotFound { ref path, .. })) if path == "pkg.missing"
        ));
    }

    #[test]
    fn project_module_resolution_is_typed_and_span_free() {
        let sources = ScriptedSources::new(&[("lib", ""), ("nested/deep", "")]);
        let resolve = |text: &str| {
            let parsed = graphcal_compiler::syntax::parser::Parser::with_name(text, "t.gcl")
                .parse_file()
                .unwrap();
            let graphcal_compiler::syntax::ast::DeclKind::Import(import) =
                &parsed.declarations[0].kind
            else {
                panic!("expected an import");
            };
            resolve_project_module(
                &import.path,
                Path::new("/p"),
                Some(&sources.manifest),
                &sources.filesystem,
            )
        };

        assert_eq!(
            resolve("import pkg.lib.inner.leaf::{x};"),
            ModuleResolution::Resolved(ResolvedFile {
                file: scripted_path("lib"),
                inline_path: vec![
                    DeclName::try_new("inner".to_string()).unwrap(),
                    DeclName::try_new("leaf".to_string()).unwrap(),
                ],
            })
        );
        assert_eq!(
            resolve("import pkg.nested.deep::{x};"),
            ModuleResolution::Resolved(ResolvedFile {
                file: scripted_path("nested/deep"),
                inline_path: Vec::new(),
            })
        );
        assert_eq!(
            resolve("import pkg.absent::{x};"),
            ModuleResolution::Failed(ResolveFailure::FileNotFound)
        );
        assert_eq!(
            resolve("import std.math::{x};"),
            ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented)
        );
        assert_eq!(
            resolve("import graphcal.math::{x};"),
            ModuleResolution::Failed(ResolveFailure::StdlibNotImplemented)
        );
        assert_eq!(
            resolve("import other.lib::{x};"),
            ModuleResolution::Failed(ResolveFailure::PackageNameMismatch {
                package_name: "pkg".to_string(),
            })
        );
        let parsed =
            graphcal_compiler::syntax::parser::Parser::with_name("import pkg.lib::{x};", "t.gcl")
                .parse_file()
                .unwrap();
        let graphcal_compiler::syntax::ast::DeclKind::Import(import) = &parsed.declarations[0].kind
        else {
            panic!("expected an import");
        };
        assert_eq!(
            resolve_project_module(&import.path, Path::new("/p"), None, &sources.filesystem),
            ModuleResolution::Failed(ResolveFailure::CrossFileImportInVirtualPackage)
        );
    }
}
