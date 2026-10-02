//! The loaded project snapshot: source files, plugin artifacts, and package
//! provenance handed from the loader to project checking.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use graphcal_compiler::dag_id::{DagId, DagPackageId};
use graphcal_compiler::plugin_identity::{ExternFnKey, PluginIdentity};
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::plugin::PluginPath;
use graphcal_package::PackageInstanceId;

use crate::dependency_ordered::DependencyOrdered;

use super::budget_violation::LoaderBudgetExceeded;
use super::loaded_file::{LoadedDag, LoadedFile, LoadedModule};
use super::module_path::{ResolvedModuleTarget, ResolvedModuleTargetError};
use graphcal_package::Sha256Digest;

/// Fuel policies resolved from each owning package manifest into compiler-owned
/// plugin and function identities.
#[derive(Debug, Clone, Default)]
pub struct PluginCallPolicy {
    pub(super) default_fuel_per_call: HashMap<DagPackageId, u64>,
    pub(super) function_fuel_per_call: HashMap<ExternFnKey, u64>,
}

impl PluginCallPolicy {
    pub(super) fn from_manifest(
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
    pub(super) files: LoadedFiles,
    /// Every loaded source text under its diagnostic name; the source ids of
    /// the loaded files and of every diagnostic about them resolve here.
    pub(super) sources: Arc<SourceRegistry>,
    /// WASM plugin files referenced by `import plugin "….wasm"` declarations
    /// keyed by the declaring package instance and artifact path.
    ///
    /// Paths resolve within the owning package root. Dependency bytes come
    /// exclusively from the authenticated snapshot. Read failures are reported
    /// at import spans; global host-registry identities never appear here.
    pub(super) plugins: HashMap<PluginIdentity, PluginFileEntry>,
    /// Package-scoped plugin fuel settings resolved to typed function identities.
    pub(super) plugin_call_policy: PluginCallPolicy,
    pub(super) package_closure: Option<LoadedPackageClosure>,
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
    pub(super) fn new(ordered: DependencyOrdered<LoadedFile>) -> Self {
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
    pub(super) fn owner(&self, dag_id: &DagId) -> Option<&LoadedFile> {
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

/// A successfully read wasm plugin file, ready for the plugin host.
#[derive(Clone)]
pub struct LoadedPlugin {
    /// The raw module bytes.
    pub(super) bytes: Arc<[u8]>,
    /// Lowercase-hex SHA-256 of the bytes — the form `graphcal.lock` pins.
    pub(super) sha256: Sha256Digest,
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
            .field("sha256", &self.sha256)
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
        expected: Sha256Digest,
        /// The digest of the file actually on disk.
        actual: Sha256Digest,
    },
}

impl LoadedProject {
    pub(super) fn from_parts(
        files: DependencyOrdered<LoadedFile>,
        sources: SourceRegistry,
        plugins: HashMap<PluginIdentity, PluginFileEntry>,
        plugin_call_policy: PluginCallPolicy,
    ) -> Self {
        Self {
            files: LoadedFiles::new(files),
            sources: Arc::new(sources),
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

    /// The registry every source id of this project resolves in.
    #[must_use]
    pub const fn sources(&self) -> &Arc<SourceRegistry> {
        &self.sources
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
}
