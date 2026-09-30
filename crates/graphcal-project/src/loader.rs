use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::compile_error::CompileError;
use crate::dependency_ordered::DependencyOrdered;
use graphcal_compiler::dag_id::{DagId, DagPackageId};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::plugin_identity::{ExternFnKey, PluginIdentity};
use graphcal_compiler::syntax::ast::ModulePath;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::plugin::PluginPath;
#[cfg(test)]
use graphcal_io::hash_source_tree;
use graphcal_io::{FileSystemEntryKind, FileSystemReadError, FileSystemReader, RealFileSystem};
mod budget;
mod build;
mod inline_dags;
mod loaded_file;
mod module_path;
mod source_authority;
mod source_snapshot;

#[cfg(test)]
use budget::MEBIBYTE;
use budget::{
    LoaderArtifact, LoaderBudgetState, LoaderReadError, PackageAuthorityError, io_not_found,
    loader_manifest_error,
};
pub use budget::{LoaderArtifactByteLimits, LoaderBudget, LoaderBudgetExceeded, LoaderResource};
pub(crate) use loaded_file::LoadedModule;
pub use loaded_file::{LoadedDag, LoadedFile};
pub use module_path::{
    InlineBodyImportResolution, ModulePathKey, ResolvedModuleTarget, ResolvedModuleTargetError,
};

use build::{build_loaded_files, reject_file_root_stem_imports};
use inline_dags::lift_inline_dags;
use source_authority::{
    ModuleSourceAuthority, ProjectSources, SelectedPackage, SourceTree, fetch_source_snapshot,
};
use source_snapshot::{PackageFileKey, ParsedFile, ResolveFailure, file_stem};

use graphcal_package::{
    GitSourceId, LockedPackage, PackageInstanceId, PackageManifest, PackageSource, STDLIB_VERSION,
    ValidatedPackageGraph, parse_lockfile_str_with_limits, parse_manifest_str,
};

#[cfg(test)]
thread_local! {
    static TEST_CACHE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
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

    /// The file root or inline DAG module `dag_id`.
    #[must_use]
    pub(crate) fn module(&self, dag_id: &DagId) -> Option<LoadedModule<'_>> {
        self.file(dag_id).map_or_else(
            || self.inline_dag(dag_id).map(|(file, dag)| dag.module(file)),
            |file| Some(file.module()),
        )
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
        let parsed = ParsedFile::parse(name, Arc::new(source.to_string()), cancellation)?;
        cancellation.checkpoint()?;
        let path = PathBuf::from(name);
        let stem = file_stem(&path);
        reject_file_root_stem_imports(&parsed.ast.declarations, stem, &parsed.named_source)?;
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
                &parsed.named_source,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
        // No project root or manifest in single-file mode — only the
        // file-stem self-reference (Concept 7) can be detected here.
        let inline_dags = lift_inline_dags(&parsed.ast, &dag_id, stem, |_| None);
        cancellation.checkpoint()?;
        // No filesystem to read wasm plugin files from; the entries carry
        // the reason so evaluation can report it at the import site.
        let plugins = wasm_plugin_paths(&parsed.ast)
            .map(|plugin| {
                (
                    PluginIdentity::resolve(plugin, dag_id.package()),
                    Err(PluginFileError::NoProjectFilesystem),
                )
            })
            .collect();
        cancellation.checkpoint()?;
        let (source, named_source, ast) = parsed.into_parts();
        let loaded_file = LoadedFile::new(
            path,
            dag_id,
            source,
            named_source,
            ast,
            HashMap::new(),
            inline_dags,
        );
        Ok(Self::from_parts(
            DependencyOrdered::root_only(loaded_file),
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

    /// Build the module resolver for every loaded file and inline DAG.
    ///
    /// The loader decides which module each `import` / `include` path names
    /// (see the [`ModuleTargets`](graphcal_compiler::resolve::builder::ModuleTargets)
    /// impl); the compiler's pure resolver builds every scope from those
    /// edges and expands includes into their instances.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`](graphcal_compiler::resolve::error::ModuleResolveError)
    /// for duplicate or ambiguous modules, duplicate symbols, recursive
    /// include expansion, or invalid resolved import surfaces.
    pub fn build_module_resolver(
        &self,
    ) -> Result<
        graphcal_compiler::resolve::ModuleResolver,
        graphcal_compiler::resolve::error::ModuleResolveError,
    > {
        let mut tables = graphcal_compiler::resolve::builder::SymbolTables::default();
        for loaded in &self.files {
            tables.add_module(loaded.dag_id.clone(), &loaded.ast.declarations)?;
            for inline in &loaded.inline_dags {
                tables.add_module(inline.dag_id.clone(), inline.body(loaded))?;
            }
        }
        tables.scopes(self)?.freeze()
    }

    /// The top-level `dag` of the file root `owner` that a single-segment
    /// module path names. File-root includes of their own DAGs are same-file
    /// references, which never drive loading.
    fn file_root_local_dag(&self, owner: &DagId, path: &ModulePath) -> Option<DagId> {
        self.file(owner)?;
        let [segment] = path.segments() else {
            return None;
        };
        let dag_id = owner.inline_dag_child(DeclName::classify(segment.name.atom().clone()));
        self.inline_dag(&dag_id).map(|_| dag_id)
    }
}

impl graphcal_compiler::resolve::builder::ModuleTargets for LoadedProject {
    fn import_target(&self, owner: &DagId, path: &ModulePath) -> Option<DagId> {
        resolved_module_target_from(owner, path, self)
    }

    fn include_target(&self, owner: &DagId, path: &ModulePath) -> Option<DagId> {
        resolved_module_target_from(owner, path, self)
            .or_else(|| self.file_root_local_dag(owner, path))
    }
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

impl ModuleSourceAuthority for PackageLoadContext<'_> {
    type Key = PackageFileKey;

    fn tree(&self, package: &PackageInstanceId) -> Result<SourceTree<'_>, PackageAuthorityError> {
        let (root, reader) = self.authority_for(package)?;
        Ok(SourceTree {
            reader,
            root,
            package: DagPackageId::new(package.as_str()),
        })
    }

    /// Resolve the first segment through the locked package graph of the
    /// importing file's package.
    fn select_package(
        &self,
        path: &ModulePath,
        from: &PackageFileKey,
    ) -> Result<SelectedPackage<PackageInstanceId>, ResolveFailure> {
        let segments = path
            .segments
            .iter()
            .map(|segment| segment.name.to_string())
            .collect::<Vec<_>>();
        let resolved = self
            .graph
            .resolve_module_path(&from.package, &segments)
            .map_err(|error| ResolveFailure::NotLocked {
                message: error.to_string(),
            })?;
        let package =
            self.graph
                .package(&resolved.package)
                .ok_or_else(|| ResolveFailure::Manifest {
                    message: format!("lockfile package `{}` is missing", resolved.package),
                })?;
        Ok(SelectedPackage {
            namespace_dir: package.source_dir.to_path_buf().join(package.name.as_str()),
            package: resolved.package,
        })
    }
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

#[cfg(test)]
mod tests;
