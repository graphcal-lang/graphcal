use crate::load_error::LoadError;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::compile_error::CompileError;
use crate::dependency_ordered::DependencyOrdered;
use graphcal_compiler::dag_id::{DagId, DagPackageId};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::plugin_identity::{ExternFnKey, PluginIdentity};
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::ast::ModulePath;
use graphcal_compiler::syntax::decl_name::DeclName;
#[cfg(test)]
use graphcal_io::hash_source_tree;
use graphcal_io::{FileSystemEntryKind, FileSystemReader, RealFileSystem};
mod budget;
pub(crate) mod budget_violation;
mod build;
mod inline_dags;
pub(crate) mod loaded_file;
pub(crate) mod loaded_module_resolver;
pub(crate) mod loaded_project;
pub(crate) mod module_path;
mod source_authority;
mod source_snapshot;

#[cfg(test)]
use budget::MEBIBYTE;
use budget::{
    LoaderArtifact, LoaderBudgetState, LoaderReadError, PackageAuthorityError, io_not_found,
    loader_manifest_error,
};
pub use budget::{LoaderArtifactByteLimits, LoaderBudget};
pub use budget_violation::{LoaderBudgetExceeded, LoaderResource};
use build::{build_loaded_files, reject_file_root_stem_imports};
use inline_dags::lift_inline_dags;
pub use loaded_file::{LoadedDag, LoadedFile};
pub use loaded_project::{
    LoadedDependency, LoadedFiles, LoadedPackageClosure, LoadedPlugin, LoadedProject,
    PluginCallPolicy, PluginFileEntry, PluginFileError,
};
use module_path::PackageSelector;
pub use module_path::ResolvedModuleTarget;
use source_authority::{
    ModuleSourceAuthority, ProjectSources, SelectedPackage, SourceTree, fetch_source_snapshot,
};
use source_snapshot::{PackageFileKey, ParsedFile, ResolveFailure, file_stem};

use graphcal_package::{
    GitSourceId, LockedPackage, PackageInstanceId, PackageManifest, PackageResolveError,
    PackageSource, STDLIB_VERSION, Sha256Digest, ValidatedPackageGraph,
    parse_lockfile_str_with_limits, parse_manifest_str,
};

#[cfg(test)]
thread_local! {
    static TEST_CACHE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

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
    files: &DependencyOrdered<LoadedFile<module_path::ModuleTarget>>,
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
) -> Result<HashMap<PluginIdentity, PluginFileEntry>, Outcome<CompileError>> {
    let mut plugins = HashMap::new();
    for (package, ast) in file_asts {
        for path in wasm_plugin_paths(ast) {
            cancellation.checkpoint()?;
            let identity = PluginIdentity::resolve(path, package);
            match plugins.entry(identity) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let entry = read_plugin_file(package_root, path, fs, budget, cancellation)?;
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
) -> Result<PluginFileEntry, graphcal_compiler::cancellation::Cancelled> {
    let artifact = match resolve_plugin_artifact_path(package_root, plugin, fs) {
        Ok(artifact) => artifact,
        Err(error) => return Ok(Err(error)),
    };
    Ok(
        match budget.read_bytes(fs, &artifact.0, LoaderArtifact::Plugin, cancellation) {
            Ok(bytes) => Ok(LoadedPlugin {
                sha256: Sha256Digest::from_bytes(Sha256::digest(&bytes).into()),
                bytes: bytes.into(),
            }),
            Err(Outcome::Cancelled) => return Err(graphcal_compiler::cancellation::Cancelled),
            Err(Outcome::Failed(LoaderReadError::Budget(error))) => {
                Err(PluginFileError::ResourceLimit(error))
            }
            Err(Outcome::Failed(LoaderReadError::Filesystem(error))) => {
                Err(PluginFileError::Unreadable {
                    resolved: artifact.0,
                    message: error.to_string(),
                })
            }
        },
    )
}

/// Enforce `graphcal.lock` pins on successfully read plugin files.
///
/// The lockfile is the trust boundary for plugin code: in a project with a
/// `graphcal.toml` manifest, a plugin binary loads only when its bytes hash
/// to the pinned digest. Missing or mismatched pins replace the loaded
/// entry with a hard error surfaced at the declaring import.
fn apply_plugin_pins(
    plugins: &mut HashMap<PluginIdentity, PluginFileEntry>,
    pins: &BTreeMap<String, Sha256Digest>,
) {
    for (path, entry) in plugins.iter_mut() {
        let Ok(loaded) = entry.as_ref() else {
            continue;
        };
        match pins.get(path.path().as_str()) {
            None => *entry = Err(PluginFileError::NotPinned),
            Some(expected) if *expected != loaded.sha256 => {
                *entry = Err(PluginFileError::HashMismatch {
                    expected: *expected,
                    actual: loaded.sha256,
                });
            }
            Some(_) => {}
        }
    }
}

impl LoadedProject {
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
        graphcal_compiler::outcome::without_cancellation(|cancellation| {
            Self::from_source_with_cancellation(source, name, cancellation)
        })
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
    ) -> Result<Self, Outcome<CompileError>> {
        cancellation.checkpoint()?;
        let mut sources = SourceRegistry::new();
        let parsed = ParsedFile::parse(
            &mut sources,
            name,
            Arc::new(source.to_string()),
            cancellation,
        )?;
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
            CompileError::semantic(
                SemanticError::internal_error(
                    format!("invalid source name `{name}`: {error}"),
                    parsed.source_id,
                    DiagnosticAnchor::WholeFile,
                ),
                &sources,
            )
        })?;
        // No project root or manifest in single-file mode — only same-file
        // DAGs and the file-stem self-reference (Concept 7) resolve here. Like
        // the file-root imports of this mode, a cross-file body path has no
        // target: an editor buffer analyzed without its project stays usable.
        let Ok(inline_dags) = lift_inline_dags(&parsed.ast, &dag_id, stem, |_| {
            Ok::<_, std::convert::Infallible>(None)
        });
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
        let (source, source_id, ast) = parsed.into_parts();
        let loaded_file = LoadedFile::new(
            path,
            dag_id,
            source,
            source_id,
            ast,
            HashMap::new(),
            inline_dags,
        );
        Self::from_parts(
            DependencyOrdered::root_only(loaded_file),
            sources,
            plugins,
            PluginCallPolicy::default(),
        )
        .map_err(Into::into)
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
        loaded_module_resolver::LoadedModuleResolver::build(self)
            .map(loaded_module_resolver::LoadedModuleResolver::into_resolver)
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
    let key = path.key();
    let resolved = project.file(source).map_or_else(
        || {
            project
                .inline_dag(source)
                .and_then(|(_, _, inline)| inline.resolved_imports.get(&key).cloned())
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
    load_project_with_budget(
        root_path,
        project_root_override,
        fs,
        LoaderBudget::default(),
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
    graphcal_compiler::outcome::without_cancellation(|cancellation| {
        load_project_with_budget_and_cancellation(
            root_path,
            project_root_override,
            fs,
            budget,
            cancellation,
        )
    })
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
) -> Result<LoadedProject, Outcome<CompileError>> {
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
) -> Result<LoadedProject, Outcome<CompileError>> {
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
) -> Result<LoadedProject, Outcome<CompileError>> {
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
) -> Result<LoadedProject, Outcome<CompileError>> {
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
        )
        .into());
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
    let (files, sources) = build_loaded_files(snapshot)?;
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
        sources,
        plugins,
        plugin_call_policy,
    )?)
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
) -> Result<BTreeMap<String, Sha256Digest>, Outcome<CompileError>> {
    let lockfile_path = project_root.join("graphcal.lock");
    let lockfile_text =
        match budget.read_text(fs, &lockfile_path, LoaderArtifact::Lockfile, cancellation) {
            Ok(text) => text,
            Err(Outcome::Failed(LoaderReadError::Filesystem(error)))
                if error.io_kind() == Some(std::io::ErrorKind::NotFound) =>
            {
                return Ok(BTreeMap::new());
            }
            Err(Outcome::Cancelled) => return Err(Outcome::Cancelled),
            Err(Outcome::Failed(error)) => {
                return Err(loader_manifest_error(format!(
                    "could not read `{}`: {error}",
                    lockfile_path.display()
                ))
                .into());
            }
        };
    let lockfile = parse_lockfile_str_with_limits(&lockfile_text, budget.lockfile_parse_limits())
        .map_err(|error| {
        CompileError::Load(LoadError::ManifestError {
            message: error.to_string(),
        })
    })?;
    let validated = lockfile
        .validated(env!("CARGO_PKG_VERSION"), STDLIB_VERSION)
        .map_err(|error| {
            CompileError::Load(LoadError::ManifestError {
                message: error.to_string(),
            })
        })?;
    Ok(validated
        .plugins()
        .map(|plugin| (plugin.path().to_string(), *plugin.sha256()))
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
) -> Result<LoadedProject, Outcome<CompileError>> {
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
    let (files, sources) = build_loaded_files(snapshot)?;
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
    let mut project = LoadedProject::from_parts(files, sources, plugins, plugin_call_policy)?;
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
    plugin_pins: BTreeMap<String, Sha256Digest>,
}

impl<'a> PackageLoadContext<'a> {
    fn from_lockfile(
        project_root: &Path,
        root_manifest: PackageManifest,
        fs: &'a dyn FileSystemReader,
        sources: crate::package_sources::DependencySources<'_>,
        budget: &mut LoaderBudgetState,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, Outcome<CompileError>> {
        cancellation.checkpoint()?;
        let lockfile_path = project_root.join("graphcal.lock");
        let lockfile_text = budget
            .read_text(fs, &lockfile_path, LoaderArtifact::Lockfile, cancellation)
            .map_err(|outcome| {
                outcome.map_failed(|error| {
                    loader_manifest_error(format!(
                    "package dependencies require graphcal.lock; run `graphcal deps lock`: {error}"
                    ))
                })
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
                )
                .into());
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
            .map(|plugin| (plugin.path().to_string(), *plugin.sha256()))
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
        selector: PackageSelector<'_>,
        from: &PackageFileKey,
    ) -> Result<SelectedPackage<PackageInstanceId>, ResolveFailure> {
        let resolved = self
            .graph
            .resolve_package_selector(&from.package, selector.name().as_str())
            .map_err(|error| match error {
                // The importing file's package instance came from this graph.
                PackageResolveError::UnknownCurrentPackage { package } => {
                    ResolveFailure::PackageAuthority(PackageAuthorityError::MissingPackage(package))
                }
                PackageResolveError::UnknownDependency { package_name, .. } => {
                    ResolveFailure::UnknownDependency {
                        package: package_name,
                    }
                }
            })?;
        let package = self.graph.package(&resolved.package).ok_or_else(|| {
            ResolveFailure::PackageAuthority(PackageAuthorityError::MissingPackage(
                resolved.package.clone(),
            ))
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
) -> Result<VerifiedDependency, Outcome<CompileError>> {
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
                ))
                .into());
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
) -> Result<VerifiedDependency, Outcome<CompileError>> {
    let manifest = read_package_manifest_from_path(root, reader, budget, cancellation)?;
    let snapshot = verify_locked_source(root, package, &manifest, reader, budget, cancellation)?;
    let filesystem = snapshot.mount(root).map_err(loader_manifest_error)?;
    let captured_manifest =
        read_package_manifest_from_path(root, &filesystem, budget, cancellation)?;
    if captured_manifest != manifest {
        return Err(loader_manifest_error(
            "package manifest changed while capturing its authenticated snapshot",
        )
        .into());
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
) -> Result<graphcal_io::SourceTreeSnapshot, Outcome<CompileError>> {
    let PackageSource::Git { tree_hashes, .. } = &package.source else {
        return Err(loader_manifest_error(
            "only Git dependencies have authenticated source snapshots",
        )
        .into());
    };
    let source_dir = manifest.source_dir.to_path_buf();
    let snapshot = crate::package_snapshot::capture_package(
        fs,
        root,
        &source_dir,
        budget.source_tree_limits(),
        cancellation,
    )
    .map_err(|outcome| outcome.map_failed(loader_manifest_error))?;
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
        ))
        .into())
    }
}

fn read_package_manifest_from_path(
    root: &Path,
    fs: &dyn FileSystemReader,
    budget: &mut LoaderBudgetState,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PackageManifest, Outcome<CompileError>> {
    let manifest_path = root.join("graphcal.toml");
    let content = budget
        .read_text(fs, &manifest_path, LoaderArtifact::Manifest, cancellation)
        .map_err(|outcome| {
            outcome.map_failed(|error| {
                loader_manifest_error(format!(
                    "could not read `{}`: {error}",
                    manifest_path.display()
                ))
            })
        })?;
    parse_manifest_str(&content).map_err(|error| loader_manifest_error(error.to_string()).into())
}

fn package_cache_root() -> Result<crate::package_cache::PackageCacheRoot, String> {
    #[cfg(test)]
    if let Some(path) = TEST_CACHE_DIR.with(|slot| slot.borrow().clone()) {
        return crate::package_cache::PackageCacheRoot::from_path(path)
            .map_err(|error| error.to_string());
    }
    crate::package_cache::PackageCacheRoot::from_environment().map_err(|error| error.to_string())
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
) -> Result<Option<PackageManifest>, Outcome<CompileError>> {
    let manifest_path = project_root.join("graphcal.toml");
    if !fs.exists(&manifest_path) {
        return Ok(None);
    }
    let manifest_content = budget
        .read_text(fs, &manifest_path, LoaderArtifact::Manifest, cancellation)
        .map_err(|outcome| {
            outcome.map_failed(|error| {
                loader_manifest_error(format!(
                    "could not read `{}`: {error}",
                    manifest_path.display()
                ))
            })
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
            CompileError::Load(LoadError::InvalidSourcePath {
                path: path.to_path_buf(),
                reason: "source path has no UTF-8 file name".to_string(),
            })
        })?;
    let stem = file_name.strip_suffix(".gcl").ok_or_else(|| {
        CompileError::Load(LoadError::InvalidSourcePath {
            path: path.to_path_buf(),
            reason: "source path must end with `.gcl`".to_string(),
        })
    })?;
    Ok(DagPackageId::new(stem))
}

#[cfg(test)]
mod tests;
