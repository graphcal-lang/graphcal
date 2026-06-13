use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use crate::eval::CompileError;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::{DeclKind, ImportKind, IncludeDecl, ModulePath};
use graphcal_compiler::syntax::phase::Phase;
use graphcal_io::{FileSystemReader, RealFileSystem};

/// Span-free identity for an `import`/`include` path.
///
/// Used as a `HashMap` key in [`LoadedFile::resolved_imports`] /
/// [`LoadedDag::resolved_imports`] so that two equal logical paths always
/// produce equal keys without depending on a shared join format
/// (e.g. `.` vs `/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePathKey(Vec<String>);

impl ModulePathKey {
    /// Build a key from a parsed [`ModulePath`] AST node. Segment names are
    /// cloned and spans are dropped — span-aware lookup is never useful at
    /// this layer.
    #[must_use]
    pub fn from_path(path: &ModulePath) -> Self {
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
    /// The module path resolved to a loaded DAG/file identity.
    Resolved(DagId),
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

/// A single inline `dag X { ... }` block lifted out of its enclosing file.
///
/// Produced by the loader so that downstream stages can iterate inline DAGs
/// uniformly with file DAGs, looking up `resolved_imports` for both the body's
/// own imports and `import <self>.{...}` references back to the parent file.
#[derive(Debug, Clone)]
pub struct LoadedDag {
    /// Abstract DAG identity for this inline dag, formed by appending the
    /// dag's name to its parent file's `DagId`.
    pub dag_id: DagId,
    /// The enclosing file's `DagId`. Imports whose path resolves to this id
    /// are dag-body self-imports (`import <self>.{...}`).
    pub parent_dag_id: DagId,
    /// The dag declaration's name in source.
    pub name: String,
    /// Raw declarations from the dag body, in source order.
    pub body: Vec<Declaration>,
    /// Loader-resolved DAG identities for each `import` declaration in the
    /// body, keyed by the import path's display string. Self-imports map to
    /// `parent_dag_id`; cross-file imports map to the dependency file's id.
    /// Imports whose path fails to resolve at load time are absent here; the
    /// downstream resolver surfaces a structured error for them.
    pub resolved_imports: HashMap<ModulePathKey, InlineBodyImportResolution>,
}

/// A single loaded and parsed file.
#[derive(Debug)]
pub struct LoadedFile {
    /// Canonical path of this file (retained for I/O: diagnostics, LSP URIs).
    pub path: PathBuf,
    /// Abstract DAG identity (filesystem-independent).
    pub dag_id: DagId,
    /// Raw source text.
    pub source: Arc<String>,
    /// Parsed AST.
    pub ast: File,
    /// Named source for diagnostics.
    pub named_source: NamedSource<Arc<String>>,
    /// Loader-resolved DAG identities for each import declaration, keyed by the
    /// import path's display string (e.g. `"./lib.gcl"` or `"nasa/rocket"`).
    /// Produced by the loader so that downstream consumers (evaluator, LSP) can
    /// look up resolved imports without re-resolving.
    pub resolved_imports: HashMap<ModulePathKey, DagId>,
    /// Inline `dag X { ... }` blocks lifted from this file, with
    /// per-dag pre-resolved imports. Order matches source order.
    ///
    /// Same source as the inline-dag declarations on `ast` — they coexist
    /// during the C1/C2 transition. Once the resolver is unified
    /// (Slice C step 2) the AST view will stop being the authority for
    /// dag-body compilation and `inline_dags` will be the single source of
    /// truth.
    pub inline_dags: Vec<LoadedDag>,
}

impl LoadedFile {
    /// Iterate over `import` declarations together with their loader-resolved
    /// DAG identities.
    pub fn imports_with_dag_ids(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::desugar::desugared_ast::Declaration,
            &graphcal_compiler::syntax::ast::ImportDecl,
            &DagId,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Import(import_decl) = &decl.kind {
                self.resolved_imports
                    .get(&ModulePathKey::from_path(&import_decl.path))
                    .map(|dag_id| (decl, import_decl, dag_id))
            } else {
                None
            }
        })
    }

    /// Iterate over `include` declarations together with their loader-resolved
    /// DAG identities.
    pub fn includes_with_dag_ids(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::desugar::desugared_ast::Declaration,
            &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
            &DagId,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Include(include_decl) = &decl.kind {
                self.resolved_imports
                    .get(&ModulePathKey::from_path(&include_decl.path))
                    .map(|dag_id| (decl, include_decl, dag_id))
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
    pub files: HashMap<DagId, LoadedFile>,
    /// The DAG identity of the root file.
    pub root: DagId,
    /// Topological load order: dependencies before dependents.
    /// The root file is last.
    pub load_order: Vec<DagId>,
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
        let source = Arc::new(source.to_string());
        let named_source = graphcal_compiler::syntax::named_source(name, Arc::clone(&source));
        let raw_ast =
            graphcal_compiler::syntax::parser::Parser::with_name(&source, name).parse_file()?;
        let ast = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);
        let path = PathBuf::from(name);
        let dag_id = DagId::from_relative_path(&path).map_err(|e| {
            CompileError::Eval(
                graphcal_compiler::registry::error::GraphcalError::EvalError {
                    message: format!("invalid source name `{name}`: {e}"),
                    src: named_source.clone(),
                    span: graphcal_compiler::syntax::span::Span::new(0, 0).into(),
                },
            )
        })?;
        // No project root or manifest in single-file mode — only the
        // file-stem self-reference (Concept 7) can be detected here.
        let inline_dags = lift_inline_dags_by_stem(&ast, &path, &dag_id);
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
        Ok(Self {
            files,
            root: dag_id.clone(),
            load_order: vec![dag_id],
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
    /// Returns [`graphcal_compiler::syntax::module_resolve::ModuleResolveError`]
    /// if duplicate symbols are found or a resolved import edge names a symbol
    /// that is absent/private in its target module.
    pub fn build_module_resolver(
        &self,
    ) -> Result<
        graphcal_compiler::syntax::module_resolve::ModuleResolver,
        graphcal_compiler::syntax::module_resolve::ModuleResolveError,
    > {
        let mut resolver = graphcal_compiler::syntax::module_resolve::ModuleResolver::default();

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            resolver.add_module(loaded.dag_id.clone(), &loaded.ast.declarations)?;
            for inline in &loaded.inline_dags {
                resolver.add_module(inline.dag_id.clone(), &inline.body)?;
            }
        }

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            add_instantiated_include_modules(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                &self.files,
            )?;
            for inline in &loaded.inline_dags {
                add_instantiated_include_modules(
                    &mut resolver,
                    &inline.dag_id,
                    &inline.body,
                    &inline.resolved_imports,
                    &self.files,
                )?;
            }
        }

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            register_module_imports(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
                &self.files,
            )?;
            for inline in &loaded.inline_dags {
                register_module_imports(
                    &mut resolver,
                    &inline.dag_id,
                    &inline.body,
                    &inline.resolved_imports,
                    &self.files,
                )?;
            }
        }

        for dag_id in &self.load_order {
            let loaded = &self.files[dag_id];
            link_instantiated_include_indexes(
                &mut resolver,
                &loaded.dag_id,
                &loaded.ast.declarations,
                &loaded.resolved_imports,
            )?;
            for inline in &loaded.inline_dags {
                link_instantiated_include_indexes(
                    &mut resolver,
                    &inline.dag_id,
                    &inline.body,
                    &inline.resolved_imports,
                )?;
            }
        }

        Ok(resolver)
    }
}

trait ResolvedModuleLookup {
    fn resolved_target(&self, key: &ModulePathKey) -> Option<&DagId>;
}

impl ResolvedModuleLookup for HashMap<ModulePathKey, DagId> {
    fn resolved_target(&self, key: &ModulePathKey) -> Option<&DagId> {
        self.get(key)
    }
}

impl ResolvedModuleLookup for HashMap<ModulePathKey, InlineBodyImportResolution> {
    fn resolved_target(&self, key: &ModulePathKey) -> Option<&DagId> {
        match self.get(key) {
            Some(InlineBodyImportResolution::Resolved(target)) => Some(target),
            Some(InlineBodyImportResolution::Unresolved) | None => None,
        }
    }
}

fn add_instantiated_include_modules(
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
        let Some(prefix) = instantiated_include_prefix(include) else {
            continue;
        };
        let Some(file_target) =
            resolved_imports.resolved_target(&ModulePathKey::from_path(&include.path))
        else {
            continue;
        };
        let target = module_resolver_target_for_path(&include.path, file_target, files);
        let Some(target_decls) = module_declarations(&target, files) else {
            continue;
        };
        resolver.add_module(owner.child(prefix.as_str()), target_decls)?;
    }
    Ok(())
}

/// Make each instantiated include's own indexes resolvable by their bare names
/// inside the importing module.
///
/// The synthetic include modules created by [`add_instantiated_include_modules`]
/// already hold the dependency's index declarations. This pass copies those
/// indexes into the importer's symbol table (skipping any the include binds or
/// overrides) so the inlined dependency bodies — `for s: Step`, `T[Step]`,
/// `Step.A` — resolve against the importer's merged registry. See
/// [`graphcal_compiler::syntax::module_resolve::ModuleResolver::inline_instantiated_include_indexes`].
fn link_instantiated_include_indexes(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &DagId,
    declarations: &[Declaration],
    resolved_imports: &impl ResolvedModuleLookup,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    for decl in declarations {
        let DeclKind::Include(include) = &decl.kind else {
            continue;
        };
        let Some(prefix) = instantiated_include_prefix(include) else {
            continue;
        };
        if resolved_imports
            .resolved_target(&ModulePathKey::from_path(&include.path))
            .is_none()
        {
            continue;
        }
        let synthetic = owner.child(prefix.as_str());
        if resolver.modules().get(&synthetic).is_none() {
            // The synthetic module is only present when the include target
            // resolved to declarations; skip silently otherwise (a missing
            // target is already reported elsewhere).
            continue;
        }
        let bound: HashSet<&str> = include
            .param_bindings
            .iter()
            .map(|binding| binding.name.name.as_str())
            .collect();
        resolver.inline_instantiated_include_indexes(owner, &synthetic, &bound)?;
    }
    Ok(())
}

fn instantiated_include_prefix<P: Phase>(include: &IncludeDecl<P>) -> Option<String> {
    (!include.param_bindings.is_empty()).then(|| match &include.kind {
        ImportKind::Module { alias } => alias.as_ref().map_or_else(
            || include.path.leaf().name.to_string(),
            |alias| alias.value.to_string(),
        ),
        ImportKind::Selective(_) => include.path.leaf().name.to_string(),
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
            .map(|inline| inline.body.as_slice())
    })
}

fn register_module_imports(
    resolver: &mut graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &DagId,
    declarations: &[Declaration],
    resolved_imports: &impl ResolvedModuleLookup,
    files: &HashMap<DagId, LoadedFile>,
) -> Result<(), graphcal_compiler::syntax::module_resolve::ModuleResolveError> {
    for decl in declarations {
        match &decl.kind {
            DeclKind::Import(import) => {
                if let Some(target) =
                    resolved_imports.resolved_target(&ModulePathKey::from_path(&import.path))
                {
                    resolver.register_import(
                        owner,
                        &import.path,
                        &import.kind,
                        &module_resolver_target_for_path(&import.path, target, files),
                    )?;
                }
            }
            DeclKind::Include(include) => {
                if let Some(target) =
                    resolved_imports.resolved_target(&ModulePathKey::from_path(&include.path))
                {
                    let synthetic_owner = instantiated_include_prefix(include)
                        .map(|prefix| owner.child(prefix.as_str()));
                    let target = synthetic_owner.unwrap_or_else(|| {
                        module_resolver_target_for_path(&include.path, target, files)
                    });
                    resolver.register_include(owner, &include.path, &include.kind, &target)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Refine a loader-resolved file target to an inline-DAG child target when
/// the source module path used the loader's `parent-file + dag leaf` fallback.
///
/// The loader still owns filesystem resolution: `file_target` is the canonical
/// file chosen for `path`. This helper only maps that already-loaded file to
/// its already-lifted inline DAG child when the path leaf names one.
fn module_resolver_target_for_path(
    path: &ModulePath,
    file_target: &DagId,
    files: &HashMap<DagId, LoadedFile>,
) -> DagId {
    let leaf = path.leaf().name.as_str();
    if leaf == file_target.name() {
        return file_target.clone();
    }

    files
        .get(file_target)
        .and_then(|loaded| {
            loaded
                .inline_dags
                .iter()
                .find(|inline| inline.name.as_str() == leaf)
                .map(|inline| inline.dag_id.clone())
        })
        .unwrap_or_else(|| file_target.clone())
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
    let manifest: Option<graphcal_compiler::registry::manifest::Manifest> =
        load_manifest_for_root(&project_root, &root_canonical, fs)?;

    load_file_dfs(
        &root_canonical,
        &project_root,
        &mut files,
        &mut path_to_dag_id,
        &mut load_order,
        &mut loading,
        &mut stack,
        manifest.as_ref(),
        fs,
    )?;

    let root_dag_id = path_to_dag_id[&root_canonical].clone();
    Ok(LoadedProject {
        files,
        root: root_dag_id,
        load_order,
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
    files: &mut HashMap<DagId, LoadedFile>,
    path_to_dag_id: &mut HashMap<PathBuf, DagId>,
    load_order: &mut Vec<DagId>,
    loading: &mut HashSet<PathBuf>,
    stack: &mut Vec<String>,
    manifest: Option<&graphcal_compiler::registry::manifest::Manifest>,
    fs: &F,
) -> Result<(), CompileError> {
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

    // Read the file via the filesystem abstraction.
    let source_str = fs
        .read_to_string(canonical_path)
        .map_err(|_| io_not_found(canonical_path))?;
    let source = Arc::new(source_str);

    // Use the canonical path as the NamedSource name (not just the basename).
    // Downstream diagnostic emitters can recover the file URL via
    // `Url::from_file_path(Path::new(name))` without an external resolver,
    // and basename ambiguity (two `lib.gcl`s in different packages) cannot
    // arise. The CLI's miette renderer trims this for display anyway.
    let name = display_name.as_str();
    let named_source = graphcal_compiler::syntax::named_source(name, Arc::clone(&source));
    let raw_ast =
        graphcal_compiler::syntax::parser::Parser::with_name(&source, name).parse_file()?;
    let ast = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);

    // Collect inline DAG names (including nested DAGs) so dependency scanning
    // can skip single-segment includes/imports that reference same-file DAG
    // modules rather than files.
    let dag_names = collect_inline_dag_names(&ast.declarations);

    // Find import and include declarations and recurse.
    let parent_dir = canonical_path.parent().unwrap_or_else(|| Path::new("."));
    let mut resolved_imports_paths: HashMap<ModulePathKey, PathBuf> = HashMap::new();
    for decl in &ast.declarations {
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

        let import_canonical =
            resolve_import_path(path, parent_dir, project_root, &named_source, manifest, fs)?;

        // Path sandboxing: reject imports that resolve outside the project root.
        if !import_canonical.starts_with(project_root) {
            return Err(CompileError::Eval(GraphcalError::ImportOutsideRoot {
                path: path.display_path(),
                src: named_source,
                span: path.span().into(),
            }));
        }

        resolved_imports_paths.insert(ModulePathKey::from_path(path), import_canonical.clone());

        // A fully-qualified import that resolves to this very file (e.g.
        // `import pkg.main.inline_dag.{x};` inside main.gcl) is a
        // self-reference, not a dependency — recursing would trip the
        // circular-import check (mirrors the inline-dag loop below).
        if import_canonical == canonical_path {
            continue;
        }

        load_file_dfs(
            &import_canonical,
            project_root,
            files,
            path_to_dag_id,
            load_order,
            loading,
            stack,
            manifest,
            fs,
        )?;
    }

    // Inline DAG bodies are semantic DAG modules in their own right: their
    // imports/includes must drive project loading just like file-root
    // declarations. Resolution failures are not reported here because the
    // body import remains in the source and the module resolver can produce
    // the span-precise diagnostic later.
    for path in inline_dag_dependency_paths(&ast.declarations) {
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

        let Ok(import_canonical) =
            resolve_import_path(path, parent_dir, project_root, &named_source, manifest, fs)
        else {
            continue;
        };
        if !import_canonical.starts_with(project_root) {
            return Err(CompileError::Eval(GraphcalError::ImportOutsideRoot {
                path: path.display_path(),
                src: named_source,
                span: path.span().into(),
            }));
        }
        if import_canonical == canonical_path {
            continue;
        }
        load_file_dfs(
            &import_canonical,
            project_root,
            files,
            path_to_dag_id,
            load_order,
            loading,
            stack,
            manifest,
            fs,
        )?;
    }

    // Compute the DagId from the path relative to the project root.
    let relative_path = canonical_path
        .strip_prefix(project_root)
        .unwrap_or(canonical_path);
    let dag_id = DagId::from_relative_path(relative_path).map_err(|e| {
        CompileError::Eval(
            graphcal_compiler::registry::error::GraphcalError::EvalError {
                message: format!("invalid module path `{}`: {e}", relative_path.display()),
                src: named_source.clone(),
                span: graphcal_compiler::syntax::span::Span::new(0, 0).into(),
            },
        )
    })?;

    // Convert resolved import paths to DagIds. A self-import resolves to
    // this file's own id, which is not in `path_to_dag_id` yet (it is
    // inserted post-order, below).
    let resolved_imports: HashMap<ModulePathKey, DagId> = resolved_imports_paths
        .iter()
        .map(|(key, canonical)| {
            let dep_dag_id = if canonical == canonical_path {
                dag_id.clone()
            } else {
                path_to_dag_id[canonical].clone()
            };
            (key.clone(), dep_dag_id)
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
    manifest: Option<&graphcal_compiler::registry::manifest::Manifest>,
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
    lift_inline_dags_from_declarations(&ast.declarations, self_dag_id, &context, &mut out);
    out
}

struct InlineLiftContext<'a, F: FileSystemReader> {
    file_dag_id: &'a DagId,
    same_file_dag_ids: &'a HashSet<DagId>,
    canonical_path: &'a Path,
    parent_dir: &'a Path,
    project_root: &'a Path,
    src: &'a NamedSource<Arc<String>>,
    manifest: Option<&'a graphcal_compiler::registry::manifest::Manifest>,
    path_to_dag_id: &'a HashMap<PathBuf, DagId>,
    fs: &'a F,
    file_stem: &'a str,
}

fn lift_inline_dags_from_declarations<F: FileSystemReader>(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
    context: &InlineLiftContext<'_, F>,
    out: &mut Vec<LoadedDag>,
) {
    for decl in declarations {
        let DeclKind::Dag(dag) = &decl.kind else {
            continue;
        };
        let name = dag.name.value.to_string();
        let dag_id = lexical_parent_id.child(name.as_str());
        let resolved_imports = resolve_inline_body_imports(&dag.body, &dag_id, context);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: context.file_dag_id.clone(),
            name,
            body: dag.body.clone(),
            resolved_imports,
        });
        lift_inline_dags_from_declarations(&dag.body, &dag_id, context, out);
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
        return InlineBodyImportResolution::Resolved(target);
    }
    // Single-segment file-stem reference — Concept 7 self-import.
    if path.segments.len() == 1 && path.segments[0].name == context.file_stem {
        return InlineBodyImportResolution::Resolved(context.file_dag_id.clone());
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
    if resolved == context.canonical_path {
        InlineBodyImportResolution::Resolved(context.file_dag_id.clone())
    } else {
        context
            .path_to_dag_id
            .get(&resolved)
            .cloned()
            .map_or(InlineBodyImportResolution::Unresolved, |dag_id| {
                InlineBodyImportResolution::Resolved(dag_id)
            })
    }
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
    out: &mut Vec<LoadedDag>,
) {
    for decl in declarations {
        let DeclKind::Dag(dag) = &decl.kind else {
            continue;
        };
        let name = dag.name.value.to_string();
        let dag_id = lexical_parent_id.child(name.as_str());
        let resolved_imports = dag
            .body
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
                                    InlineBodyImportResolution::Resolved(file_dag_id.clone())
                                } else {
                                    InlineBodyImportResolution::Unresolved
                                }
                            },
                            InlineBodyImportResolution::Resolved,
                        );
                (key, resolution)
            })
            .collect();
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: file_dag_id.clone(),
            name,
            body: dag.body.clone(),
            resolved_imports,
        });
        lift_inline_dags_by_stem_from_declarations(
            &dag.body,
            file_dag_id,
            &dag_id,
            file_stem,
            same_file_dag_ids,
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
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return None,
        }
    }
}

/// Build a [`RealFileSystem`] sandboxed to the project root for compiling
/// `file`. Used by the CLI and the LSP so both discover the same root.
///
/// Resolution order:
/// 1. If `root_override` canonicalizes, return [`RealFileSystem::rooted`] there.
/// 2. Otherwise pick a starting directory: the parent of the canonicalized
///    file when it exists on disk, falling back to the canonicalized parent
///    of the input path (so unsaved LSP buffers can still walk up from their
///    enclosing directory).
/// 3. From that directory, walk up looking for `graphcal.toml`; if found,
///    root the FS there.
/// 4. Otherwise return an unrooted [`RealFileSystem::default`] so one-shot
///    evals of loose files outside any project keep working.
#[must_use]
pub fn build_rooted_filesystem(file: &Path, root_override: Option<&Path>) -> RealFileSystem {
    if let Some(explicit) = root_override
        && let Ok(fs) = RealFileSystem::rooted(explicit)
    {
        return fs;
    }

    let start_dir = file
        .canonicalize()
        .ok()
        .and_then(|c| c.parent().map(Path::to_path_buf))
        .or_else(|| file.parent().and_then(|p| p.canonicalize().ok()));

    let Some(start_dir) = start_dir else {
        return RealFileSystem::default();
    };

    let fs = RealFileSystem::default();
    discover_project_root(&start_dir, &fs)
        .and_then(|root| RealFileSystem::rooted(&root).ok())
        .unwrap_or_default()
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
) -> Result<Option<graphcal_compiler::registry::manifest::Manifest>, CompileError> {
    let manifest_path = project_root.join("graphcal.toml");
    if !fs.exists(&manifest_path) {
        return Ok(None);
    }
    let manifest_content = fs.read_to_string(&manifest_path).map_err(|e| {
        CompileError::Eval(GraphcalError::ManifestError {
            message: e.to_string(),
        })
    })?;
    let parsed = graphcal_compiler::registry::manifest::parse_manifest_str(&manifest_content)
        .map_err(|e| {
            CompileError::Eval(GraphcalError::ManifestError {
                message: e.to_string(),
            })
        })?;

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
    manifest: &graphcal_compiler::registry::manifest::Manifest,
    fs: &F,
) -> bool {
    let pkg_dir = project_root
        .join(&manifest.source_dir)
        .join(&manifest.package_name);
    let pkg_file = project_root
        .join(&manifest.source_dir)
        .join(format!("{}.gcl", manifest.package_name));

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
    manifest: Option<&graphcal_compiler::registry::manifest::Manifest>,
    fs: &F,
) -> Result<PathBuf, CompileError> {
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
    manifest: Option<&graphcal_compiler::registry::manifest::Manifest>,
    fs: &F,
) -> Result<PathBuf, CompileError> {
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
        if !segments.is_empty() && segments[0].name != m.package_name {
            return Err(CompileError::Eval(GraphcalError::PackageNameMismatch {
                path_first: segments[0].name.to_string(),
                package_name: m.package_name.clone(),
                src: src.clone(),
                span: span.into(),
            }));
        }

        // Build path: <project_root>/<source_dir>/seg0/seg1/.../segN.gcl
        let mut file_path = project_root.join(&m.source_dir);
        for seg in segments {
            file_path = file_path.join(seg.name.as_str());
        }
        file_path.set_extension("gcl");

        if let Ok(canonical) = fs.canonicalize(&file_path) {
            return Ok(canonical);
        }

        // Fallback: 2+ segments — try the parent file. E.g. for
        // `nasa.rocket.velocity`, try `nasa/rocket.gcl` and expect
        // `velocity` to be a DAG defined inside it.
        if segments.len() >= 2 {
            let mut parent_path = project_root.join(&m.source_dir);
            for seg in &segments[..segments.len() - 1] {
                parent_path = parent_path.join(seg.name.as_str());
            }
            parent_path.set_extension("gcl");
            if let Ok(canonical) = fs.canonicalize(&parent_path) {
                return Ok(canonical);
            }
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

/// Helper to create a `FileNotFound` error (used for the root file itself).
fn io_not_found(path: &Path) -> CompileError {
    CompileError::Eval(GraphcalError::FileNotFound {
        path: path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use graphcal_io::RealFileSystem;

    fn fs() -> RealFileSystem {
        RealFileSystem::default()
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
    fn load_standalone_file() {
        let dir = setup_temp_dir(&[("standalone.gcl", "param x: Dimensionless = 1.0;")]);
        let project = load_project(&dir.path().join("standalone.gcl"), None, &fs()).unwrap();
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.load_order.len(), 1);
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
        let lib_dag_id = DagId::new("src", ["helper", "lib"]);
        let main_dag_id = DagId::new("src", ["helper", "main"]);
        assert_eq!(project.load_order[0], lib_dag_id);
        assert_eq!(project.load_order[1], main_dag_id);
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
        let lib_dag_id = DagId::new("src", ["helper", "lib"]);

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
            .find(|dag| dag.name == "calc")
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
        let lib_dag_id = DagId::new("src", ["helper", "lib"]);
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
        let d_dag_id = DagId::new("graph", ["d"]);
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
}
