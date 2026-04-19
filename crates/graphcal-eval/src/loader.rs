use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use crate::eval::CompileError;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::{DeclKind, File, ImportPath};
use graphcal_compiler::syntax::dag_id::DagId;
use graphcal_io::FileSystemReader;

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
    pub resolved_imports: HashMap<String, DagId>,
}

impl LoadedFile {
    /// Iterate over `import` declarations together with their loader-resolved
    /// DAG identities.
    pub fn imports_with_dag_ids(
        &self,
    ) -> impl Iterator<
        Item = (
            &graphcal_compiler::syntax::ast::Declaration,
            &graphcal_compiler::syntax::ast::ImportDecl,
            &DagId,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Import(import_decl) = &decl.kind {
                self.resolved_imports
                    .get(&import_decl.path.display_path())
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
            &graphcal_compiler::syntax::ast::Declaration,
            &graphcal_compiler::syntax::ast::IncludeDecl,
            &DagId,
        ),
    > {
        self.ast.declarations.iter().filter_map(|decl| {
            if let DeclKind::Include(include_decl) = &decl.kind {
                self.resolved_imports
                    .get(&include_decl.path.display_path())
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
    /// Returns a [`CompileError`] if parsing fails.
    pub fn from_source(source: &str, name: &str) -> Result<Self, CompileError> {
        let source = Arc::new(source.to_string());
        let named_source = graphcal_compiler::syntax::named_source(name, Arc::clone(&source));
        let mut ast =
            graphcal_compiler::syntax::parser::Parser::with_name(&source, name).parse_file()?;
        graphcal_compiler::syntax::ast::desugar_tuple_matches(&mut ast);
        graphcal_compiler::syntax::name_resolve::resolve_name_refs(&mut ast);
        let path = PathBuf::from(name);
        let dag_id = DagId::from_relative_path(&path);
        let loaded_file = LoadedFile {
            path,
            dag_id: dag_id.clone(),
            source,
            ast,
            named_source,
            resolved_imports: HashMap::new(),
        };
        let mut files = HashMap::new();
        files.insert(dag_id.clone(), loaded_file);
        Ok(Self {
            files,
            root: dag_id.clone(),
            load_order: vec![dag_id],
        })
    }
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

    let mut manifest: Option<graphcal_compiler::registry::manifest::Manifest> = None;

    load_file_dfs(
        &root_canonical,
        &project_root,
        &mut files,
        &mut path_to_dag_id,
        &mut load_order,
        &mut loading,
        &mut stack,
        &mut manifest,
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
fn load_file_dfs<F: FileSystemReader>(
    canonical_path: &Path,
    project_root: &Path,
    files: &mut HashMap<DagId, LoadedFile>,
    path_to_dag_id: &mut HashMap<PathBuf, DagId>,
    load_order: &mut Vec<DagId>,
    loading: &mut HashSet<PathBuf>,
    stack: &mut Vec<String>,
    manifest: &mut Option<graphcal_compiler::registry::manifest::Manifest>,
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

    let name = canonical_path
        .file_name()
        .map_or_else(|| display_name.clone(), |n| n.to_string_lossy().to_string());
    let named_source = graphcal_compiler::syntax::named_source(&name, Arc::clone(&source));
    let mut ast =
        graphcal_compiler::syntax::parser::Parser::with_name(&source, &name).parse_file()?;
    graphcal_compiler::syntax::ast::desugar_tuple_matches(&mut ast);
    graphcal_compiler::syntax::name_resolve::resolve_name_refs(&mut ast);

    // Collect inline DAG names so we can skip includes that reference them.
    let dag_names: HashSet<String> = ast
        .declarations
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Dag(dag) => Some(dag.name.value.to_string()),
            _ => None,
        })
        .collect();

    // Find import and include declarations and recurse.
    let parent_dir = canonical_path.parent().unwrap_or_else(|| Path::new("."));
    let mut resolved_imports_paths: HashMap<String, PathBuf> = HashMap::new();
    for decl in &ast.declarations {
        let path = match &decl.kind {
            DeclKind::Import(import_decl) => &import_decl.path,
            DeclKind::Include(include_decl) => &include_decl.path,
            _ => continue,
        };

        // Skip parent scope paths (`import .. { ... }`) — resolved at eval time.
        if path.is_parent_scope() {
            continue;
        }

        // Skip single-segment module paths that reference inline DAGs.
        if let ImportPath::ModulePath { segments, .. } = path
            && segments.len() == 1
            && dag_names.contains(segments[0].name.as_str())
        {
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

        resolved_imports_paths.insert(path.display_path(), import_canonical.clone());

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
    let dag_id = DagId::from_relative_path(relative_path);

    // Convert resolved import paths to DagIds.
    let resolved_imports: HashMap<String, DagId> = resolved_imports_paths
        .iter()
        .map(|(display, canonical)| {
            let dep_dag_id = path_to_dag_id[canonical].clone();
            (display.clone(), dep_dag_id)
        })
        .collect();

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
        },
    );

    Ok(())
}

/// Derive a module name from a file path string.
///
/// Extracts the filename stem (e.g. `"constants"` from `"./constants.gcl"`)
/// and validates it is a valid `lower_snake_case` identifier.
///
/// # Errors
///
/// Returns `Err` with the invalid stem if the filename stem is not a valid
/// `lower_snake_case` identifier.
pub fn derive_module_name(path: &str) -> Result<String, String> {
    let file_path = Path::new(path);
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| path.to_string())?;

    if is_valid_module_name(stem) {
        Ok(stem.to_string())
    } else {
        Err(stem.to_string())
    }
}

/// Check if a string is a valid module name (`lower_snake_case` identifier).
fn is_valid_module_name(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Determine the project root directory for import path sandboxing.
///
/// Walks from the entry-point file's parent directory upward, looking for a
/// `graphcal.toml` manifest file. If found, the directory containing the
/// manifest becomes the project root, widening the import boundary to that
/// entire directory tree. If no manifest is found, the project root defaults
/// to the entry-point file's parent directory (the simplest predictable
/// default: a file can import siblings and descendants but not files above
/// its own directory).
fn project_root_for<F: FileSystemReader>(root_file_dir: &Path, fs: &F) -> PathBuf {
    let mut dir = root_file_dir;
    loop {
        if fs.is_file(&dir.join("graphcal.toml")) {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    root_file_dir.to_path_buf()
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

/// Resolve an `ImportPath` to a canonical file path.
///
/// - `FilePath`: resolved relative to `parent_dir` (the importing file's directory).
/// - `ModulePath`: resolved via the project manifest (`graphcal.toml`).
fn resolve_import_path<F: FileSystemReader>(
    import_path: &ImportPath,
    parent_dir: &Path,
    project_root: &Path,
    src: &NamedSource<Arc<String>>,
    manifest: &mut Option<graphcal_compiler::registry::manifest::Manifest>,
    fs: &F,
) -> Result<PathBuf, CompileError> {
    match import_path {
        ImportPath::FilePath { path, span } => {
            let file_path = parent_dir.join(path);
            fs.canonicalize(&file_path).map_err(|_| {
                CompileError::Eval(GraphcalError::ImportFileNotFound {
                    path: path.clone(),
                    src: src.clone(),
                    span: (*span).into(),
                })
            })
        }
        ImportPath::ModulePath { segments, span } => {
            resolve_module_path(segments, *span, project_root, src, manifest, fs)
        }
        ImportPath::ParentScope { span, .. } => {
            // Parent scope paths (`import .. { ... }`) are resolved at eval time,
            // not during file loading. They should be skipped by load_file_dfs.
            Err(CompileError::Eval(GraphcalError::EvalError {
                message: "`..` parent scope paths cannot be resolved to files".to_string(),
                src: src.clone(),
                span: (*span).into(),
            }))
        }
        ImportPath::CrossFileDag {
            file_path, span, ..
        } => {
            // Cross-file DAG paths resolve the file component to a canonical path.
            // The DAG name is resolved at eval time.
            let full_path = parent_dir.join(file_path);
            fs.canonicalize(&full_path).map_err(|_| {
                CompileError::Eval(GraphcalError::ImportFileNotFound {
                    path: file_path.clone(),
                    src: src.clone(),
                    span: (*span).into(),
                })
            })
        }
    }
}

/// Resolve a bare module path to a canonical file path.
///
/// For `nasa/rocket`, resolves to `<project_root>/<source_dir>/nasa/rocket.gcl`.
fn resolve_module_path<F: FileSystemReader>(
    segments: &[graphcal_compiler::syntax::ast::Ident],
    span: graphcal_compiler::syntax::span::Span,
    project_root: &Path,
    src: &NamedSource<Arc<String>>,
    manifest: &mut Option<graphcal_compiler::registry::manifest::Manifest>,
    fs: &F,
) -> Result<PathBuf, CompileError> {
    let display_path = segments
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join("/");

    // Check for stdlib imports (deferred).
    if !segments.is_empty() && segments[0].name == "graphcal" {
        return Err(CompileError::Eval(GraphcalError::StdlibNotImplemented {
            path: display_path,
            src: src.clone(),
            span: span.into(),
        }));
    }

    // Load manifest if not already cached.
    if manifest.is_none() {
        let manifest_path = project_root.join("graphcal.toml");
        if !fs.exists(&manifest_path) {
            return Err(CompileError::Eval(
                GraphcalError::BareImportWithoutManifest {
                    path: display_path,
                    src: src.clone(),
                    span: span.into(),
                },
            ));
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
        *manifest = Some(parsed);
    }

    // Unwrap is safe: we just ensured `manifest` is `Some` above.
    #[expect(clippy::unwrap_used, reason = "manifest was just set to Some above")]
    let m = manifest.as_ref().unwrap();

    // Validate first segment matches package name.
    if !segments.is_empty() && segments[0].name != m.package_name {
        return Err(CompileError::Eval(GraphcalError::PackageNameMismatch {
            path_first: segments[0].name.clone(),
            package_name: m.package_name.clone(),
            src: src.clone(),
            span: span.into(),
        }));
    }

    // Build path: <project_root>/<source_dir>/seg0/seg1/.../segN.gcl
    let mut file_path = project_root.join(&m.source_dir);
    for seg in segments {
        file_path = file_path.join(&seg.name);
    }
    file_path.set_extension("gcl");

    // Try the full path first.
    if let Ok(canonical) = fs.canonicalize(&file_path) {
        return Ok(canonical);
    }

    // Fallback: if there are 2+ segments (beyond the package name), try the
    // parent file. E.g. for `nasa/rocket/velocity`, try `nasa/rocket.gcl`
    // and expect `velocity` to be a DAG defined inside it.
    if segments.len() >= 2 {
        let mut parent_path = project_root.join(&m.source_dir);
        for seg in &segments[..segments.len() - 1] {
            parent_path = parent_path.join(&seg.name);
        }
        parent_path.set_extension("gcl");
        if let Ok(canonical) = fs.canonicalize(&parent_path) {
            return Ok(canonical);
        }
    }

    Err(CompileError::Eval(GraphcalError::ImportFileNotFound {
        path: display_path,
        src: src.clone(),
        span: span.into(),
    }))
}

/// Helper to create a `FileNotFound` error (used for the root file itself).
fn io_not_found(path: &Path) -> CompileError {
    CompileError::Eval(GraphcalError::FileNotFound {
        path: path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;
    use std::fs;

    use graphcal_io::RealFileSystem;

    const FS: RealFileSystem = RealFileSystem;

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

    #[test]
    fn load_standalone_file() {
        let dir = setup_temp_dir(&[("standalone.gcl", "param x: Dimensionless = 1.0;")]);
        let project = load_project(&dir.path().join("standalone.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.load_order.len(), 1);
    }

    #[test]
    fn load_simple_import() {
        let dir = setup_temp_dir(&[
            ("helper.gcl", "param y: Dimensionless = 2.0;"),
            (
                "main.gcl",
                "import \"./helper.gcl\" { y };\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("main.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 2);
        assert_eq!(project.load_order.len(), 2);
        // helper should be loaded before main (topological order)
        let helper_dag_id = DagId::new(["helper"]);
        let main_dag_id = DagId::new(["main"]);
        assert_eq!(project.load_order[0], helper_dag_id);
        assert_eq!(project.load_order[1], main_dag_id);
    }

    #[test]
    fn load_circular_import_detected() {
        let dir = setup_temp_dir(&[
            (
                "a.gcl",
                "import \"./b.gcl\" { y };\nparam x: Dimensionless = 1.0;",
            ),
            (
                "b.gcl",
                "import \"./a.gcl\" { x };\nparam y: Dimensionless = 2.0;",
            ),
        ]);
        let result = load_project(&dir.path().join("a.gcl"), None, &FS);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("circular") || err.contains("Circular"),
            "error should mention circular: {err}"
        );
    }

    #[test]
    fn load_missing_import_file() {
        let dir = setup_temp_dir(&[("main.gcl", "import \"./nonexistent.gcl\" { x };")]);
        let result = load_project(&dir.path().join("main.gcl"), None, &FS);
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
            RealFileSystem,
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
            ("helper.gcl", "param y: Dimensionless = 2.0;"),
            (
                "main.gcl",
                "import \"./helper.gcl\" { y };\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let root_path = dir.path().join("main.gcl");

        let overlay_source = "import \"./helper.gcl\" { y };\nnode z: Dimensionless = @y + 99.0;";
        let canonical = root_path.canonicalize().unwrap();
        let fs = graphcal_io::OverlayFileSystem::new(
            RealFileSystem,
            canonical,
            overlay_source.to_string(),
        );
        let project = load_project(&root_path, None, &fs).unwrap();

        // Root file should use overlay content
        let root_file = &project.files[&project.root];
        assert_eq!(root_file.source.as_str(), overlay_source);

        // Helper file should use disk content
        let helper_dag_id = DagId::new(["helper"]);
        let helper_file = &project.files[&helper_dag_id];
        assert_eq!(helper_file.source.as_str(), "param y: Dimensionless = 2.0;");
    }

    #[test]
    fn load_with_overlay_parse_error_propagates() {
        let dir = setup_temp_dir(&[("main.gcl", "param x: Dimensionless = 1.0;")]);
        let root_path = dir.path().join("main.gcl");

        let bad_overlay = "this is not valid graphcal";
        let canonical = root_path.canonicalize().unwrap();
        let fs =
            graphcal_io::OverlayFileSystem::new(RealFileSystem, canonical, bad_overlay.to_string());
        let result = load_project(&root_path, None, &fs);
        assert!(result.is_err());
    }

    #[test]
    fn load_diamond_import_deduplication() {
        // A imports B and C; both B and C import D.
        // D should only be loaded once.
        let dir = setup_temp_dir(&[
            ("d.gcl", "param w: Dimensionless = 4.0;"),
            (
                "b.gcl",
                "import \"./d.gcl\" { w };\nparam x: Dimensionless = @w + 1.0;",
            ),
            (
                "c.gcl",
                "import \"./d.gcl\" { w };\nparam y: Dimensionless = @w + 2.0;",
            ),
            (
                "a.gcl",
                "import \"./b.gcl\" { x };\nimport \"./c.gcl\" { y };\nnode z: Dimensionless = @x + @y;",
            ),
        ]);
        let project = load_project(&dir.path().join("a.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 4);
        // d should appear first in load order
        let d_dag_id = DagId::new(["d"]);
        assert_eq!(project.load_order[0], d_dag_id);
    }

    #[test]
    fn derive_module_name_simple() {
        assert_eq!(derive_module_name("./constants.gcl").unwrap(), "constants");
    }

    #[test]
    fn derive_module_name_nested_path() {
        assert_eq!(derive_module_name("./lib/orbital.gcl").unwrap(), "orbital");
    }

    #[test]
    fn derive_module_name_with_underscores() {
        assert_eq!(derive_module_name("./my_utils.gcl").unwrap(), "my_utils");
    }

    #[test]
    fn derive_module_name_with_digits() {
        assert_eq!(derive_module_name("./lib2.gcl").unwrap(), "lib2");
    }

    #[test]
    fn derive_module_name_invalid_hyphen() {
        assert_eq!(
            derive_module_name("./my-utils.gcl").unwrap_err(),
            "my-utils"
        );
    }

    #[test]
    fn derive_module_name_invalid_uppercase_start() {
        assert_eq!(derive_module_name("./MyUtils.gcl").unwrap_err(), "MyUtils");
    }

    #[test]
    fn derive_module_name_invalid_all_caps() {
        assert_eq!(
            derive_module_name("./CONSTANTS.gcl").unwrap_err(),
            "CONSTANTS"
        );
    }

    #[test]
    fn import_outside_project_root_rejected() {
        // Create two sibling temp directories: one for the "project" and one
        // for an "external" file that should not be importable.
        let parent = tempfile::tempdir().unwrap();
        let project_dir = parent.path().join("project");
        let external_dir = parent.path().join("external");
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&external_dir).unwrap();

        fs::write(
            external_dir.join("secret.gcl"),
            "param secret: Dimensionless = 42.0;",
        )
        .unwrap();
        fs::write(
            project_dir.join("main.gcl"),
            "import \"../external/secret.gcl\" { secret };",
        )
        .unwrap();

        let result = load_project(&project_dir.join("main.gcl"), None, &FS);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("outside") || err.contains("ImportOutsideRoot"),
            "error should mention outside project root: {err}"
        );
    }

    #[test]
    fn import_subdirectory_from_parent_allowed() {
        let dir = setup_temp_dir(&[
            ("sub/helper.gcl", "param x: Dimensionless = 1.0;"),
            (
                "main.gcl",
                "import \"./sub/helper.gcl\" { x };\nnode y: Dimensionless = @x + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("main.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn import_parent_from_subdirectory_rejected() {
        let dir = setup_temp_dir(&[
            ("lib.gcl", "param x: Dimensionless = 1.0;"),
            (
                "sub/main.gcl",
                "import \"../lib.gcl\" { x };\nnode y: Dimensionless = @x + 1.0;",
            ),
        ]);
        let result = load_project(&dir.path().join("sub/main.gcl"), None, &FS);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("outside") || err.contains("ImportOutsideRoot"),
            "error should mention outside project root: {err}"
        );
    }

    #[test]
    fn project_root_is_entry_point_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = project_root_for(dir.path(), &FS);
        assert_eq!(result, dir.path());
    }

    #[test]
    fn project_root_for_with_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.path().join("graphcal.toml"), "").unwrap();

        // From the subdirectory, the manifest in the parent should be found.
        let result = project_root_for(&sub, &FS);
        assert_eq!(result, dir.path());
    }

    #[test]
    fn graphcal_toml_widens_project_root() {
        let dir = setup_temp_dir(&[
            ("shared/constants.gcl", "param c: Dimensionless = 3.0;"),
            (
                "sub/main.gcl",
                "import \"../shared/constants.gcl\" { c };\nnode y: Dimensionless = @c + 1.0;",
            ),
        ]);
        fs::write(dir.path().join("graphcal.toml"), "").unwrap();

        let project = load_project(&dir.path().join("sub/main.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn graphcal_toml_in_ancestor_directory() {
        let dir = setup_temp_dir(&[
            ("lib/helpers.gcl", "param h: Dimensionless = 10.0;"),
            (
                "deep/nested/main.gcl",
                "import \"../../lib/helpers.gcl\" { h };\nnode z: Dimensionless = @h + 1.0;",
            ),
        ]);
        fs::write(dir.path().join("graphcal.toml"), "").unwrap();

        let project = load_project(&dir.path().join("deep/nested/main.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn no_graphcal_toml_fallback() {
        let dir = setup_temp_dir(&[
            ("shared/constants.gcl", "param c: Dimensionless = 3.0;"),
            (
                "sub/main.gcl",
                "import \"../shared/constants.gcl\" { c };\nnode y: Dimensionless = @c + 1.0;",
            ),
        ]);
        let result = load_project(&dir.path().join("sub/main.gcl"), None, &FS);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("outside") || err.contains("ImportOutsideRoot"),
            "error should mention outside project root: {err}"
        );
    }

    #[test]
    fn explicit_root_overrides_graphcal_toml() {
        let dir = setup_temp_dir(&[
            ("shared/constants.gcl", "param c: Dimensionless = 3.0;"),
            (
                "sub/main.gcl",
                "import \"../shared/constants.gcl\" { c };\nnode y: Dimensionless = @c + 1.0;",
            ),
        ]);
        fs::write(dir.path().join("graphcal.toml"), "").unwrap();

        let sub_dir = dir.path().join("sub");
        let result = load_project(&dir.path().join("sub/main.gcl"), Some(&sub_dir), &FS);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("outside") || err.contains("ImportOutsideRoot"),
            "explicit root should restrict boundary: {err}"
        );
    }

    #[test]
    fn explicit_root_widens_boundary() {
        let dir = setup_temp_dir(&[
            ("shared/constants.gcl", "param c: Dimensionless = 3.0;"),
            (
                "sub/main.gcl",
                "import \"../shared/constants.gcl\" { c };\nnode y: Dimensionless = @c + 1.0;",
            ),
        ]);

        let project =
            load_project(&dir.path().join("sub/main.gcl"), Some(dir.path()), &FS).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    // ---- Bare module path loader tests ----

    #[test]
    fn load_bare_import_selective() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            ("src/nasa/rocket.gcl", "param x: Dimensionless = 1.0;"),
            (
                "src/main.gcl",
                "import nasa/rocket { x };\nnode y: Dimensionless = @x + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/main.gcl"), None, &FS).unwrap();
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
                "src/main.gcl",
                "import nasa/orbital/transfer { dv };\nnode x: Dimensionless = @dv;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/main.gcl"), None, &FS).unwrap();
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
                "lib/main.gcl",
                "import myproject/helpers { x };\nnode y: Dimensionless = @x + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("lib/main.gcl"), None, &FS).unwrap();
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn load_bare_import_without_manifest_error() {
        let dir = setup_temp_dir(&[("main.gcl", "import nasa/rocket { x };")]);
        let result = load_project(&dir.path().join("main.gcl"), None, &FS);
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("BareImportWithoutManifest") || err.contains("graphcal.toml"),
            "error should mention missing manifest: {err}"
        );
    }

    #[test]
    fn load_bare_import_package_name_mismatch_error() {
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            ("src/other/rocket.gcl", "param x: Dimensionless = 1.0;"),
            ("src/main.gcl", "import other/rocket { x };"),
        ]);
        let result = load_project(&dir.path().join("src/main.gcl"), None, &FS);
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
            ("src/main.gcl", "import graphcal/math { sin };"),
        ]);
        let result = load_project(&dir.path().join("src/main.gcl"), None, &FS);
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
            ("src/main.gcl", "import nasa/nonexistent { x };"),
        ]);
        let result = load_project(&dir.path().join("src/main.gcl"), None, &FS);
        assert!(result.is_err());
    }

    // ---- Bare module path DAG fallback tests ----

    #[test]
    fn load_bare_module_dag_fallback() {
        // `nasa/rocket/double` should resolve to `nasa/rocket.gcl` when
        // `nasa/rocket/double.gcl` doesn't exist.
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            (
                "src/nasa/rocket.gcl",
                "dag double {\n    param x: Dimensionless;\n    node result: Dimensionless = @x * 2.0;\n}\n",
            ),
            (
                "src/main.gcl",
                "include nasa/rocket/double(x: 5.0) { result as y };\nnode z: Dimensionless = @y;",
            ),
        ]);
        let project = load_project(&dir.path().join("src/main.gcl"), None, &FS).unwrap();
        // The parent file `nasa/rocket.gcl` should be loaded.
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn load_bare_module_dag_fallback_not_found() {
        // Neither `nasa/rocket/nonexistent.gcl` nor `nasa/rocket.gcl` exist.
        let dir = setup_temp_dir(&[
            ("graphcal.toml", "[package]\nname = \"nasa\"\n"),
            (
                "src/main.gcl",
                "include nasa/rocket/nonexistent(x: 5.0) { result as y };",
            ),
        ]);
        let result = load_project(&dir.path().join("src/main.gcl"), None, &FS);
        assert!(result.is_err());
    }
}
