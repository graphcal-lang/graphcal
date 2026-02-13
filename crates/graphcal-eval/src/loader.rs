use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use crate::error::GraphcalError;
use crate::eval::CompileError;
use graphcal_syntax::ast::{DeclKind, File};

/// A single loaded and parsed file.
#[derive(Debug)]
pub struct LoadedFile {
    /// Canonical path of this file.
    pub path: PathBuf,
    /// Raw source text.
    pub source: Arc<String>,
    /// Parsed AST.
    pub ast: File,
    /// Named source for diagnostics.
    pub named_source: NamedSource<Arc<String>>,
}

/// A loaded project: a root file plus all transitively imported files.
#[derive(Debug)]
pub struct LoadedProject {
    /// All loaded files keyed by canonical path.
    pub files: HashMap<PathBuf, LoadedFile>,
    /// The canonical path of the root file.
    pub root: PathBuf,
    /// Topological load order: dependencies before dependents.
    /// The root file is last.
    pub load_order: Vec<PathBuf>,
}

/// Load a project starting from `root_path`, recursively loading all
/// files referenced by `use` declarations. Detects circular imports.
///
/// # Errors
///
/// Returns a [`CompileError`] if a file cannot be read, parsed, or
/// if circular imports are detected.
pub fn load_project(root_path: &Path) -> Result<LoadedProject, CompileError> {
    let root_canonical = root_path
        .canonicalize()
        .map_err(|_| io_not_found(root_path))?;

    let mut files: HashMap<PathBuf, LoadedFile> = HashMap::new();
    let mut load_order: Vec<PathBuf> = Vec::new();
    let mut loading: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();

    load_file_dfs(
        &root_canonical,
        &mut files,
        &mut load_order,
        &mut loading,
        &mut stack,
    )?;

    Ok(LoadedProject {
        files,
        root: root_canonical,
        load_order,
    })
}

/// DFS helper: load a single file and recurse into its `use` declarations.
fn load_file_dfs(
    canonical_path: &Path,
    files: &mut HashMap<PathBuf, LoadedFile>,
    load_order: &mut Vec<PathBuf>,
    loading: &mut HashSet<PathBuf>,
    stack: &mut Vec<String>,
) -> Result<(), CompileError> {
    // Already fully loaded — skip.
    if files.contains_key(canonical_path) {
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

    // Read and parse the file.
    let source_str =
        std::fs::read_to_string(canonical_path).map_err(|_| io_not_found(canonical_path))?;
    let source = Arc::new(source_str);
    let name = canonical_path
        .file_name()
        .map_or_else(|| display_name.clone(), |n| n.to_string_lossy().to_string());
    let named_source = NamedSource::new(&name, Arc::clone(&source));
    let ast = graphcal_syntax::parser::Parser::with_name(&source, &name).parse_file()?;

    // Find use declarations and recurse.
    let parent_dir = canonical_path.parent().unwrap_or_else(|| Path::new("."));
    for decl in &ast.declarations {
        if let DeclKind::Use(use_decl) = &decl.kind {
            let import_path = parent_dir.join(&use_decl.path);
            let import_canonical = import_path.canonicalize().map_err(|_| {
                CompileError::Eval(GraphcalError::ImportFileNotFound {
                    path: use_decl.path.clone(),
                    src: named_source.clone(),
                    span: use_decl.path_span.into(),
                })
            })?;

            load_file_dfs(&import_canonical, files, load_order, loading, stack)?;
        }
    }

    // Post-order: add this file after its dependencies.
    load_order.push(canonical_path.to_path_buf());
    loading.remove(canonical_path);
    stack.pop();

    files.insert(
        canonical_path.to_path_buf(),
        LoadedFile {
            path: canonical_path.to_path_buf(),
            source,
            ast,
            named_source,
        },
    );

    Ok(())
}

/// Helper to create a `FileNotFound` error (used for the root file itself).
fn io_not_found(path: &Path) -> CompileError {
    CompileError::Eval(GraphcalError::FileNotFound {
        path: path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;
    use std::fs;

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
        let project = load_project(&dir.path().join("standalone.gcl")).unwrap();
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.load_order.len(), 1);
    }

    #[test]
    fn load_simple_import() {
        let dir = setup_temp_dir(&[
            ("helper.gcl", "param y: Dimensionless = 2.0;"),
            (
                "main.gcl",
                "use \"./helper.gcl\" { y };\nnode z: Dimensionless = @y + 1.0;",
            ),
        ]);
        let project = load_project(&dir.path().join("main.gcl")).unwrap();
        assert_eq!(project.files.len(), 2);
        assert_eq!(project.load_order.len(), 2);
        // helper should be loaded before main (topological order)
        let helper_canonical = dir.path().join("helper.gcl").canonicalize().unwrap();
        let main_canonical = dir.path().join("main.gcl").canonicalize().unwrap();
        assert_eq!(project.load_order[0], helper_canonical);
        assert_eq!(project.load_order[1], main_canonical);
    }

    #[test]
    fn load_circular_import_detected() {
        let dir = setup_temp_dir(&[
            (
                "a.gcl",
                "use \"./b.gcl\" { y };\nparam x: Dimensionless = 1.0;",
            ),
            (
                "b.gcl",
                "use \"./a.gcl\" { x };\nparam y: Dimensionless = 2.0;",
            ),
        ]);
        let result = load_project(&dir.path().join("a.gcl"));
        assert!(result.is_err());
        let err = format!("{:?}", result.unwrap_err());
        assert!(
            err.contains("circular") || err.contains("Circular"),
            "error should mention circular: {err}"
        );
    }

    #[test]
    fn load_missing_import_file() {
        let dir = setup_temp_dir(&[("main.gcl", "use \"./nonexistent.gcl\" { x };")]);
        let result = load_project(&dir.path().join("main.gcl"));
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
                "use \"./d.gcl\" { w };\nparam x: Dimensionless = @w + 1.0;",
            ),
            (
                "c.gcl",
                "use \"./d.gcl\" { w };\nparam y: Dimensionless = @w + 2.0;",
            ),
            (
                "a.gcl",
                "use \"./b.gcl\" { x };\nuse \"./c.gcl\" { y };\nnode z: Dimensionless = @x + @y;",
            ),
        ]);
        let project = load_project(&dir.path().join("a.gcl")).unwrap();
        assert_eq!(project.files.len(), 4);
        // d should appear first in load order
        let d_canonical = dir.path().join("d.gcl").canonicalize().unwrap();
        assert_eq!(project.load_order[0], d_canonical);
    }
}
