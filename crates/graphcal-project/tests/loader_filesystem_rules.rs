//! Regression tests for the loader's filesystem rules (lane L14, #2255):
//! unreadable sources, sandbox escapes, and module file names.
#![cfg(test)]

use std::path::{Path, PathBuf};

use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::load_error::LoadError;
use graphcal_project::loader::load_project;

/// Write `files` (relative path, contents) below a fresh temporary directory.
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    directory
}

fn code(error: &CompileError) -> Option<String> {
    miette::Diagnostic::code(error).map(|code| code.to_string())
}

/// The readers the CLI and LSP use (rooted at the project) and the ambient one.
fn readers(project_root: &Path) -> [RealFileSystem; 2] {
    [
        RealFileSystem::rooted(project_root).unwrap(),
        RealFileSystem::default(),
    ]
}

/// An existing entry that cannot be read is a source-read failure, not a
/// manifest error (M015 "failed to parse graphcal.toml").
#[test]
fn unreadable_entry_source_is_a_source_read_failure() {
    let directory = tree(&[("graphcal.toml", "[package]\nname = \"pkg\"\n")]);
    let entry: PathBuf = directory.path().join("src/pkg/main.gcl");
    // A directory spelled like a source file exists but is not a regular file.
    std::fs::create_dir_all(&entry).unwrap();

    for reader in readers(directory.path()) {
        let error = load_project(&entry, None, &reader).expect_err("entry is unreadable");
        assert!(
            matches!(
                &error,
                CompileError::Load(LoadError::SourceReadFailed { path, .. })
                    if path.ends_with("src/pkg/main.gcl")
            ),
            "{error:?}"
        );
        assert_eq!(code(&error).as_deref(), Some("graphcal::M039"));
    }
}

fn package(files: &[(&str, &str)]) -> tempfile::TempDir {
    let mut all = vec![("graphcal.toml", "[package]\nname = \"pkg\"\n")];
    all.extend_from_slice(files);
    tree(&all)
}

fn load_error_code(directory: &tempfile::TempDir, entry: &str) -> Option<String> {
    let entry = directory.path().join(entry);
    let [rooted, ambient] = readers(directory.path()).map(|reader| {
        let error = load_project(&entry, None, &reader).expect_err("the load must fail");
        code(&error)
    });
    assert_eq!(rooted, ambient, "readers disagree");
    rooted
}

/// PRJ-14: a non-`.gcl` entry inside the package namespace is an invalid
/// source path (M023), not X001 "compiler bug".
#[test]
fn non_gcl_entry_in_package_namespace_is_an_invalid_source_path() {
    let directory = package(&[("src/pkg/notes.txt", "node x: Dimensionless = 1.0;")]);
    assert_eq!(
        load_error_code(&directory, "src/pkg/notes.txt").as_deref(),
        Some("graphcal::M023")
    );
}

/// PRJ-10: file names that an import cannot spell, or that are not
/// `lower_snake_case`, are rejected as the multi-file docs say, whether the
/// file is the entry or imported.
#[test]
fn module_files_must_have_lower_snake_case_identifier_names() {
    for (entry, files) in [
        (
            "src/pkg/my-helpers.gcl",
            vec![(
                "src/pkg/my-helpers.gcl",
                "pub const node k: Dimensionless = 2.0;",
            )],
        ),
        (
            "src/pkg/MyModule.gcl",
            vec![(
                "src/pkg/MyModule.gcl",
                "pub const node k: Dimensionless = 1.0;",
            )],
        ),
        (
            "src/pkg/main.gcl",
            vec![
                (
                    "src/pkg/main.gcl",
                    "import pkg.MyModule::{k};\nnode a: Dimensionless = @k;",
                ),
                (
                    "src/pkg/MyModule.gcl",
                    "pub const node k: Dimensionless = 1.0;",
                ),
            ],
        ),
        (
            "src/pkg/Sub/main.gcl",
            vec![("src/pkg/Sub/main.gcl", "node a: Dimensionless = 1.0;")],
        ),
    ] {
        let directory = package(&files);
        assert_eq!(
            load_error_code(&directory, entry).as_deref(),
            Some("graphcal::M040"),
            "{entry}"
        );
    }
}

/// PRJ-10 on the host filesystem: `import pkg.Lib` never loads `lib.gcl`.
/// A case-sensitive filesystem has no `Lib.gcl` (M002); a case-insensitive
/// one finds the file under the wrong spelling (M041).
#[test]
fn import_spelled_in_another_case_never_loads_the_file() {
    let directory = package(&[
        (
            "src/pkg/main.gcl",
            "import pkg.lib::{type Thing, Thing};\nimport pkg.Lib::{type Thing as Thing2};\nnode a: Thing = Thing(v: 1.0);\nnode b: Thing2 = @a;",
        ),
        (
            "src/pkg/lib.gcl",
            "pub type Thing { Thing(v: Dimensionless) }",
        ),
    ]);
    let code = load_error_code(&directory, "src/pkg/main.gcl");
    assert!(
        matches!(code.as_deref(), Some("graphcal::M002" | "graphcal::M041")),
        "{code:?}"
    );
}

/// A non-`.gcl` entry is rejected under the name it is loaded by, even when
/// it is a symlink to a `.gcl` file.
#[cfg(unix)]
#[test]
fn non_gcl_symlink_to_a_gcl_entry_is_an_invalid_source_path() {
    let directory = package(&[("src/pkg/main.gcl", "node x: Dimensionless = 1.0;")]);
    std::os::unix::fs::symlink("main.gcl", directory.path().join("src/pkg/notes.txt")).unwrap();
    assert_eq!(
        load_error_code(&directory, "src/pkg/notes.txt").as_deref(),
        Some("graphcal::M023")
    );
}
