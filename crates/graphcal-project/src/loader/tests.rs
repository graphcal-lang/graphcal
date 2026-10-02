use super::*;
use crate::load_error::LoadError;
use std::cell::Cell;
use std::fs;
use std::io;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::plugin::PluginPath;
use graphcal_io::{
    ByteLimit, EntryLimit, FileSystemReadError, RealFileSystem, SourceTreeHashLimits,
};
use graphcal_package::Sha256Digest;

use source_snapshot::{ModuleResolution, ResolvedFile, SourceSnapshot};

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
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
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
        cancellation: &CancellationToken,
    ) -> Result<Vec<std::ffi::OsString>, Outcome<FileSystemReadError>> {
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
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        if path.file_name() == Some(std::ffi::OsStr::new("graphcal.toml")) {
            let read = self.manifest_reads.get().checked_add(1).ok_or_else(|| {
                FileSystemReadError::Io(io::Error::other("manifest read counter overflow"))
            })?;
            self.manifest_reads.set(read);
            if read > 1 {
                let bytes = b"[package]\nname = \"mutated\"\n";
                if bytes.len() as u64 > limit.get() {
                    return Err(FileSystemReadError::ByteLimitExceeded { limit }.into());
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
        cancellation: &CancellationToken,
    ) -> Result<Vec<std::ffi::OsString>, Outcome<FileSystemReadError>> {
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
    let atom = |segment: &str| graphcal_compiler::syntax::names::NameAtom::parse(segment).unwrap();
    let (leaf, owner) = segments.split_last().unwrap();
    graphcal_compiler::syntax::names::NamePath::from_parts(
        graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(
            owner.iter().map(|segment| atom(segment)).collect(),
        )
        .ok(),
        atom(leaf),
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

    let Some(Outcome::Failed(error)) = result.err() else {
        panic!("outside path source must be rejected");
    };
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
        CompileError::Load(LoadError::FileNotFound { path })
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
        fs.read_bytes_bounded(&main, ByteLimit::new(1024), &CancellationToken::unbounded())
            .is_ok()
    );
    assert!(
        fs.read_bytes_bounded(
            &outside,
            ByteLimit::new(1024),
            &CancellationToken::unbounded()
        )
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
    let project =
        LoadedProject::from_source("param x: Dimensionless = 1.0;", "/virtual/project/main.gcl")
            .unwrap();
    let root_file = project.root_file();

    assert_eq!(
        project
            .sources()
            .named_source(root_file.source_id())
            .unwrap()
            .name(),
        "/virtual/project/main.gcl"
    );
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
        .find(|dag| {
            dag.dag_id
                .leaf()
                .inline_dag()
                .is_some_and(|name| name.as_str() == "calc")
        })
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
    let lib_file = project.file(&lib_dag_id).unwrap();
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
    let project = load_project(&dir.path().join("lib/myproject/main.gcl"), None, &fs()).unwrap();
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
            &CancellationToken::unbounded(),
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
                dependencies: BTreeMap::from([(dependency_name.clone(), dependency_id.clone())]),
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
        CompileError::Load(LoadError::FileRootSelfImport { .. })
    ));
}

#[test]
fn dependency_enabled_loader_reports_an_unknown_dependency_at_the_import() {
    let fixture = locked_package_fixture(
        "import units.si::{ one };",
        "pub const node one: Dimensionless = 1.0;",
    );

    let error = load_project(&fixture.root_file, None, &RealFileSystem::default())
        .expect_err("`units` is a package name, not the root's dependency alias");
    let CompileError::Load(load_error @ LoadError::UnknownDependency { name, package, .. }) =
        &error
    else {
        panic!("expected an unknown-dependency error, got {error:?}");
    };
    assert_eq!(name.as_str(), "units");
    assert_eq!(package.as_str(), "mission");
    assert_eq!(
        miette::Diagnostic::code(load_error)
            .map(|code| code.to_string())
            .as_deref(),
        Some("graphcal::M035")
    );
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

    let result = crate::prepare::compile_and_eval_project(
        &fixture.root_file,
        &HashMap::new(),
        None,
        &RealFileSystem::default(),
    );
    assert!(result.is_ok(), "dependency import failed: {result:?}");
}

#[cfg(unix)]
fn assert_outside_root(error: &CompileError, expected_path: &str) {
    assert!(
        matches!(
            error,
            CompileError::Load(LoadError::ImportOutsideRoot { path, .. })
                if path.to_string() == expected_path
        ),
        "expected an outside-root import error, got {error:?}"
    );
}

/// Both project modes reject a module symlinked outside its package root,
/// at file root and in inline-DAG bodies alike.
#[cfg(unix)]
#[test]
fn single_package_import_symlinked_outside_root_is_rejected() {
    use std::os::unix::fs::symlink;

    for main in [
        "import mission.escape::{ x };",
        "dag calc {\n    import mission.escape::{ x };\n}",
    ] {
        let directory = setup_temp_dir(&[
            ("project/graphcal.toml", "[package]\nname = \"mission\"\n"),
            ("project/src/mission/main.gcl", main),
            ("outside.gcl", "pub const node x: Dimensionless = 1.0;"),
        ]);
        symlink(
            directory.path().join("outside.gcl"),
            directory.path().join("project/src/mission/escape.gcl"),
        )
        .unwrap();

        let error = load_project(
            &directory.path().join("project/src/mission/main.gcl"),
            None,
            &RealFileSystem::default(),
        )
        .expect_err("a module outside the project root must be rejected");
        assert_outside_root(&error, "mission.escape");
    }
}

#[cfg(unix)]
#[test]
fn locked_package_import_symlinked_outside_root_is_rejected() {
    use std::os::unix::fs::symlink;

    for main in [
        "import mission.escape::{ x };",
        // Regression: package mode used to leave this inline-body import
        // unresolved instead of reporting the sandbox escape.
        "dag calc {\n    import mission.escape::{ x };\n}",
    ] {
        let fixture = locked_package_fixture(main, "pub const node one: Dimensionless = 1.0;");
        let outside = fixture.directory.path().join("outside.gcl");
        std::fs::write(&outside, "pub const node x: Dimensionless = 1.0;").unwrap();
        symlink(
            &outside,
            fixture.root_file.parent().unwrap().join("escape.gcl"),
        )
        .unwrap();

        let error = load_project(&fixture.root_file, None, &RealFileSystem::default())
            .expect_err("a module outside the package root must be rejected");
        assert_outside_root(&error, "mission.escape");
    }
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
    assert!(matches!(result, Ok(Err(PluginFileError::OutsideRoot))));
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
    assert!(
        matches!(result, Ok(Err(PluginFileError::ResourceLimit(_)))),
        "oversized plugin was read into memory"
    );
}

struct LockReadFailureFileSystem(RealFileSystem);

impl FileSystemReader for LockReadFailureFileSystem {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        if path.file_name() == Some(std::ffi::OsStr::new("graphcal.lock")) {
            return Err(FileSystemReadError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "lockfile denied by test filesystem",
            ))
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
        cancellation: &CancellationToken,
    ) -> Result<Vec<std::ffi::OsString>, Outcome<FileSystemReadError>> {
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

/// In-memory package `pkg` rooted at `/p` whose reader records the order
/// in which source files are read.
struct ScriptedSources {
    manifest: PackageManifest,
    filesystem: RecordingFileSystem,
    package_id: DagPackageId,
}

struct RecordingFileSystem {
    inner: graphcal_io::InMemoryFileSystem,
    fetched: std::cell::RefCell<Vec<PathBuf>>,
}

impl FileSystemReader for RecordingFileSystem {
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, Outcome<FileSystemReadError>> {
        self.fetched.borrow_mut().push(path.to_path_buf());
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
        cancellation: &CancellationToken,
    ) -> Result<Vec<std::ffi::OsString>, Outcome<FileSystemReadError>> {
        self.inner.read_directory_bounded(path, limit, cancellation)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.inner.is_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
}

impl ScriptedSources {
    fn new(files: &[(&str, &'static str)]) -> Self {
        let mut filesystem = graphcal_io::InMemoryFileSystem::new();
        for (name, text) in files {
            filesystem
                .add_file(
                    graphcal_io::VirtualAbsolutePath::new(scripted_path(name)).unwrap(),
                    (*text).to_string(),
                )
                .unwrap();
        }
        Self {
            manifest: parse_manifest_str("[package]\nname = \"pkg\"\n").unwrap(),
            filesystem: RecordingFileSystem {
                inner: filesystem,
                fetched: std::cell::RefCell::new(Vec::new()),
            },
            package_id: DagPackageId::new("pkg"),
        }
    }

    fn authority(&self) -> ProjectSources<'_> {
        ProjectSources {
            project_root: Path::new("/p"),
            package_id: &self.package_id,
            manifest: Some(&self.manifest),
            fs: &self.filesystem,
        }
    }

    fn fetched(&self) -> Vec<PathBuf> {
        self.filesystem.fetched.borrow().clone()
    }
}

fn fetch_scripted(sources: &ScriptedSources, root: &str) -> SourceSnapshot<PathBuf> {
    fetch_source_snapshot(
        &sources.authority(),
        scripted_path(root),
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
        sources.fetched(),
        ["main", "b", "c", "d"].map(scripted_path)
    );
    assert_eq!(snapshot.files.len(), 4);
    let files = build_files(snapshot).unwrap();
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

    assert_eq!(sources.fetched(), ["main", "b"].map(scripted_path));
    assert!(snapshot.files[&scripted_path("b")].is_err());
    assert!(matches!(build_files(snapshot), Err(CompileError::Parse(_))));
}

#[test]
fn snapshot_fetch_skips_unresolved_and_self_paths() {
    let sources = ScriptedSources::new(&[(
        "main",
        "import pkg.missing::{y};\nimport pkg.main.inner::{x};\ndag inner { param x: Dimensionless = 1.0; }",
    )]);
    let snapshot = fetch_scripted(&sources, "main");

    assert_eq!(sources.fetched(), [scripted_path("main")]);
    assert!(matches!(
        build_files(snapshot),
        Err(CompileError::Load(LoadError::ImportFileNotFound { ref path, .. })) if path.to_string() == "pkg.missing"
    ));
}

#[test]
fn project_module_resolution_is_typed_and_span_free() {
    let sources = ScriptedSources::new(&[("lib", ""), ("nested/deep", "")]);
    let resolve = |text: &str| {
        let parsed = graphcal_compiler::syntax::parser::Parser::new(text)
            .parse_file()
            .unwrap();
        let graphcal_compiler::syntax::ast::DeclKind::Import(import) = &parsed.declarations[0].kind
        else {
            panic!("expected an import");
        };
        source_authority::resolve_module(
            &sources.authority(),
            import.path(),
            &scripted_path("main"),
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
            package_name: graphcal_package::PackageName::new("pkg").unwrap(),
        })
    );
    let parsed = graphcal_compiler::syntax::parser::Parser::new("import pkg.lib::{x};")
        .parse_file()
        .unwrap();
    let graphcal_compiler::syntax::ast::DeclKind::Import(import) = &parsed.declarations[0].kind
    else {
        panic!("expected an import");
    };
    assert_eq!(
        source_authority::resolve_module(
            &ProjectSources {
                manifest: None,
                ..sources.authority()
            },
            import.path(),
            &scripted_path("main"),
        ),
        ModuleResolution::Failed(ResolveFailure::CrossFileImportInVirtualPackage)
    );
}

/// Build the loaded files of `snapshot`, without its source registry.
fn build_files<K: super::source_snapshot::SourceKey>(
    snapshot: super::source_snapshot::SourceSnapshot<K>,
) -> Result<
    crate::dependency_ordered::DependencyOrdered<super::loaded_file::LoadedFile>,
    CompileError,
> {
    super::build::build_loaded_files(snapshot).map(|(files, _)| files)
}

/// Cancellation observed inside a bounded read (manifest, lockfile, plugin,
/// or source) unwinds as [`Outcome::Cancelled`], never as a read diagnostic.
#[test]
fn cancellation_during_any_package_read_is_never_reported_as_a_diagnostic() {
    let directory = setup_temp_dir(&[
        ("graphcal.toml", "[package]\nname = \"mission\"\n"),
        (
            "src/mission/main.gcl",
            "import plugin \"plugin.wasm\" as plugin {\nfn value() -> Dimensionless;\n}\nnode x: Dimensionless = 1.0;",
        ),
        ("plugin.wasm", "not wasm"),
    ]);
    let root = directory.path().join("src/mission/main.gcl");
    let load = |successful_checkpoints| {
        load_project_with_cancellation(
            &root,
            None,
            &RealFileSystem::default(),
            &CancellationToken::cancel_after_successful_checkpoints(successful_checkpoints),
        )
    };
    let checkpoints = (0..256)
        .find(|successful_checkpoints| load(*successful_checkpoints).is_ok())
        .expect("the load must complete within the sweep");
    assert!(checkpoints > 1, "loading must expose internal checkpoints");
    for successful_checkpoints in 0..checkpoints {
        let result = load(successful_checkpoints);
        assert!(
            matches!(result, Err(Outcome::Cancelled)),
            "checkpoint {successful_checkpoints} produced {:?}",
            result.err()
        );
    }
}
